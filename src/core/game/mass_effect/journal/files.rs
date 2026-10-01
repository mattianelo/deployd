use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;
use walkdir::WalkDir;

use super::super::baseline::{directory, relative};
use super::{Control, Identity, Journal, Operation};

pub(in crate::core::game::mass_effect) fn check_path(root: &Path, path: &str) -> Result<()> {
    directory(root)?;
    relative(path)?;
    let mut parent = root.to_path_buf();
    let parts: Vec<_> = path.split('/').collect();
    for (index, part) in parts.iter().enumerate() {
        if !parent.try_exists()? {
            break;
        }
        directory(&parent)?;
        for entry in fs::read_dir(&parent)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .context("Non-UTF-8 entry in a MELE deployment folder")?;
            ensure!(
                name.to_lowercase() != part.to_lowercase() || name == *part,
                "Case-colliding MELE destination '{path}'"
            );
        }
        parent.push(part);
        match fs::symlink_metadata(&parent) {
            Ok(metadata) => ensure!(
                if index + 1 == parts.len() {
                    metadata.is_file()
                } else {
                    metadata.is_dir()
                },
                "Link or unexpected entry at MELE destination '{path}'"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .context("Cannot inspect MELE destination; restore folder access");
            }
        }
    }
    Ok(())
}

pub(in crate::core::game::mass_effect) fn verify(
    root: &Path,
    path: &str,
    expected: Option<&Identity>,
    control: &Control,
) -> Result<()> {
    check_path(root, path)?;
    let target = root.join(path);
    if let Some(expected) = expected {
        let mut file = open(&target, expected)?;
        let sha256 = crate::utils::verified_files::hash(&mut file, || control.check(), |_, _| {})?;
        let current = fs::symlink_metadata(&target)?;
        let opened = file.metadata()?;
        ensure!(
            current.is_file()
                && current.nlink() == 1
                && current.dev() == opened.dev()
                && current.ino() == opened.ino()
                && current.ctime() == opened.ctime()
                && current.ctime_nsec() == opened.ctime_nsec(),
            "MELE file changed during deployment verification"
        );
        ensure!(
            sha256 == expected.sha256,
            "MELE file hash changed; reconcile external changes before deploying"
        );
        control.check()
    } else {
        ensure!(
            !target.try_exists()?,
            "Unexpected MELE file '{path}'; reconcile external changes before deploying"
        );
        Ok(())
    }
}

fn open(path: &Path, expected: &Identity) -> Result<File> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Cannot read MELE file '{}'", path.display()))?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1,
        "MELE deployment files must be independent regular files"
    );
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file()
            && opened.nlink() == 1
            && opened.len() == expected.size
            && opened.dev() == metadata.dev()
            && opened.ino() == metadata.ino(),
        "MELE file changed before deployment verification"
    );
    Ok(file)
}

fn transfer(
    source: &mut File,
    mut destination: Option<&mut File>,
    expected: &Identity,
    control: &Control,
) -> Result<()> {
    let before = source.metadata()?;
    let mut hash = Sha256::new();
    let mut remaining = expected.size;
    let mut buffer = [0_u8; 65536];
    while remaining > 0 {
        control.check()?;
        let length = remaining.min(buffer.len() as u64) as usize;
        let count = source.read(&mut buffer[..length])?;
        ensure!(count > 0, "MELE deployment input was truncated");
        hash.update(&buffer[..count]);
        if let Some(destination) = destination.as_mut() {
            destination
                .write_all(&buffer[..count])
                .context("MELE deployment copy failed; check free space and folder access")?;
            crate::utils::verified_files::record_copy(count as u64);
        }
        remaining -= count as u64;
    }
    let after = source.metadata()?;
    ensure!(
        source.read(&mut buffer[..1])? == 0
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec(),
        "MELE file changed during deployment verification"
    );
    ensure!(
        format!("{:x}", hash.finalize()) == expected.sha256,
        "MELE file hash changed; reconcile external changes before deploying"
    );
    control.check()
}

pub(in crate::core::game::mass_effect) fn create_directory(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute()
            && path.components().all(|part| matches!(
                part,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )),
        "MELE deployment storage requires an absolute path without relative components"
    );
    if path == Path::new("/") {
        return Ok(());
    }
    if fs::symlink_metadata(path).is_ok() {
        return directory(path);
    }
    let parent = path.parent().context("Invalid MELE deployment storage")?;
    create_directory(parent)?;
    match fs::create_dir(path) {
        Ok(()) => sync(parent)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("Cannot create MELE deployment storage"),
    }
    directory(path)
}

pub(in crate::core::game::mass_effect) fn sync(path: &Path) -> Result<()> {
    if path != Path::new("/") {
        directory(path)?;
    }
    File::options()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?
        .sync_all()
        .context("Cannot flush MELE deployment storage")
}

pub(in crate::core::game::mass_effect) fn copy(
    source_root: &Path,
    source: &str,
    target_root: &Path,
    target: &str,
    expected: &Identity,
    control: &Control,
) -> Result<()> {
    check_path(source_root, source)?;
    check_path(target_root, target)?;
    let mut source = open(&source_root.join(source), expected)?;
    let destination = target_root.join(target);
    let parent = destination
        .parent()
        .context("MELE deployment file has no parent")?;
    create_directory(parent)?;
    let mut temp = NamedTempFile::with_prefix_in(".deployd-mele-", parent)?;
    control.check()?;
    if crate::utils::verified_files::try_clone(&source, temp.as_file_mut())? {
        let hash = crate::utils::verified_files::hash(&mut source, || control.check(), |_, _| {})?;
        ensure!(hash == expected.sha256, "MELE copy source changed");
    } else {
        transfer(&mut source, Some(temp.as_file_mut()), expected, control)?;
    }
    temp.as_file().sync_all()?;
    temp.persist_noclobber(&destination)
        .map_err(|error| error.error)
        .context("Cannot stage MELE deployment file without overwriting existing data")?;
    sync(parent)?;
    verify(target_root, target, Some(expected), control)
}

pub(super) fn stage(
    game: &Path,
    source: &super::super::candidate::Sources,
    root: &Path,
    journal: &Journal,
    originals: Option<&Path>,
    control: &Control,
) -> Result<()> {
    directory(game)?;
    directory(source)?;
    ensure!(
        !root.starts_with(game)
            && !game.starts_with(root)
            && !root.starts_with(source)
            && !source.starts_with(root),
        "MELE deployment storage must be separate from game files and staged inputs"
    );
    create_directory(root)?;
    let desired: BTreeMap<_, _> = journal
        .desired
        .files
        .iter()
        .map(|file| {
            (
                file.relative.as_str(),
                Identity {
                    size: file.size,
                    sha256: file.sha256.clone(),
                },
            )
        })
        .collect();
    let changed: BTreeSet<_> = journal
        .operations
        .iter()
        .map(|operation| operation.path.as_str())
        .chain(journal.missing_components.iter().map(String::as_str))
        .collect();
    for path in &journal.missing_components {
        verify(game, path, None, control)?;
    }
    for (path, identity) in &desired {
        source.verify(path, Some(identity), control)?;
        if !changed.contains(path) {
            verify(game, path, Some(identity), control)?;
        }
    }
    if let Some(previous) = &journal.previous {
        for file in &previous.files {
            if !changed.contains(file.relative.as_str()) {
                verify(
                    game,
                    &file.relative,
                    Some(&Identity {
                        size: file.size,
                        sha256: file.sha256.clone(),
                    }),
                    control,
                )?;
            }
        }
    }
    for operation in &journal.operations {
        control.check()?;
        verify(game, &operation.path, operation.before.as_ref(), control)?;
        for direction in ["old", "new"] {
            verify(game, &temporary(root, operation, direction)?, None, control)?;
        }
        if let Some(before) = &operation.before {
            copy(
                game,
                &operation.path,
                root,
                &format!("old/{}", operation.path),
                before,
                control,
            )?;
        }
        if let Some(after) = &operation.after {
            let (source, relative) = if desired.contains_key(operation.path.as_str()) {
                source.location(&operation.path)
            } else {
                (
                    originals.context("Missing preserved original for MELE restoration")?,
                    operation.path.as_str(),
                )
            };
            copy(
                source,
                relative,
                root,
                &format!("new/{}", operation.path),
                after,
                control,
            )?;
        }
    }
    sync(root)
}

pub(super) fn missing_directories(game: &Path, operations: &[Operation]) -> Result<Vec<String>> {
    let mut directories = BTreeSet::new();
    for operation in operations.iter().filter(|entry| entry.after.is_some()) {
        check_path(game, &operation.path)?;
        let mut parent = Path::new(&operation.path).parent();
        while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
            if !game.join(path).try_exists()? {
                directories.insert(path.to_string_lossy().into_owned());
            }
            parent = path.parent();
        }
    }
    Ok(directories.into_iter().collect())
}

pub(in crate::core::game::mass_effect) fn replace(
    game: &Path,
    stage: &Path,
    operation: &Operation,
    reverse: bool,
    control: &Control,
) -> Result<()> {
    let (before, after, source_dir) = if reverse {
        (&operation.after, &operation.before, "old")
    } else {
        (&operation.before, &operation.after, "new")
    };
    if reverse && verify(game, &operation.path, after.as_ref(), control).is_ok() {
        let destination = game.join(&operation.path);
        let parent = destination
            .parent()
            .context("Missing MELE recovery parent")?;
        if parent.try_exists()? {
            sync(parent)?;
        }
        return Ok(());
    }
    verify(game, &operation.path, before.as_ref(), control)?;
    let destination = game.join(&operation.path);
    let parent = destination
        .parent()
        .context("MELE deployment target has no parent")?;
    if let Some(after) = after {
        create_directory(parent)?;
        let source_path = format!("{source_dir}/{}", operation.path);
        check_path(stage, &source_path)?;
        let mut source = open(&stage.join(source_path), after)?;
        let name = temporary(stage, operation, source_dir)?;
        let name = Path::new(&name)
            .file_name()
            .context("Missing MELE temporary filename")?;
        let mut temp = tempfile::Builder::new()
            .prefix(name)
            .rand_bytes(0)
            .tempfile_in(parent)?;
        transfer(&mut source, Some(temp.as_file_mut()), after, control)?;
        temp.as_file().sync_all()?;
        control.check()?;
        verify(game, &operation.path, before.as_ref(), control)?;
        if before.is_none() {
            temp.persist_noclobber(&destination)
                .map_err(|error| error.error)?;
        } else {
            temp.persist(&destination).map_err(|error| error.error)?;
        }
    } else {
        control.check()?;
        fs::remove_file(&destination)?;
    }
    sync(parent)?;
    verify(game, &operation.path, after.as_ref(), control)
}

pub(in crate::core::game::mass_effect) fn temporary(
    stage: &Path,
    operation: &Operation,
    direction: &str,
) -> Result<String> {
    let id = stage
        .file_name()
        .and_then(|name| name.to_str())
        .context("Missing MELE transaction identity")?;
    ensure!(
        uuid::Uuid::parse_str(id)?.to_string() == id,
        "Invalid MELE transaction identity"
    );
    let digest = format!("{:x}", Sha256::digest(operation.path.as_bytes()));
    let parent = Path::new(&operation.path)
        .parent()
        .context("Missing MELE target parent")?;
    Ok(parent
        .join(format!(".deployd-mele-{id}-{digest}-{direction}"))
        .to_str()
        .context("Invalid MELE temporary path")?
        .to_owned())
}

pub(in crate::core::game::mass_effect) fn clear_temporary(
    game: &Path,
    stage: &Path,
    operation: &Operation,
) -> Result<()> {
    for (direction, identity) in [("old", &operation.before), ("new", &operation.after)] {
        let path = temporary(stage, operation, direction)?;
        check_path(game, &path)?;
        let destination = game.join(&path);
        let metadata = match fs::symlink_metadata(&destination) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let identity = identity
            .as_ref()
            .context("Unexpected MELE temporary output")?;
        ensure!(
            metadata.len() <= identity.size,
            "MELE temporary output was externally changed"
        );
        let expected = Identity {
            size: metadata.len(),
            sha256: String::new(),
        };
        let mut partial = open(&destination, &expected)?;
        let source_path = format!("{direction}/{}", operation.path);
        verify(stage, &source_path, Some(identity), &Control::recovery())?;
        let mut source = open(&stage.join(source_path), identity)?;
        let before = partial.metadata()?;
        let mut buffer = [0_u8; 65536];
        let mut reference = [0_u8; 65536];
        loop {
            let count = partial.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            source.read_exact(&mut reference[..count])?;
            ensure!(
                buffer[..count] == reference[..count],
                "MELE temporary output was externally changed; preserve it before retrying recovery"
            );
        }
        let after = partial.metadata()?;
        ensure!(
            partial.stream_position()? == expected.size
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec(),
            "MELE temporary output changed during recovery"
        );
        fs::remove_file(destination)?;
        sync(
            game.join(path)
                .parent()
                .context("Missing MELE temporary parent")?,
        )?;
    }
    Ok(())
}

pub(super) fn rollback(game: &Path, root: &Path, journal: &Journal) -> Result<()> {
    let control = Control::recovery();
    for operation in journal.operations.iter().rev() {
        clear_temporary(game, root, operation)?;
        replace(game, root, operation, true, &control)?;
    }
    for path in journal.directories.iter().rev() {
        let directory_path = game.join(path);
        match fs::symlink_metadata(&directory_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("Cannot inspect MELE recovery directory"),
            Ok(_) => {}
        }
        directory(&directory_path)?;
        fs::remove_dir(&directory_path).context("MELE recovery found unexpected files in a created directory; preserve them and reconcile before retrying")?;
        sync(
            directory_path
                .parent()
                .context("MELE recovery directory has no parent")?,
        )?;
    }
    Ok(())
}

pub(super) fn cleanup(root: &Path, journal: &Journal) -> Result<()> {
    cleanup_operations(root, &journal.operations)
}

pub(in crate::core::game::mass_effect) fn cleanup_operations(
    root: &Path,
    operations: &[Operation],
) -> Result<()> {
    match fs::symlink_metadata(root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Cannot inspect MELE recovery storage"),
        Ok(_) => {}
    }
    directory(root)?;
    let mut allowed = BTreeMap::new();
    let mut directories = BTreeSet::new();
    for operation in operations {
        for (prefix, identity) in [("old", &operation.before), ("new", &operation.after)] {
            if let Some(identity) = identity {
                let name = format!("{prefix}/{}", operation.path);
                let mut parent = Path::new(&name).parent();
                while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
                    directories.insert(path.to_string_lossy().into_owned());
                    parent = path.parent();
                }
                allowed.insert(name, identity);
            }
        }
    }
    let control = Control::recovery();
    for entry in WalkDir::new(root).min_depth(1).follow_links(false) {
        let entry = entry?;
        let path = entry
            .path()
            .strip_prefix(root)?
            .to_str()
            .context("Invalid MELE recovery storage entry")?;
        if entry.file_type().is_dir() {
            ensure!(
                directories.contains(path),
                "Unexpected directory in MELE recovery storage"
            );
        } else {
            verify(
                root,
                path,
                Some(allowed.get(path).context(
                    "Unexpected file in MELE recovery storage; preserve it before retrying",
                )?),
                &control,
            )?;
        }
    }
    fs::remove_dir_all(root)?;
    sync(
        root.parent()
            .context("MELE recovery storage has no parent")?,
    )
}
