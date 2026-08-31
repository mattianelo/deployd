use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::core::tracker::Tracker;
use crate::core::{detector, game, save_manager};
use crate::models::game::Game;
use crate::models::profile::SaveMode;
use crate::utils::snap::{self, SelectedFolderKind};

use super::types::LoadedData;

mod state;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct VanillaHeaderCacheKey {
    game_id: String,
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

static VANILLA_HEADER_CACHE: OnceLock<Mutex<HashMap<VanillaHeaderCacheKey, usize>>> =
    OnceLock::new();

#[derive(Clone, Copy)]
pub(crate) enum GameLoadMode {
    OpenGame,
    Refresh,
}

/// Load all game data (mods, plugins, overrides, profiles, tools) for a game.
pub(crate) async fn load_game_data(
    tracker: &Tracker,
    game: &Game,
    mode: GameLoadMode,
) -> Result<LoadedData, String> {
    let game_id = &game.id;
    let mut access_warnings = Vec::new();
    let game_folder_accessible = if matches!(mode, GameLoadMode::OpenGame) {
        match snap::validate_selected_folder(&game.path, SelectedFolderKind::GameFolder) {
            Ok(()) => true,
            Err(error) => {
                access_warnings.push(format!(
                    "Access to {}'s game folder is no longer valid. Open Settings → Manage Games \
                     and reselect the installation folder. {error}",
                    game.title
                ));
                false
            }
        }
    } else {
        true
    };
    let wine_prefix_accessible = if matches!(mode, GameLoadMode::OpenGame) {
        match game.wine_prefix.as_deref() {
            Some(prefix) => {
                match snap::validate_selected_folder(prefix, SelectedFolderKind::WinePrefix) {
                    Ok(()) => true,
                    Err(error) => {
                        access_warnings.push(format!(
                            "Access to {}'s Wine prefix is no longer valid. Open Settings → \
                             Manage Games and reselect the Wine prefix. {error}",
                            game.title
                        ));
                        false
                    }
                }
            }
            None => true,
        }
    } else {
        true
    };

    // Ensure a "Default" profile exists. This is idempotent and covers both the
    // normal startup path and games added later via the wizard or Manage Games dialog.
    let active_profile = tracker
        .ensure_default_profile(game_id)
        .await
        .map_err(|e| e.to_string())?;
    if game::has_save_management(game) && wine_prefix_accessible {
        let active_set = save_manager::SaveSetId::for_profile(
            game_id,
            &active_profile.id,
            &active_profile.save_mode,
        );
        save_manager::recover_interrupted_transition(game, &active_set)
            .await
            .map_err(|e| e.to_string())?;
    }

    if matches!(mode, GameLoadMode::OpenGame) {
        let transition = tracker
            .restore_last_deployed_profile(game_id)
            .await
            .map_err(|e| e.to_string())?;
        if let Some((active_profile, deployed_profile)) = transition
            && game::has_save_management(game)
            && wine_prefix_accessible
        {
            let source = save_manager::SaveSetId::for_profile(
                game_id,
                &active_profile.id,
                &active_profile.save_mode,
            );
            let target = save_manager::SaveSetId::for_profile(
                game_id,
                &deployed_profile.id,
                &deployed_profile.save_mode,
            );
            let backup_cap = save_manager::configured_backup_cap_bytes(tracker).await;
            match save_manager::prepare_transition(
                game,
                &source,
                &target,
                save_manager::BackupTrigger::ProfileSwitch,
                backup_cap,
            )
            .await
            {
                Ok(transition) => {
                    transition.commit().await.map_err(|e| e.to_string())?;
                }
                Err(error) => {
                    tracker
                        .switch_profile(game_id, &active_profile.id)
                        .await
                        .map_err(|rollback| {
                            format!("{error}; failed to restore the active profile: {rollback}")
                        })?;
                    return Err(error.to_string());
                }
            }
        }
    }

    // Take a one-time vanilla snapshot so the external-file detector can exclude
    // files that were already present before any mod was installed.
    if game_folder_accessible {
        let vanilla_entries = detector::snapshot_game_files(game);
        tracker
            .ensure_vanilla_snapshot(game_id, &vanilla_entries)
            .await
            .map_err(|e| e.to_string())?;
    }

    // Sync plugin order from Plugins.txt (written by LOOT or other tools).
    // Only performed on initial game select, not on every in-session reload.
    if matches!(mode, GameLoadMode::OpenGame) && wine_prefix_accessible {
        let txt_paths = game::plugins_txt_paths(game);
        let txt_entries = txt_paths
            .iter()
            .find_map(|p| {
                crate::utils::plugins_txt::read_plugins_txt(p)
                    .ok()
                    .filter(|v| !v.is_empty())
            })
            .unwrap_or_default();
        if !txt_entries.is_empty() {
            tracker
                .sync_plugins_from_txt(game_id, &txt_entries)
                .await
                .map_err(|e| e.to_string())?;
        }
    }

    let mods = tracker
        .list_mods(game_id)
        .await
        .map_err(|e| e.to_string())?;
    // Remove plugin DB records whose cache files were deleted by the user.
    tracker
        .cleanup_orphaned_plugins(game_id)
        .await
        .map_err(|e| e.to_string())?;
    let plugins = tracker
        .list_plugins(game_id)
        .await
        .map_err(|e| e.to_string())?;
    let plugin_masters: HashMap<String, Vec<String>> = tracker
        .list_all_plugin_masters(game_id)
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(plugin_id, masters)| {
            (
                plugin_id,
                masters.into_iter().map(|m| m.to_lowercase()).collect(),
            )
        })
        .collect();
    let mod_names: HashMap<String, String> = mods
        .iter()
        .map(|m| (m.id.clone(), m.name.clone()))
        .collect();
    let overrides = tracker
        .compute_overrides(game_id, game::handler_for(&game.engine), &mod_names)
        .await
        .map_err(|e| e.to_string())?;
    // Ensure a profile exists before listing (handles first-ever load of a
    // newly-detected game, e.g. one found during a rescan).
    tracker
        .ensure_default_profile(game_id)
        .await
        .map_err(|e| e.to_string())?;
    let mut profiles = tracker
        .list_profiles(game_id)
        .await
        .map_err(|e| e.to_string())?;
    // Populate the filesystem-derived sync timestamp for each ProfileSpecific profile.
    for p in &mut profiles {
        if p.save_mode == SaveMode::ProfileSpecific {
            p.save_synced_at = save_manager::last_save_sync_time(game_id, &p.id);
        }
    }
    let active_profile_idx = profiles.iter().position(|p| p.is_active).unwrap_or(0);
    let tools = tracker
        .list_tools(game_id)
        .await
        .map_err(|e| e.to_string())?;
    let groups = tracker
        .list_groups(game_id)
        .await
        .map_err(|e| e.to_string())?;

    // Scan the game's Data directory for ALL plugin files (vanilla, DLC, CC, mod-managed).
    // Original casing is preserved for display; lowercasing is done at the point of comparison.
    let data_dir = game::deploy_dir(game);
    let (vanilla_plugins, plugin_scan_complete) = if game_folder_accessible {
        match scan_vanilla_plugins(&data_dir) {
            Ok(plugins) => (plugins, true),
            Err(error) => {
                access_warnings.push(format!(
                    "Deployd could not scan {}'s Data folder, so dependency warnings are hidden \
                     until access is restored. Open Settings → Manage Games and reselect the \
                     installation folder. {error}",
                    game.title
                ));
                (HashSet::new(), false)
            }
        }
    } else {
        (HashSet::new(), false)
    };

    // Read TES4 master counts for every vanilla/on-disk plugin so they can be sorted by
    // dependency depth (root masters with 0 declared masters first, DLCs after).
    let timing_start = std::time::Instant::now();
    let vanilla_plugin_master_counts =
        cached_vanilla_plugin_master_counts(game_id, &data_dir, &vanilla_plugins);
    crate::app::timing::log_phase(
        "plugins.vanilla_header_counts",
        game_id,
        timing_start,
        Some(vanilla_plugins.len()),
    );

    // Managed plugins whose on-disk file was originally a vanilla game file: deployd backed
    // up the original before overwriting it (e.g. a user-cleaned Fallout4.esm installed as a mod).
    let vanilla_derived_plugins: HashSet<String> = tracker
        .get_all_vanilla_backups(game_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(rel, _)| {
            let lower = std::path::Path::new(&rel)
                .file_name()?
                .to_string_lossy()
                .to_lowercase();
            (lower.ends_with(".esm") || lower.ends_with(".esp") || lower.ends_with(".esl"))
                .then_some(lower)
        })
        .collect();

    Ok(LoadedData {
        game_id: game_id.clone(),
        mods,
        plugins,
        plugin_masters,
        overrides,
        profiles,
        active_profile_idx,
        tools,
        vanilla_plugins,
        groups,
        vanilla_plugin_master_counts,
        vanilla_derived_plugins,
        access_warnings,
        plugin_scan_complete,
    })
}

fn scan_vanilla_plugins(data_dir: &Path) -> Result<HashSet<String>, String> {
    let entries = std::fs::read_dir(data_dir)
        .map_err(|error| format!("Cannot read '{}': {error}", data_dir.display()))?;
    let mut plugins = HashSet::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Cannot enumerate '{}': {error}", data_dir.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_lowercase();
        if lower.ends_with(".esp") || lower.ends_with(".esm") || lower.ends_with(".esl") {
            plugins.insert(name);
        }
    }
    Ok(plugins)
}

fn cached_vanilla_plugin_master_counts(
    game_id: &str,
    data_dir: &Path,
    vanilla_plugins: &HashSet<String>,
) -> HashMap<String, usize> {
    let cache = VANILLA_HEADER_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    vanilla_plugins
        .iter()
        .map(|name| {
            let path = data_dir.join(name);
            let key = std::fs::metadata(&path)
                .ok()
                .map(|metadata| VanillaHeaderCacheKey {
                    game_id: game_id.to_string(),
                    path: path.clone(),
                    modified: metadata.modified().ok(),
                    len: metadata.len(),
                });

            let count = key
                .as_ref()
                .and_then(|key| cache.lock().ok().and_then(|cache| cache.get(key).copied()))
                .unwrap_or_else(|| {
                    let count = crate::utils::plugin_header::read_masters(&path)
                        .map(|masters| masters.len())
                        .unwrap_or(0);
                    if let Some(key) = key
                        && let Ok(mut cache) = cache.lock()
                    {
                        cache.insert(key, count);
                    }
                    count
                });

            (name.to_lowercase(), count)
        })
        .collect()
}

/// Fetch avatar image bytes from a URL. Returns None on any error so the caller
/// silently falls back to the initials display.
pub(crate) async fn fetch_avatar_bytes(url: &str) -> Option<Vec<u8>> {
    crate::dlog!("[avatar] fetching: {url}");
    let client = reqwest::Client::builder()
        .user_agent(concat!("Deployd/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| crate::dlog!("[avatar] failed to build client: {e}"))
        .ok()?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| crate::dlog!("[avatar] request failed: {e}"))
        .ok()?;
    let status = resp.status();
    crate::dlog!("[avatar] HTTP {status}");
    if !status.is_success() {
        return None;
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| crate::dlog!("[avatar] failed to read body: {e}"))
        .ok()
        .map(|b| b.to_vec());
    crate::dlog!(
        "[avatar] got {} bytes",
        bytes.as_ref().map_or(0, |b| b.len())
    );
    bytes
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::tempdir;

    use super::scan_vanilla_plugins;

    // Regression: a stale Snap document-portal grant must not look like an empty Data folder.
    // @variants: snap
    #[test]
    fn reports_inaccessible_plugin_data_dir() -> Result<()> {
        let temp = tempdir()?;
        let missing = temp.path().join("stale-portal-id").join("Data");

        let error = scan_vanilla_plugins(&missing)
            .expect_err("an inaccessible Data folder must return an error");

        assert!(error.contains("Cannot read"));
        assert!(error.contains("Data"));
        Ok(())
    }

    // @variants: both
    #[test]
    fn discovers_starfield_free_update_plugins() -> Result<()> {
        let temp = tempdir()?;
        std::fs::write(temp.path().join("SFBGS007.esm"), b"")?;
        std::fs::write(temp.path().join("future-update.ESM"), b"")?;
        std::fs::write(temp.path().join("not-a-plugin.ba2"), b"")?;

        let plugins = scan_vanilla_plugins(temp.path()).map_err(anyhow::Error::msg)?;

        assert!(plugins.contains("SFBGS007.esm"));
        assert!(plugins.contains("future-update.ESM"));
        assert!(!plugins.contains("not-a-plugin.ba2"));
        Ok(())
    }
}
