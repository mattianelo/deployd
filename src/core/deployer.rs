use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::game;
use crate::core::game::eclipse::DOCS_PREFIX;
use crate::core::game::engine_handler::EngineHandler;
use crate::core::mod_folders;
use crate::core::tracker::Tracker;
use crate::dlog;
use crate::models::game::Game;
use crate::models::manifest::ModFile;
use crate::utils::paths;

#[derive(Debug)]
pub struct DeployResult {
    pub files_total: usize,
    pub files_added: usize,
    pub files_removed: usize,
    pub conflicts_resolved: usize,
}

pub async fn purge(game: &Game, tracker: &Tracker, cache_root: &Path) -> Result<usize> {
    let game_data = paths::game_data_dir(game);
    bake_modified_plugins(game, tracker, &game_data).await;

    let deployed = tracker.get_deployed_files(&game.id).await?;
    let count = deployed.len();
    for f in &deployed {
        remove_deployed_file(f, game, &game_data);
    }
    tracker.clear_deployed_files(&game.id).await?;

    // Restore all vanilla files that were backed up before mod deployment.
    if let Ok(backups) = tracker.get_all_vanilla_backups(&game.id).await {
        for (rel, backup_path) in backups {
            let deploy_path = resolve_deploy_path(&rel, &game.path, &game_data);
            if deploy_path.exists() {
                // File already present — nothing to restore; clean up the record.
                let _ = tracker.delete_vanilla_backup(&game.id, &rel).await;
                continue;
            }
            // remove_empty_parents may have deleted the parent dir; recreate it.
            if let Some(parent) = deploy_path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if fs::copy(&backup_path, &deploy_path).is_ok() {
                let _ = tracker.delete_vanilla_backup(&game.id, &rel).await;
            }
            // If copy failed, keep the DB record so the restore can be retried.
        }
    }

    if let Err(e) = mod_folders::refresh_named_mod_folders(tracker, &game.id, cache_root).await {
        eprintln!("[deployd] named_mods refresh failed: {e}");
    }

    Ok(count)
}

pub async fn deploy(game: &Game, tracker: &Tracker, cache_root: &Path) -> Result<DeployResult> {
    let game_data = paths::game_data_dir(game);

    bake_modified_plugins(game, tracker, &game_data).await;

    let deployed = tracker.get_deployed_files(&game.id).await?;
    let deployed_map: HashMap<&str, &ModFile> = deployed
        .iter()
        .map(|f| (f.game_rel_lowercase.as_str(), f))
        .collect();

    let (winners, conflicts_resolved) =
        compute_winners(tracker, &game.id, game::handler_for(&game.engine)).await?;
    let winners_map: HashMap<&str, &ModFile> = winners
        .iter()
        .map(|f| (f.game_rel_lowercase.as_str(), f))
        .collect();

    eprintln!(
        "[deployd] delta: deployed={}, winners={}",
        deployed.len(),
        winners.len()
    );

    let mut to_remove: Vec<&ModFile> = Vec::new();
    for dep in &deployed {
        match winners_map.get(dep.game_rel_lowercase.as_str()) {
            None => to_remove.push(dep),
            Some(want) if want.cache_path != dep.cache_path => to_remove.push(dep),
            _ => {}
        }
    }

    let mut to_add: Vec<&ModFile> = Vec::new();
    for winner in &winners {
        match deployed_map.get(winner.game_rel_lowercase.as_str()) {
            None => to_add.push(winner),
            Some(dep) if dep.cache_path != winner.cache_path => to_add.push(winner),
            _ => {}
        }
    }

    eprintln!(
        "[deployd] delta: to_remove={}, to_add={}",
        to_remove.len(),
        to_add.len()
    );
    if !to_add.is_empty() {
        let w = &to_add[0];
        if let Some(dep) = deployed_map.get(w.game_rel_lowercase.as_str()) {
            eprintln!("[deployd] sample mismatch path={:?}", w.game_rel_lowercase);
            eprintln!("[deployd]   deployed cache_path={:?}", dep.cache_path);
            eprintln!("[deployd]   winner  cache_path={:?}", w.cache_path);
        } else {
            eprintln!(
                "[deployd] sample new path={:?} (not in deployed_map)",
                w.game_rel_lowercase
            );
        }
    }

    // Pre-build lowercase path set from all previously deployed files,
    // used to distinguish stale case-variant files from vanilla game files.
    let deployed_lower: HashSet<String> = deployed
        .iter()
        .map(|d| {
            resolve_deploy_path(&d.game_rel_original, &game.path, &game_data)
                .to_string_lossy()
                .to_lowercase()
        })
        .collect();

    // Collect game_rel_original values for files being removed, so we can
    // restore vanilla backups after the removal loop.
    let removed_rels: Vec<String> = to_remove
        .iter()
        .map(|f| f.game_rel_original.clone())
        .collect();

    for f in &to_remove {
        remove_deployed_file(f, game, &game_data);
    }

    // Restore vanilla files for paths that are no longer claimed by any mod.
    for removed_rel in &removed_rels {
        if let Ok(Some(backup_path)) = tracker.get_vanilla_backup(&game.id, removed_rel).await {
            let deploy_path = resolve_deploy_path(removed_rel, &game.path, &game_data);
            if !deploy_path.exists() {
                // remove_empty_parents may have deleted the parent dir; recreate it.
                if let Some(parent) = deploy_path.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if fs::copy(&backup_path, &deploy_path).is_ok() {
                    let _ = tracker.delete_vanilla_backup(&game.id, removed_rel).await;
                }
                // If copy failed, keep the DB record so the restore can be retried.
            }
        }
    }

    let canonical_dirs = build_dir_canonical_map(&winners);

    let mut newly_linked: Vec<ModFile> = Vec::new();
    let mut dir_cache: HashMap<PathBuf, HashMap<String, PathBuf>> = HashMap::new();
    for f in &to_add {
        let cache_file = PathBuf::from(&f.cache_path);

        if f.game_rel_lowercase.ends_with('/') {
            let docs = docs_base(&game_data);
            let (base, rel): (&PathBuf, &str) =
                if let Some(root_rel) = f.game_rel_original.strip_prefix("../") {
                    (&game.path, root_rel.trim_end_matches('/'))
                } else if let Some(docs_rel) = f.game_rel_original.strip_prefix(DOCS_PREFIX) {
                    (&docs, docs_rel.trim_end_matches('/'))
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
            } else if f.game_rel_original.starts_with(DOCS_PREFIX) {
                format!("{DOCS_PREFIX}{actual_rel}/")
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

        let docs = docs_base(&game_data);
        let (base, rel): (&PathBuf, &str) =
            if let Some(root_rel) = f.game_rel_original.strip_prefix("../") {
                (&game.path, root_rel)
            } else if let Some(docs_rel) = f.game_rel_original.strip_prefix(DOCS_PREFIX) {
                (&docs, docs_rel)
            } else {
                (&game_data, f.game_rel_original.as_str())
            };
        let deploy_target =
            ensure_dirs_case_insensitive(base, rel, &canonical_dirs, &mut dir_cache)?;

        if deploy_target.exists() {
            // If this file was not previously deployed by deployd it's a vanilla/user
            // file — copy it to our backup store before replacing it with the mod file.
            let deploy_target_lower = deploy_target.to_string_lossy().to_lowercase();
            let is_ours = deployed_lower.contains(&deploy_target_lower);
            if !is_ours && let Ok(backup_dir) = paths::vanilla_backup_dir(&game.id) {
                let _ = fs::create_dir_all(&backup_dir);
                let backup_name = f.game_rel_lowercase.replace('/', "__");
                let backup_path = backup_dir.join(&backup_name);
                // Idempotent: skip if backup copy already exists on disk.
                if !backup_path.exists() && fs::copy(&deploy_target, &backup_path).is_ok() {
                    let _ = tracker
                        .save_vanilla_backup(&game.id, &f.game_rel_original, &backup_path)
                        .await;
                }
            }
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
                        let was_ours =
                            deployed_lower.contains(&entry_path.to_string_lossy().to_lowercase());
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

    let remove_paths: Vec<&str> = to_remove
        .iter()
        .map(|f| f.game_rel_lowercase.as_str())
        .collect();
    tracker
        .remove_deployed_files(&game.id, &remove_paths)
        .await?;
    tracker
        .record_deployed_files(&game.id, &newly_linked)
        .await?;

    game::handler_for(&game.engine)
        .post_deploy(game, tracker)
        .await?;

    if let Err(e) = mod_folders::refresh_named_mod_folders(tracker, &game.id, cache_root).await {
        eprintln!("[deployd] named_mods refresh failed: {e}");
    }

    Ok(DeployResult {
        files_total: winners.len(),
        files_added: newly_linked.len(),
        files_removed: to_remove.len(),
        conflicts_resolved,
    })
}

/// Bake externally-modified plugins back into cache before deploy.
///
/// Handles the case where a tool (e.g. xEdit safe-save) broke the hardlink:
/// the on-disk file has a new inode while the cache still holds the original.
/// In-place saves (same inode) are a no-op.
async fn bake_modified_plugins(game: &Game, tracker: &Tracker, game_data: &Path) {
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

async fn compute_winners(
    tracker: &Tracker,
    game_id: &str,
    handler: &dyn EngineHandler,
) -> Result<(Vec<ModFile>, usize)> {
    let all_files = tracker.get_all_mod_files_by_priority(game_id).await?;

    // Group file indices by engine-specific conflict key. For most engines the
    // key is the full path, reproducing the previous behaviour. For Aurora,
    // Override/ files are keyed by filename so that override/ModA/path/foo.xml
    // and override/foo.xml are treated as one conflict — only the
    // highest-priority mod's file is deployed.
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, (game_rel, ..)) in all_files.iter().enumerate() {
        let key = handler.conflict_key(game_rel).to_string();
        groups.entry(key).or_default().push(i);
    }

    let mut winners: Vec<ModFile> = Vec::with_capacity(groups.len());
    let mut conflicts_resolved: usize = 0;

    for (_, mut indices) in groups {
        if indices.len() > 1 {
            conflicts_resolved += 1;
            // Highest priority wins; game_rel is a stable tiebreaker.
            indices.sort_by(|&a, &b| {
                all_files[b]
                    .4
                    .cmp(&all_files[a].4)
                    .then_with(|| all_files[a].0.cmp(&all_files[b].0))
            });
        }
        let (game_rel, mod_id, cache_path, game_rel_original, _) = &all_files[indices[0]];
        winners.push(ModFile {
            mod_id: mod_id.clone(),
            game_rel_lowercase: game_rel.clone(),
            game_rel_original: game_rel_original.clone(),
            cache_path: cache_path.clone(),
        });
    }

    Ok((winners, conflicts_resolved))
}

fn remove_deployed_file(f: &ModFile, game: &Game, game_data: &PathBuf) {
    let deploy_path = resolve_deploy_path(&f.game_rel_original, &game.path, game_data);
    let docs = docs_base(game_data);
    let stop_at: &PathBuf = if f.game_rel_original.starts_with("../") {
        &game.path
    } else if f.game_rel_original.starts_with(DOCS_PREFIX) {
        &docs
    } else {
        game_data
    };

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

fn resolve_deploy_path(game_rel: &str, game_root: &Path, game_data: &Path) -> PathBuf {
    if let Some(root_rel) = game_rel.strip_prefix("../") {
        game_root.join(root_rel)
    } else if let Some(docs_rel) = game_rel.strip_prefix(DOCS_PREFIX) {
        docs_base(game_data).join(docs_rel)
    } else {
        game_data.join(game_rel)
    }
}

fn docs_base(game_data: &Path) -> PathBuf {
    game_data
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| game_data.to_path_buf())
}

/// Build a lowercase→best-cased directory name map from winner paths.
/// Non-all-lowercase form wins to preserve readability for tools browsing the game folder.
fn build_dir_canonical_map(winners: &[ModFile]) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for f in winners {
        let rel = if let Some(r) = f.game_rel_original.strip_prefix("../") {
            r
        } else if let Some(r) = f.game_rel_original.strip_prefix(DOCS_PREFIX) {
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

fn create_dirs_case_insensitive(
    base: &Path,
    components: &[&str],
    canonical: &HashMap<String, String>,
    dir_cache: &mut HashMap<PathBuf, HashMap<String, PathBuf>>,
) -> Result<PathBuf> {
    let mut current = base.to_path_buf();

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

fn ensure_dirs_case_insensitive(
    base: &Path,
    rel_path: &str,
    canonical: &HashMap<String, String>,
    dir_cache: &mut HashMap<PathBuf, HashMap<String, PathBuf>>,
) -> Result<PathBuf> {
    let components: Vec<&str> = rel_path.split('/').collect();
    let dir_components = &components[..components.len().saturating_sub(1)];
    let parent = create_dirs_case_insensitive(base, dir_components, canonical, dir_cache)?;

    if let Some(filename) = components.last() {
        // Scan the parent directory case-insensitively so that deploying
        // "Scripts/Mod.lua" correctly finds and reuses the existing
        // "Scripts/mod.lua" path rather than creating a second file alongside it.
        let fname_lower = filename.to_lowercase();
        if let Ok(rd) = fs::read_dir(&parent) {
            for entry in rd.flatten() {
                if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                    && entry.file_name().to_string_lossy().to_lowercase() == fname_lower
                {
                    return Ok(entry.path());
                }
            }
        }
        Ok(parent.join(filename))
    } else {
        Ok(parent)
    }
}

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
