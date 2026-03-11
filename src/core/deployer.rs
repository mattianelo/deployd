use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::game;
use crate::core::tracker::Tracker;
use crate::dlog;
use crate::models::game::{Game, GameEngine};
use crate::models::manifest::ModFile;
use crate::utils::{paths, plugins_txt};

/// Result of a deployment operation.
#[derive(Debug)]
pub struct DeployResult {
    /// Total number of files in the final deployed state.
    pub files_total: usize,
    /// Files actually hardlinked during this deploy (additions + updates).
    pub files_added: usize,
    /// Files removed from the game folder during this deploy.
    pub files_removed: usize,
    pub conflicts_resolved: usize,
}

/// Purge all deployed hardlinks from the game folder without redeploying.
/// Returns the number of files removed.
pub async fn purge(game: &Game, tracker: &Tracker) -> Result<usize> {
    let game_data = paths::game_data_dir(game);
    bake_modified_plugins(game, tracker, &game_data).await;

    let deployed = tracker.get_deployed_files().await?;
    let count = deployed.len();
    for f in &deployed {
        remove_deployed_file(f, game, &game_data);
    }
    tracker.clear_deployed_files().await?;
    Ok(count)
}

/// Deploy all enabled mods for a game using delta deployment: compute the desired
/// state, then only remove files no longer needed and add files not yet present.
/// Files that are already correctly deployed are left untouched.
pub async fn deploy(game: &Game, tracker: &Tracker) -> Result<DeployResult> {
    let game_data = paths::game_data_dir(game);

    // 0. Bake any externally-modified plugins back into cache so their changes
    //    survive the re-hardlink cycle (xEdit safe-save breaks hardlinks).
    bake_modified_plugins(game, tracker, &game_data).await;

    // 1. Current deployment state indexed by lowercase path.
    let deployed = tracker.get_deployed_files().await?;
    let deployed_map: HashMap<&str, &ModFile> = deployed
        .iter()
        .map(|f| (f.game_rel_lowercase.as_str(), f))
        .collect();

    // 2. Desired state: conflict-resolved winners in priority order.
    let (winners, conflicts_resolved) = compute_winners(tracker, &game.id).await?;
    let winners_map: HashMap<&str, &ModFile> = winners
        .iter()
        .map(|f| (f.game_rel_lowercase.as_str(), f))
        .collect();

    eprintln!("[deployd] delta: deployed={}, winners={}", deployed.len(), winners.len());

    // 3. Classify each deployed file as "keep" or "remove".
    let mut to_remove: Vec<&ModFile> = Vec::new();
    for dep in &deployed {
        match winners_map.get(dep.game_rel_lowercase.as_str()) {
            // No longer wanted.
            None => to_remove.push(dep),
            // Conflict winner changed — remove old, re-link new.
            Some(want) if want.cache_path != dep.cache_path => to_remove.push(dep),
            _ => {}
        }
    }

    // 4. Classify each winner as "add" or "skip".
    let mut to_add: Vec<&ModFile> = Vec::new();
    for winner in &winners {
        match deployed_map.get(winner.game_rel_lowercase.as_str()) {
            None => to_add.push(winner),
            Some(dep) if dep.cache_path != winner.cache_path => to_add.push(winner),
            _ => {}
        }
    }

    eprintln!("[deployd] delta: to_remove={}, to_add={}", to_remove.len(), to_add.len());
    // Sample first mismatch to diagnose cache_path differences
    if !to_add.is_empty() {
        let w = &to_add[0];
        if let Some(dep) = deployed_map.get(w.game_rel_lowercase.as_str()) {
            eprintln!("[deployd] sample mismatch path={:?}", w.game_rel_lowercase);
            eprintln!("[deployd]   deployed cache_path={:?}", dep.cache_path);
            eprintln!("[deployd]   winner  cache_path={:?}", w.cache_path);
        } else {
            eprintln!("[deployd] sample new path={:?} (not in deployed_map)", w.game_rel_lowercase);
        }
    }

    // 5. Pre-build lowercase path set from *all* previously deployed files.
    //    Used to distinguish our stale case-variant files from vanilla game files.
    let deployed_lower: HashSet<String> = deployed
        .iter()
        .map(|d| {
            resolve_deploy_path(&d.game_rel_original, &game.path, &game_data)
                .to_string_lossy()
                .to_lowercase()
        })
        .collect();

    // 6. Apply removals first so that when we re-link updated files the old
    //    hardlinks are already gone.
    for f in &to_remove {
        remove_deployed_file(f, game, &game_data);
    }

    // 7. Build canonical directory casing map from all winners (unchanged + new).
    let canonical_dirs = build_dir_canonical_map(&winners);

    // 8. Hardlink additions; collect only the newly-linked files for DB insertion.
    //    Unchanged files remain in deployed_files as-is — no write needed.
    let mut newly_linked: Vec<ModFile> = Vec::new();
    let mut dir_cache: HashMap<PathBuf, HashMap<String, PathBuf>> = HashMap::new();
    for f in &to_add {
        let cache_file = PathBuf::from(&f.cache_path);

        // Directory sentinel: ensure the directory exists, record it.
        if f.game_rel_lowercase.ends_with('/') {
            let (base, rel) = if let Some(root_rel) = f.game_rel_original.strip_prefix("../") {
                (&game.path, root_rel.trim_end_matches('/'))
            } else {
                (&game_data, f.game_rel_original.trim_end_matches('/'))
            };
            let dir_comps: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
            let dir_path = match create_dirs_case_insensitive(
                base,
                &dir_comps,
                &canonical_dirs,
                &mut dir_cache,
            ) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[deployd] WARNING: cannot create sentinel dir '{rel}': {e}");
                    continue;
                }
            };
            dlog!("[deployd] sentinel dir: {}", dir_path.display());
            let actual_rel = dir_path
                .strip_prefix(base)
                .unwrap_or(&dir_path)
                .to_string_lossy()
                .to_string();
            let actual_original = if f.game_rel_original.starts_with("../") {
                format!("../{actual_rel}/")
            } else {
                format!("{actual_rel}/")
            };
            newly_linked.push(ModFile {
                mod_id: f.mod_id.clone(),
                game_rel_lowercase: f.game_rel_lowercase.clone(),
                game_rel_original: actual_original,
                cache_path: f.cache_path.clone(),
            });
            continue;
        }

        let (base, rel) = if let Some(root_rel) = f.game_rel_original.strip_prefix("../") {
            (&game.path, root_rel)
        } else {
            (&game_data, f.game_rel_original.as_str())
        };
        let deploy_target =
            ensure_dirs_case_insensitive(base, rel, &canonical_dirs, &mut dir_cache)?;

        if deploy_target.exists() {
            fs::remove_file(&deploy_target)?;
        } else if let (Some(parent), Some(fname)) =
            (deploy_target.parent(), deploy_target.file_name())
        {
            // Remove stale case-variant files that were previously deployed by us
            // but survived because the purge step only removed exact-path entries.
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
                        && entry.file_name().eq_ignore_ascii_case(fname)
                        && entry_path != deploy_target
                    {
                        let was_ours = deployed_lower
                            .contains(&entry_path.to_string_lossy().to_lowercase());
                        if was_ours {
                            let _ = fs::remove_file(&entry_path);
                        }
                    }
                }
            }
        }

        fs::hard_link(&cache_file, &deploy_target).with_context(|| {
            format!(
                "Hardlink failed: {} → {}",
                cache_file.display(),
                deploy_target.display()
            )
        })?;

        let actual_rel = deploy_target
            .strip_prefix(base)
            .unwrap_or(&deploy_target)
            .to_string_lossy()
            .to_string();
        let actual_original = if f.game_rel_original.starts_with("../") {
            format!("../{actual_rel}")
        } else {
            actual_rel
        };
        newly_linked.push(ModFile {
            mod_id: f.mod_id.clone(),
            game_rel_lowercase: f.game_rel_lowercase.clone(),
            game_rel_original: actual_original,
            cache_path: f.cache_path.clone(),
        });
    }

    // 9. Apply delta to the database: remove stale records, insert new ones.
    //    Unchanged files already in deployed_files are left untouched.
    let remove_paths: Vec<&str> = to_remove
        .iter()
        .map(|f| f.game_rel_lowercase.as_str())
        .collect();
    tracker.remove_deployed_files(&remove_paths).await?;
    tracker.record_deployed_files(&newly_linked).await?;

    // 10. Write Plugins.txt and ArchiveInvalidation INI — Bethesda games only.
    if game.engine == GameEngine::Bethesda {
        let plugins = tracker.list_plugins(&game.id).await?;
        let plugins_paths = game::plugins_txt_paths(game);
        if plugins_paths.is_empty() {
            eprintln!("deployd: WINE prefix not found, skipping Plugins.txt");
        }
        for plugins_path in &plugins_paths {
            plugins_txt::write_plugins_txt(plugins_path, &plugins)?;
        }

        let ini_paths = game::custom_ini_paths(game);
        if ini_paths.is_empty() {
            eprintln!("deployd: WINE prefix not found, skipping custom INI");
        }
        for ini_path in &ini_paths {
            plugins_txt::ensure_archive_invalidation(ini_path)?;
        }
    }

    Ok(DeployResult {
        files_total: winners.len(),
        files_added: newly_linked.len(),
        files_removed: to_remove.len(),
        conflicts_resolved,
    })
}

/// Bake externally-modified plugins back into cache before any purge/deploy step.
///
/// Covers the case where a tool (e.g. xEdit safe-save/rename) broke the hardlink:
/// the on-disk file has a new inode with cleaned content while the cache still holds
/// the dirty original. Copying disk → cache here ensures the next hardlink uses the
/// cleaned version. In-place saves (same inode) are a no-op.
async fn bake_modified_plugins(game: &Game, tracker: &Tracker, game_data: &PathBuf) {
    use std::os::unix::fs::MetadataExt;
    let Ok(plugin_files) = tracker.get_deployed_plugin_files(&game.id).await else {
        return;
    };
    for (_, ref game_rel_orig, ref cache_path) in plugin_files {
        let disk_path = resolve_deploy_path(game_rel_orig, &game.path, game_data);
        if !disk_path.exists() || !cache_path.exists() {
            continue;
        }
        let disk_ino = match fs::metadata(&disk_path) {
            Ok(m) => m.ino(),
            Err(_) => continue,
        };
        let cache_ino = match fs::metadata(cache_path) {
            Ok(m) => m.ino(),
            Err(_) => continue,
        };
        if disk_ino != cache_ino {
            if let Err(e) = fs::copy(&disk_path, cache_path) {
                eprintln!(
                    "[deployd] WARNING: could not bake modified plugin \
                     '{game_rel_orig}' to cache: {e}"
                );
            }
        }
    }
}

/// Resolve conflict winners from the full mod file list ordered by priority DESC.
/// Returns the winner list and the number of conflicts resolved.
async fn compute_winners(
    tracker: &Tracker,
    game_id: &str,
) -> Result<(Vec<ModFile>, usize)> {
    let all_files = tracker.get_all_mod_files_by_priority(game_id).await?;
    let mut winners: Vec<ModFile> = Vec::new();
    let mut conflicts_resolved: usize = 0;
    let mut last_path: Option<String> = None;
    let mut current_path_count: usize = 0;

    for (game_rel, mod_id, cache_path, game_rel_original, _priority) in &all_files {
        if last_path.as_deref() == Some(game_rel.as_str()) {
            current_path_count += 1;
            continue;
        }
        if current_path_count > 1 {
            conflicts_resolved += 1;
        }
        winners.push(ModFile {
            mod_id: mod_id.clone(),
            game_rel_lowercase: game_rel.clone(),
            game_rel_original: game_rel_original.clone(),
            cache_path: cache_path.clone(),
        });
        last_path = Some(game_rel.clone());
        current_path_count = 1;
    }
    if current_path_count > 1 {
        conflicts_resolved += 1;
    }
    Ok((winners, conflicts_resolved))
}

/// Remove a single deployed file or directory sentinel from the game folder.
fn remove_deployed_file(f: &ModFile, game: &Game, game_data: &PathBuf) {
    let is_root = f.game_rel_original.starts_with("../");
    let deploy_path = resolve_deploy_path(&f.game_rel_original, &game.path, game_data);
    let stop_at = if is_root { &game.path } else { game_data };

    if f.game_rel_original.ends_with('/') {
        let _ = fs::remove_dir(&deploy_path);
        if let Some(parent) = deploy_path.parent() {
            remove_empty_parents(parent, stop_at);
        }
    } else {
        if deploy_path.exists() {
            let _ = fs::remove_file(&deploy_path);
        }
        if let Some(parent) = deploy_path.parent() {
            remove_empty_parents(parent, stop_at);
        }
    }
}

/// Resolve the actual filesystem path for a recorded relative path.
/// Paths starting with "../" are relative to the game root (script extenders, ENB).
/// All other paths are relative to the game data directory.
fn resolve_deploy_path(game_rel: &str, game_root: &PathBuf, game_data: &PathBuf) -> PathBuf {
    if let Some(root_rel) = game_rel.strip_prefix("../") {
        game_root.join(root_rel)
    } else {
        game_data.join(game_rel)
    }
}

/// Build a map from lowercase directory component name to the best-cased version
/// seen across all winner files.
///
/// When multiple mods use the same directory with different casings, the non-all-lowercase
/// form wins, preserving readability for tools that browse the game folder.
fn build_dir_canonical_map(winners: &[ModFile]) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for f in winners {
        let rel = if let Some(r) = f.game_rel_original.strip_prefix("../") {
            r
        } else {
            &f.game_rel_original
        };
        let path = Path::new(rel.trim_end_matches('/'));
        let dir_path = if f.game_rel_lowercase.ends_with('/') {
            path
        } else {
            path.parent().unwrap_or(Path::new(""))
        };
        for component in dir_path.components() {
            if let std::path::Component::Normal(c) = component {
                let c_str = c.to_string_lossy();
                let c_lower = c_str.to_lowercase();
                map.entry(c_lower)
                    .and_modify(|existing| {
                        if c_str.chars().any(|ch| ch.is_uppercase()) {
                            *existing = c_str.to_string();
                        }
                    })
                    .or_insert_with(|| c_str.to_string());
            }
        }
    }
    map
}

/// Create all given directory components under `base`, reusing existing directories
/// case-insensitively. Returns the actual on-disk path of the deepest component created.
///
/// Shared by `ensure_dirs_case_insensitive` (for file parent dirs) and the sentinel
/// handler (where all components are directories, not a filename).
fn create_dirs_case_insensitive(
    base: &PathBuf,
    components: &[&str],
    canonical: &HashMap<String, String>,
    dir_cache: &mut HashMap<PathBuf, HashMap<String, PathBuf>>,
) -> Result<PathBuf> {
    let mut current = base.clone();

    for component in components {
        if component.is_empty() {
            continue;
        }
        let component_lower = component.to_lowercase();

        if !dir_cache.contains_key(&current) {
            let listing = fs::read_dir(&current)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .filter(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
                        .map(|e| (e.file_name().to_string_lossy().to_lowercase(), e.path()))
                        .collect::<HashMap<String, PathBuf>>()
                })
                .unwrap_or_default();
            dir_cache.insert(current.clone(), listing);
        }

        let existing = dir_cache
            .get(&current)
            .and_then(|m| m.get(&component_lower))
            .cloned();
        current = if let Some(path) = existing {
            path
        } else {
            let name = canonical
                .get(&component_lower)
                .map(|s| s.as_str())
                .unwrap_or(component);
            let new_dir = current.join(name);
            fs::create_dir_all(&new_dir)?;
            dir_cache.remove(new_dir.parent().unwrap_or(&new_dir));
            new_dir
        };
    }

    Ok(current)
}

/// Create parent directories for a deploy target, reusing existing directories
/// that match case-insensitively. Returns the resolved path with the filename appended.
fn ensure_dirs_case_insensitive(
    base: &PathBuf,
    rel_path: &str,
    canonical: &HashMap<String, String>,
    dir_cache: &mut HashMap<PathBuf, HashMap<String, PathBuf>>,
) -> Result<PathBuf> {
    let components: Vec<&str> = rel_path.split('/').collect();
    let dir_components = &components[..components.len().saturating_sub(1)];
    let mut current = create_dirs_case_insensitive(base, dir_components, canonical, dir_cache)?;

    if let Some(filename) = components.last() {
        current = current.join(filename);
    }

    Ok(current)
}

/// Remove empty parent directories up to (but not including) the stop directory.
fn remove_empty_parents(dir: &std::path::Path, stop_at: &PathBuf) {
    let mut current = dir.to_path_buf();
    while current != *stop_at {
        if fs::read_dir(&current)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            let _ = fs::remove_dir(&current);
        } else {
            break;
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => break,
        }
    }
}
