use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::core::generations::content::{self, Control, Identity};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    content: Option<Identity>,
    mode: u32,
    modified: Option<(i64, i64)>,
}

pub(super) type Tree = BTreeMap<String, Entry>;

pub(super) fn scan(root: &Path) -> Result<Option<Tree>> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_dir(),
        "Save directory was replaced; recovery information was preserved"
    );
    let mut tree = Tree::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        ensure!(
            entry.file_type().is_dir() || entry.file_type().is_file(),
            "Unsupported save entry was preserved: {}",
            entry.path().display()
        );
        let path = entry
            .path()
            .strip_prefix(root)?
            .to_str()
            .context("Save path is not UTF-8")?
            .to_owned();
        let metadata = fs::symlink_metadata(entry.path())?;
        tree.insert(
            path,
            Entry {
                content: if metadata.is_file() {
                    Some(content::inspect(entry.path(), &Control::default())?)
                } else {
                    None
                },
                mode: metadata.permissions().mode() & 0o777,
                modified: metadata
                    .is_file()
                    .then(|| (metadata.mtime(), metadata.mtime_nsec())),
            },
        );
    }
    Ok(Some(tree))
}

pub(super) fn copy(source: &Path, destination: &Path, tree: &Tree) -> Result<()> {
    validate(tree)?;
    for (relative, entry) in tree {
        let path = destination.join(relative);
        if let Some(identity) = &entry.content {
            content::copy(
                &source.join(relative),
                &path,
                identity,
                entry.mode | 0o200,
                &Control::default(),
            )?;
            let (seconds, nanos) = entry.modified.context("Save timestamp is missing")?;
            let duration = std::time::Duration::from_secs(seconds.unsigned_abs());
            let time = if seconds < 0 {
                std::time::UNIX_EPOCH.checked_sub(duration)
            } else {
                std::time::UNIX_EPOCH.checked_add(duration)
            }
            .context("Save timestamp is outside the supported range")?;
            let time = time
                .checked_add(std::time::Duration::from_nanos(nanos.try_into()?))
                .context("Save timestamp is outside the supported range")?;
            let file = fs::OpenOptions::new().write(true).open(&path)?;
            file.set_modified(time)?;
            file.set_permissions(fs::Permissions::from_mode(entry.mode))?;
            file.sync_all()?;
        } else {
            fs::create_dir_all(&path)?;
        }
    }
    for (relative, entry) in tree
        .iter()
        .rev()
        .filter(|(_, entry)| entry.content.is_none())
    {
        let path = destination.join(relative);
        fs::set_permissions(&path, fs::Permissions::from_mode(entry.mode))?;
        fs::File::open(path)?.sync_all()?;
    }
    ensure!(
        scan(source)?.as_ref() == Some(tree) && scan(destination)?.as_ref() == Some(tree),
        "Save contents changed during preparation; live saves were not switched"
    );
    Ok(())
}

pub(super) fn validate(tree: &Tree) -> Result<()> {
    ensure!(
        tree.get("").is_some_and(|entry| entry.content.is_none()),
        "Save inventory has no root directory"
    );
    for (path, entry) in tree {
        ensure!(entry.mode & !0o777 == 0, "Invalid save permissions");
        if !path.is_empty() {
            ensure!(
                path.split('/')
                    .all(|part| !part.is_empty() && part != "." && part != "..")
                    && !path.contains(['\\', '\0']),
                "Invalid save inventory path"
            );
            let parent = Path::new(path)
                .parent()
                .and_then(Path::to_str)
                .context("Invalid save parent")?;
            ensure!(
                tree.get(parent)
                    .is_some_and(|entry| entry.content.is_none()),
                "Incomplete save inventory"
            );
        }
        ensure!(
            entry.content.is_some() == entry.modified.is_some(),
            "Invalid save timestamp metadata"
        );
        if let Some((_, nanos)) = entry.modified {
            ensure!(
                (0..1_000_000_000).contains(&nanos),
                "Invalid save timestamp"
            );
        }
        if let Some(identity) = &entry.content {
            identity.validate()?;
        }
    }
    Ok(())
}

pub(super) fn remove(root: &Path, expected: &Tree) -> Result<()> {
    validate(expected)?;
    let Some(current) = scan(root)? else {
        return Ok(());
    };
    ensure!(
        current
            .iter()
            .all(|(path, entry)| expected.get(path) == Some(entry)),
        "External changes in temporary saves were preserved; recovery is blocked"
    );
    for (relative, entry) in current.iter().rev() {
        let path = root.join(relative);
        if entry.content.is_some() {
            fs::remove_file(&path)?;
        } else {
            fs::remove_dir(&path)?;
        }
        if let Some(parent) = path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}
