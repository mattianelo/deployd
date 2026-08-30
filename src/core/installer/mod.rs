mod cache;
mod dazip;
mod deployment;
mod file_list;
mod inspection;
mod paths;

pub(crate) use file_list::is_ignorable_file;
pub use inspection::{PrepareResult, prepare_mod};
pub use paths::auto_detect_install_target;
pub(crate) use paths::{
    apply_redengine_path_fixups, route_aurora_paths, strip_data_subdir_prefix_str,
};

/// Re-scan a staging directory for its current file contents, picking up any
/// modifications the user made after initial extraction. Returns a fresh
/// `(abs_src, rel_dest)` list; falls back to empty if the root is missing.
///
/// Call this just before installation so that folder renames or file additions
/// in the staging area are reflected in the install (e.g. moving files into a
/// `system/` subfolder for Aurora/Witcher-1 mods).
pub fn rescan_staged_files(
    tmp_dir: &std::path::Path,
    stripped_wrapper: Option<&str>,
) -> Vec<(std::path::PathBuf, std::path::PathBuf)> {
    file_list::rescan(tmp_dir, stripped_wrapper)
}

/// Apply engine-specific path routing to a file list for preview purposes.
///
/// Mirrors the routing applied by the installer so that the pre-install dialog
/// shows paths that match where files will actually land on disk.
pub fn route_paths_for_preview(
    _engine: GameEngine,
    _data_subdir: &str,
    file_list: Vec<(PathBuf, PathBuf)>,
) -> Vec<(PathBuf, PathBuf)> {
    // Return original archive paths unchanged. Routing happens at install time
    // once file_targets are known (set by the pre-install dialog).
    file_list
}

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::Utc;
use uuid::Uuid;

use crate::core::rules;
use crate::core::tracker::Tracker;
use crate::models::download::NexusIds;
use crate::models::game::{Game, GameEngine};
use crate::models::mod_entry::{InstallTarget, ModEntry};
use crate::models::plugin::Plugin;
use crate::utils::paths as utils_paths;

#[derive(Debug)]
pub struct AddResult {
    pub mod_entry: ModEntry,
    pub files_cached: usize,
    pub plugins_found: Vec<String>,
}

pub(crate) struct AddModRequest<'a> {
    pub(crate) file_list: Vec<(PathBuf, PathBuf)>,
    pub(crate) game: &'a Game,
    pub(crate) mod_name: &'a str,
    pub(crate) tracker: &'a Tracker,
    pub(crate) cache_root: &'a Path,
    pub(crate) nexus_ids: Option<NexusIds>,
    pub(crate) archive_hash: Option<String>,
    pub(crate) archive_path: Option<String>,
    pub(crate) file_targets: HashMap<String, InstallTarget>,
    pub(crate) stripped_wrapper: Option<String>,
    pub(crate) excluded_files: &'a HashSet<String>,
    pub(crate) on_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
}

pub(crate) async fn add_mod_with_file_list(request: AddModRequest<'_>) -> Result<AddResult> {
    let AddModRequest {
        file_list,
        game,
        mod_name,
        tracker,
        cache_root,
        nexus_ids,
        archive_hash,
        archive_path,
        file_targets,
        stripped_wrapper,
        excluded_files,
        on_progress,
    } = request;
    let mod_id = Uuid::new_v4().to_string();

    let plan = deployment::route_and_plan(
        file_list,
        game,
        mod_name,
        stripped_wrapper.as_deref(),
        &file_targets,
        excluded_files,
    );
    let cache_dir = utils_paths::mod_cache_dir_in(cache_root, &mod_id);
    let cached = cache::write_files(&mod_id, &cache_dir, plan, on_progress.as_deref())?;
    let mod_files = cached.mod_files;
    let plugin_cache_files = cached.plugin_cache_files;
    let plugins_found = plugin_cache_files
        .iter()
        .map(|(name, _)| name.clone())
        .collect();

    let priority = tracker.next_priority(&game.id).await?;

    let (nexus_mod_id, nexus_file_id, nexus_domain) = match nexus_ids {
        Some(n) => (Some(n.mod_id), Some(n.file_id), Some(n.domain)),
        None => (None, None, None),
    };

    let all_root =
        !file_targets.is_empty() && file_targets.values().all(|t| *t == InstallTarget::Root);
    let mod_install_target = if all_root {
        InstallTarget::Root
    } else {
        InstallTarget::Data
    };

    let mod_entry = ModEntry {
        id: mod_id.clone(),
        game_id: game.id.clone(),
        name: mod_name.to_string(),
        archive_hash,
        archive_path,
        installed_at: Some(Utc::now().to_rfc3339()),
        enabled: true,
        priority,
        nexus_mod_id,
        nexus_file_id,
        nexus_domain,
        version: None,
        author: None,
        nexus_description: None,
        latest_version: None,
        nexus_file_name: None,
        nexus_is_primary: false,
        archive_md5: None,
        install_target: mod_install_target,
        notes: None,
    };

    tracker.insert_mod(&mod_entry).await?;
    tracker.record_files(&mod_files).await?;

    if !plugin_cache_files.is_empty() {
        let mut load_order = tracker.next_load_order(&game.id).await?;
        let mut plugin_records = Vec::with_capacity(plugin_cache_files.len());
        let mut masters_to_store: Vec<(String, Vec<String>)> =
            Vec::with_capacity(plugin_cache_files.len());

        for (filename, cache_path) in &plugin_cache_files {
            let plugin_id = Uuid::new_v4().to_string();
            let masters = crate::utils::plugin_header::read_masters(cache_path).unwrap_or_default();
            if !masters.is_empty() {
                masters_to_store.push((plugin_id.clone(), masters));
            }
            plugin_records.push(Plugin {
                id: plugin_id,
                mod_id: mod_id.clone(),
                filename: filename.clone(),
                load_order,
                enabled: true,
            });
            load_order += 1;
        }
        tracker.insert_plugins(&plugin_records).await?;
        for (plugin_id, masters) in masters_to_store {
            tracker.insert_plugin_masters(&plugin_id, &masters).await?;
        }
    }

    Ok(AddResult {
        mod_entry,
        files_cached: mod_files.len(),
        plugins_found,
    })
}

pub(crate) struct MergeModRequest<'a> {
    pub(crate) file_list: Vec<(PathBuf, PathBuf)>,
    pub(crate) game: &'a Game,
    pub(crate) mod_name: &'a str,
    pub(crate) existing_mod_id: &'a str,
    pub(crate) tracker: &'a Tracker,
    pub(crate) cache_root: &'a Path,
    pub(crate) file_targets: HashMap<String, InstallTarget>,
    pub(crate) stripped_wrapper: Option<String>,
    pub(crate) excluded_files: &'a HashSet<String>,
    pub(crate) on_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
}

pub(crate) async fn merge_files_into_mod(request: MergeModRequest<'_>) -> Result<usize> {
    let MergeModRequest {
        file_list,
        game,
        mod_name,
        existing_mod_id,
        tracker,
        cache_root,
        file_targets,
        stripped_wrapper,
        excluded_files,
        on_progress,
    } = request;
    let plan = deployment::route_and_plan(
        file_list,
        game,
        mod_name,
        stripped_wrapper.as_deref(),
        &file_targets,
        excluded_files,
    );
    let cache_dir = utils_paths::mod_cache_dir_in(cache_root, existing_mod_id);
    let cached = cache::write_files(existing_mod_id, &cache_dir, plan, on_progress.as_deref())?;
    let mod_files = cached.mod_files;
    let new_plugin_cache_files = cached.plugin_cache_files;

    let files_merged = mod_files.len();
    tracker.upsert_mod_files(&mod_files).await?;

    if !new_plugin_cache_files.is_empty() {
        let existing_plugins = tracker
            .get_plugins_for_mod(existing_mod_id)
            .await
            .unwrap_or_default();
        let existing_names: HashSet<String> = existing_plugins
            .iter()
            .map(|(_, name, _, _)| name.to_lowercase())
            .collect();

        let mut load_order = tracker.next_load_order(&game.id).await?;
        let mut new_plugin_records = Vec::new();
        let mut masters_to_store: Vec<(String, Vec<String>)> = Vec::new();

        for (filename, cache_path) in &new_plugin_cache_files {
            if !existing_names.contains(&filename.to_lowercase()) {
                let plugin_id = Uuid::new_v4().to_string();
                let masters =
                    crate::utils::plugin_header::read_masters(cache_path).unwrap_or_default();
                if !masters.is_empty() {
                    masters_to_store.push((plugin_id.clone(), masters));
                }
                new_plugin_records.push(Plugin {
                    id: plugin_id,
                    mod_id: existing_mod_id.to_string(),
                    filename: filename.clone(),
                    load_order,
                    enabled: true,
                });
                load_order += 1;
            }
        }

        if !new_plugin_records.is_empty() {
            tracker.insert_plugins(&new_plugin_records).await?;
            for (plugin_id, masters) in masters_to_store {
                tracker.insert_plugin_masters(&plugin_id, &masters).await?;
            }
        }
    }

    Ok(files_merged)
}

fn filter_excluded_files(
    file_list: Vec<(PathBuf, PathBuf)>,
    rules: &[rules::Rule],
    engine: &GameEngine,
    data_subdir: &str,
    excluded_files: &HashSet<String>,
) -> Vec<(PathBuf, PathBuf)> {
    if excluded_files.is_empty() {
        return file_list;
    }

    file_list
        .into_iter()
        .filter(|(_, dest)| {
            let key = exclusion_key_for_preview(dest, rules, engine, data_subdir);
            !excluded_files.contains(&key)
        })
        .collect()
}

fn exclusion_key_for_preview(
    dest: &Path,
    rules: &[rules::Rule],
    engine: &GameEngine,
    data_subdir: &str,
) -> String {
    let raw = dest.to_string_lossy();
    let s = rules::apply_rules(rules, &raw).replace('\\', "/");

    if *engine == GameEngine::Aurora {
        let data_prefix = format!("{}/", data_subdir.to_lowercase());
        let lower_s = s.to_lowercase();
        if lower_s.starts_with(&data_prefix) {
            return s[data_prefix.len()..].to_string();
        }
    }

    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(paths: &[&str]) -> Vec<(PathBuf, PathBuf)> {
        paths
            .iter()
            .map(|path| (PathBuf::from("src").join(path), PathBuf::from(path)))
            .collect()
    }

    fn dests(file_list: Vec<(PathBuf, PathBuf)>) -> Vec<PathBuf> {
        file_list.into_iter().map(|(_, dest)| dest).collect()
    }

    #[test]
    fn excludes_aurora_bare_file_before_override_routing() {
        let mut excluded = HashSet::new();
        excluded.insert("items/foo.uti".to_string());

        let filtered = filter_excluded_files(
            pairs(&["items/foo.uti", "items/bar.uti"]),
            &[],
            &GameEngine::Aurora,
            "Data",
            &excluded,
        );

        assert_eq!(dests(filtered), vec![PathBuf::from("items/bar.uti")]);
    }

    #[test]
    fn excludes_aurora_data_prefixed_file_by_preview_key() {
        let mut excluded = HashSet::new();
        excluded.insert("system/foo.dll".to_string());

        let filtered = filter_excluded_files(
            pairs(&["Data/system/foo.dll", "Data/system/bar.dll"]),
            &[],
            &GameEngine::Aurora,
            "Data",
            &excluded,
        );

        assert_eq!(dests(filtered), vec![PathBuf::from("Data/system/bar.dll")]);
    }

    #[test]
    fn excludes_bethesda_root_file_before_root_target_recording() {
        let mut excluded = HashSet::new();
        excluded.insert("skse64_loader.dll".to_string());

        let filtered = filter_excluded_files(
            pairs(&["skse64_loader.dll", "Data/Scripts/keep.pex"]),
            &[],
            &GameEngine::Bethesda,
            "Data",
            &excluded,
        );

        assert_eq!(
            dests(filtered),
            vec![PathBuf::from("Data/Scripts/keep.pex")]
        );
    }

    #[test]
    fn excludes_bethesda_data_file() {
        let mut excluded = HashSet::new();
        excluded.insert("Data/Scripts/drop.pex".to_string());

        let filtered = filter_excluded_files(
            pairs(&["Data/Scripts/drop.pex", "Data/Scripts/keep.pex"]),
            &[],
            &GameEngine::Bethesda,
            "Data",
            &excluded,
        );

        assert_eq!(
            dests(filtered),
            vec![PathBuf::from("Data/Scripts/keep.pex")]
        );
    }

    #[test]
    fn excludes_bethesda_directory_sentinel() {
        let mut excluded = HashSet::new();
        excluded.insert("EmptyDir".to_string());

        let filtered = filter_excluded_files(
            pairs(&["EmptyDir", "Data/Scripts/keep.pex"]),
            &[],
            &GameEngine::Bethesda,
            "Data",
            &excluded,
        );

        assert_eq!(
            dests(filtered),
            vec![PathBuf::from("Data/Scripts/keep.pex")]
        );
    }

    #[test]
    fn non_bethesda_root_anchor_is_not_reinterpreted_as_data_path() {
        let mut excluded = HashSet::new();
        excluded.insert("system/foo.dll".to_string());

        let filtered = filter_excluded_files(
            pairs(&["../system/foo.dll", "system/foo.dll"]),
            &[],
            &GameEngine::Aurora,
            "Data",
            &excluded,
        );

        assert_eq!(dests(filtered), vec![PathBuf::from("../system/foo.dll")]);
    }

    #[test]
    fn eclipse_docs_anchor_is_not_reinterpreted() {
        let mut excluded = HashSet::new();
        excluded.insert("BioWare/Settings.xml".to_string());

        let filtered = filter_excluded_files(
            pairs(&["~docs~/BioWare/Settings.xml", "BioWare/Settings.xml"]),
            &[],
            &GameEngine::Eclipse,
            "packages/core/override",
            &excluded,
        );

        assert_eq!(
            dests(filtered),
            vec![PathBuf::from("~docs~/BioWare/Settings.xml")]
        );
    }

    #[test]
    fn redengine_routed_mods_path_is_not_reinterpreted() {
        let mut excluded = HashSet::new();
        excluded.insert("content/scripts/foo.ws".to_string());

        let filtered = filter_excluded_files(
            pairs(&[
                "Mods/SomeMod/content/scripts/foo.ws",
                "content/scripts/foo.ws",
            ]),
            &[],
            &GameEngine::REDEngine,
            "Data",
            &excluded,
        );

        assert_eq!(
            dests(filtered),
            vec![PathBuf::from("Mods/SomeMod/content/scripts/foo.ws")]
        );
    }
}
