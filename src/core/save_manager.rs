use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::core::game::detect_save_dir;
use crate::models::game::Game;
use crate::models::profile::SaveMode;
use crate::utils::{paths, snap};

const SAVE_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_AUTOMATIC_BACKUP_CAP_BYTES: u64 = 5 * 1024 * 1024 * 1024;

pub async fn configured_backup_cap_bytes(tracker: &crate::core::tracker::Tracker) -> u64 {
    tracker
        .get_setting("save_backup_cap_gib")
        .await
        .ok()
        .flatten()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| (1..=100).contains(value))
        .map(|gib| gib * 1024 * 1024 * 1024)
        .unwrap_or(DEFAULT_AUTOMATIC_BACKUP_CAP_BYTES)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SaveSetId {
    Global { game_id: String },
    Profile { game_id: String, profile_id: String },
}

impl SaveSetId {
    pub fn for_profile(game_id: &str, profile_id: &str, mode: &SaveMode) -> Self {
        match mode {
            SaveMode::Global => Self::Global {
                game_id: game_id.to_string(),
            },
            SaveMode::ProfileSpecific => Self::Profile {
                game_id: game_id.to_string(),
                profile_id: profile_id.to_string(),
            },
        }
    }

    pub fn game_id(&self) -> &str {
        match self {
            Self::Global { game_id } | Self::Profile { game_id, .. } => game_id,
        }
    }

    pub fn profile_id(&self) -> Option<&str> {
        match self {
            Self::Global { .. } => None,
            Self::Profile { profile_id, .. } => Some(profile_id),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::Global { .. } => "Global saves".to_string(),
            Self::Profile { profile_id, .. } => format!("Profile {profile_id}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SaveFileManifest {
    pub path: String,
    pub size: u64,
    pub modified_unix_seconds: i64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveBankManifest {
    pub schema_version: u32,
    pub save_set: SaveSetId,
    pub captured_at: String,
    pub files: Vec<SaveFileManifest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackupTrigger {
    ProfileSwitch,
    ModeChange,
    ManualSync,
    Manual,
    Restore,
    Clone,
}

impl BackupTrigger {
    fn is_automatic(self) -> bool {
        self != Self::Manual
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ProfileSwitch => "Profile switch",
            Self::ModeChange => "Save mode change",
            Self::ManualSync => "Manual sync",
            Self::Manual => "Manual backup",
            Self::Restore => "Before restore",
            Self::Clone => "Before profile clone",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveBackupManifest {
    pub schema_version: u32,
    pub backup_id: String,
    pub payload_kind: String,
    pub save_set: SaveSetId,
    pub label: Option<String>,
    pub trigger: BackupTrigger,
    pub created_at: String,
    pub files: Vec<SaveFileManifest>,
}

impl SaveBackupManifest {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn size_bytes(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveSyncResult {
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
}

impl SaveSyncResult {
    pub fn has_changes(&self) -> bool {
        self.added > 0 || self.modified > 0 || self.removed > 0
    }

    pub fn to_toast(&self) -> String {
        if !self.has_changes() {
            return "Saves already up to date".to_string();
        }
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("{} new", self.added));
        }
        if self.modified > 0 {
            parts.push(format!("{} updated", self.modified));
        }
        if self.removed > 0 {
            parts.push(format!("{} removed", self.removed));
        }
        format!("Saves synced: {}", parts.join(", "))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct TransitionJournal {
    source: SaveSetId,
    target: SaveSetId,
    live_path: PathBuf,
    rollback_path: PathBuf,
    phase: TransitionPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransitionPhase {
    LivePending,
    LiveRestored,
}

pub struct SaveTransition {
    live_path: PathBuf,
    rollback_path: Option<PathBuf>,
    journal_path: Option<PathBuf>,
    pub sync_result: Option<SaveSyncResult>,
}

impl SaveTransition {
    fn no_op() -> Self {
        Self {
            live_path: PathBuf::new(),
            rollback_path: None,
            journal_path: None,
            sync_result: None,
        }
    }

    pub async fn commit(mut self) -> Result<Option<SaveSyncResult>> {
        if let Some(rollback) = self.rollback_path.take()
            && rollback.exists()
            && let Err(error) = tokio::fs::remove_dir_all(&rollback).await
        {
            crate::dlog!(
                "[saves] committed transition left rollback directory {}: {error}",
                rollback.display()
            );
            return Ok(self.sync_result.take());
        }
        if let Some(journal) = self.journal_path.take()
            && journal.exists()
            && let Err(error) = tokio::fs::remove_file(&journal).await
        {
            crate::dlog!(
                "[saves] committed transition left journal {}: {error}",
                journal.display()
            );
        }
        Ok(self.sync_result.take())
    }

    pub async fn rollback(mut self) -> Result<()> {
        let Some(rollback) = self.rollback_path.take() else {
            return Ok(());
        };
        if self.live_path.exists() {
            tokio::fs::remove_dir_all(&self.live_path)
                .await
                .with_context(|| {
                    format!(
                        "Failed to remove incomplete save restore {}",
                        self.live_path.display()
                    )
                })?;
        }
        tokio::fs::rename(&rollback, &self.live_path)
            .await
            .context("Failed to put the original game saves back after an error")?;
        if let Some(journal) = self.journal_path.take()
            && journal.exists()
        {
            tokio::fs::remove_file(journal).await?;
        }
        Ok(())
    }
}

fn game_root(game_id: &str) -> Result<PathBuf> {
    validate_component("game ID", game_id)?;
    Ok(paths::saves_root()?.join(game_id))
}

fn bank_root(save_set: &SaveSetId) -> Result<PathBuf> {
    bank_root_in(&paths::saves_root()?, save_set)
}

fn bank_root_in(saves_root: &Path, save_set: &SaveSetId) -> Result<PathBuf> {
    validate_component("game ID", save_set.game_id())?;
    let game = saves_root.join(save_set.game_id()).join("sets");
    Ok(match save_set {
        SaveSetId::Global { .. } => game.join("global"),
        SaveSetId::Profile { profile_id, .. } => {
            validate_component("profile ID", profile_id)?;
            game.join("profiles").join(profile_id)
        }
    })
}

fn backup_root(game_id: &str, backup_id: &str) -> Result<PathBuf> {
    validate_component("game ID", game_id)?;
    validate_component("backup ID", backup_id)?;
    Ok(paths::deployd_data_dir()?
        .join("backups")
        .join(game_id)
        .join(backup_id))
}

fn validate_component(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || matches!(value, "." | "..")
        || value.contains('/')
        || value.contains('\\')
    {
        bail!("Invalid {label}");
    }
    Ok(())
}

fn bank_data(root: &Path) -> PathBuf {
    root.join("data")
}

fn bank_manifest(root: &Path) -> PathBuf {
    root.join("manifest.json")
}

pub fn last_save_sync_time(game_id: &str, profile_id: &str) -> Option<std::time::SystemTime> {
    let set = SaveSetId::Profile {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    };
    std::fs::metadata(bank_manifest(&bank_root(&set).ok()?))
        .ok()?
        .modified()
        .ok()
}

fn validate_live_save_access(game: &Game) -> Result<PathBuf> {
    validate_live_save_access_with(game, |prefix| {
        snap::validate_selected_folder(prefix, snap::SelectedFolderKind::WinePrefix)
    })
}

fn validate_live_save_access_with<E>(
    game: &Game,
    validate: impl FnOnce(&Path) -> std::result::Result<(), E>,
) -> Result<PathBuf>
where
    E: std::fmt::Display,
{
    let prefix = game
        .wine_prefix
        .as_deref()
        .ok_or_else(|| anyhow!("No Wine prefix is configured for {}", game.title))?;
    validate(prefix).map_err(|error| {
        anyhow!(
            "Cannot safely access {} saves. Open Settings → Manage Games and reselect the Wine prefix. {error}",
            game.title
        )
    })?;
    detect_save_dir(game).ok_or_else(|| anyhow!("Could not locate {}'s save directory", game.title))
}

async fn hash_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .with_context(|| format!("Failed to open save file {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn scan_tree(root: &Path) -> Result<Vec<SaveFileManifest>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), PathBuf::new())];
    while let Some((absolute, relative)) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&absolute)
            .await
            .with_context(|| format!("Failed to read save directory {}", absolute.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let child_absolute = entry.path();
            let child_relative = relative.join(entry.file_name());
            if file_type.is_symlink() {
                bail!(
                    "Save management cannot safely copy the symbolic link {}",
                    child_absolute.display()
                );
            }
            if file_type.is_dir() {
                stack.push((child_absolute, child_relative));
                continue;
            }
            if !file_type.is_file() {
                bail!(
                    "Save management cannot safely copy the special file {}",
                    child_absolute.display()
                );
            }
            let metadata = entry.metadata().await?;
            let modified_unix_seconds = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or_default();
            files.push(SaveFileManifest {
                path: child_relative.to_string_lossy().replace('\\', "/"),
                size: metadata.len(),
                modified_unix_seconds,
                sha256: hash_file(&child_absolute).await?,
            });
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

async fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    tokio::fs::create_dir_all(destination).await?;
    if !source.exists() {
        return Ok(());
    }
    let mut stack = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&from)
            .await
            .with_context(|| format!("Failed to read save directory {}", from.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let source_path = entry.path();
            let destination_path = to.join(entry.file_name());
            if file_type.is_symlink() || (!file_type.is_dir() && !file_type.is_file()) {
                bail!("Unsupported save entry {}", source_path.display());
            }
            if file_type.is_dir() {
                tokio::fs::create_dir_all(&destination_path).await?;
                stack.push((source_path, destination_path));
                continue;
            }
            tokio::fs::copy(&source_path, &destination_path)
                .await
                .with_context(|| format!("Failed to copy save file {}", source_path.display()))?;
            if let Ok(metadata) = tokio::fs::metadata(&source_path).await
                && let Ok(modified) = metadata.modified()
                && let Ok(file) = std::fs::File::options().write(true).open(&destination_path)
            {
                let _ = file.set_times(std::fs::FileTimes::new().set_modified(modified));
            }
        }
    }
    Ok(())
}

async fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Invalid manifest path"))?;
    tokio::fs::create_dir_all(parent).await?;
    let temporary = parent.join(format!(".manifest-{}.tmp", uuid::Uuid::new_v4()));
    tokio::fs::write(&temporary, serde_json::to_vec_pretty(value)?)
        .await
        .with_context(|| format!("Failed to write manifest {}", temporary.display()))?;
    tokio::fs::rename(&temporary, path)
        .await
        .with_context(|| format!("Failed to commit manifest {}", path.display()))
}

async fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("Failed to read manifest {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid save manifest {}", path.display()))
}

async fn verify_tree(root: &Path, expected: &[SaveFileManifest]) -> Result<()> {
    let actual = scan_tree(root).await?;
    if actual != expected {
        bail!("Copied save files failed integrity verification");
    }
    Ok(())
}

async fn replace_bank_from_dir(save_set: &SaveSetId, source: &Path) -> Result<SaveBankManifest> {
    let root = bank_root(save_set)?;
    let parent = root
        .parent()
        .ok_or_else(|| anyhow!("Invalid save bank path"))?;
    tokio::fs::create_dir_all(parent).await?;
    let staging = parent.join(format!(".bank-{}.tmp", uuid::Uuid::new_v4()));
    let data = bank_data(&staging);
    let staged = async {
        let files = scan_tree(source).await?;
        copy_tree(source, &data).await?;
        verify_tree(&data, &files).await?;
        let manifest = SaveBankManifest {
            schema_version: SAVE_SCHEMA_VERSION,
            save_set: save_set.clone(),
            captured_at: Utc::now().to_rfc3339(),
            files,
        };
        write_json(&bank_manifest(&staging), &manifest).await?;
        Result::<SaveBankManifest>::Ok(manifest)
    };
    let manifest = match staged.await {
        Ok(manifest) => manifest,
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(error);
        }
    };
    let old = parent.join(format!(".bank-{}.old", uuid::Uuid::new_v4()));
    if root.exists() {
        tokio::fs::rename(&root, &old).await?;
    }
    if let Err(error) = tokio::fs::rename(&staging, &root).await {
        if old.exists() {
            let _ = tokio::fs::rename(&old, &root).await;
        }
        return Err(error).context("Failed to commit save bank");
    }
    if old.exists() {
        tokio::fs::remove_dir_all(old).await?;
    }
    Ok(manifest)
}

async fn load_bank(save_set: &SaveSetId) -> Result<SaveBankManifest> {
    let root = bank_root(save_set)?;
    if !bank_manifest(&root).exists() {
        bail!(
            "{} has no initialized save state. Initialize it from the current live saves, change its save mode, or cancel the switch.",
            save_set.display_name()
        );
    }
    let manifest: SaveBankManifest = read_json(&bank_manifest(&root)).await?;
    if manifest.schema_version != SAVE_SCHEMA_VERSION || manifest.save_set != *save_set {
        bail!("The target save bank has incompatible metadata");
    }
    verify_tree(&bank_data(&root), &manifest.files).await?;
    Ok(manifest)
}

async fn migrate_legacy_profile_bank(save_set: &SaveSetId) -> Result<()> {
    let SaveSetId::Profile {
        game_id,
        profile_id,
    } = save_set
    else {
        return Ok(());
    };
    let root = bank_root(save_set)?;
    if bank_manifest(&root).exists() {
        return Ok(());
    }
    let legacy = paths::saves_root()?.join(game_id).join(profile_id);
    if !legacy.exists() {
        return Ok(());
    }
    replace_bank_from_dir(save_set, &legacy).await?;
    tokio::fs::remove_dir_all(legacy)
        .await
        .context("Failed to remove migrated legacy save snapshot")
}

async fn create_backup_from_dir(
    save_set: &SaveSetId,
    source: &Path,
    trigger: BackupTrigger,
    label: Option<String>,
) -> Result<SaveBackupManifest> {
    let backup_id = uuid::Uuid::new_v4().to_string();
    let root = backup_root(save_set.game_id(), &backup_id)?;
    let backup = async {
        let files = scan_tree(source).await?;
        copy_tree(source, &root.join("data")).await?;
        verify_tree(&root.join("data"), &files).await?;
        let manifest = SaveBackupManifest {
            schema_version: SAVE_SCHEMA_VERSION,
            backup_id,
            payload_kind: "save_files".to_string(),
            save_set: save_set.clone(),
            label,
            trigger,
            created_at: Utc::now().to_rfc3339(),
            files,
        };
        write_json(&root.join("manifest.json"), &manifest).await?;
        Result::<SaveBackupManifest>::Ok(manifest)
    };
    match backup.await {
        Ok(manifest) => Ok(manifest),
        Err(error) => {
            let _ = tokio::fs::remove_dir_all(root).await;
            Err(error)
        }
    }
}

pub async fn list_backups(game_id: &str) -> Result<Vec<SaveBackupManifest>> {
    validate_component("game ID", game_id)?;
    let root = paths::deployd_data_dir()?.join("backups").join(game_id);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut backups: Vec<SaveBackupManifest> = Vec::new();
    let mut entries = tokio::fs::read_dir(&root).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        match read_json(&entry.path().join("manifest.json")).await {
            Ok(manifest) => backups.push(manifest),
            Err(error) => crate::dlog!("[saves] ignoring invalid backup: {error}"),
        }
    }
    backups.sort_by_key(|backup| std::cmp::Reverse(backup_created_at(backup)));
    Ok(backups)
}

fn backup_created_at(backup: &SaveBackupManifest) -> i64 {
    chrono::DateTime::parse_from_rfc3339(&backup.created_at)
        .map(|timestamp| timestamp.timestamp_millis())
        .unwrap_or_default()
}

pub async fn create_manual_backup(
    game: &Game,
    save_set: &SaveSetId,
    label: String,
) -> Result<SaveBackupManifest> {
    let live = validate_live_save_access(game)?;
    create_backup_from_dir(
        save_set,
        &live,
        BackupTrigger::Manual,
        Some(label.trim().to_string()).filter(|label| !label.is_empty()),
    )
    .await
}

pub async fn delete_backup(game_id: &str, backup_id: &str) -> Result<()> {
    let root = backup_root(game_id, backup_id)?;
    let manifest: SaveBackupManifest = read_json(&root.join("manifest.json")).await?;
    if manifest.save_set.game_id() != game_id || manifest.backup_id != backup_id {
        bail!("Backup metadata does not match the requested backup");
    }
    tokio::fs::remove_dir_all(root)
        .await
        .context("Failed to delete save backup")
}

pub async fn prune_automatic_backups(game_id: &str, cap_bytes: u64) -> Result<()> {
    let automatic: Vec<_> = list_backups(game_id).await?;
    for backup_id in automatic_backup_prune_candidates(&automatic, cap_bytes) {
        delete_backup(game_id, &backup_id).await?;
    }
    Ok(())
}

fn automatic_backup_prune_candidates(
    backups: &[SaveBackupManifest],
    cap_bytes: u64,
) -> Vec<String> {
    let mut automatic: Vec<_> = backups
        .iter()
        .filter(|backup| backup.trigger.is_automatic())
        .collect();
    automatic.sort_by_key(|backup| backup_created_at(backup));
    let mut total: u64 = automatic.iter().map(|backup| backup.size_bytes()).sum();
    let mut prune = Vec::new();
    while automatic.len() > 1 && total > cap_bytes {
        let oldest = automatic.remove(0);
        total = total.saturating_sub(oldest.size_bytes());
        prune.push(oldest.backup_id.clone());
    }
    prune
}

async fn diff_dir_against_bank(source: &Path, save_set: &SaveSetId) -> Result<SaveSyncResult> {
    let live = scan_tree(source).await?;
    let stored = match load_bank(save_set).await {
        Ok(manifest) => manifest.files,
        Err(_) => Vec::new(),
    };
    Ok(diff_files(&live, &stored))
}

fn diff_files(live: &[SaveFileManifest], stored: &[SaveFileManifest]) -> SaveSyncResult {
    let live: HashMap<_, _> = live.iter().map(|file| (file.path.as_str(), file)).collect();
    let stored: HashMap<_, _> = stored
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect();
    let mut result = SaveSyncResult::default();
    for (path, live_file) in &live {
        match stored.get(path) {
            None => result.added += 1,
            Some(stored_file) if stored_file.sha256 != live_file.sha256 => result.modified += 1,
            Some(_) => {}
        }
    }
    result.removed = stored
        .keys()
        .filter(|path| !live.contains_key(*path))
        .count();
    result
}

pub async fn initialize_save_set(game: &Game, save_set: &SaveSetId) -> Result<()> {
    let live = validate_live_save_access(game)?;
    replace_bank_from_dir(save_set, &live).await?;
    Ok(())
}

pub async fn sync_save_set(
    game: &Game,
    save_set: &SaveSetId,
    cap_bytes: u64,
) -> Result<SaveSyncResult> {
    capture_save_set(game, save_set, BackupTrigger::ManualSync, cap_bytes).await
}

pub async fn capture_save_set(
    game: &Game,
    save_set: &SaveSetId,
    trigger: BackupTrigger,
    cap_bytes: u64,
) -> Result<SaveSyncResult> {
    let live = validate_live_save_access(game)?;
    migrate_legacy_profile_bank(save_set).await?;
    let diff = diff_dir_against_bank(&live, save_set).await?;
    let bank = bank_root(save_set)?;
    let recovery_source = if bank_manifest(&bank).exists() {
        bank_data(&bank)
    } else {
        live.clone()
    };
    create_backup_from_dir(save_set, &recovery_source, trigger, None).await?;
    replace_bank_from_dir(save_set, &live).await?;
    prune_automatic_backups(save_set.game_id(), cap_bytes).await?;
    Ok(diff)
}

pub async fn prepare_transition(
    game: &Game,
    source: &SaveSetId,
    target: &SaveSetId,
    trigger: BackupTrigger,
    cap_bytes: u64,
) -> Result<SaveTransition> {
    if source == target {
        return Ok(SaveTransition::no_op());
    }
    let live = validate_live_save_access(game)?;
    migrate_legacy_profile_bank(source).await?;
    migrate_legacy_profile_bank(target).await?;
    let target_manifest = load_bank(target).await?;
    let diff = diff_dir_against_bank(&live, source).await?;
    create_backup_from_dir(source, &live, trigger, None).await?;
    replace_bank_from_dir(source, &live).await?;
    prune_automatic_backups(source.game_id(), cap_bytes).await?;

    let parent = live
        .parent()
        .ok_or_else(|| anyhow!("The game save directory has no parent"))?;
    tokio::fs::create_dir_all(parent).await?;
    let rollback = parent.join(format!(".deployd-save-rollback-{}", uuid::Uuid::new_v4()));
    let journal_path = game_root(game.id.as_str())?.join("transition.json");
    let journal = TransitionJournal {
        source: source.clone(),
        target: target.clone(),
        live_path: live.clone(),
        rollback_path: rollback.clone(),
        phase: TransitionPhase::LivePending,
    };
    write_json(&journal_path, &journal).await?;

    if live.exists() {
        tokio::fs::rename(&live, &rollback)
            .await
            .context("Failed to preserve the live saves before switching")?;
    } else {
        tokio::fs::create_dir_all(&rollback).await?;
    }
    let restore = async {
        copy_tree(&bank_data(&bank_root(target)?), &live).await?;
        verify_tree(&live, &target_manifest.files).await
    }
    .await;
    if let Err(error) = restore {
        if live.exists() {
            let _ = tokio::fs::remove_dir_all(&live).await;
        }
        let _ = tokio::fs::rename(&rollback, &live).await;
        let _ = tokio::fs::remove_file(&journal_path).await;
        return Err(error)
            .context("Failed to restore the target save bank; original saves restored");
    }
    if let Err(error) = write_json(
        &journal_path,
        &TransitionJournal {
            phase: TransitionPhase::LiveRestored,
            ..journal
        },
    )
    .await
    {
        let _ = tokio::fs::remove_dir_all(&live).await;
        let _ = tokio::fs::rename(&rollback, &live).await;
        let _ = tokio::fs::remove_file(&journal_path).await;
        return Err(error)
            .context("Failed to commit the save transition journal; original saves restored");
    }
    Ok(SaveTransition {
        live_path: live,
        rollback_path: Some(rollback),
        journal_path: Some(journal_path),
        sync_result: Some(diff),
    })
}

pub async fn recover_interrupted_transition(game: &Game, active: &SaveSetId) -> Result<()> {
    let journal_path = game_root(&game.id)?.join("transition.json");
    if !journal_path.exists() {
        return Ok(());
    }
    let live = validate_live_save_access(game)?;
    recover_transition_journal(&journal_path, &live, active).await
}

async fn recover_transition_journal(
    journal_path: &Path,
    live: &Path,
    active: &SaveSetId,
) -> Result<()> {
    let journal: TransitionJournal = read_json(journal_path).await?;
    if journal.live_path.as_path() != live {
        bail!("The interrupted save transition targets an unexpected live save directory");
    }
    if journal.target == *active && journal.phase == TransitionPhase::LiveRestored {
        if journal.rollback_path.exists() {
            tokio::fs::remove_dir_all(&journal.rollback_path).await?;
        }
    } else if journal.rollback_path.exists() {
        if journal.live_path.exists() {
            tokio::fs::remove_dir_all(&journal.live_path).await?;
        }
        tokio::fs::rename(&journal.rollback_path, &journal.live_path).await?;
    }
    tokio::fs::remove_file(journal_path).await?;
    Ok(())
}

pub async fn clone_profile_bank(game_id: &str, source_id: &str, target_id: &str) -> Result<()> {
    let source = SaveSetId::Profile {
        game_id: game_id.to_string(),
        profile_id: source_id.to_string(),
    };
    let target = SaveSetId::Profile {
        game_id: game_id.to_string(),
        profile_id: target_id.to_string(),
    };
    migrate_legacy_profile_bank(&source).await?;
    let source_manifest = load_bank(&source).await?;
    let source_root = bank_root(&source)?;
    let target_manifest = SaveBankManifest {
        save_set: target.clone(),
        captured_at: Utc::now().to_rfc3339(),
        ..source_manifest
    };
    let target_root = bank_root(&target)?;
    copy_tree(&bank_data(&source_root), &bank_data(&target_root)).await?;
    verify_tree(&bank_data(&target_root), &target_manifest.files).await?;
    write_json(&bank_manifest(&target_root), &target_manifest).await
}

pub async fn delete_profile_save_data(game_id: &str, profile_id: &str) -> Result<()> {
    let set = SaveSetId::Profile {
        game_id: game_id.to_string(),
        profile_id: profile_id.to_string(),
    };
    let bank = bank_root(&set)?;
    if bank.exists() {
        tokio::fs::remove_dir_all(bank).await?;
    }
    let legacy = paths::saves_root()?.join(game_id).join(profile_id);
    if legacy.exists() {
        tokio::fs::remove_dir_all(legacy).await?;
    }
    for backup in list_backups(game_id).await? {
        if backup.save_set.profile_id() == Some(profile_id) {
            delete_backup(game_id, &backup.backup_id).await?;
        }
    }
    Ok(())
}

pub async fn restore_backup(
    game: &Game,
    backup_id: &str,
    active_set: &SaveSetId,
    cap_bytes: u64,
) -> Result<()> {
    let root = backup_root(&game.id, backup_id)?;
    let backup: SaveBackupManifest = read_json(&root.join("manifest.json")).await?;
    verify_tree(&root.join("data"), &backup.files).await?;
    if &backup.save_set == active_set {
        let live = validate_live_save_access(game)?;
        create_backup_from_dir(active_set, &live, BackupTrigger::Restore, None).await?;
        let rollback = live
            .parent()
            .ok_or_else(|| anyhow!("The game save directory has no parent"))?
            .join(format!(".deployd-save-rollback-{}", uuid::Uuid::new_v4()));
        let journal_path = game_root(&game.id)?.join("transition.json");
        let mut journal = TransitionJournal {
            source: active_set.clone(),
            target: active_set.clone(),
            live_path: live.clone(),
            rollback_path: rollback.clone(),
            phase: TransitionPhase::LivePending,
        };
        write_json(&journal_path, &journal).await?;
        if live.exists() {
            tokio::fs::rename(&live, &rollback).await?;
        } else {
            tokio::fs::create_dir_all(&rollback).await?;
        }
        let restore_result = async {
            copy_tree(&root.join("data"), &live).await?;
            verify_tree(&live, &backup.files).await?;
            replace_bank_from_dir(active_set, &root.join("data")).await?;
            Result::<()>::Ok(())
        }
        .await;
        if let Err(error) = restore_result {
            let _ = tokio::fs::remove_dir_all(&live).await;
            let _ = tokio::fs::rename(&rollback, &live).await;
            let _ = tokio::fs::remove_file(&journal_path).await;
            return Err(error).context("Failed to restore backup; original saves restored");
        }
        journal.phase = TransitionPhase::LiveRestored;
        if let Err(error) = write_json(&journal_path, &journal).await {
            let _ = tokio::fs::remove_dir_all(&live).await;
            let _ = tokio::fs::rename(&rollback, &live).await;
            let _ = tokio::fs::remove_file(&journal_path).await;
            return Err(error)
                .context("Failed to commit the restore journal; original saves restored");
        }
        tokio::fs::remove_dir_all(rollback).await?;
        tokio::fs::remove_file(journal_path).await?;
    } else {
        let current_bank = bank_root(&backup.save_set)?;
        if bank_manifest(&current_bank).exists() {
            create_backup_from_dir(
                &backup.save_set,
                &bank_data(&current_bank),
                BackupTrigger::Restore,
                None,
            )
            .await?;
        }
        replace_bank_from_dir(&backup.save_set, &root.join("data")).await?;
    }
    prune_automatic_backups(&game.id, cap_bytes).await
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    async fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
        tokio::fs::create_dir_all(path.parent().unwrap()).await?;
        tokio::fs::write(path, bytes).await?;
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn distinguishes_initialized_empty_bank_from_missing_bank() -> Result<()> {
        let temp = tempdir()?;
        let source = temp.path().join("empty");
        tokio::fs::create_dir_all(&source).await?;
        let set = SaveSetId::Global {
            game_id: "game".to_string(),
        };
        let root = bank_root_in(temp.path(), &set)?;
        let files = scan_tree(&source).await?;
        let manifest = SaveBankManifest {
            schema_version: SAVE_SCHEMA_VERSION,
            save_set: set,
            captured_at: Utc::now().to_rfc3339(),
            files,
        };
        tokio::fs::create_dir_all(bank_data(&root)).await?;
        write_json(&bank_manifest(&root), &manifest).await?;

        assert!(bank_manifest(&root).exists());
        assert_eq!(manifest.files.len(), 0);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn hashes_content_instead_of_trusting_size() -> Result<()> {
        let temp = tempdir()?;
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        write_file(&first.join("save.dat"), b"aaaa").await?;
        write_file(&second.join("save.dat"), b"bbbb").await?;

        let first_files = scan_tree(&first).await?;
        let second_files = scan_tree(&second).await?;
        assert_eq!(first_files[0].size, second_files[0].size);
        assert_ne!(first_files[0].sha256, second_files[0].sha256);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn rejects_symbolic_links_before_copying() -> Result<()> {
        use std::os::unix::fs::symlink;

        let temp = tempdir()?;
        let source = temp.path().join("source");
        tokio::fs::create_dir_all(&source).await?;
        write_file(&temp.path().join("outside"), b"save").await?;
        symlink(temp.path().join("outside"), source.join("save-link"))?;

        let error = scan_tree(&source).await.unwrap_err().to_string();
        assert!(error.contains("symbolic link"));
        Ok(())
    }

    async fn capture_test_bank(root: &Path, save_set: &SaveSetId, live: &Path) -> Result<()> {
        let files = scan_tree(live).await?;
        copy_tree(live, &bank_data(root)).await?;
        verify_tree(&bank_data(root), &files).await?;
        write_json(
            &bank_manifest(root),
            &SaveBankManifest {
                schema_version: SAVE_SCHEMA_VERSION,
                save_set: save_set.clone(),
                captured_at: Utc::now().to_rfc3339(),
                files,
            },
        )
        .await
    }

    async fn restore_test_bank(root: &Path, live: &Path) -> Result<()> {
        let manifest: SaveBankManifest = read_json(&bank_manifest(root)).await?;
        if live.exists() {
            tokio::fs::remove_dir_all(live).await?;
        }
        copy_tree(&bank_data(root), live).await?;
        verify_tree(live, &manifest.files).await
    }

    // @variants: both
    #[tokio::test]
    async fn switching_back_preserves_unsynced_profile_saves() -> Result<()> {
        let temp = tempdir()?;
        let saves_root = temp.path().join("saves");
        let live = temp.path().join("live");
        let first = SaveSetId::Profile {
            game_id: "game".to_string(),
            profile_id: "first".to_string(),
        };
        let second = SaveSetId::Profile {
            game_id: "game".to_string(),
            profile_id: "second".to_string(),
        };
        let first_root = bank_root_in(&saves_root, &first)?;
        let second_root = bank_root_in(&saves_root, &second)?;
        write_file(&live.join("save.dat"), b"first-before-play").await?;
        capture_test_bank(&first_root, &first, &live).await?;
        write_file(&live.join("save.dat"), b"first-after-play").await?;
        let second_seed = temp.path().join("second-seed");
        write_file(&second_seed.join("save.dat"), b"second").await?;
        capture_test_bank(&second_root, &second, &second_seed).await?;

        capture_test_bank(&first_root, &first, &live).await?;
        restore_test_bank(&second_root, &live).await?;
        capture_test_bank(&second_root, &second, &live).await?;
        restore_test_bank(&first_root, &live).await?;

        assert_eq!(
            tokio::fs::read(live.join("save.dat")).await?,
            b"first-after-play"
        );
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn interrupted_transition_reconciles_with_active_save_set() -> Result<()> {
        let temp = tempdir()?;
        let live = temp.path().join("live");
        let rollback = temp.path().join("rollback");
        let journal = temp.path().join("transition.json");
        let source = SaveSetId::Global {
            game_id: "game".to_string(),
        };
        let target = SaveSetId::Profile {
            game_id: "game".to_string(),
            profile_id: "profile".to_string(),
        };
        write_file(&live.join("save.dat"), b"target").await?;
        write_file(&rollback.join("save.dat"), b"source").await?;
        write_json(
            &journal,
            &TransitionJournal {
                source: source.clone(),
                target: target.clone(),
                live_path: live.clone(),
                rollback_path: rollback.clone(),
                phase: TransitionPhase::LiveRestored,
            },
        )
        .await?;

        recover_transition_journal(&journal, &live, &target).await?;
        assert_eq!(tokio::fs::read(live.join("save.dat")).await?, b"target");
        assert!(!rollback.exists());

        write_file(&rollback.join("save.dat"), b"source").await?;
        write_json(
            &journal,
            &TransitionJournal {
                source: source.clone(),
                target,
                live_path: live.clone(),
                rollback_path: rollback,
                phase: TransitionPhase::LiveRestored,
            },
        )
        .await?;
        recover_transition_journal(&journal, &live, &source).await?;
        assert_eq!(tokio::fs::read(live.join("save.dat")).await?, b"source");
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn rollback_restores_live_saves_after_failed_switch() -> Result<()> {
        let temp = tempdir()?;
        let live = temp.path().join("live");
        let rollback = temp.path().join("rollback");
        let journal = temp.path().join("transition.json");
        write_file(&live.join("save.dat"), b"partial-target").await?;
        write_file(&rollback.join("save.dat"), b"original").await?;
        write_file(&journal, b"pending").await?;
        let transition = SaveTransition {
            live_path: live.clone(),
            rollback_path: Some(rollback),
            journal_path: Some(journal.clone()),
            sync_result: None,
        };

        transition.rollback().await?;

        assert_eq!(tokio::fs::read(live.join("save.dat")).await?, b"original");
        assert!(!journal.exists());
        Ok(())
    }

    // @variants: snap
    #[test]
    fn inaccessible_wine_prefix_blocks_save_mutation_preflight() {
        let game = Game {
            id: "skyrim-se".to_string(),
            title: "Skyrim Special Edition".to_string(),
            path: PathBuf::from("/game"),
            data_subdir: "Data".to_string(),
            engine: crate::models::game::GameEngine::Bethesda,
            wine_prefix: Some(PathBuf::from("/prefix")),
        };

        let error = validate_live_save_access_with(&game, |_| Err("access denied"))
            .unwrap_err()
            .to_string();

        assert!(error.contains("reselect the Wine prefix"));
        assert!(error.contains("access denied"));
    }

    fn manifest_file(path: &str, hash: &str) -> SaveFileManifest {
        SaveFileManifest {
            path: path.to_string(),
            size: 4,
            modified_unix_seconds: 0,
            sha256: hash.to_string(),
        }
    }

    // @variants: both
    #[test]
    fn reports_added_modified_and_removed_save_files() {
        let live = vec![
            manifest_file("new.sav", "new"),
            manifest_file("changed.sav", "after"),
        ];
        let stored = vec![
            manifest_file("old.sav", "old"),
            manifest_file("changed.sav", "before"),
        ];

        assert_eq!(
            diff_files(&live, &stored),
            SaveSyncResult {
                added: 1,
                modified: 1,
                removed: 1,
            }
        );
    }

    fn backup(id: &str, created_at: &str, trigger: BackupTrigger, size: u64) -> SaveBackupManifest {
        SaveBackupManifest {
            schema_version: SAVE_SCHEMA_VERSION,
            backup_id: id.to_string(),
            payload_kind: "save_files".to_string(),
            save_set: SaveSetId::Global {
                game_id: "game".to_string(),
            },
            label: None,
            trigger,
            created_at: created_at.to_string(),
            files: vec![SaveFileManifest {
                size,
                ..manifest_file("save.sav", id)
            }],
        }
    }

    // @variants: both
    #[test]
    fn storage_cap_keeps_manual_and_newest_automatic_backup() {
        let backups = vec![
            backup(
                "old",
                "2026-01-01T00:00:00Z",
                BackupTrigger::ProfileSwitch,
                8,
            ),
            backup(
                "new",
                "2026-01-02T00:00:00Z",
                BackupTrigger::ProfileSwitch,
                8,
            ),
            backup("manual", "2025-01-01T00:00:00Z", BackupTrigger::Manual, 50),
        ];

        assert_eq!(automatic_backup_prune_candidates(&backups, 5), vec!["old"]);
    }

    // @variants: both
    #[test]
    fn rejects_path_traversal_in_save_set_identifiers() {
        let set = SaveSetId::Profile {
            game_id: "game".to_string(),
            profile_id: "../outside".to_string(),
        };

        assert!(bank_root_in(Path::new("/data"), &set).is_err());
        assert!(backup_root("../outside", "backup").is_err());
        assert!(backup_root("game", "../outside").is_err());
    }
}
