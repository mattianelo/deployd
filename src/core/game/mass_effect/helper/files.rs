use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};

use anyhow::{Context, Result, ensure};
use walkdir::WalkDir;

use super::Control;
use super::protocol::FileIdentity;

pub(super) fn directory(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute() && path.parent().is_some(),
        "An explicit MELE working folder is required"
    );
    ensure!(
        path.components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_))),
        "MELE folders cannot contain relative components"
    );
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)
            .context("MELE folder access is unavailable; restore its folder permission")?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "MELE working folders cannot contain symbolic links"
        );
    }
    Ok(())
}

pub(super) fn relative(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty()
            && path.len() <= 4096
            && !path.contains(['\\', ':'])
            && !path.chars().any(char::is_control)
            && path
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..") && !part.ends_with(['.', ' '])),
        "Invalid helper file path"
    );
    Ok(())
}

pub(super) fn identity(root: &Path, input: &FileIdentity, control: &Control) -> Result<()> {
    relative(&input.path)?;
    ensure!(
        input.size > 0
            && input.size <= 512 * 1024 * 1024
            && input.sha256.len() == 64
            && input.sha256.bytes().all(|ch| ch.is_ascii_hexdigit()),
        "Invalid helper file identity"
    );
    let path = root.join(&input.path);
    directory(path.parent().context("Missing helper file parent")?)?;
    let metadata = fs::symlink_metadata(&path)?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1 && !metadata.file_type().is_symlink(),
        "Helper files must be regular, independent copies without links"
    );
    let mut file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file()
            && opened.nlink() == 1
            && opened.len() == input.size
            && opened.ino() == metadata.ino()
            && opened.dev() == metadata.dev(),
        "Helper file changed before verification"
    );
    let hash = crate::utils::verified_files::hash(&mut file, || control.check(), |_, _| {})?;
    let current = fs::symlink_metadata(&path)?;
    let after = file.metadata()?;
    ensure!(
        current.is_file()
            && current.nlink() == 1
            && current.dev() == opened.dev()
            && current.ino() == opened.ino()
            && current.len() == opened.len()
            && current.ctime() == opened.ctime()
            && current.ctime_nsec() == opened.ctime_nsec()
            && after.ctime() == opened.ctime()
            && after.ctime_nsec() == opened.ctime_nsec(),
        "Helper file changed during verification"
    );
    ensure!(
        hash.eq_ignore_ascii_case(&input.sha256),
        "Helper file hash does not match its identity"
    );
    control.check()?;
    Ok(())
}

pub(super) fn outputs(
    root: &Path,
    expected: &[String],
    outputs: &[FileIdentity],
    limit: u64,
    control: &Control,
) -> Result<()> {
    directory(root)?;
    let expected: BTreeSet<_> = expected.iter().cloned().collect();
    let mut declared = BTreeSet::new();
    let mut folded = BTreeSet::new();
    let mut total = 0_u64;
    for output in outputs {
        relative(&output.path)?;
        ensure!(
            declared.insert(output.path.clone()) && folded.insert(output.path.to_lowercase()),
            "Duplicate or case-colliding helper outputs"
        );
        total = total
            .checked_add(output.size)
            .context("Helper output size overflow")?;
        ensure!(total <= limit, "Helper outputs exceed the job size limit");
    }
    ensure!(
        declared == expected,
        "Helper output manifest does not match the planned targets"
    );
    let mut allowed = BTreeMap::new();
    for path in &expected {
        allowed.insert(path.clone(), false);
        let mut parent = Path::new(path).parent();
        while let Some(directory) = parent.filter(|path| !path.as_os_str().is_empty()) {
            allowed.insert(directory.to_string_lossy().into_owned(), true);
            parent = directory.parent();
        }
    }
    let mut count = 0;
    for entry in WalkDir::new(root).follow_links(false).min_depth(1) {
        control.check()?;
        let entry = entry?;
        count += 1;
        ensure!(count <= allowed.len(), "Unexpected helper output entries");
        let path = entry
            .path()
            .strip_prefix(root)?
            .to_str()
            .context("Non-UTF-8 helper output path")?;
        let directory = *allowed
            .get(path)
            .context("Unlisted file or directory in helper output")?;
        ensure!(
            if directory {
                entry.file_type().is_dir()
            } else {
                entry.file_type().is_file()
            },
            "Link or special file in helper output"
        );
    }
    ensure!(count == allowed.len(), "Missing helper output entries");
    for output in outputs {
        identity(root, output, control)?;
    }
    Ok(())
}
