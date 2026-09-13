use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameEngine};
use crate::utils::paths;

use super::super::operation::{Control, Lease};
use super::{Abandoned, Baseline, BaselineFile, Stamp, directory, relative};

pub(crate) type Progress = Arc<dyn Fn(u64, u64) + Send + Sync>;

#[derive(Debug)]
pub(crate) struct Preserved {
    pub(crate) root: PathBuf,
    pub(crate) files: Vec<BaselineFile>,
}

pub(crate) async fn preserve(
    tracker: Tracker,
    game: Game,
    targets: Vec<String>,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<Preserved> {
    preserve_in(
        tracker,
        game,
        targets,
        paths::deployd_data_dir()?,
        cancelled,
        progress,
    )
    .await
}

pub(crate) async fn preserve_in(
    tracker: Tracker,
    game: Game,
    targets: Vec<String>,
    data_root: PathBuf,
    cancelled: Arc<AtomicBool>,
    progress: Progress,
) -> Result<Preserved> {
    let abandoned = Arc::new(AtomicBool::new(false));
    let _abandoned = Abandoned(abandoned.clone());
    let control = Control {
        cancelled,
        abandoned,
    };
    let worker_control = control.clone();
    // The detached worker owns the leases until blocking writes and database work finish.
    let result = tokio::spawn(async move {
        let control = worker_control;
        let lease = Lease::acquire(&control).await?;
        preserve_with_lease(tracker, game, targets, data_root, control, progress, lease).await
    })
    .await
    .context("MELE original-file preservation worker failed")?;
    control.check()?;
    result
}

pub(in crate::core::game::mass_effect) async fn preserve_with_lease(
    tracker: Tracker,
    game: Game,
    targets: Vec<String>,
    data_root: PathBuf,
    control: Control,
    progress: Progress,
    _lease: Arc<Lease>,
) -> Result<Preserved> {
    control.check()?;
    ensure!(
        game.engine == GameEngine::MassEffect && game.data_subdir == "BioGame",
        "Original preservation requires a MELE game"
    );
    tracker.ensure_location_ready(&game.id).await?;
    tracker.ensure_no_mele_journal(&game.id).await?;
    let baseline = tracker
        .load_mele_baseline(&game.id)
        .await?
        .context("MELE has no restoration baseline; finish game setup before installing mods")?;
    let selected = select(&baseline, &targets)?;
    let records = tracker.mele_originals(&game.id).await?;
    for (path, identity) in records {
        ensure!(
            identity == baseline.sha256
                && baseline
                    .files
                    .binary_search_by(|file| file.relative.cmp(&path))
                    .is_ok(),
            "MELE original-file ownership does not match its restoration baseline"
        );
    }
    let root = store_root(&data_root, &baseline);
    let prepared = {
        let root = root.clone();
        let selected = selected.clone();
        let control = control.clone();
        let progress = progress.clone();
        tokio::task::spawn_blocking(move || {
            prepare(&game.path, &root, selected, &control, &progress)
        })
        .await
        .context("MELE original-file copy worker failed")??
    };
    control.check()?;
    tracker
        .record_mele_originals(&baseline, &prepared.files)
        .await?;
    control.check()?;
    let total = total_size(&prepared.files)?;
    progress(total, total);
    Ok(prepared)
}

fn store_root(data_root: &Path, baseline: &Baseline) -> PathBuf {
    data_root
        .join("mele-originals")
        .join(&baseline.game_id)
        .join(&baseline.sha256)
}

fn select(baseline: &Baseline, targets: &[String]) -> Result<Vec<BaselineFile>> {
    ensure!(
        !targets.is_empty() && targets.len() <= baseline.files.len(),
        "Select original files from the MELE restoration baseline"
    );
    let index: BTreeMap<_, _> = baseline
        .files
        .iter()
        .map(|file| (file.relative.to_lowercase(), file))
        .collect();
    let mut selected = BTreeMap::new();
    let mut folded = BTreeSet::new();
    for target in targets {
        relative(target)?;
        let lower = target.to_lowercase();
        ensure!(
            folded.insert(lower.clone()),
            "Duplicate MELE original-file target"
        );
        let file = index
            .get(&lower)
            .with_context(|| format!("'{target}' is not part of the MELE restoration baseline"))?;
        selected.insert(file.relative.clone(), (*file).clone());
    }
    Ok(selected.into_values().collect())
}

fn total_size(files: &[BaselineFile]) -> Result<u64> {
    files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.size)
            .context("MELE original-file size overflow")
    })
}

fn prepare(
    game: &Path,
    root: &Path,
    files: Vec<BaselineFile>,
    control: &Control,
    progress: &Progress,
) -> Result<Preserved> {
    directory(game)?;
    ensure!(
        root.is_absolute()
            && root
                .components()
                .all(|part| matches!(part, Component::RootDir | Component::Normal(_))),
        "MELE original storage requires an absolute folder without relative components"
    );
    ensure!(
        !root.starts_with(game) && !game.starts_with(root),
        "MELE originals must be stored outside the game folder"
    );
    control.check()?;
    create_directory(root)?;
    let total = total_size(&files)?;
    let mut completed = 0_u64;
    for file in &files {
        control.check()?;
        let destination = root.join(&file.relative);
        let parent = destination
            .parent()
            .context("MELE original has no parent folder")?;
        create_directory(parent)?;
        let mut advance = |count: u64| {
            completed += count;
            if total > 0 {
                progress(completed.min(total - 1), total);
            }
        };
        match fs::symlink_metadata(&destination) {
            Ok(_) => {
                let mut original = open(&destination, file, true)?;
                verify(&mut original, file, control, &mut advance)?;
                original.sync_all()?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                copy(
                    &game.join(&file.relative),
                    &destination,
                    file,
                    control,
                    &mut advance,
                )?;
            }
            Err(error) => {
                return Err(error).context(
                    "Cannot inspect preserved MELE originals; restore folder access and retry",
                );
            }
        }
        sync_directory(parent)?;
    }
    control.check()?;
    Ok(Preserved {
        root: root.to_path_buf(),
        files,
    })
}

fn create_directory(path: &Path) -> Result<()> {
    if path == Path::new("/") {
        ensure!(
            fs::symlink_metadata(path)?.is_dir(),
            "Invalid MELE original storage root"
        );
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        match fs::symlink_metadata(path) {
            Ok(_) => return directory(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Cannot access MELE original storage"),
        }
        create_directory(parent)?;
        match fs::create_dir(path) {
            Ok(()) => sync_directory(parent)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).context(
                    "Cannot create MELE original storage; check folder access and free space",
                );
            }
        }
    }
    if path.parent().is_none() {
        ensure!(path == Path::new("/"), "Invalid MELE original storage root");
        return Ok(());
    }
    directory(path)
}

fn sync_directory(path: &Path) -> Result<()> {
    if path != Path::new("/") {
        directory(path)?;
    }
    File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(path)?
        .sync_all()
        .context("Cannot flush MELE original storage to disk")
}

fn open(path: &Path, expected: &BaselineFile, independent: bool) -> Result<File> {
    directory(path.parent().context("MELE original file has no parent")?)?;
    let metadata = fs::symlink_metadata(path).with_context(|| {
        format!(
            "Cannot read MELE original '{}'; restore folder access or the original game file",
            expected.relative
        )
    })?;
    ensure!(
        metadata.is_file() && (!independent || metadata.nlink() == 1),
        "MELE originals cannot be links or special files: '{}'",
        expected.relative
    );
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file()
            && opened.len() == expected.size
            && (!independent || opened.nlink() == 1)
            && Stamp::from(&opened) == Stamp::from(&metadata),
        "MELE original '{}' changed; reconcile the installation or its backup before continuing",
        expected.relative
    );
    Ok(file)
}

fn verify(
    file: &mut File,
    expected: &BaselineFile,
    control: &Control,
    advance: &mut impl FnMut(u64),
) -> Result<()> {
    transfer(file, None, expected, control, advance)
}

fn transfer(
    source: &mut File,
    mut output: Option<&mut File>,
    expected: &BaselineFile,
    control: &Control,
    advance: &mut impl FnMut(u64),
) -> Result<()> {
    let before = Stamp::from(&source.metadata()?);
    let mut hash = Sha256::new();
    let mut remaining = expected.size;
    let mut buffer = [0_u8; 65536];
    while remaining > 0 {
        control.check()?;
        let length = remaining.min(buffer.len() as u64) as usize;
        let count = source.read(&mut buffer[..length])?;
        ensure!(count > 0, "MELE original was truncated during preservation");
        hash.update(&buffer[..count]);
        if let Some(output) = output.as_mut() {
            output
                .write_all(&buffer[..count])
                .context("Cannot preserve MELE originals; check free space and folder access")?;
        }
        remaining -= count as u64;
        advance(count as u64);
    }
    control.check()?;
    ensure!(
        source.read(&mut buffer[..1])? == 0 && Stamp::from(&source.metadata()?) == before,
        "MELE original changed during preservation; close the game and updater, then retry"
    );
    ensure!(
        format!("{:x}", hash.finalize()) == expected.sha256,
        "MELE original '{}' does not match the setup baseline; reconcile the installation or its backup before continuing",
        expected.relative
    );
    Ok(())
}

fn copy(
    source: &Path,
    destination: &Path,
    expected: &BaselineFile,
    control: &Control,
    advance: &mut impl FnMut(u64),
) -> Result<()> {
    let mut source = open(source, expected, false)?;
    let parent = destination
        .parent()
        .context("MELE original destination has no parent")?;
    let mut temporary = NamedTempFile::with_prefix_in(".deployd-original-", parent)?;
    transfer(
        &mut source,
        Some(temporary.as_file_mut()),
        expected,
        control,
        advance,
    )?;
    temporary.as_file_mut().seek(SeekFrom::Start(0))?;
    verify(temporary.as_file_mut(), expected, control, &mut |_| {})?;
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(0o400))?;
    temporary.as_file().sync_all()?;
    control.check()?;
    match temporary.persist_noclobber(destination) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut existing = open(destination, expected, true)?;
            verify(&mut existing, expected, control, &mut |_| {})?;
            existing.sync_all()?;
        }
        Err(error) => {
            return Err(error.error)
                .context("Cannot publish MELE original; check free space and folder access");
        }
    }
    let mut published = open(destination, expected, true)?;
    verify(&mut published, expected, control, &mut |_| {})?;
    published.sync_all()?;
    sync_directory(parent)
}

#[cfg(test)]
mod tests;
