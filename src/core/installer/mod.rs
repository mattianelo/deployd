mod dazip;
mod file_list;
mod paths;

pub(crate) use file_list::is_ignorable_file;
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
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use tempfile::TempDir;
use uuid::Uuid;

use crate::core::game::engine_handler;
use crate::core::rules;
use crate::core::tracker::Tracker;
use crate::dlog;
use crate::models::download::NexusIds;
use crate::models::game::{Game, GameEngine};
use crate::models::manifest::ModFile;
use crate::models::mod_entry::{InstallTarget, ModEntry};
use crate::models::plugin::Plugin;
use crate::utils::paths as utils_paths;
use crate::utils::{archive, fomod_resolver};

#[derive(Debug)]
pub struct AddResult {
    pub mod_entry: ModEntry,
    pub files_cached: usize,
    pub plugins_found: Vec<String>,
}

pub enum PrepareResult {
    Normal {
        file_list: Vec<(PathBuf, PathBuf)>,
        /// Original wrapper dir name stripped by detect_wrapper (e.g. `"modSkipMovies"`).
        /// Used by REDEngine path fixups to preserve the archive's folder name under `Mods/`.
        stripped_wrapper: Option<String>,
        tmp_dir: TempDir,
    },
    Fomod {
        config: fomod_resolver::FomodUiConfig,
        config_path: PathBuf,
        tmp_dir: TempDir,
    },
}

pub async fn prepare_mod(
    archive_path: &Path,
    on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
    on_processing: Option<Box<dyn FnOnce() + Send>>,
) -> Result<PrepareResult> {
    dlog!("[deployd] prepare_mod: {}", archive_path.display());
    let path = archive_path.to_path_buf();
    let tmp_dir = tokio::task::spawn_blocking(move || {
        archive::extract_archive(&path, on_extract_progress)
            .with_context(|| format!("Extraction failed for: {}", path.display()))
    })
    .await
    .context("Extraction task panicked")??;

    // Notify the UI that extraction finished and post-processing is starting.
    // This keeps the progress message meaningful during the blocking work below.
    if let Some(cb) = on_processing {
        cb();
    }

    let is_dazip = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("dazip"))
        .unwrap_or(false);

    let stem = if is_dazip {
        archive_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    } else {
        String::new()
    };

    // Post-extraction work (dazip expansion, FOMOD/file-list discovery) involves
    // directory scans and recursive I/O. Wrap in spawn_blocking so the tokio
    // executor stays free and the UI remains responsive.
    tokio::task::spawn_blocking(move || {
        let extracted_root = tmp_dir.path();
        dlog!("[deployd] extracted to: {}", extracted_root.display());

        if is_dazip {
            dazip::process_dazip_root(extracted_root, &stem)
                .context("Failed to process dazip archive")?;
        } else {
            dazip::expand_dazip_files_in_place(extracted_root)
                .context("Failed to expand nested .dazip files")?;
        }

        if let Some(config_path) = fomod_resolver::detect_fomod(extracted_root) {
            dlog!("[deployd] FOMOD detected: {}", config_path.display());
            let config = fomod_resolver::parse_fomod_config(&config_path).with_context(|| {
                format!("Failed to parse FOMOD config: {}", config_path.display())
            })?;
            dlog!(
                "[deployd] FOMOD config parsed: {} steps",
                config.steps.len()
            );
            Ok(PrepareResult::Fomod {
                config,
                config_path,
                tmp_dir,
            })
        } else {
            let (file_list, stripped_wrapper) = file_list::resolve_file_list(extracted_root)
                .context("Failed to resolve file list from extracted archive")?;
            dlog!("[deployd] normal mod: {} files resolved", file_list.len());
            Ok(PrepareResult::Normal {
                file_list,
                stripped_wrapper,
                tmp_dir,
            })
        }
    })
    .await
    .context("Post-extraction task panicked")?
}

#[allow(clippy::too_many_arguments)] // All parameters are meaningfully distinct; a builder struct would add complexity without clarity.
pub async fn add_mod_with_file_list(
    file_list: Vec<(PathBuf, PathBuf)>,
    game: &Game,
    mod_name: &str,
    tracker: &Tracker,
    cache_root: &Path,
    nexus_ids: Option<NexusIds>,
    archive_hash: Option<String>,
    archive_path: Option<String>,
    file_targets: HashMap<String, InstallTarget>,
    stripped_wrapper: Option<String>,
    excluded_files: &HashSet<String>,
    on_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
) -> Result<AddResult> {
    let mod_id = Uuid::new_v4().to_string();

    let game_rules = rules::rules_for_game(&game.id);
    let file_list = filter_excluded_files(
        file_list,
        &game_rules,
        &game.engine,
        &game.data_subdir,
        excluded_files,
    );
    let handler = engine_handler::handler_for(&game.engine);
    let file_list = handler.route_file_list(
        game,
        mod_name,
        stripped_wrapper.as_deref(),
        file_list,
        &file_targets,
    );

    let cache_dir = utils_paths::mod_cache_dir_in(cache_root, &mod_id);
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Cannot create cache dir: {}", cache_dir.display()))?;

    let total_files = file_list.len();
    let mut mod_files: Vec<ModFile> = Vec::with_capacity(total_files);
    let mut plugins_found: Vec<String> = Vec::new();
    let mut plugin_cache_files: Vec<(String, PathBuf)> = Vec::new();
    for (file_idx, (src_abs, dest_rel)) in file_list.iter().enumerate() {
        let ruled_path = rules::apply_rules(&game_rules, &dest_rel.to_string_lossy());
        let original_rel = ruled_path.replace('\\', "/");
        let explicit_root = original_rel.starts_with("../");
        let original_rel = if explicit_root {
            original_rel[3..].to_string()
        } else {
            original_rel
        };
        let lowercase_rel = utils_paths::lowercase_path(Path::new(&original_rel));
        let lowercase_rel =
            self::paths::strip_data_subdir_prefix(&lowercase_rel, &game.data_subdir);
        let original_rel =
            self::paths::strip_data_subdir_prefix_str(&original_rel, &game.data_subdir);

        // Directory sentinel: recorded with trailing '/' so the deployer creates the dir
        // rather than hardlinking (e.g. JContainers' Domains/ folder must exist).
        if src_abs.is_dir() {
            let cache_sentinel = cache_dir.join(&lowercase_rel);
            if let Err(e) = fs::create_dir_all(&cache_sentinel) {
                eprintln!(
                    "[deployd] WARNING: cannot create cache sentinel '{}': {e}",
                    cache_sentinel.display()
                );
            }
            if let Some(ref cb) = on_progress {
                cb(file_idx + 1, total_files);
            }
            let rel_str = lowercase_rel.to_string_lossy();
            let (recorded_rel, original_recorded_rel) = if explicit_root {
                (format!("../{rel_str}/"), format!("../{original_rel}/"))
            } else {
                (format!("{rel_str}/"), format!("{original_rel}/"))
            };
            mod_files.push(ModFile {
                mod_id: mod_id.clone(),
                game_rel_lowercase: recorded_rel,
                game_rel_original: original_recorded_rel,
                cache_path: cache_sentinel.to_string_lossy().to_string(),
            });
            continue;
        }

        // Skip files the user opted to exclude in the pre-install dialog.
        let file_key = ruled_path.replace('\\', "/");
        if excluded_files.contains(&file_key) {
            if let Some(ref cb) = on_progress {
                cb(file_idx + 1, total_files);
            }
            continue;
        }

        let cache_file = cache_dir.join(&lowercase_rel);
        if let Some(parent) = cache_file.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::copy(src_abs, &cache_file) {
            Ok(_) => {}
            Err(e) if e.raw_os_error() == Some(21) => {
                // EISDIR: the source path resolved to a directory at copy time even
                // though it appeared to be a file during FOMOD resolution. This can
                // happen with malformed archives where a file and a same-named directory
                // entry coexist. Skip with a warning rather than aborting the install.
                eprintln!(
                    "[deployd] WARNING: skipping '{}' — resolved as directory at copy time (EISDIR)",
                    src_abs.display()
                );
                if let Some(ref cb) = on_progress {
                    cb(file_idx + 1, total_files);
                }
                continue;
            }
            Err(e) => {
                return Err(e).with_context(|| format!("Cache copy failed: {}", src_abs.display()));
            }
        }

        if let Some(ref cb) = on_progress {
            cb(file_idx + 1, total_files);
        }

        let deploy_to_root = handler.deploy_to_root(&file_key, &file_targets, explicit_root);

        let rel_str = lowercase_rel.to_string_lossy();
        let recorded_rel = if deploy_to_root {
            format!("../{rel_str}")
        } else {
            rel_str.to_string()
        };
        let original_recorded_rel = if deploy_to_root {
            format!("../{original_rel}")
        } else {
            original_rel.clone()
        };

        if is_plugin(&rel_str) {
            let orig_path = Path::new(&original_rel);
            if let Some(filename) = orig_path.file_name() {
                let name = filename.to_string_lossy().to_string();
                plugins_found.push(name.clone());
                plugin_cache_files.push((name, cache_file.clone()));
            }
        }

        mod_files.push(ModFile {
            mod_id: mod_id.clone(),
            game_rel_lowercase: recorded_rel,
            game_rel_original: original_recorded_rel,
            cache_path: cache_file.to_string_lossy().to_string(),
        });
    }

    {
        let mut seen = HashSet::new();
        let mut deduped = Vec::with_capacity(mod_files.len());
        for f in mod_files.into_iter().rev() {
            if seen.insert(f.game_rel_lowercase.clone()) {
                deduped.push(f);
            }
        }
        deduped.reverse();
        mod_files = deduped;
    }

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

#[allow(clippy::too_many_arguments)] // All parameters are meaningfully distinct; a builder struct would add complexity without clarity.
pub async fn merge_files_into_mod(
    file_list: Vec<(PathBuf, PathBuf)>,
    game: &Game,
    mod_name: &str,
    existing_mod_id: &str,
    tracker: &Tracker,
    cache_root: &Path,
    file_targets: HashMap<String, InstallTarget>,
    stripped_wrapper: Option<String>,
    excluded_files: &HashSet<String>,
    on_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
) -> Result<usize> {
    let game_rules = rules::rules_for_game(&game.id);
    let file_list = filter_excluded_files(
        file_list,
        &game_rules,
        &game.engine,
        &game.data_subdir,
        excluded_files,
    );
    let handler = engine_handler::handler_for(&game.engine);
    let file_list = handler.route_file_list(
        game,
        mod_name,
        stripped_wrapper.as_deref(),
        file_list,
        &file_targets,
    );

    let cache_dir = utils_paths::mod_cache_dir_in(cache_root, existing_mod_id);
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Cannot create cache dir: {}", cache_dir.display()))?;

    let total_files = file_list.len();
    let mut mod_files: Vec<ModFile> = Vec::with_capacity(total_files);
    let mut new_plugin_cache_files: Vec<(String, PathBuf)> = Vec::new();
    for (file_idx, (src_abs, dest_rel)) in file_list.iter().enumerate() {
        let ruled_path = rules::apply_rules(&game_rules, &dest_rel.to_string_lossy());
        let original_rel = ruled_path.replace('\\', "/");
        let explicit_root = original_rel.starts_with("../");
        let original_rel = if explicit_root {
            original_rel[3..].to_string()
        } else {
            original_rel
        };
        let lowercase_rel = utils_paths::lowercase_path(Path::new(&original_rel));
        let lowercase_rel =
            self::paths::strip_data_subdir_prefix(&lowercase_rel, &game.data_subdir);
        let original_rel =
            self::paths::strip_data_subdir_prefix_str(&original_rel, &game.data_subdir);

        if src_abs.is_dir() {
            let cache_sentinel = cache_dir.join(&lowercase_rel);
            if let Err(e) = fs::create_dir_all(&cache_sentinel) {
                eprintln!(
                    "[deployd] WARNING: cannot create cache sentinel '{}': {e}",
                    cache_sentinel.display()
                );
            }
            if let Some(ref cb) = on_progress {
                cb(file_idx + 1, total_files);
            }
            let rel_str = lowercase_rel.to_string_lossy();
            let (recorded_rel, original_recorded_rel) = if explicit_root {
                (format!("../{rel_str}/"), format!("../{original_rel}/"))
            } else {
                (format!("{rel_str}/"), format!("{original_rel}/"))
            };
            mod_files.push(ModFile {
                mod_id: existing_mod_id.to_string(),
                game_rel_lowercase: recorded_rel,
                game_rel_original: original_recorded_rel,
                cache_path: cache_sentinel.to_string_lossy().to_string(),
            });
            continue;
        }

        // Skip files the user opted to exclude in the pre-install dialog.
        let file_key = ruled_path.replace('\\', "/");
        if excluded_files.contains(&file_key) {
            if let Some(ref cb) = on_progress {
                cb(file_idx + 1, total_files);
            }
            continue;
        }

        let cache_file = cache_dir.join(&lowercase_rel);
        if let Some(parent) = cache_file.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::copy(src_abs, &cache_file) {
            Ok(_) => {}
            Err(e) if e.raw_os_error() == Some(21) => {
                eprintln!(
                    "[deployd] WARNING: skipping '{}' — resolved as directory at copy time (EISDIR)",
                    src_abs.display()
                );
                if let Some(ref cb) = on_progress {
                    cb(file_idx + 1, total_files);
                }
                continue;
            }
            Err(e) => {
                return Err(e).with_context(|| format!("Cache copy failed: {}", src_abs.display()));
            }
        }

        if let Some(ref cb) = on_progress {
            cb(file_idx + 1, total_files);
        }

        let deploy_to_root = handler.deploy_to_root(&file_key, &file_targets, explicit_root);

        let rel_str = lowercase_rel.to_string_lossy();
        let recorded_rel = if deploy_to_root {
            format!("../{rel_str}")
        } else {
            rel_str.to_string()
        };
        let original_recorded_rel = if deploy_to_root {
            format!("../{original_rel}")
        } else {
            original_rel.clone()
        };

        if is_plugin(&rel_str)
            && let Some(filename) = Path::new(&original_rel).file_name()
        {
            let name = filename.to_string_lossy().to_string();
            new_plugin_cache_files.push((name, cache_file.clone()));
        }

        mod_files.push(ModFile {
            mod_id: existing_mod_id.to_string(),
            game_rel_lowercase: recorded_rel,
            game_rel_original: original_recorded_rel,
            cache_path: cache_file.to_string_lossy().to_string(),
        });
    }

    {
        let mut seen = HashSet::new();
        let mut deduped = Vec::with_capacity(mod_files.len());
        for f in mod_files.into_iter().rev() {
            if seen.insert(f.game_rel_lowercase.clone()) {
                deduped.push(f);
            }
        }
        deduped.reverse();
        mod_files = deduped;
    }

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

fn is_plugin(rel_path: &str) -> bool {
    let l = rel_path.to_lowercase();
    l.ends_with(".esp") || l.ends_with(".esm") || l.ends_with(".esl")
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
}
