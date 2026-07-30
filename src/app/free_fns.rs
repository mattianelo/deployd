use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use gtk::prelude::{ListModelExt, WidgetExt};

use crate::core::tracker::Tracker;
use crate::core::{detector, game, save_manager};
use crate::models::game::Game;
use crate::models::profile::SaveMode;

use super::types::LoadedData;

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct VanillaHeaderCacheKey {
    game_id: String,
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

static VANILLA_HEADER_CACHE: OnceLock<Mutex<HashMap<VanillaHeaderCacheKey, usize>>> =
    OnceLock::new();

/// Extracts the 10-digit Nexus CDN timestamp appended to downloaded filenames
/// (e.g. `ModName-12345-1.0-1604483725.7z` → `Some(1604483725)`).
/// This timestamp corresponds to `NexusFileEntry::uploaded_timestamp` and is used
/// as a tiebreaker when multiple Nexus files normalize to the same base name.
pub(crate) fn extract_nexus_timestamp(filename: &str) -> Option<i64> {
    let stem = filename
        .rsplit_once('.')
        .map(|(l, _)| l)
        .unwrap_or(filename);
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"-(\d{10})$").unwrap());
    re.captures(stem)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
}

/// Normalizes a Nexus filename for comparison against `NexusFileEntry.file_name`.
///
/// Strips the file extension and then the 10-digit Unix timestamp that the Nexus CDN
/// appends during browser/manager downloads (e.g. `foo-1.0-1756684569.7z` → `foo-1.0`).
/// The API stores the canonical name without the timestamp, so both sides must be
/// normalized before comparing.
pub(crate) fn normalize_nexus_filename(s: &str) -> String {
    let stem = s.rsplit_once('.').map(|(l, _)| l).unwrap_or(s);
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"-\d{10}$").unwrap());
    re.replace(stem, "").into_owned()
}

/// Parse a Nexus mod ID from a filename following the convention `ModName-MODID-VERSION.ext`.
///
/// Primary strategy: first 3+ digit number between dashes (skips 1-2 digit version segments).
/// Fallback: first all-digit segment < 1_000_000_000 (handles 1-2 digit mod IDs, filters out
/// the 10-digit Unix timestamp file_id that Nexus appends as the last segment).
///
/// Examples:
/// - `SkyUI_5_2_SE-12604-5-2SE.zip` → Some(12604)
/// - `Unofficial Skyrim Special Edition Patch-266-4-3-0a.zip` → Some(266)
/// - `LooksMenu v1-6-20-12631-1-6-20-1604483725.7z` → Some(12631)
/// - `BodySlide and Outfit Studio - v5.7.1-201-5-7-1-1753636918` → Some(201)
/// - `my_custom_mod.zip` → None
pub(crate) fn parse_nexus_mod_id(filename: &str) -> Option<i64> {
    // Primary: first 3+ digit number between dashes (handles most mods; version segments are ≤2 digits)
    if let Ok(re) = regex::Regex::new(r"-(\d{3,})-")
        && let Some(caps) = re.captures(filename)
        && let Some(id) = caps.get(1).and_then(|m| m.as_str().parse::<i64>().ok())
        && id > 0
        && id < 1_000_000_000
    {
        return Some(id);
    }
    // Fallback: first all-digit segment < 1B (handles 1-2 digit mod IDs; excludes timestamp file_ids)
    let stem = filename
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(filename);
    for part in stem.split('-') {
        if !part.is_empty()
            && part.chars().all(|c| c.is_ascii_digit())
            && let Ok(n) = part.parse::<i64>()
            && n > 0
            && n < 1_000_000_000
        {
            return Some(n);
        }
    }
    None
}

/// Parse a Nexus mod ID from direct user input.
///
/// Accepts a bare positive integer or a Nexus URL whose path contains the mod ID
/// as a numeric segment, such as `https://www.nexusmods.com/witcher/mods/101`.
pub(crate) fn parse_nexus_mod_id_from_input(raw: &str) -> Option<i64> {
    let raw = raw.trim();
    if let Ok(id) = raw.parse::<i64>() {
        return (id > 0).then_some(id);
    }

    let path = raw.split(['?', '#']).next().unwrap_or(raw);
    path.trim_end_matches('/')
        .rsplit('/')
        .find_map(|seg| seg.parse::<i64>().ok().filter(|&id| id > 0))
}

/// Clear all drop indicator CSS classes from a ListBox's rows.
pub(crate) fn clear_drop_indicators(list_box: &gtk::ListBox) {
    let mut idx = 0;
    while let Some(row) = list_box.row_at_index(idx) {
        row.remove_css_class("drop-above");
        row.remove_css_class("drop-below");
        idx += 1;
    }
}

/// Show a drop indicator on the row under the cursor.
/// Separator rows (group headers) are skipped — the indicator is placed on the
/// nearest mod row above or below the cursor instead.
pub(crate) fn update_drop_indicator(list_box: &gtk::ListBox, y: f64) {
    clear_drop_indicators(list_box);
    // If the cursor is past the last row, show a drop-below indicator on it.
    let row = list_box.row_at_y(y as i32).or_else(|| {
        let n = list_box.observe_children().n_items();
        n.checked_sub(1)
            .and_then(|i| list_box.row_at_index(i as i32))
    });
    if let Some(row) = row {
        if row.has_css_class("mod-separator-row") {
            return;
        }
        let alloc = row.allocation();
        let mid = alloc.y() + alloc.height() / 2;
        if (y as i32) < mid {
            row.add_css_class("drop-above");
        } else {
            row.add_css_class("drop-below");
        }
    }
}

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

    // Ensure a "Default" profile exists. This is idempotent and covers both the
    // normal startup path and games added later via the wizard or Manage Games dialog.
    tracker
        .ensure_default_profile(game_id)
        .await
        .map_err(|e| e.to_string())?;

    if matches!(mode, GameLoadMode::OpenGame) {
        let transition = tracker
            .restore_last_deployed_profile(game_id)
            .await
            .map_err(|e| e.to_string())?;
        if let Some((active_profile, deployed_profile)) = transition
            && game::has_save_management(game)
        {
            save_manager::swap_saves(
                game,
                Some(&active_profile.id),
                &active_profile.save_mode,
                &deployed_profile.id,
                &deployed_profile.save_mode,
            )
            .await
            .map_err(|e| e.to_string())?;
        }
    }

    // Take a one-time vanilla snapshot so the external-file detector can exclude
    // files that were already present before any mod was installed.
    {
        let vanilla_entries = detector::snapshot_game_files(game);
        tracker
            .ensure_vanilla_snapshot(game_id, &vanilla_entries)
            .await
            .map_err(|e| e.to_string())?;
    }

    // Sync plugin order from Plugins.txt (written by LOOT or other tools).
    // Only performed on initial game select, not on every in-session reload.
    if matches!(mode, GameLoadMode::OpenGame) {
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
    let vanilla_plugins: HashSet<String> = match std::fs::read_dir(&data_dir) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| {
                let orig = e.file_name().to_string_lossy().to_string();
                let lower = orig.to_lowercase();
                (lower.ends_with(".esp") || lower.ends_with(".esm") || lower.ends_with(".esl"))
                    .then_some(orig) // store original casing
            })
            .collect(),
        Err(_) => HashSet::new(),
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
    })
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
    use super::{parse_nexus_mod_id, parse_nexus_mod_id_from_input};

    // Regression: manually fetched metadata for NEO must target the page ID, not the CDN timestamp.
    // @variants: both
    #[test]
    fn parses_nexus_mod_id_from_timestamped_archive_name() {
        assert_eq!(
            parse_nexus_mod_id("NEO-65761-3-1-1-1763043682"),
            Some(65761)
        );
    }

    #[test]
    fn parses_bare_nexus_mod_id() {
        assert_eq!(parse_nexus_mod_id_from_input("101"), Some(101));
        assert_eq!(parse_nexus_mod_id_from_input("  101  "), Some(101));
    }

    #[test]
    fn parses_nexus_mod_id_from_url() {
        assert_eq!(
            parse_nexus_mod_id_from_input("https://www.nexusmods.com/witcher/mods/101"),
            Some(101)
        );
        assert_eq!(
            parse_nexus_mod_id_from_input(
                "https://www.nexusmods.com/skyrimspecialedition/mods/12604/"
            ),
            Some(12604)
        );
        assert_eq!(
            parse_nexus_mod_id_from_input(
                "https://www.nexusmods.com/skyrimspecialedition/mods/12604?tab=files"
            ),
            Some(12604)
        );
    }

    #[test]
    fn rejects_invalid_nexus_mod_id_input() {
        assert_eq!(parse_nexus_mod_id_from_input(""), None);
        assert_eq!(parse_nexus_mod_id_from_input("0"), None);
        assert_eq!(parse_nexus_mod_id_from_input("-1"), None);
        assert_eq!(parse_nexus_mod_id_from_input("not a nexus id"), None);
    }
}
