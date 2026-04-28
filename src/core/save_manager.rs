use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::game::detect_save_dir;
use crate::models::game::Game;
use crate::models::profile::SaveMode;
use crate::utils::paths;

/// Returns the per-profile save storage directory:
/// `<deployd-data>/saves/{game_id}/{profile_id}/`
pub fn deployd_save_dir(game_id: &str, profile_id: &str) -> PathBuf {
    paths::saves_root()
        .unwrap_or_else(|_| {
            if let Some(common) = std::env::var_os("SNAP_USER_COMMON") {
                PathBuf::from(common).join("deployd").join("saves")
            } else {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join("deployd")
                    .join("saves")
            }
        })
        .join(game_id)
        .join(profile_id)
}

#[derive(Debug, Clone, Default)]
pub struct SaveSyncResult {
    pub added: usize,
    pub modified: usize,
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

pub fn last_save_sync_time(game_id: &str, profile_id: &str) -> Option<std::time::SystemTime> {
    let storage = deployd_save_dir(game_id, profile_id);
    std::fs::metadata(&storage).ok()?.modified().ok()
}

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

pub async fn backup_saves(game: &Game, profile_id: &str) -> Result<()> {
    let Some(save_dir) = detect_save_dir(game) else {
        return Ok(());
    };
    if !save_dir.exists() {
        return Ok(());
    }
    let storage = deployd_save_dir(&game.id, profile_id);

    if storage.exists() {
        clear_dir(&storage)
            .await
            .context("Failed to clear profile save storage before backup")?;
    }

    copy_dir_all(&save_dir, &storage)
        .await
        .context("Failed to back up save files")
}

pub async fn restore_saves(game: &Game, profile_id: &str) -> Result<()> {
    let Some(save_dir) = detect_save_dir(game) else {
        return Ok(());
    };
    let storage = deployd_save_dir(&game.id, profile_id);

    if save_dir.exists() {
        clear_dir(&save_dir)
            .await
            .context("Failed to clear game save directory before restore")?;
    } else {
        tokio::fs::create_dir_all(&save_dir).await?;
    }

    if storage.exists() {
        copy_dir_all(&storage, &save_dir)
            .await
            .context("Failed to restore save files")?;
    }

    Ok(())
}

pub async fn initialize_profile_saves(game: &Game, profile_id: &str) -> Result<()> {
    backup_saves(game, profile_id).await
}

pub async fn sync_profile_saves(game: &Game, profile_id: &str) -> Result<SaveSyncResult> {
    let diff = scan_saves_diff(game, profile_id).await?;
    backup_saves(game, profile_id).await?;
    Ok(diff)
}

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
