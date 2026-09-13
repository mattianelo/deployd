use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Component, Path};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::core::location_recovery;
use crate::core::tracker::Tracker;
use crate::models::game::{Game, GameConfig, GameEngine};

pub(crate) mod progress;

use progress::{Callback, Phase, Progress, Reporter};

const MAX_ENTRIES: usize = 100_000;

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "Original preservation awaits the in-app package coordinator"
    )
)]
pub(crate) mod originals;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BaselineFile {
    pub(crate) relative: String,
    pub(crate) size: u64,
    pub(crate) modified: i64,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Baseline {
    pub(crate) game_id: String,
    pub(crate) files: Vec<BaselineFile>,
    pub(crate) sha256: String,
}

impl Baseline {
    pub(crate) fn validate(&self) -> Result<()> {
        let executable = executable(&self.game_id)?;
        ensure!(
            !self.files.is_empty() && self.files.len() <= MAX_ENTRIES,
            "Invalid MELE restoration baseline inventory"
        );
        let mut names = BTreeSet::new();
        let mut previous: Option<&str> = None;
        for file in &self.files {
            relative(&file.relative)?;
            ensure!(
                previous.is_none_or(|name| name < file.relative.as_str())
                    && names.insert(file.relative.to_lowercase()),
                "Duplicate, unordered, or case-colliding MELE baseline files"
            );
            previous = Some(&file.relative);
            ensure!(
                file.size <= i64::MAX as u64
                    && file.sha256.len() == 64
                    && file
                        .sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()),
                "Invalid MELE baseline file identity"
            );
        }
        ensure!(
            self.files.iter().any(|file| file.relative == executable),
            "MELE restoration baseline is missing its game executable"
        );
        ensure!(
            self.sha256 == digest(&self.game_id, &self.files),
            "MELE restoration baseline identity is damaged"
        );
        Ok(())
    }
}

pub(crate) async fn configure(
    tracker: &Tracker,
    configs: &[GameConfig],
    hidden_ids: &[String],
    progress: Callback,
) -> Result<()> {
    let mut games = Vec::new();
    for config in configs {
        if config.game.engine == GameEngine::MassEffect
            && tracker.load_mele_baseline(&config.game.id).await?.is_none()
        {
            tracker.ensure_location_ready(&config.game.id).await?;
            ensure!(
                tracker.list_mods(&config.game.id).await?.is_empty(),
                "MELE has managed mods but no restoration baseline; restore the original installation before continuing"
            );
            games.push(config.game.clone());
        }
    }
    let baselines = capture(games, progress.clone()).await?;
    progress(Progress {
        game: String::new(),
        index: 0,
        count: 0,
        phase: Phase::Saving,
    });
    if baselines.is_empty() {
        return tracker.persist_game_configs(configs, hidden_ids).await;
    }
    tracker
        .persist_game_configs_with_baselines(configs, hidden_ids, &baselines)
        .await
}

pub(crate) async fn ensure_baseline(tracker: &Tracker, game: &Game) -> Result<()> {
    ensure!(
        game.engine == GameEngine::MassEffect,
        "Only MELE games use this restoration baseline"
    );
    if tracker.load_mele_baseline(&game.id).await?.is_some() {
        return Ok(());
    }
    tracker.ensure_location_ready(&game.id).await?;
    ensure!(
        tracker.list_mods(&game.id).await?.is_empty(),
        "MELE has managed mods but no restoration baseline; restore the original installation before continuing"
    );
    for baseline in capture(vec![game.clone()], Arc::new(|_| {})).await? {
        tracker.save_mele_baseline(&baseline).await?;
    }
    Ok(())
}

struct Abandoned(Arc<AtomicBool>);

impl Drop for Abandoned {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

async fn capture(games: Vec<Game>, progress: Callback) -> Result<Vec<Baseline>> {
    if games.is_empty() {
        return Ok(Vec::new());
    }
    // Callers may already hold a location lease; waiting behind a writer would deadlock.
    let lease = location_recovery::activity_lock()
        .try_read_owned()
        .context("Folder access is being changed; retry MELE setup when it finishes")?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let _abandoned = Abandoned(cancelled.clone());
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        games
            .iter()
            .enumerate()
            .map(|(index, game)| {
                let mut reporter = Reporter {
                    callback: progress.clone(),
                    game: game.title.clone(),
                    index: index + 1,
                    count: games.len(),
                    last: None,
                };
                scan_with_progress(game, &cancelled, &mut |phase| reporter.emit(phase))
            })
            .collect()
    })
    .await
    .context("MELE restoration baseline scan stopped unexpectedly")?
}

fn executable(game_id: &str) -> Result<&'static str> {
    match game_id {
        "mass-effect-le1" => Ok("Binaries/Win64/MassEffect1.exe"),
        "mass-effect-le2" => Ok("Binaries/Win64/MassEffect2.exe"),
        "mass-effect-le3" => Ok("Binaries/Win64/MassEffect3.exe"),
        _ => anyhow::bail!("Unknown MELE restoration baseline target '{game_id}'"),
    }
}

fn check(cancelled: &AtomicBool) -> Result<()> {
    ensure!(
        !cancelled.load(Ordering::Acquire),
        "MELE restoration baseline scan was cancelled"
    );
    Ok(())
}

pub(super) fn relative(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 4096
            && !value.contains(['\\', ':'])
            && !value.chars().any(char::is_control)
            && value
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..") && !part.ends_with(['.', ' '])),
        "Invalid MELE restoration baseline path"
    );
    Ok(())
}

pub(super) fn directory(root: &Path) -> Result<()> {
    ensure!(
        root.is_absolute() && root.parent().is_some(),
        "Select an absolute MELE game folder"
    );
    ensure!(
        root.components()
            .all(|part| matches!(part, Component::RootDir | Component::Normal(_))),
        "MELE folders cannot contain relative components"
    );
    for ancestor in root.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)
            .context("Cannot access the MELE game folder; restore folder access and retry")?;
        ensure!(
            metadata.is_dir(),
            "MELE restoration folders cannot contain symbolic links or special files"
        );
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: i64,
    modified_ns: i64,
    changed: i64,
    changed_ns: i64,
    device: u64,
    inode: u64,
    directory: bool,
}

impl From<&Metadata> for Stamp {
    fn from(metadata: &Metadata) -> Self {
        Self {
            size: metadata.len(),
            modified: metadata.mtime(),
            modified_ns: metadata.mtime_nsec(),
            changed: metadata.ctime(),
            changed_ns: metadata.ctime_nsec(),
            device: metadata.dev(),
            inode: metadata.ino(),
            directory: metadata.is_dir(),
        }
    }
}

fn inventory(
    root: &Path,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(usize),
) -> Result<BTreeMap<String, Stamp>> {
    progress(0);
    directory(root)?;
    let mut entries = BTreeMap::new();
    let mut folded = BTreeSet::new();
    for entry in WalkDir::new(root).follow_links(false).min_depth(1) {
        check(cancelled)?;
        let entry =
            entry.context("Cannot read the complete MELE game folder; restore access and retry")?;
        let path = entry
            .path()
            .strip_prefix(root)?
            .to_str()
            .context("MELE game paths must be UTF-8")?;
        relative(path)?;
        ensure!(
            entries.len() < MAX_ENTRIES,
            "MELE restoration baseline has too many entries"
        );
        ensure!(
            folded.insert(path.to_lowercase()),
            "Case-colliding MELE game entries: '{path}'"
        );
        let metadata = fs::symlink_metadata(entry.path())?;
        ensure!(
            metadata.is_dir() || metadata.is_file(),
            "MELE restoration baseline cannot include a link or special file: '{path}'"
        );
        entries.insert(path.to_string(), Stamp::from(&metadata));
        progress(entries.len());
    }
    Ok(entries)
}

#[cfg(test)]
fn scan(game: &Game, cancelled: &AtomicBool) -> Result<Baseline> {
    scan_with_progress(game, cancelled, &mut |_| {})
}

fn scan_with_progress(
    game: &Game,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(Phase),
) -> Result<Baseline> {
    ensure!(
        game.engine == GameEngine::MassEffect && game.data_subdir == "BioGame",
        "Invalid MELE restoration baseline game configuration"
    );
    let executable = executable(&game.id)?;
    let before = inventory(&game.path, cancelled, &mut |entries| {
        progress(Phase::Discovering(entries))
    })?;
    ensure!(
        before.get("BioGame").is_some_and(|entry| entry.directory)
            && before.get(executable).is_some_and(|entry| !entry.directory),
        "MELE setup requires BioGame and the selected game's executable"
    );
    let total = before
        .values()
        .filter(|entry| !entry.directory)
        .try_fold(0u64, |sum, entry| {
            sum.checked_add(entry.size)
                .context("MELE baseline size exceeds supported limits")
        })?;
    let mut bytes = 0;
    progress(Phase::Hashing { bytes, total });
    let mut files = Vec::new();
    for (relative, expected) in &before {
        check(cancelled)?;
        if expected.directory {
            continue;
        }
        let path = game.path.join(relative);
        directory(path.parent().context("MELE baseline file has no parent")?)?;
        let mut file = File::options()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
            .with_context(|| {
                format!(
                    "Cannot read MELE baseline file '{relative}'; restore folder access and retry"
                )
            })?;
        let opened = file.metadata()?;
        ensure!(
            opened.is_file() && Stamp::from(&opened) == *expected,
            "MELE game files changed during baseline capture; close the game and updater, then retry"
        );
        let mut remaining = expected.size;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        while remaining > 0 {
            check(cancelled)?;
            let length = remaining.min(buffer.len() as u64) as usize;
            let count = file.read(&mut buffer[..length])?;
            ensure!(count > 0, "MELE baseline file was truncated during capture");
            hash.update(&buffer[..count]);
            remaining -= count as u64;
            bytes += count as u64;
            progress(Phase::Hashing { bytes, total });
        }
        ensure!(
            file.read(&mut buffer[..1])? == 0 && Stamp::from(&file.metadata()?) == *expected,
            "MELE game files changed during baseline capture; close the game and updater, then retry"
        );
        files.push(BaselineFile {
            relative: relative.clone(),
            size: expected.size,
            modified: expected.modified,
            sha256: format!("{:x}", hash.finalize()),
        });
    }
    ensure!(
        inventory(&game.path, cancelled, &mut |entries| progress(
            Phase::Checking(entries)
        ))? == before,
        "MELE game files changed during baseline capture; close the game and updater, then retry"
    );
    let baseline = Baseline {
        game_id: game.id.clone(),
        sha256: digest(&game.id, &files),
        files,
    };
    baseline.validate()?;
    Ok(baseline)
}

fn digest(game_id: &str, files: &[BaselineFile]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"deployd-mele-baseline-v1\0");
    hash.update(game_id.as_bytes());
    for file in files {
        hash.update((file.relative.len() as u64).to_le_bytes());
        hash.update(file.relative.as_bytes());
        hash.update(file.size.to_le_bytes());
        hash.update(file.sha256.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

#[cfg(test)]
mod tests;
