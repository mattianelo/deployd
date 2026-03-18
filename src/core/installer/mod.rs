mod dazip;
mod file_list;
mod paths;

pub use paths::auto_detect_install_target;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use tempfile::TempDir;
use uuid::Uuid;

use crate::core::rules;
use crate::core::tracker::Tracker;
use crate::dlog;
use crate::models::game::{Game, GameEngine};
use crate::models::manifest::ModFile;
use crate::models::mod_entry::{InstallTarget, ModEntry};
use crate::models::plugin::Plugin;
use crate::utils::{archive, fomod_resolver};
use crate::utils::paths as utils_paths;

/// Result of adding a mod (cache-only, no deployment).
#[derive(Debug)]
pub struct AddResult {
    pub mod_entry: ModEntry,
    pub files_cached: usize,
    pub plugins_found: Vec<String>,
}

/// Result of preparing a mod archive for installation.
pub enum PrepareResult {
    /// Normal mod — file list already resolved, ready to install.
    Normal {
        file_list: Vec<(PathBuf, PathBuf)>,
        /// If `detect_wrapper` stripped a single wrapper directory from the archive
        /// root, this holds that directory's original name (e.g. `"modSkipMovies"`).
        /// Used by REDEngine path fixups so W3 mods keep their original folder name
        /// under `Mods/` instead of being renamed to the user's display name.
        stripped_wrapper: Option<String>,
        tmp_dir: TempDir,
    },
    /// FOMOD mod — needs user input before continuing.
    Fomod {
        config: fomod_resolver::FomodUiConfig,
        config_path: PathBuf,
        tmp_dir: TempDir,
    },
}

/// Extract archive and detect whether it's a FOMOD or normal mod.
/// For FOMOD mods, returns the parsed config for UI display.
/// For normal mods, returns the resolved file list ready for installation.
///
/// Extraction runs on a blocking thread to avoid starving the async runtime
/// (7z/LZMA decompression is CPU-intensive).
///
/// `on_extract_progress` is called with `(done, total)` as files are extracted.
pub async fn prepare_mod(
    archive_path: &Path,
    on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
) -> Result<PrepareResult> {
    dlog!("[deployd] prepare_mod: {}", archive_path.display());
    let path = archive_path.to_path_buf();
    let tmp_dir = tokio::task::spawn_blocking(move || {
        archive::extract_archive(&path, on_extract_progress)
            .with_context(|| format!("Extraction failed for: {}", path.display()))
    })
    .await
    .context("Extraction task panicked")??;

    let extracted_root = tmp_dir.path();
    dlog!("[deployd] extracted to: {}", extracted_root.display());

    dazip::expand_dazip_files_in_place(extracted_root)
        .context("Failed to expand nested .dazip files")?;

    if let Some(config_path) = fomod_resolver::detect_fomod(extracted_root) {
        dlog!("[deployd] FOMOD detected: {}", config_path.display());
        let config = fomod_resolver::parse_fomod_config(&config_path)
            .with_context(|| format!("Failed to parse FOMOD config: {}", config_path.display()))?;
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
}

/// Install a mod from a pre-resolved file list.
/// Applies game rules, caches files, and records in the database.
///
/// `file_targets` maps dest_rel path strings to their install target.
/// Files not present in the map are auto-detected: root-level .exe/.dll/.asi → Root,
/// everything else → Data.
pub async fn add_mod_with_file_list(
    file_list: Vec<(PathBuf, PathBuf)>,
    game: &Game,
    mod_name: &str,
    tracker: &Tracker,
    nexus_ids: Option<(i64, i64, String)>,
    archive_hash: Option<String>,
    file_targets: HashMap<String, InstallTarget>,
    // Wrapper directory name stripped by detect_wrapper (e.g. "modSkipMovies").
    // Used by W3 path fixups to preserve the archive's original mod folder name.
    stripped_wrapper: Option<String>,
    on_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
) -> Result<AddResult> {
    let mod_id = Uuid::new_v4().to_string();

    // Apply REDEngine-specific path fixups before any further processing.
    // For Bethesda games this is a no-op.
    let file_list = if game.engine == GameEngine::REDEngine {
        paths::apply_redengine_path_fixups(game, mod_name, stripped_wrapper.as_deref(), file_list)
    } else {
        file_list
    };

    // Route Eclipse (Dragon Age) files: DAZIP-expanded files already carry an
    // AddIns/<uid>/ prefix; tool mods go to Documents/<mod_name>/; everything
    // else goes to packages/core/override/.
    let file_list = if game.engine == GameEngine::Eclipse {
        paths::route_eclipse_paths(file_list, mod_name)
    } else {
        file_list
    };

    let game_rules = rules::rules_for_game(&game.id);
    let cache_dir = utils_paths::mod_cache_dir(&mod_id)?;
    fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Cannot create cache dir: {}", cache_dir.display()))?;

    let total_files = file_list.len();
    let mut mod_files: Vec<ModFile> = Vec::with_capacity(total_files);
    let mut plugins_found: Vec<String> = Vec::new();
    // (filename, cache_path) — used to extract master-file requirements
    let mut plugin_cache_files: Vec<(String, PathBuf)> = Vec::new();
    for (file_idx, (src_abs, dest_rel)) in file_list.iter().enumerate() {
        // Apply game-specific rules
        let ruled_path = rules::apply_rules(&game_rules, &dest_rel.to_string_lossy());

        // Normalize backslashes but preserve original casing
        let original_rel = ruled_path.replace('\\', "/");

        // Detect if this file came from the game root (../ prefix from the external-file
        // detector). Strip it now so paths into the cache and data-subdir stripping work
        // correctly — the root-destination is tracked separately via explicit_root.
        let explicit_root = original_rel.starts_with("../");
        let original_rel = if explicit_root {
            original_rel[3..].to_string()
        } else {
            original_rel
        };

        // Lowercase for conflict detection key
        let lowercase_rel = utils_paths::lowercase_path(Path::new(&original_rel));

        // Strip data subdir prefix to avoid nesting (e.g. data/ inside Data/).
        let lowercase_rel =
            self::paths::strip_data_subdir_prefix(&lowercase_rel, &game.data_subdir);
        let original_rel =
            self::paths::strip_data_subdir_prefix_str(&original_rel, &game.data_subdir);

        // Directory sentinel: src_abs is a directory (no files to deploy, but the game
        // requires the folder to exist — e.g. JContainers' Domains/ folder).
        // Create the directory in the cache and record it with a trailing '/' so the
        // deployer knows to call create_dir_all rather than hardlink a file.
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

        // Copy to cache (cache uses lowercase paths internally)
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

        // Determine per-file install target.
        // explicit_root (../prefix) wins; user override in file_targets second; auto-detect last.
        //
        // Key is the rules-normalized path (matching what pre_install_dialog shows), not the
        // raw archive dest_rel, so user overrides set in the dialog are looked up correctly.
        let file_key = ruled_path.replace('\\', "/");
        let deploy_to_root = if explicit_root {
            file_targets
                .get(file_key.as_str())
                .cloned()
                .unwrap_or(InstallTarget::Root)
                == InstallTarget::Root
        } else if game.engine == GameEngine::Bethesda {
            // Only Bethesda games use root-level auto-detection (SKSE loaders, ASI plugins…).
            file_targets
                .get(file_key.as_str())
                .cloned()
                .unwrap_or_else(|| auto_detect_install_target(&file_key))
                == InstallTarget::Root
        } else {
            // For all other engines (REDEngine, and any future ones), data_subdir IS the
            // game root — auto-detecting Root would place files outside the game directory.
            // Explicit user overrides via file_targets are still respected.
            file_targets
                .get(file_key.as_str())
                .cloned()
                .unwrap_or(InstallTarget::Data)
                == InstallTarget::Root
        };

        // Determine the recorded relative paths (lowercase key + original for deployment).
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

        // Detect plugin files (preserve original casing for filenames)
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

    // Deduplicate by game_rel_lowercase — keep last occurrence (later entries win)
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

    // Record in database
    let priority = tracker.next_priority(&game.id).await?;

    let (nexus_mod_id, nexus_file_id, nexus_domain) = match nexus_ids {
        Some((mid, fid, dom)) => (Some(mid), Some(fid), Some(dom)),
        None => (None, None, None),
    };

    // Derive mod-level install_target for the properties dialog: Root only if every file is Root.
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

    // Record plugins and their master-file requirements
    if !plugin_cache_files.is_empty() {
        let mut load_order = tracker.next_load_order(&game.id).await?;
        let mut plugin_records = Vec::with_capacity(plugin_cache_files.len());
        // (plugin_id, master list) pairs collected for a second-pass insert
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

/// Merge a file list into an already-existing mod.
///
/// Files are cached into the existing mod's cache directory (overwriting
/// conflicts) and their `mod_files` records are upserted so the new paths
/// win on the next deploy.  Plugins contained in the new files are inserted
/// only when they are not already part of the mod (preserving existing
/// load-order and enabled state).
///
/// Returns the number of files merged.
pub async fn merge_files_into_mod(
    file_list: Vec<(PathBuf, PathBuf)>,
    game: &Game,
    mod_name: &str,
    existing_mod_id: &str,
    tracker: &Tracker,
    file_targets: HashMap<String, InstallTarget>,
    stripped_wrapper: Option<String>,
    on_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
) -> Result<usize> {
    // Apply REDEngine-specific path fixups before any further processing.
    // For Bethesda games this is a no-op.
    let file_list = if game.engine == GameEngine::REDEngine {
        paths::apply_redengine_path_fixups(game, mod_name, stripped_wrapper.as_deref(), file_list)
    } else {
        file_list
    };

    let game_rules = rules::rules_for_game(&game.id);
    let cache_dir = utils_paths::mod_cache_dir(existing_mod_id)?;
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

        // Directory sentinel (see add_mod_with_file_list for rationale).
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

        let file_key = ruled_path.replace('\\', "/");
        let deploy_to_root = if explicit_root {
            file_targets
                .get(file_key.as_str())
                .cloned()
                .unwrap_or(InstallTarget::Root)
                == InstallTarget::Root
        } else if game.engine == GameEngine::Bethesda {
            // Only Bethesda games use root-level auto-detection (SKSE loaders, ASI plugins…).
            file_targets
                .get(file_key.as_str())
                .cloned()
                .unwrap_or_else(|| auto_detect_install_target(&file_key))
                == InstallTarget::Root
        } else {
            // For all other engines (REDEngine, and any future ones), data_subdir IS the
            // game root — auto-detecting Root would place files outside the game directory.
            // Explicit user overrides via file_targets are still respected.
            file_targets
                .get(file_key.as_str())
                .cloned()
                .unwrap_or(InstallTarget::Data)
                == InstallTarget::Root
        };

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

    // Deduplicate: keep last occurrence per key
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

    // Insert plugins that are not already tracked under this mod.
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn is_plugin(rel_path: &str) -> bool {
    let l = rel_path.to_lowercase();
    l.ends_with(".esp") || l.ends_with(".esm") || l.ends_with(".esl")
}
