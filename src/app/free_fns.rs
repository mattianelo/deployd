use std::collections::{HashMap, HashSet};

use gtk::prelude::WidgetExt;

use crate::core::tracker::Tracker;
use crate::core::{detector, game, save_manager};
use crate::models::game::Game;
use crate::models::profile::SaveMode;

use super::types::LoadedData;

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

/// Check whether a proposed plugin ordering violates any master-dependency constraint.
///
/// `new_order` is the ordered list of `(plugin_id, filename)` after the proposed move.
/// `masters_map` maps `plugin_id → Vec<master_filename>` (lowercased; loaded via `load_game_data`).
///
/// Returns the filename of the first master found to load *after* its dependent, or `None` if the
/// order is valid.
pub(crate) fn check_order_violates_masters(
    new_order: &[(String, String)],
    masters_map: &HashMap<String, Vec<String>>,
) -> Option<String> {
    // Keep the *first* (earliest-loading) occurrence of each lowercase filename.
    // HashMap::collect() would keep the last entry for duplicate keys, which causes false
    // violations when a vanilla plugin (position 0) and a same-named managed plugin (higher
    // position) both appear in the list (show_vanilla_plugins = true).
    let mut pos: HashMap<String, usize> = HashMap::new();
    for (i, (_, f)) in new_order.iter().enumerate() {
        pos.entry(f.to_lowercase()).or_insert(i);
    }

    for (i, (plugin_id, _)) in new_order.iter().enumerate() {
        let Some(masters) = masters_map.get(plugin_id) else {
            continue;
        };
        for master in masters {
            let master_lc = master.to_lowercase();
            if let Some(&mpos) = pos.get(&master_lc)
                && mpos > i
            {
                return Some(master.clone());
            }
        }
    }
    None
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
    if let Some(row) = list_box.row_at_y(y as i32) {
        // Skip separator rows: find the nearest non-separator row
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

/// Load all game data (mods, plugins, overrides, profiles, tools) for a game.
///
/// `sync_txt` controls whether to sync plugin enabled/order state from the
/// on-disk Plugins.txt.  Pass `true` only on initial game select so that
/// external LOOT edits are honoured at session start.  In-session reloads
/// (conflict recomputation, priority saves, etc.) must pass `false` to avoid
/// reading back the cascaded-disabled Plugins.txt that Deployd wrote during
/// the last deploy and corrupting plugins.enabled.
pub(crate) async fn load_game_data(
    tracker: &Tracker,
    game: &Game,
    sync_txt: bool,
) -> Result<LoadedData, String> {
    let game_id = &game.id;

    // Ensure a "Default" profile exists. This is idempotent and covers both the
    // normal startup path and games added later via the wizard or Manage Games dialog.
    tracker
        .ensure_default_profile(game_id)
        .await
        .map_err(|e| e.to_string())?;

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
    if sync_txt {
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
    let vanilla_plugins: HashSet<String> = {
        let data_dir = game::deploy_dir(game);
        match std::fs::read_dir(&data_dir) {
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
        }
    };

    Ok(LoadedData {
        mods,
        plugins,
        plugin_masters,
        overrides,
        profiles,
        active_profile_idx,
        tools,
        vanilla_plugins,
        groups,
    })
}
