use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Utc;
use tempfile::TempDir;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::core::rules;
use crate::core::tracker::Tracker;
use crate::dlog;
use crate::models::game::{Game, GameEngine};
use crate::models::manifest::ModFile;
use crate::models::mod_entry::{InstallTarget, ModEntry};
use crate::models::plugin::Plugin;
use crate::utils::{archive, fomod_resolver, paths};

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
        let (file_list, stripped_wrapper) = resolve_file_list(extracted_root)
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
        apply_redengine_path_fixups(game, mod_name, stripped_wrapper.as_deref(), file_list)
    } else {
        file_list
    };

    let game_rules = rules::rules_for_game(&game.id);
    let cache_dir = paths::mod_cache_dir(&mod_id)?;
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
        let lowercase_rel = paths::lowercase_path(Path::new(&original_rel));

        // Strip data subdir prefix to avoid nesting (e.g. data/ inside Data/).
        let lowercase_rel = strip_data_subdir_prefix(&lowercase_rel, &game.data_subdir);
        let original_rel = strip_data_subdir_prefix_str(&original_rel, &game.data_subdir);

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
        apply_redengine_path_fixups(game, mod_name, stripped_wrapper.as_deref(), file_list)
    } else {
        file_list
    };

    let game_rules = rules::rules_for_game(&game.id);
    let cache_dir = paths::mod_cache_dir(existing_mod_id)?;
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
        let lowercase_rel = paths::lowercase_path(Path::new(&original_rel));
        let lowercase_rel = strip_data_subdir_prefix(&lowercase_rel, &game.data_subdir);
        let original_rel = strip_data_subdir_prefix_str(&original_rel, &game.data_subdir);

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

/// Check if a relative path is a plugin file (.esp, .esm, .esl).
fn is_plugin(rel_path: &str) -> bool {
    let l = rel_path.to_lowercase();
    l.ends_with(".esp") || l.ends_with(".esm") || l.ends_with(".esl")
}

/// Auto-detect install target for a file based on its relative path.
///
/// Files at the archive root with executable/library extensions go to the game
/// root directory (Root). Everything else goes to the Data subdirectory (Data).
pub fn auto_detect_install_target(rel_path: &str) -> InstallTarget {
    let path = Path::new(rel_path);
    let is_root_level = path.parent().is_none_or(|p| p == Path::new(""));
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if is_root_level && matches!(ext.as_str(), "exe" | "dll" | "asi") {
        InstallTarget::Root
    } else {
        InstallTarget::Data
    }
}

// ---------------------------------------------------------------------------
// REDEngine path fixups
// ---------------------------------------------------------------------------

/// Apply REDEngine-specific path transformations to the extracted file list.
///
/// **Witcher 3 wrapping**: W3 mods must live under `Mods/{name}/` in the game
/// root. Archives are packaged many ways: already in `Mods/ModName/content/`,
/// or just `content/` / `scripts/` at the top level. If no file already starts
/// with a `Mods/` component, every file is prefixed with `Mods/{name}/` so
/// files never land in the game's own `content/` folder.
///
/// **Bare `.archive` routing**: for CP2077 (and any game whose `archive_mod_dir`
/// is set), a `.archive` file with no directory prefix is redirected into the
/// game's archive mod subdirectory (e.g. `archive/pc/mod/foo.archive`).
///
/// **Flat REDmod detection**: if `info.json` is present at the extracted root
/// (i.e. `dest_rel == "info.json"`), the package is a REDmod distributed without
/// its `mods/{name}/` wrapper. Every file is prefixed with `mods/{sanitized}/`
/// so the game's REDmod loader can find it.
///
/// Both transforms are no-ops for Bethesda games.
fn apply_redengine_path_fixups(
    game: &Game,
    mod_name: &str,
    stripped_wrapper: Option<&str>,
    file_list: Vec<(PathBuf, PathBuf)>,
) -> Vec<(PathBuf, PathBuf)> {
    // ── Witcher 3 ────────────────────────────────────────────────────────────
    // W3 mods go in Mods/{name}/ at the game root. Many mod archives ship with
    // content/ or scripts/ directly at the top, which would otherwise collide
    // with the base-game content/ folder. Wrap everything in Mods/{name}/ when
    // the archive doesn't already have a Mods/ top-level component.
    //
    // Exception: tool archives (Script Merger, debug tools, etc.) ship with
    // executables at the archive root and must be deployed to the game root
    // directly — the same heuristic Bethesda uses for SKSE/ENB.
    if is_witcher3(game) {
        let has_mods_root = file_list.iter().any(|(_, dest)| {
            dest.components()
                .next()
                .and_then(|c| c.as_os_str().to_str())
                .map(|s| s.eq_ignore_ascii_case("mods"))
                .unwrap_or(false)
        });
        if has_mods_root {
            // Archive already has the Mods/ModName/… structure — leave it alone.
            return file_list;
        }

        // Tool archives have .exe / .dll at the root level (e.g. Script Merger).
        // Deploy them directly to the game root without adding Mods/ wrapping.
        let is_tool_archive = file_list.iter().any(|(_, dest)| {
            let is_root_level = dest.parent().is_none_or(|p| p.as_os_str().is_empty());
            let ext = dest
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            is_root_level && matches!(ext.as_str(), "exe" | "dll")
        });
        if is_tool_archive {
            return file_list;
        }

        // Choose the Mods/ sub-folder name:
        // • If detect_wrapper stripped a wrapper dir (e.g. `modSkipMovies`), that IS
        //   the mod's canonical folder name — use it to preserve archive structure.
        // • Otherwise fall back to the user-provided mod name (archive shipped files
        //   like content/ directly at the root with no mod-name wrapper).
        let mod_folder = stripped_wrapper
            .map(sanitize_mod_name_preserve_case)
            .unwrap_or_else(|| sanitize_mod_name_preserve_case(mod_name));

        return file_list
            .into_iter()
            .map(|(src, dest)| {
                (
                    src,
                    PathBuf::from(format!("Mods/{mod_folder}/{}", dest.to_string_lossy())),
                )
            })
            .collect();
    }

    // ── CP2077 / generic REDEngine ───────────────────────────────────────────
    let archive_subdir = crate::core::game::archive_mod_dir(game);

    // Detect flat REDmod: info.json at the extracted root (no directory component).
    let is_flat_redmod = file_list
        .iter()
        .any(|(_, dest)| dest == Path::new("info.json"));

    if archive_subdir.is_none() && !is_flat_redmod {
        return file_list;
    }

    // Sanitize mod_name for use as a directory name: lowercase, spaces → underscores,
    // strip characters that are unsafe on common filesystems.
    let sanitized: String = mod_name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let redmod_prefix = format!("mods/{sanitized}");

    file_list
        .into_iter()
        .map(|(src, dest)| {
            let dest_str = dest.to_string_lossy();

            // Flat REDmod: prefix everything with mods/{name}/
            if is_flat_redmod {
                return (src, PathBuf::from(format!("{redmod_prefix}/{dest_str}")));
            }

            // Bare .archive routing (only when there is no directory component).
            if let Some(subdir) = archive_subdir {
                let has_dir = dest
                    .parent()
                    .map(|p| !p.as_os_str().is_empty())
                    .unwrap_or(false);
                let ext = dest
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !has_dir && ext == "archive" {
                    return (src, PathBuf::from(format!("{subdir}/{dest_str}")));
                }
            }

            (src, dest)
        })
        .collect()
}

/// Returns `true` for all known Witcher 3 game IDs.
fn is_witcher3(game: &Game) -> bool {
    matches!(
        game.id.as_str(),
        "witcher3" | "witcher3-goty" | "witcher3-steam"
    )
}

/// Sanitize a mod name for use as a filesystem directory name while
/// preserving original casing (used for Witcher 3 mod folders).
fn sanitize_mod_name_preserve_case(mod_name: &str) -> String {
    mod_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// File list resolution
// ---------------------------------------------------------------------------

/// Returns (file_list, stripped_wrapper) where stripped_wrapper is the name of the
/// single wrapper directory removed from the archive root (if any).
fn resolve_file_list(extracted_root: &Path) -> Result<(Vec<(PathBuf, PathBuf)>, Option<String>)> {
    // Check for FOMOD first
    if let Some(config_path) = fomod_resolver::detect_fomod(extracted_root) {
        let mappings = fomod_resolver::resolve_fomod_default(extracted_root, &config_path)?;
        let result = mappings
            .into_iter()
            .map(|m| (extracted_root.join(&m.source_relative), m.dest_relative))
            .collect();
        return Ok((result, None));
    }

    // Normal mod: apply wrapper stripping, then collect all files
    let (effective_root, stripped_wrapper) = detect_wrapper(extracted_root);

    let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
    // Track every non-fomod subdirectory and which ones have at least one tracked file
    // anywhere in their subtree. Used below to emit directory sentinels.
    let mut all_dirs: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut dirs_with_files: HashSet<PathBuf> = HashSet::new();

    for entry in WalkDir::new(&effective_root) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(&effective_root)?;
        // Skip fomod metadata even in non-FOMOD mods
        let rel_lower = rel.to_string_lossy().to_lowercase();
        if rel_lower.starts_with("fomod/") || rel_lower.starts_with("fomod\\") {
            continue;
        }

        if entry.file_type().is_file() {
            // Mark every ancestor directory as having at least one tracked file.
            let mut parent = rel.parent();
            while let Some(p) = parent {
                if p.as_os_str().is_empty() {
                    break;
                }
                dirs_with_files.insert(p.to_path_buf());
                parent = p.parent();
            }
            files.push((entry.path().to_path_buf(), rel.to_path_buf()));
        } else if entry.file_type().is_dir() && !rel.as_os_str().is_empty() {
            all_dirs.push((entry.path().to_path_buf(), rel.to_path_buf()));
        }
    }

    // Emit directory sentinels for folders whose entire subtree has no tracked files.
    // These folders may contain only hidden/system files excluded by the archiving tool
    // (e.g. JContainers' Domains/ folder) or must simply exist at runtime.
    // The installer detects sentinels via src_abs.is_dir() and creates the directory
    // in the game folder during deployment even with no file to hardlink.
    for (dir_abs, dir_rel) in all_dirs {
        if !dirs_with_files.contains(&dir_rel) {
            dlog!("[deployd] empty-dir sentinel: {}", dir_rel.display());
            files.push((dir_abs, dir_rel));
        }
    }

    Ok((files, stripped_wrapper))
}

// ---------------------------------------------------------------------------
// Wrapper stripping
// ---------------------------------------------------------------------------

/// Detect a single wrapper directory.
///
/// A wrapper exists when the root contains exactly one non-fomod subdirectory
/// and zero meaningful files (ignoring readmes, changelogs, etc.), AND the
/// subdirectory is not a known Bethesda content directory (SKSE, Meshes, etc.).
///
/// Returns `(effective_root, stripped_wrapper_name)` where `stripped_wrapper_name`
/// is the original directory name if a wrapper was stripped (`Some`), else `None`.
fn detect_wrapper(extracted_root: &Path) -> (PathBuf, Option<String>) {
    let entries: Vec<_> = match fs::read_dir(extracted_root) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return (extracted_root.to_path_buf(), None),
    };

    let mut dirs = Vec::new();
    let mut has_meaningful_file = false;

    for entry in &entries {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let name_lower = entry.file_name().to_string_lossy().to_lowercase();
        if ft.is_dir() {
            if name_lower != "fomod" {
                // Store original name alongside lowercase for later use
                let orig_name = entry.file_name().to_string_lossy().to_string();
                dirs.push((entry.path(), name_lower, orig_name));
            }
        } else if ft.is_file() && !is_ignorable_file(&name_lower) {
            has_meaningful_file = true;
        }
    }

    if dirs.len() == 1 && !has_meaningful_file {
        let (path, name_lower, orig_name) = dirs.into_iter().next().unwrap();
        if is_known_content_dir(&name_lower) {
            (extracted_root.to_path_buf(), None)
        } else {
            (path, Some(orig_name))
        }
    } else {
        (extracted_root.to_path_buf(), None)
    }
}

/// Check whether a directory name is a known game content directory
/// that should never be stripped as a wrapper.
/// Covers both Bethesda (SKSE, Meshes, …) and REDEngine (archive, mods, r6, …) layouts.
fn is_known_content_dir(name_lower: &str) -> bool {
    matches!(
        name_lower,
        // Bethesda
        "data"
            | "skse"
            | "f4se"
            | "nvse"
            | "fose"
            | "obse"
            | "mwse"
            | "meshes"
            | "textures"
            | "sound"
            | "music"
            | "scripts"
            | "source"
            | "interface"
            | "strings"
            | "seq"
            | "grass"
            | "lodsettings"
            | "shadersfx"
            | "vis"
            | "materials"
            | "geometries"
            | "animations"
            | "plugins"
            | "docs"
            | "tools"
            | "edit scripts"
            | "calientetools"
            | "netscriptframework"
            | "dllplugins"
            | "asi"
            | "video"
            | "videos"
            | "mcm"   // MCM (Mod Configuration Menu) — Config/ and Settings/ live inside
            // REDEngine (Cyberpunk 2077 / The Witcher 3)
            | "archive"   // archive/pc/mod/ — CP2077 & W3 mod archives
            | "mods"      // REDmod directory (CP2077) and Mods/ (W3)
            | "r6"        // CP2077 scripts, tweaks, config
            | "red4ext"   // CP2077 REDscript extensions
            | "bin"       // CP2077 binary plugins (CET: bin/x64/plugins/…)
            | "content"   // W3 content subdirectories
            | "dlc" // W3 DLC-style mods
    )
}

/// Strip a leading data-subdir prefix from the deployment-relative path.
///
/// We deploy into `game/Data/`, so a relative path of `data/textures/foo.dds`
/// would create `game/Data/data/textures/foo.dds`. This strips the redundant
/// prefix regardless of how it got there (archive layout, rules, FOMOD mapping).
fn strip_data_subdir_prefix(rel: &Path, data_subdir: &str) -> PathBuf {
    let s = rel.to_string_lossy();
    let prefix = format!("{}/", data_subdir.to_lowercase());
    if s.starts_with(&prefix) {
        PathBuf::from(&s[prefix.len()..])
    } else {
        rel.to_path_buf()
    }
}

/// Strip a leading data-subdir prefix from an original-cased path string.
/// Case-insensitive prefix match, but preserves original casing of the remainder.
fn strip_data_subdir_prefix_str(rel: &str, data_subdir: &str) -> String {
    let prefix_lower = format!("{}/", data_subdir.to_lowercase());
    if rel.to_lowercase().starts_with(&prefix_lower) {
        rel[prefix_lower.len()..].to_string()
    } else {
        rel.to_string()
    }
}

fn is_ignorable_file(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "readme.txt"
            | "readme.md"
            | "readme"
            | "changelog.txt"
            | "changelog.md"
            | "license.txt"
            | "license"
            | "credits.txt"
            | "version.txt"
    )
}
