use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::backup::{backup_vanilla_file, bake_modified_plugins, restore_vanilla_for_paths};
use super::filesystem::{
    build_dir_canonical_map, create_dirs_case_insensitive, ensure_dirs_case_insensitive,
    remove_deployed_file, resolve_deploy_path, split_deploy_target,
};
use super::planning::compute_winners;
use super::report::DeployOutcome;
use crate::core::game;
use crate::core::mod_folders;
use crate::core::tracker::Tracker;
use crate::dlog;
use crate::models::game::Game;
use crate::models::manifest::ModFile;

pub async fn deploy(game: &Game, tracker: &Tracker, cache_root: &Path) -> Result<DeployOutcome> {
    let game_data = game::deploy_dir(game);
    let mut warnings = Vec::new();

    bake_modified_plugins(game, tracker, &game_data).await?;

    let deployed = tracker.get_deployed_files(&game.id).await?;
    let deployed_map: HashMap<&str, &ModFile> = deployed
        .iter()
        .map(|f| (f.game_rel_lowercase.as_str(), f))
        .collect();

    let vanilla_snapshot = tracker.get_vanilla_metadata(&game.id).await?;

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

    // Distinguish stale case-variant paths from vanilla game files during removal.
    let deployed_lower: HashSet<String> = deployed
        .iter()
        .map(|deployed| {
            resolve_deploy_path(&deployed.game_rel_original, &game.path, &game_data)
                .map(|path| path.to_string_lossy().to_lowercase())
                .with_context(|| {
                    format!(
                        "Invalid tracked deploy path '{}'",
                        deployed.game_rel_original
                    )
                })
        })
        .collect::<Result<_>>()?;

    // Needed to restore vanilla backups after the removal loop.
    let removed_rels: Vec<String> = to_remove
        .iter()
        .map(|f| f.game_rel_original.clone())
        .collect();

    for f in &to_remove {
        warnings.extend(remove_deployed_file(f, game, &game_data)?);
    }

    warnings.extend(restore_vanilla_for_paths(game, tracker, &game_data, &removed_rels).await?);

    let canonical_dirs = build_dir_canonical_map(&winners);

    let mut newly_linked: Vec<ModFile> = Vec::new();
    let mut dir_cache: HashMap<PathBuf, HashMap<String, PathBuf>> = HashMap::new();
    for f in &to_add {
        let cache_file = PathBuf::from(&f.cache_path);

        if f.game_rel_lowercase.ends_with('/') {
            let (base, rel, anchor) =
                split_deploy_target(&f.game_rel_original, &game.path, &game_data)?;
            let rel = rel.trim_end_matches('/');
            let dir_comps: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
            let dir_path =
                create_dirs_case_insensitive(&base, &dir_comps, &canonical_dirs, &mut dir_cache)
                    .with_context(|| format!("Cannot create deployed directory '{rel}'"))?;
            dlog!("[deployd] sentinel dir: {}", dir_path.display());
            let actual_rel = dir_path
                .strip_prefix(&base)
                .unwrap_or(&dir_path)
                .to_string_lossy()
                .to_string();
            let actual_original = anchor.with_prefix(&format!("{actual_rel}/"));
            newly_linked.push(ModFile {
                mod_id: f.mod_id.clone(),
                game_rel_lowercase: f.game_rel_lowercase.clone(),
                game_rel_original: actual_original,
                cache_path: f.cache_path.clone(),
            });
            continue;
        }

        let (base, rel, anchor) =
            split_deploy_target(&f.game_rel_original, &game.path, &game_data)?;
        let deploy_target =
            ensure_dirs_case_insensitive(&base, rel, &canonical_dirs, &mut dir_cache)?;

        if deploy_target.exists() {
            // If this file was not previously deployed by deployd it's a vanilla/user
            // file — copy it to our backup store before replacing it with the mod file.
            let deploy_target_lower = deploy_target.to_string_lossy().to_lowercase();
            let is_ours = deployed_lower.contains(&deploy_target_lower);
            let is_vanilla = vanilla_snapshot.contains_key(&f.game_rel_lowercase);
            if !is_ours && is_vanilla {
                backup_vanilla_file(game, tracker, &f.game_rel_original, &deploy_target).await?;
            }
            fs::remove_file(&deploy_target)?;
        } else if let (Some(parent), Some(fname)) =
            (deploy_target.parent(), deploy_target.file_name())
        {
            // Remove stale case-variant files that were previously deployed by us
            // but survived because the purge step only removed exact-path entries.
            match fs::read_dir(parent) {
                Ok(entries) => {
                    for entry in entries {
                        let entry = match entry {
                            Ok(entry) => entry,
                            Err(error) => {
                                warnings.push(format!(
                                    "Could not inspect an entry in '{}' for stale deployed files: {error}",
                                    parent.display()
                                ));
                                continue;
                            }
                        };
                        let entry_path = entry.path();
                        if entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
                            && entry.file_name().eq_ignore_ascii_case(fname)
                            && entry_path != deploy_target
                        {
                            let was_ours = deployed_lower
                                .contains(&entry_path.to_string_lossy().to_lowercase());
                            if was_ours && let Err(error) = fs::remove_file(&entry_path) {
                                warnings.push(format!(
                                    "Could not remove stale deployed file '{}': {error}",
                                    entry_path.display()
                                ));
                            }
                        }
                    }
                }
                Err(error) => warnings.push(format!(
                    "Could not inspect '{}' for stale deployed files: {error}",
                    parent.display()
                )),
            }
        }

        // Try hardlink first (zero-copy, same inode). Game dirs accessed via a
        // separate filesystem permission (e.g. Steam Snap) are on a
        // different bind-mount from the cache, so hard_link returns EXDEV
        // (errno=18). Fall back to a plain copy in that case.
        if let Err(e) = fs::hard_link(&cache_file, &deploy_target) {
            if e.raw_os_error() == Some(18) {
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

        let actual_rel = deploy_target
            .strip_prefix(&base)
            .unwrap_or(&deploy_target)
            .to_string_lossy()
            .to_string();
        let actual_original = anchor.with_prefix(&actual_rel);
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
        warnings.push(format!("Named mod-folder refresh failed: {e}"));
    }

    Ok(DeployOutcome {
        files_total: winners.len(),
        files_added: newly_linked.len(),
        files_removed: to_remove.len(),
        conflicts_resolved,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::tempdir;

    use crate::core::tracker::Tracker;
    use crate::models::game::{Game, GameEngine};
    use crate::models::manifest::ModFile;
    use crate::models::mod_entry::{InstallTarget, ModEntry};

    use super::deploy;

    #[tokio::test]
    async fn deployment_removes_previous_data_route_after_root_merge() -> Result<()> {
        let temp = tempdir()?;
        let game_root = temp.path().join("game");
        let game_data = game_root.join("Data");
        let cache_root = temp.path().join("cache");
        let cache_file = cache_root.join("mod/enbseries/settings.ini");
        std::fs::create_dir_all(game_data.join("enbseries"))?;
        std::fs::create_dir_all(cache_file.parent().expect("cache file parent"))?;
        std::fs::write(game_data.join("enbseries/settings.ini"), b"old Data route")?;
        std::fs::write(&cache_file, b"merged Root route")?;

        let game = Game {
            id: "game".to_string(),
            title: "Game".to_string(),
            path: game_root.clone(),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: None,
        };
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker
            .upsert_game(
                &game.id,
                &game.title,
                &game.path,
                &game.data_subdir,
                "bethesda",
                None,
                false,
            )
            .await?;
        tracker
            .insert_mod(&ModEntry {
                id: "mod".to_string(),
                game_id: game.id.clone(),
                name: "External Changes".to_string(),
                archive_hash: None,
                archive_path: None,
                installed_at: None,
                enabled: true,
                priority: 0,
                nexus_mod_id: None,
                nexus_file_id: None,
                nexus_domain: None,
                version: None,
                author: None,
                nexus_description: None,
                latest_version: None,
                nexus_file_name: None,
                nexus_is_primary: false,
                archive_md5: None,
                install_target: InstallTarget::Root,
                notes: None,
            })
            .await?;
        tracker
            .record_files(&[ModFile {
                mod_id: "mod".to_string(),
                game_rel_lowercase: "../enbseries/settings.ini".to_string(),
                game_rel_original: "../enbseries/settings.ini".to_string(),
                cache_path: cache_file.to_string_lossy().to_string(),
            }])
            .await?;
        tracker
            .record_deployed_files(
                &game.id,
                &[ModFile {
                    mod_id: "mod".to_string(),
                    game_rel_lowercase: "enbseries/settings.ini".to_string(),
                    game_rel_original: "enbseries/settings.ini".to_string(),
                    cache_path: cache_file.to_string_lossy().to_string(),
                }],
            )
            .await?;

        let outcome = deploy(&game, &tracker, &cache_root).await?;

        assert_eq!(outcome.files_removed, 1);
        assert_eq!(outcome.files_added, 1);
        assert!(!game_data.join("enbseries/settings.ini").exists());
        assert_eq!(
            std::fs::read(game_root.join("enbseries/settings.ini"))?,
            b"merged Root route"
        );
        let deployed = tracker.get_deployed_files(&game.id).await?;
        assert_eq!(deployed.len(), 1);
        assert_eq!(deployed[0].game_rel_lowercase, "../enbseries/settings.ini");
        Ok(())
    }
}
