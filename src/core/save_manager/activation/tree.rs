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

type Stamp = (u64, u64, u32, u64, i64, i64, i64, i64);

fn inventory(root: &Path, control: &Control) -> Result<Option<BTreeMap<String, Stamp>>> {
    control.check()?;
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        metadata.is_dir(),
        "Save directory was replaced; recovery information was preserved"
    );
    let mut entries = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        control.check()?;
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            (entry.file_type().is_dir() && metadata.is_dir())
                || (entry.file_type().is_file() && metadata.is_file()),
            "Unsupported or changed save entry was preserved: {}",
            entry.path().display()
        );
        let path = entry
            .path()
            .strip_prefix(root)?
            .to_str()
            .context("Save path is not UTF-8")?
            .to_owned();
        entries.insert(
            path,
            (
                metadata.dev(),
                metadata.ino(),
                metadata.mode(),
                metadata.len(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            ),
        );
    }
    Ok(Some(entries))
}

pub(super) fn scan(root: &Path) -> Result<Option<Tree>> {
    scan_with_control(root, &Control::default())
}

pub(super) fn scan_with_control(root: &Path, control: &Control) -> Result<Option<Tree>> {
    let Some(before) = inventory(root, control)? else {
        return Ok(None);
    };
    let mut tree = Tree::new();
    for (path, stamp) in &before {
        control.check()?;
        let file = stamp.2 & libc::S_IFMT == libc::S_IFREG;
        tree.insert(
            path.clone(),
            Entry {
                content: if file {
                    Some(content::inspect(&root.join(path), control)?)
                } else {
                    None
                },
                mode: stamp.2 & 0o777,
                modified: file.then_some((stamp.4, stamp.5)),
            },
        );
    }
    ensure!(
        inventory(root, control)?.as_ref() == Some(&before),
        "Save inventory changed during inspection; close games and tools before retrying"
    );
    validate(&tree)?;
    Ok(Some(tree))
}

pub(super) fn copy(
    source: &Path,
    destination: &Path,
    tree: &Tree,
    control: &Control,
) -> Result<()> {
    validate(tree)?;
    for (relative, entry) in tree {
        control.check()?;
        let path = destination.join(relative);
        if let Some(identity) = &entry.content {
            content::copy(
                &source.join(relative),
                &path,
                identity,
                entry.mode | 0o200,
                control,
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
        scan_with_control(source, control)?.as_ref() == Some(tree)
            && scan_with_control(destination, control)?.as_ref() == Some(tree),
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

pub(super) fn files(tree: &Tree) -> Result<Vec<super::super::SaveFileManifest>> {
    validate(tree)?;
    tree.iter()
        .filter_map(|(path, entry)| {
            entry.content.as_ref().map(|content| {
                Ok(super::super::SaveFileManifest {
                    path: path.clone(),
                    size: content.size,
                    sha256: content.sha256.clone(),
                    modified_unix_seconds: entry
                        .modified
                        .context("Save timestamp is missing")?
                        .0
                        .max(0),
                })
            })
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    // @variants: both
    #[test]
    fn rejects_files_added_while_hashing_the_save_inventory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("save.dat"), b"progress")?;
        let added = temp.path().join("new.dat");
        let control = Control {
            progress: Arc::new(move |_, _| {
                fs::write(&added, b"new progress").unwrap();
            }),
            ..Control::default()
        };
        assert!(scan_with_control(temp.path(), &control).is_err());
        assert_eq!(fs::read(temp.path().join("new.dat"))?, b"new progress");
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_replacement_of_an_already_hashed_save() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let first = temp.path().join("a.dat");
        fs::write(&first, b"first")?;
        fs::write(temp.path().join("b.dat"), b"second")?;
        let seen = AtomicBool::new(false);
        let control = Control {
            progress: Arc::new(move |_, _| {
                if seen.swap(true, Ordering::AcqRel) {
                    fs::remove_file(&first).unwrap();
                    fs::write(&first, b"changed").unwrap();
                }
            }),
            ..Control::default()
        };
        assert!(scan_with_control(temp.path(), &control).is_err());
        assert_eq!(fs::read(temp.path().join("a.dat"))?, b"changed");
        Ok(())
    }

    // @variants: both
    #[test]
    fn cancellation_stops_save_inspection_and_copying() -> Result<()> {
        let source = tempfile::tempdir()?;
        let destination = tempfile::tempdir()?;
        fs::write(source.path().join("save.dat"), b"progress")?;
        let tree = scan(source.path())?.context("Missing tree")?;
        let control = Control::default();
        control.cancelled.store(true, Ordering::Release);
        assert!(scan_with_control(source.path(), &control).is_err());
        assert!(copy(source.path(), destination.path(), &tree, &control).is_err());
        assert_eq!(fs::read_dir(destination.path())?.count(), 0);
        Ok(())
    }
}
