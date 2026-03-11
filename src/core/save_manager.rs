use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::game::detect_save_dir;
use crate::models::game::Game;
use crate::models::profile::SaveMode;
use crate::utils::paths;

/// Returns the per-profile save storage directory:
/// `<deployd-data>/saves/{game_id}/{profile_id}/`
///
/// Delegates to `paths::saves_root()` so the base path is resolved with the
/// same Flatpak-aware logic used for the DB and cache (avoids the app-private
/// `~/.var/app/…/data` path that `dirs::data_local_dir()` returns inside the
/// Flatpak sandbox, which differs from the `~/.local/share` path used by
/// `cargo run`).
pub fn deployd_save_dir(game_id: &str, profile_id: &str) -> PathBuf {
    paths::saves_root()
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(".local/share/deployd/saves")
        })
        .join(game_id)
        .join(profile_id)
}

/// Summary of differences detected between the live game save directory and a
/// profile's stored snapshot.
#[derive(Debug, Clone, Default)]
pub struct SaveSyncResult {
    /// Files present in the game directory but not in the stored snapshot.
    pub added: usize,
    /// Files present in both locations but with differing size or modification time.
    pub modified: usize,
    /// Files present in the stored snapshot but no longer in the game directory.
    pub removed: usize,
}

impl SaveSyncResult {
    pub fn has_changes(&self) -> bool {
        self.added > 0 || self.modified > 0 || self.removed > 0
    }

    /// Human-readable summary suitable for a toast notification.
    pub fn to_toast(&self) -> String {
        if !self.has_changes() {
            "Saves already up to date".to_string()
        } else {
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
}

/// Returns the modification time of the profile's save storage directory,
/// which is updated every time `backup_saves` runs. Returns `None` if the
/// storage directory does not exist (saves have never been backed up for this profile).
pub fn last_save_sync_time(game_id: &str, profile_id: &str) -> Option<std::time::SystemTime> {
    let storage = deployd_save_dir(game_id, profile_id);
    std::fs::metadata(&storage).ok()?.modified().ok()
}

/// Remove all *contents* of a directory without deleting the directory itself.
/// Subdirectories are removed recursively.
async fn clear_dir(dir: &Path) -> Result<()> {
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("Failed to read dir {}", dir.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if entry.file_type().await?.is_dir() {
            tokio::fs::remove_dir_all(&path)
                .await
                .with_context(|| format!("Failed to remove dir {}", path.display()))?;
        } else {
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("Failed to remove file {}", path.display()))?;
        }
    }
    Ok(())
}

/// Recursively copy all files from `src` into `dst`, creating directories as needed.
/// Existing files in `dst` are overwritten. The caller is responsible for clearing
/// `dst` first if a clean snapshot is required.
async fn copy_dir_all(src: &Path, dst: &Path) -> Result<()> {
    tokio::fs::create_dir_all(dst).await?;
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&from)
            .await
            .with_context(|| format!("Failed to read dir {}", from.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let src_path = entry.path();
            let dst_path = to.join(entry.file_name());
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                tokio::fs::create_dir_all(&dst_path).await?;
                stack.push((src_path, dst_path));
            } else {
                tokio::fs::copy(&src_path, &dst_path)
                    .await
                    .with_context(|| {
                        format!(
                            "Failed to copy {} → {}",
                            src_path.display(),
                            dst_path.display()
                        )
                    })?;
                // Restore the original modification time so games sort saves
                // by actual play date rather than the time of the copy.
                if let Ok(meta) = tokio::fs::metadata(&src_path).await
                    && let Ok(mtime) = meta.modified()
                    && let Ok(f) = std::fs::File::options().write(true).open(&dst_path)
                {
                    let _ = f.set_times(std::fs::FileTimes::new().set_modified(mtime));
                }
            }
        }
    }
    Ok(())
}

/// Walk `root` and return a map from relative-path string to `(file_size, mtime_secs)`.
/// Directories are not included; only regular files.
async fn collect_save_entries(root: &Path) -> Result<HashMap<String, (u64, i64)>> {
    let mut map = HashMap::new();
    if !root.exists() {
        return Ok(map);
    }
    let mut stack = vec![(root.to_path_buf(), PathBuf::new())];
    while let Some((abs, rel)) = stack.pop() {
        let mut entries = tokio::fs::read_dir(&abs)
            .await
            .with_context(|| format!("Failed to read dir {}", abs.display()))?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name();
            let child_abs = abs.join(&name);
            let child_rel = rel.join(&name);
            let file_type = entry.file_type().await?;
            if file_type.is_dir() {
                stack.push((child_abs, child_rel));
            } else {
                let meta = tokio::fs::metadata(&child_abs).await?;
                let size = meta.len();
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                map.insert(child_rel.to_string_lossy().into_owned(), (size, mtime));
            }
        }
    }
    Ok(map)
}

/// Compare the game's live save directory against the stored profile snapshot.
/// Returns a `SaveSyncResult` describing the differences without modifying anything.
async fn scan_saves_diff(game: &Game, profile_id: &str) -> Result<SaveSyncResult> {
    let Some(save_dir) = detect_save_dir(game) else {
        return Ok(SaveSyncResult::default());
    };
    let storage = deployd_save_dir(&game.id, profile_id);

    let live = collect_save_entries(&save_dir).await?;
    let stored = collect_save_entries(&storage).await?;

    let mut result = SaveSyncResult::default();
    for (path, (live_size, live_mtime)) in &live {
        match stored.get(path) {
            None => result.added += 1,
            Some((stored_size, stored_mtime)) => {
                if live_size != stored_size || live_mtime != stored_mtime {
                    result.modified += 1;
                }
            }
        }
    }
    for path in stored.keys() {
        if !live.contains_key(path) {
            result.removed += 1;
        }
    }
    Ok(result)
}

/// Copy the game's live save directory into Deployd's per-profile storage.
///
/// Clears the storage snapshot first so that saves deleted in-game are not
/// retained. No-op if the game's save directory does not exist.
pub async fn backup_saves(game: &Game, profile_id: &str) -> Result<()> {
    let Some(save_dir) = detect_save_dir(game) else {
        return Ok(());
    };
    if !save_dir.exists() {
        return Ok(());
    }
    let storage = deployd_save_dir(&game.id, profile_id);

    // Wipe previous snapshot so deleted saves don't persist.
    if storage.exists() {
        clear_dir(&storage)
            .await
            .context("Failed to clear profile save storage before backup")?;
    }

    copy_dir_all(&save_dir, &storage)
        .await
        .context("Failed to back up save files")
}

/// Replace the game's live save directory contents with this profile's snapshot.
///
/// Clears the game save directory first so saves from the previous profile do
/// not bleed through. If no snapshot exists for this profile, the directory is
/// left empty (the profile never had saves).
pub async fn restore_saves(game: &Game, profile_id: &str) -> Result<()> {
    let Some(save_dir) = detect_save_dir(game) else {
        return Ok(());
    };
    let storage = deployd_save_dir(&game.id, profile_id);

    // Clear current game saves so no files from the previous profile remain.
    if save_dir.exists() {
        clear_dir(&save_dir)
            .await
            .context("Failed to clear game save directory before restore")?;
    } else {
        tokio::fs::create_dir_all(&save_dir).await?;
    }

    // Restore from snapshot (if one exists; otherwise the empty dir is correct).
    if storage.exists() {
        copy_dir_all(&storage, &save_dir)
            .await
            .context("Failed to restore save files")?;
    }

    Ok(())
}

/// Called the first time a profile is switched to `ProfileSpecific` mode.
/// Snapshots current game saves into this profile's storage without touching
/// the game save directory (non-destructive).
pub async fn initialize_profile_saves(game: &Game, profile_id: &str) -> Result<()> {
    backup_saves(game, profile_id).await
}

/// Scan the game's live save directory for changes against the active profile's
/// stored snapshot, then write the updated backup. Returns the detected change
/// counts. Call this for the "Sync Saves" button or whenever a manual sync is needed.
pub async fn sync_profile_saves(game: &Game, profile_id: &str) -> Result<SaveSyncResult> {
    let diff = scan_saves_diff(game, profile_id).await?;
    backup_saves(game, profile_id).await?;
    Ok(diff)
}

/// Perform a save swap when switching between profiles:
/// - Scans and backs up the old profile's saves (if `ProfileSpecific`), returning the diff.
/// - Restores the new profile's saves (if `ProfileSpecific`), clearing the game dir first.
///
/// Returns `Some(SaveSyncResult)` if the old profile was backed up, `None` otherwise.
pub async fn swap_saves(
    game: &Game,
    old_profile_id: Option<&str>,
    old_save_mode: &SaveMode,
    new_profile_id: &str,
    new_save_mode: &SaveMode,
) -> Result<Option<SaveSyncResult>> {
    let sync_result = if *old_save_mode == SaveMode::ProfileSpecific
        && let Some(old_id) = old_profile_id
    {
        let diff = scan_saves_diff(game, old_id).await?;
        backup_saves(game, old_id).await?;
        Some(diff)
    } else {
        None
    };
    if *new_save_mode == SaveMode::ProfileSpecific {
        restore_saves(game, new_profile_id).await?;
    }
    Ok(sync_result)
}
