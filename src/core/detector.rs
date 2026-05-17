use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::PathBuf;
use std::time::UNIX_EPOCH;

use walkdir::WalkDir;

use crate::models::game::Game;
use crate::utils::paths::lowercase_path_str;

/// A file found in the game folder that is not tracked by any installed mod.
#[derive(Debug, Clone)]
pub struct ExternalFile {
    pub abs_path: PathBuf,
    /// Relative deployment key (lowercase). Files inside Data/ use bare names
    /// like `"textures/foo.dds"`; root-level files use `"../skse64_loader.exe"`,
    /// and files inside root subdirectories use `"../enbseries/effect.fx"`.
    /// This key is used for conflict-detection lookups.
    pub game_rel: String,
    /// Same path as `game_rel` but with the original filesystem casing preserved.
    /// This is what the installer should use as the deployment destination so that
    /// files are not recreated with an all-lowercase name on disk.
    pub game_rel_original: String,
    /// `true` when this file is a deployd-tracked managed plugin (.esp/.esm/.esl)
    /// whose on-disk copy was replaced externally (hardlink broken by xEdit's safe-save).
    /// The deployd cache still holds the original content. The user can "Adopt Changes"
    /// to copy the cleaned content into the cache and re-hardlink.
    pub is_managed_plugin: bool,
    /// For managed plugins that were cleaned in-place (inode unchanged, both Data and cache
    /// already reflect the cleaned content), this is the path to the most recent xEdit
    /// backup file. The backup holds the original pre-clean content and can be used by the
    /// "Restore from backup" action to undo the clean.
    pub xedit_backup_path: Option<PathBuf>,
}

/// Walk the game's Data directory and the game root (recursively), returning
/// files that are NOT managed by Deployd and NOT matching their vanilla baseline.
///
/// Decision per file:
/// 1. Path in `tracked` (deployed by Deployd) → skip.
/// 2. Path in `vanilla` AND `(size, mtime_secs)` match stored values → unchanged vanilla, skip.
/// 3. Path in `vanilla` but attributes differ → user replaced it, include.
/// 4. Path in neither → new file, include.
pub fn scan_external_files(
    game: &Game,
    tracked: &HashSet<String>,
    vanilla: &HashMap<String, (u64, i64)>,
) -> Vec<ExternalFile> {
    let mut results = Vec::new();

    let data_dir = game.data_dir();

    // Walk game/Data/ recursively
    if data_dir.is_dir() {
        for entry in WalkDir::new(&data_dir).min_depth(1) {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&data_dir) else {
                continue;
            };
            let rel_str = rel.to_string_lossy();
            let game_rel = rel_str.to_lowercase().replace('\\', "/");
            let game_rel_original = rel_str.replace('\\', "/");

            if let Some(result) = classify(&entry, &game_rel, &game_rel_original, tracked, vanilla)
            {
                results.push(result);
            }
        }
    }

    // Walk game root recursively, skipping the Data/ subtree (already covered above).
    // Skip entirely when data_subdir="." (REDEngine): data_dir IS the game root, so the
    // data walk already covered every file. Running this walk would produce "../"-prefixed
    // duplicates that never match the bare-path tracked set, causing all deployed files to
    // be falsely reported as external.
    if game.path.is_dir() && !matches!(game.data_subdir.as_str(), "" | ".") {
        for entry in WalkDir::new(&game.path)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| e.path() != data_dir)
        {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&game.path) else {
                continue;
            };
            let rel_str = rel.to_string_lossy();
            let game_rel = format!("../{}", rel_str.to_lowercase().replace('\\', "/"));
            let game_rel_original = format!("../{}", rel_str.replace('\\', "/"));

            if let Some(result) = classify(&entry, &game_rel, &game_rel_original, tracked, vanilla)
            {
                results.push(result);
            }
        }
    }

    results
}

/// Classify a single directory entry against the tracked and vanilla sets.
/// Returns `Some(ExternalFile)` if the file should be reported, `None` to skip.
fn classify(
    entry: &walkdir::DirEntry,
    game_rel: &str,
    game_rel_original: &str,
    tracked: &HashSet<String>,
    vanilla: &HashMap<String, (u64, i64)>,
) -> Option<ExternalFile> {
    // Deployd-managed file — always skip regardless of content.
    if tracked.contains(game_rel) {
        return None;
    }

    if let Some(&(snap_size, snap_mtime)) = vanilla.get(game_rel) {
        // File was present at vanilla snapshot time — check if it has changed.
        let meta = entry.metadata().ok()?;
        let cur_size = meta.len();
        let cur_mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        if cur_size == snap_size && cur_mtime == snap_mtime {
            return None; // unchanged vanilla file
        }
        // Size or mtime differ — file was replaced.
    }
    // Either a brand-new file or a replaced vanilla/mod file.
    Some(ExternalFile {
        abs_path: entry.path().to_path_buf(),
        game_rel: game_rel.to_string(),
        game_rel_original: game_rel_original.to_string(),
        is_managed_plugin: false,
        xedit_backup_path: None,
    })
}

/// Walk the game folder and collect `(game_rel_lowercase, size_bytes, mtime_secs)` for
/// every file currently on disk.
///
/// Used to build the one-time vanilla snapshot. The paths use the same format as
/// `scan_external_files`: Data-relative for Data/ files, `../`-prefixed for root files.
pub fn snapshot_game_files(game: &Game) -> Vec<(String, u64, i64)> {
    let mut entries = Vec::new();

    let data_dir = game.data_dir();

    if data_dir.is_dir() {
        for entry in WalkDir::new(&data_dir).min_depth(1) {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&data_dir) else {
                continue;
            };
            let game_rel = lowercase_path_str(rel);
            let (size, mtime) = file_attrs(&entry);
            entries.push((game_rel, size, mtime));
        }
    }

    // Skip root walk when data_subdir="." (REDEngine): data_dir IS the game root,
    // so the data walk above already snapshotted every file with bare paths.
    // Adding "../"-prefixed duplicates would corrupt the vanilla baseline.
    if game.path.is_dir() && !matches!(game.data_subdir.as_str(), "" | ".") {
        for entry in WalkDir::new(&game.path)
            .min_depth(1)
            .into_iter()
            .filter_entry(|e| e.path() != data_dir)
        {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&game.path) else {
                continue;
            };
            let game_rel = format!("../{}", lowercase_path_str(rel));
            let (size, mtime) = file_attrs(&entry);
            entries.push((game_rel, size, mtime));
        }
    }

    entries
}

/// Returns the xEdit backup directory name inside Data/ for the given game, if applicable.
///
/// xEdit creates `Data/<name>/<PluginName>/<Timestamp>/<PluginName>` when cleaning.
/// The subdirectory named after the plugin acts as the detection signal.
pub fn xedit_backup_dir_name(game_id: &str) -> Option<&'static str> {
    match game_id {
        "skyrimse" => Some("SSEEdit Backups"),
        "fallout4" => Some("FO4Edit Backups"),
        "falloutnv" => Some("FNVEdit Backups"),
        _ => None,
    }
}

/// Walk `Data/<xEdit Backups>/<plugin_name>/` and return the path to the most recently
/// modified file found inside (the newest backup version of that plugin).
fn find_newest_xedit_backup(plugin_backup_dir: &std::path::Path) -> Option<PathBuf> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in WalkDir::new(plugin_backup_dir).min_depth(1) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if newest.as_ref().map(|(t, _)| mtime > *t).unwrap_or(true) {
            newest = Some((mtime, entry.into_path()));
        }
    }
    newest.map(|(_, p)| p)
}

/// Check every tracked plugin file for external modification, using two complementary
/// detection strategies:
///
/// 1. **Broken hardlink** (rename-save): the on-disk inode differs from the cache inode.
///    xEdit's Windows safe-save pattern renames a temp file into place, which creates a
///    new inode in Data/ while the cache keeps the original.  `xedit_backup_path` is `None`
///    for these entries (the cache still holds the dirty original, so "Restore from cache"
///    already works).
///
/// 2. **In-place save** (inode unchanged): xEdit overwrites the file without renaming.
///    Both Data/ and cache share the same (now-cleaned) inode — the inode check finds
///    nothing. Instead we look for an xEdit backup directory
///    (`Data/<GameEdit> Backups/<plugin_name>/`) whose presence proves xEdit touched the
///    plugin.  The most recent backup file is stored in `xedit_backup_path` and can be
///    used to restore the pre-clean content if needed.
pub fn scan_modified_managed_plugins(
    game_id: &str,
    game: &Game,
    plugin_cache_paths: &[(String, String, PathBuf)],
) -> Vec<ExternalFile> {
    use std::os::unix::fs::MetadataExt;

    let data_dir = game.data_dir();
    let backup_dir_name = xedit_backup_dir_name(game_id);
    let mut results = Vec::new();

    for (game_rel, game_rel_original, cache_path) in plugin_cache_paths {
        // Use the actual on-disk path (original casing) for disk lookup.
        // On case-sensitive Linux, game_rel (lowercase) ≠ the deployed filename.
        let disk_path = data_dir.join(game_rel_original);
        if !disk_path.exists() || !cache_path.exists() {
            continue;
        }

        let disk_meta = match std::fs::metadata(&disk_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let cache_meta = match std::fs::metadata(cache_path) {
            Ok(m) => m,
            Err(_) => continue,
        };
        let disk_ino = disk_meta.ino();
        let cache_ino = cache_meta.ino();

        if disk_ino != cache_ino {
            let should_report = should_report_inode_mismatch(
                &disk_path,
                cache_path,
                &disk_meta,
                &cache_meta,
                disk_meta.dev() != cache_meta.dev(),
            );
            match should_report {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => eprintln!(
                    "[deployd] WARNING: could not compare managed plugin '{}' with cache: {e}",
                    game_rel_original
                ),
            }
            // Strategy 1: broken hardlink (rename-save). Cache has the dirty original;
            // "Restore from cache" works without a backup.
            results.push(ExternalFile {
                abs_path: disk_path,
                game_rel: game_rel.clone(),
                game_rel_original: game_rel.clone(), // already lowercase from DB
                is_managed_plugin: true,
                xedit_backup_path: None,
            });
        } else if let Some(dir_name) = backup_dir_name {
            // Strategy 2: same inode (in-place save). Check whether xEdit left a backup
            // directory for this plugin.
            //
            // The plugin filename is the last path component of game_rel
            // (e.g. "combatzonerestored.esp" from "combatzonerestored.esp").
            // The backup folder uses the original casing on disk so we match case-insensitively.
            let plugin_filename = std::path::Path::new(game_rel)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();

            let backup_root = data_dir.join(dir_name);
            let backup_plugin_dir: Option<PathBuf> =
                std::fs::read_dir(&backup_root).ok().and_then(|entries| {
                    entries.flatten().find_map(|e| {
                        (e.file_name().to_string_lossy().to_lowercase() == plugin_filename)
                            .then(|| e.path())
                    })
                });

            if let Some(plugin_dir) = backup_plugin_dir {
                // Found a backup directory for this plugin — it was cleaned in-place.
                // Both Data/ and cache already hold the cleaned content.
                let backup_file = find_newest_xedit_backup(&plugin_dir);
                results.push(ExternalFile {
                    abs_path: disk_path,
                    game_rel: game_rel.clone(),
                    game_rel_original: game_rel.clone(),
                    is_managed_plugin: true,
                    xedit_backup_path: backup_file,
                });
            }
        }
    }

    results
}

/// Read size and mtime (seconds since UNIX epoch) from a walkdir entry.
fn file_attrs(entry: &walkdir::DirEntry) -> (u64, i64) {
    let meta = entry.metadata().ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (size, mtime)
}

fn should_report_inode_mismatch(
    disk_path: &std::path::Path,
    cache_path: &std::path::Path,
    disk_meta: &std::fs::Metadata,
    cache_meta: &std::fs::Metadata,
    copied_across_devices: bool,
) -> io::Result<bool> {
    if !copied_across_devices {
        return Ok(true);
    }

    file_contents_match_with_metadata(disk_path, cache_path, disk_meta, cache_meta)
        .map(|same_content| !same_content)
}

fn file_contents_match_with_metadata(
    left: &std::path::Path,
    right: &std::path::Path,
    left_meta: &std::fs::Metadata,
    right_meta: &std::fs::Metadata,
) -> io::Result<bool> {
    if left_meta.len() != right_meta.len() {
        return Ok(false);
    }

    let mut left_reader = BufReader::new(File::open(left)?);
    let mut right_reader = BufReader::new(File::open(right)?);
    let mut left_buf = [0u8; 8192];
    let mut right_buf = [0u8; 8192];

    loop {
        let left_read = left_reader.read(&mut left_buf)?;
        let right_read = right_reader.read(&mut right_buf)?;
        if left_read != right_read {
            return Ok(false);
        }
        if left_read == 0 {
            return Ok(true);
        }
        if left_buf[..left_read] != right_buf[..right_read] {
            return Ok(false);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::models::game::{Game, GameEngine};

    use super::{scan_modified_managed_plugins, should_report_inode_mismatch};

    #[test]
    fn copied_managed_plugins_are_not_reported_as_cleaned() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let game_root = temp.path().join("game");
        let data_dir = game_root.join("Data");
        let cache_dir = temp.path().join("cache");
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(&cache_dir)?;

        let cache_path = cache_dir.join("example.esp");
        let disk_path = data_dir.join("Example.esp");
        fs::write(&cache_path, b"plugin bytes")?;
        fs::copy(&cache_path, &disk_path)?;

        let cache_meta = fs::metadata(&cache_path)?;
        let disk_meta = fs::metadata(&disk_path)?;
        let unchanged =
            should_report_inode_mismatch(&disk_path, &cache_path, &disk_meta, &cache_meta, true)?;
        assert!(
            !unchanged,
            "copy-fallback deployment should remain managed when cache and disk bytes match"
        );

        fs::write(&disk_path, b"cleaned plugin bytes")?;
        let disk_meta = fs::metadata(&disk_path)?;
        let modified =
            should_report_inode_mismatch(&disk_path, &cache_path, &disk_meta, &cache_meta, true)?;
        assert!(
            modified,
            "content changes should still be offered for adoption"
        );

        Ok(())
    }

    #[test]
    fn same_device_inode_changes_are_reported_without_content_comparison() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let game_root = temp.path().join("game");
        let data_dir = game_root.join("Data");
        let cache_dir = temp.path().join("cache");
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(&cache_dir)?;

        let cache_path = cache_dir.join("example.esp");
        let disk_path = data_dir.join("Example.esp");
        fs::write(&cache_path, b"plugin bytes")?;
        fs::copy(&cache_path, &disk_path)?;

        let game = Game {
            id: "fallout4".to_string(),
            title: "Fallout 4".to_string(),
            path: game_root,
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: None,
        };
        let plugin_files = vec![(
            "example.esp".to_string(),
            "Example.esp".to_string(),
            cache_path.clone(),
        )];

        let modified = scan_modified_managed_plugins(&game.id, &game, &plugin_files);
        assert_eq!(
            modified.len(),
            1,
            "same-device inode changes should keep the original hardlink-break detection path"
        );

        Ok(())
    }
}
