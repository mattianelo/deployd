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
    pub files_deployed: usize,
    pub conflicts_resolved: usize,
}

/// Purge all deployed hardlinks from the game folder without redeploying.
/// Returns the number of files removed.
pub async fn purge(game: &Game, tracker: &Tracker) -> Result<usize> {
    let game_data = paths::game_data_dir(game);

    // Bake any externally-modified plugins into cache before removing them,
    // so a subsequent redeploy restores the cleaned version rather than the
    // dirty cached original.
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(plugin_files) = tracker.get_deployed_plugin_files(&game.id).await {
            for (_, ref game_rel_orig, ref cache_path) in plugin_files {
                let disk_path = resolve_deploy_path(game_rel_orig, &game.path, &game_data);
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
                if disk_ino != cache_ino
                    && let Err(e) = fs::copy(&disk_path, cache_path)
                {
                    eprintln!(
                        "[deployd] WARNING: could not bake modified plugin \
                         '{game_rel_orig}' to cache before purge: {e}"
                    );
                }
            }
        }
    }

    let deployed = tracker.get_deployed_files().await?;
    let count = deployed.len();
    for f in &deployed {
        let is_root = f.game_rel_original.starts_with("../");
        let deploy_path = resolve_deploy_path(&f.game_rel_original, &game.path, &game_data);

        // Directory sentinel: attempt to remove the directory only if it is empty.
        // Non-empty dirs (e.g. Domains/ populated by the game at runtime) are left in place.
        if f.game_rel_original.ends_with('/') {
            let _ = fs::remove_dir(&deploy_path);
            // Clean up any empty parent dirs the sentinel may have created on its own
            // (e.g. the bare "uio/" parent left after removing "uio/public/").
            if let Some(parent) = deploy_path.parent() {
                let stop_at = if is_root { &game.path } else { &game_data };
                remove_empty_parents(parent, stop_at);
            }
            continue;
        }

        if deploy_path.exists() {
            let _ = fs::remove_file(&deploy_path);
        }
        if let Some(parent) = deploy_path.parent() {
            // Stop at the appropriate base: game root for root-level files, game data otherwise.
            let stop_at = if is_root { &game.path } else { &game_data };
            remove_empty_parents(parent, stop_at);
        }
    }
    tracker.clear_deployed_files().await?;
    Ok(count)
}

/// Deploy all enabled mods for a game: purge old hardlinks, resolve conflicts
/// by priority, and create new hardlinks from cache to game directory.
pub async fn deploy(game: &Game, tracker: &Tracker) -> Result<DeployResult> {
    let game_data = paths::game_data_dir(game);

    // 0. Pre-purge: bake any externally-modified managed plugins back into cache so
    //    their changes survive the purge + re-hardlink cycle below.
    //    Covers the case where xEdit (safe-save/rename) broke the hardlink: the on-disk
    //    file now has a new inode with cleaned content while the cache still holds the
    //    dirty original.  Copying disk → cache here ensures the re-hardlink below uses
    //    the cleaned version.  In-place saves (same inode) are a no-op.
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(plugin_files) = tracker.get_deployed_plugin_files(&game.id).await {
            for (_, ref game_rel_orig, ref cache_path) in plugin_files {
                let disk_path = resolve_deploy_path(game_rel_orig, &game.path, &game_data);
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
                if disk_ino != cache_ino
                    && let Err(e) = fs::copy(&disk_path, cache_path)
                {
                    eprintln!(
                        "[deployd] WARNING: could not bake modified plugin \
                             '{game_rel_orig}' to cache: {e}"
                    );
                }
            }
        }
    }

    // 1. Purge existing deployment
    let deployed = tracker.get_deployed_files().await?;
    for f in &deployed {
        let is_root = f.game_rel_original.starts_with("../");
        let deploy_path = resolve_deploy_path(&f.game_rel_original, &game.path, &game_data);

        if f.game_rel_original.ends_with('/') {
            // Directory sentinel: only remove if empty.
            let _ = fs::remove_dir(&deploy_path);
            if let Some(parent) = deploy_path.parent() {
                let stop_at = if is_root { &game.path } else { &game_data };
                remove_empty_parents(parent, stop_at);
            }
            continue;
        }

        if deploy_path.exists() {
            let _ = fs::remove_file(&deploy_path);
        }
        // Clean up empty parent directories
        if let Some(parent) = deploy_path.parent() {
            let stop_at = if is_root { &game.path } else { &game_data };
            remove_empty_parents(parent, stop_at);
        }
    }
    tracker.clear_deployed_files().await?;

    // Resolve conflicts: get all files ordered by priority DESC per path
    let all_files = tracker.get_all_mod_files_by_priority(&game.id).await?;

    // Group by game_rel_lowercase, take the first (highest priority) per path
    let mut winners: Vec<ModFile> = Vec::new();
    let mut conflicts_resolved: usize = 0;
    let mut last_path: Option<String> = None;
    let mut current_path_count: usize = 0;

    for (game_rel, mod_id, cache_path, game_rel_original, _priority) in &all_files {
        if last_path.as_deref() == Some(game_rel.as_str()) {
            // Same path as previous row — this is a conflict loser, skip it
            current_path_count += 1;
            continue;
        }

        // New path — if previous path had conflicts, count them
        if current_path_count > 1 {
            conflicts_resolved += 1;
        }

        // This is the winner (highest priority for this path)
        winners.push(ModFile {
            mod_id: mod_id.clone(),
            game_rel_lowercase: game_rel.clone(),
            game_rel_original: game_rel_original.clone(),
            cache_path: cache_path.clone(),
        });

        last_path = Some(game_rel.clone());
        current_path_count = 1;
    }
    // Count the last group
    if current_path_count > 1 {
        conflicts_resolved += 1;
    }

    // Pre-compute canonical directory casings from all winners.
    // When multiple winners use the same directory with different casings, prefer
    // the non-all-lowercase version to preserve readability
    let canonical_dirs = build_dir_canonical_map(&winners);

    // Pre-build a lowercase path set from the previous deployment so the case-conflict
    // check below is O(1) per file instead of O(deployed.len()).
    let deployed_lower: HashSet<String> = deployed
        .iter()
        .map(|d| {
            resolve_deploy_path(&d.game_rel_original, &game.path, &game_data)
                .to_string_lossy()
                .to_lowercase()
        })
        .collect();

    // Deploy winners: create hardlinks (use original casing for filesystem paths,
    // with case-insensitive directory matching to avoid duplicate dirs like Interface/ vs interface/)
    // dir_cache amortises repeated fs::read_dir calls: each directory is scanned at most once.
    let mut dir_cache: HashMap<PathBuf, HashMap<String, PathBuf>> = HashMap::new();
    let mut deployed_files: Vec<ModFile> = Vec::with_capacity(winners.len());
    for f in &winners {
        let cache_file = PathBuf::from(&f.cache_path);

        // Directory sentinel: no file to hardlink, just ensure the directory exists.
        // Recorded so purge() can attempt a cleanup via remove_dir (only removes if empty).
        // Use create_dirs_case_insensitive so we reuse an already-present directory that
        // only differs in casing (e.g. ySI's "../uio/public/" must not duplicate UIO's
        // existing "../UIO/" folder).
        if f.game_rel_lowercase.ends_with('/') {
            let (base, rel) = if let Some(root_rel) = f.game_rel_original.strip_prefix("../") {
                (&game.path, root_rel.trim_end_matches('/'))
            } else {
                (&game_data, f.game_rel_original.trim_end_matches('/'))
            };
            let dir_comps: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
            let dir_path =
                match create_dirs_case_insensitive(base, &dir_comps, &canonical_dirs, &mut dir_cache) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!(
                            "[deployd] WARNING: cannot create sentinel dir '{}': {e}",
                            rel
                        );
                        continue;
                    }
                };
            dlog!("[deployd] sentinel dir: {}", dir_path.display());
            // Record the actual on-disk path so purge() targets the correct casing.
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
            deployed_files.push(ModFile {
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
            // Remove stale Deployd-managed files whose casing changed between deployments.
            // We ONLY remove a file here if it was present in the previous deployment
            // (`deployed` list). Vanilla game files and user-added files that were never
            // tracked by Deployd must never be deleted at this stage — the purge above
            // already cleaned up all tracked files, so any surviving case-insensitive
            // duplicate that is NOT in `deployed` is not ours to touch.
            if let Ok(entries) = fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let entry_path = entry.path();
                    if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
                        && entry.file_name().eq_ignore_ascii_case(fname)
                        && entry_path != deploy_target
                    {
                        let was_ours =
                            deployed_lower.contains(&entry_path.to_string_lossy().to_lowercase());
                        if was_ours {
                            let _ = fs::remove_file(&entry_path);
                        }
                    }
                }
            }
        }
        // Try hardlink first (zero-copy, same inode). In Flatpak, game dirs accessed
        // via a separate filesystem permission (e.g. Steam's ~/.var/app path) are on
        // a different bind mount from the cache, so hard_link returns EXDEV (cross-
        // device). Fall back to a plain copy in that case.
        if let Err(e) = fs::hard_link(&cache_file, &deploy_target) {
            if e.raw_os_error() == Some(18) {
                // EXDEV – different bind-mount point (typical for Steam Flatpak games).
                fs::copy(&cache_file, &deploy_target).with_context(|| {
                    format!(
                        "Copy fallback failed: {} → {}",
                        cache_file.display(),
                        deploy_target.display()
                    )
                })?;
            } else {
                return Err(e).with_context(|| {
                    format!(
                        "Hardlink failed: {} → {}",
                        cache_file.display(),
                        deploy_target.display()
                    )
                });
            }
        }

        // Record the actual on-disk relative path for accurate purging
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
        deployed_files.push(ModFile {
            mod_id: f.mod_id.clone(),
            game_rel_lowercase: f.game_rel_lowercase.clone(),
            game_rel_original: actual_original,
            cache_path: f.cache_path.clone(),
        });
    }

    // Record deployed state (with actual on-disk paths)
    tracker.record_deployed_files(&deployed_files).await?;

    // Write Plugins.txt and ArchiveInvalidation INI — Bethesda games only.
    // REDEngine games (CP2077, Witcher 3) have no plugin list or INI management.
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

    let files_deployed = winners.len();
    Ok(DeployResult {
        files_deployed,
        conflicts_resolved,
    })
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
/// When multiple mods use the same directory with different casings, the non-all-lowercase form wins. This ensures that the first
/// properly-cased mod to reference a directory establishes its on-disk name, and
/// subsequent mods with lowercase paths reuse that directory via case-insensitive matching.
fn build_dir_canonical_map(winners: &[ModFile]) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for f in winners {
        let rel = if let Some(r) = f.game_rel_original.strip_prefix("../") {
            r
        } else {
            &f.game_rel_original
        };
        // For sentinels (trailing '/'), all components are directory names.
        // For files, only the parent path components are directories.
        // Trim trailing slash so Path treats the last segment as a Normal component.
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
                        // Prefer any casing that has at least one uppercase letter
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

        // Populate cache for `current` on first visit.
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
            // No existing directory found — create one. Prefer canonical casing if known.
            let name = canonical
                .get(&component_lower)
                .map(|s| s.as_str())
                .unwrap_or(component);
            let new_dir = current.join(name);
            fs::create_dir_all(&new_dir)?;
            // Invalidate parent cache entry so sibling files see the new subdirectory.
            dir_cache.remove(new_dir.parent().unwrap_or(&new_dir));
            new_dir
        };
    }

    Ok(current)
}

/// Create parent directories for a deploy target, reusing existing directories
/// that match case-insensitively. Returns the resolved path with the filename appended.
///
/// `dir_cache` maps each visited directory to a `{lowercase_name → actual_path}` index,
/// avoiding repeated `fs::read_dir` calls on the same parent across files.
/// When a new subdirectory is created the parent's entry is removed so the next file
/// in that directory sees a fresh listing.
fn ensure_dirs_case_insensitive(
    base: &PathBuf,
    rel_path: &str,
    canonical: &HashMap<String, String>,
    dir_cache: &mut HashMap<PathBuf, HashMap<String, PathBuf>>,
) -> Result<PathBuf> {
    let components: Vec<&str> = rel_path.split('/').collect();
    let dir_components = &components[..components.len().saturating_sub(1)];
    let mut current = create_dirs_case_insensitive(base, dir_components, canonical, dir_cache)?;

    // Append filename with original casing
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
