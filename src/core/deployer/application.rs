use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::backup::{
    backup_exists, backup_vanilla_file, backup_vanilla_file_in, bake_modified_plugins, files_match,
    restore_vanilla_for_paths,
};
use super::filesystem::{
    build_dir_canonical_map, create_dirs_case_insensitive, ensure_dirs_case_insensitive,
    find_existing_deploy_path_case_insensitive, remove_deployed_file, resolve_deploy_path,
    split_deploy_target,
};
use super::planning::build_plan;
use super::report::{
    DeployOutcome, DeploymentPreflight, VanillaReplacement, VanillaReplacementStatus,
};
use crate::core::game;
use crate::core::mod_folders;
use crate::core::tracker::Tracker;
use crate::dlog;
use crate::models::game::Game;
use crate::models::manifest::ModFile;
use crate::utils::paths;

pub(crate) fn vanilla_protection_enabled(value: Option<&str>) -> bool {
    value
        .and_then(|value| value.parse::<bool>().ok())
        .unwrap_or(true)
}

pub(crate) async fn deployment_preflight(
    game: &Game,
    tracker: &Tracker,
) -> Result<DeploymentPreflight> {
    let game_data = game::deploy_dir(game);
    let plan = build_plan(tracker, &game.id, game::handler_for(&game.engine)).await?;
    let vanilla_snapshot = tracker.get_vanilla_metadata(&game.id).await?;
    let deployed_paths: HashSet<&str> = plan
        .deployed
        .iter()
        .map(|file| file.game_rel_lowercase.as_str())
        .collect();
    let mut vanilla_replacements = Vec::new();

    for index in plan.to_add {
        let winner = &plan.winners[index];
        if winner.game_rel_lowercase.ends_with('/')
            || !vanilla_snapshot.contains_key(&winner.game_rel_lowercase)
        {
            continue;
        }
        let status = if backup_exists(game, tracker, &winner.game_rel_lowercase).await? {
            VanillaReplacementStatus::Protected
        } else if deployed_paths.contains(winner.game_rel_lowercase.as_str()) {
            let deployed = plan
                .deployed
                .iter()
                .find(|file| file.game_rel_lowercase == winner.game_rel_lowercase)
                .context("Deployment plan lost the currently deployed vanilla path")?;
            let live_path = find_existing_deploy_path_case_insensitive(
                &deployed.game_rel_original,
                &game.path,
                &game_data,
            )?;
            let cache_path = Path::new(&deployed.cache_path);
            if let Some(live_path) = live_path
                && cache_path.try_exists().with_context(|| {
                    format!(
                        "Failed to inspect deployed cache '{}'",
                        cache_path.display()
                    )
                })?
                && !files_match(&live_path, cache_path)?
            {
                VanillaReplacementStatus::ReadyToBackUp
            } else {
                VanillaReplacementStatus::BackupUnavailable
            }
        } else {
            if find_existing_deploy_path_case_insensitive(
                &winner.game_rel_original,
                &game.path,
                &game_data,
            )?
            .is_some()
            {
                VanillaReplacementStatus::ReadyToBackUp
            } else {
                VanillaReplacementStatus::BackupUnavailable
            }
        };
        vanilla_replacements.push(VanillaReplacement {
            path: winner.game_rel_original.clone(),
            status,
        });
    }
    vanilla_replacements.sort_by(|left, right| left.path.cmp(&right.path));

    let protection_setting = tracker.get_setting("protect_vanilla_files").await?;
    let protect_vanilla_files = vanilla_protection_enabled(protection_setting.as_deref());
    Ok(DeploymentPreflight {
        protect_vanilla_files,
        vanilla_replacements,
    })
}

pub async fn deploy(
    game: &Game,
    tracker: &Tracker,
    cache_root: &Path,
    protect_vanilla_files: bool,
) -> Result<DeployOutcome> {
    deploy_with_backup_dir(game, tracker, cache_root, protect_vanilla_files, None).await
}

pub(super) async fn deploy_with_backup_dir(
    game: &Game,
    tracker: &Tracker,
    cache_root: &Path,
    protect_vanilla_files: bool,
    backup_dir: Option<&Path>,
) -> Result<DeployOutcome> {
    let game_data = game::deploy_dir(game);
    let mut warnings = Vec::new();

    let plan = build_plan(tracker, &game.id, game::handler_for(&game.engine)).await?;
    let deployed = &plan.deployed;
    let deployed_map: HashMap<&str, &ModFile> = deployed
        .iter()
        .map(|f| (f.game_rel_lowercase.as_str(), f))
        .collect();

    let vanilla_snapshot = tracker.get_vanilla_metadata(&game.id).await?;

    let winners = &plan.winners;
    let winners_map: HashMap<&str, &ModFile> = winners
        .iter()
        .map(|f| (f.game_rel_lowercase.as_str(), f))
        .collect();

    eprintln!(
        "[deployd] delta: deployed={}, winners={}",
        deployed.len(),
        winners.len()
    );

    let to_remove: Vec<&ModFile> = plan
        .to_remove
        .iter()
        .map(|index| &deployed[*index])
        .collect();
    let to_add: Vec<&ModFile> = plan.to_add.iter().map(|index| &winners[*index]).collect();

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

    let mut restore_paths: Vec<String> = to_remove
        .iter()
        .filter(|file| !winners_map.contains_key(file.game_rel_lowercase.as_str()))
        .map(|file| file.game_rel_lowercase.clone())
        .collect();
    for (backup_path, _) in tracker.get_all_vanilla_backups(&game.id).await? {
        let canonical_path = paths::lowercase_path_str(Path::new(&backup_path));
        if !winners_map.contains_key(canonical_path.as_str()) {
            restore_paths.push(canonical_path);
        }
    }
    restore_paths.sort();
    restore_paths.dedup();

    let canonical_dirs = build_dir_canonical_map(winners);
    let mut dir_cache: HashMap<PathBuf, HashMap<String, PathBuf>> = HashMap::new();
    let mut vanilla_files_backed_up = 0;
    if protect_vanilla_files {
        for f in &to_add {
            if f.game_rel_lowercase.ends_with('/')
                || !vanilla_snapshot.contains_key(&f.game_rel_lowercase)
            {
                continue;
            }
            let (base, rel, anchor) =
                split_deploy_target(&f.game_rel_original, &game.path, &game_data)?;
            let deploy_target =
                ensure_dirs_case_insensitive(&base, rel, &canonical_dirs, &mut dir_cache)?;
            if !deploy_target.try_exists().with_context(|| {
                format!(
                    "Failed to inspect vanilla target '{}'",
                    deploy_target.display()
                )
            })? {
                continue;
            }
            let is_ours = if let Some(deployed) = deployed_map.get(f.game_rel_lowercase.as_str()) {
                let cache_path = Path::new(&deployed.cache_path);
                if cache_path.try_exists().with_context(|| {
                    format!(
                        "Failed to inspect deployed cache '{}'",
                        cache_path.display()
                    )
                })? {
                    files_match(&deploy_target, cache_path)?
                } else {
                    true
                }
            } else {
                false
            };
            if !is_ours {
                let actual_rel = deploy_target
                    .strip_prefix(&base)
                    .unwrap_or(&deploy_target)
                    .to_string_lossy();
                let original_path = anchor.with_prefix(&actual_rel);
                let created = if let Some(backup_dir) = backup_dir {
                    backup_vanilla_file_in(
                        game,
                        tracker,
                        &f.game_rel_lowercase,
                        &original_path,
                        &deploy_target,
                        backup_dir,
                    )
                    .await?
                } else {
                    backup_vanilla_file(
                        game,
                        tracker,
                        &f.game_rel_lowercase,
                        &original_path,
                        &deploy_target,
                    )
                    .await?
                };
                if created {
                    vanilla_files_backed_up += 1;
                }
            }
        }
    }

    bake_modified_plugins(game, tracker, &game_data).await?;

    let mut missing_restore_paths = Vec::new();
    for path in &restore_paths {
        if vanilla_snapshot.contains_key(path)
            && tracker.get_vanilla_backup(&game.id, path).await?.is_none()
        {
            missing_restore_paths.push(path);
        }
    }

    for f in &to_remove {
        warnings.extend(remove_deployed_file(f, game, &game_data)?);
    }
    dir_cache.clear();

    for path in missing_restore_paths {
        warnings.push(format!(
            "No vanilla backup is available for '{path}'. Restore it with the game's platform verification tool if needed"
        ));
    }
    let restore = restore_vanilla_for_paths(game, tracker, &game_data, &restore_paths).await?;
    let vanilla_files_restored = restore.restored;
    warnings.extend(restore.warnings);

    let mut newly_linked: Vec<ModFile> = Vec::new();
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
        conflicts_resolved: plan.conflicts_resolved,
        vanilla_files_backed_up,
        vanilla_files_restored,
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

    use super::{deploy, deployment_preflight, vanilla_protection_enabled};

    fn mod_entry(id: &str, game_id: &str, priority: i32) -> ModEntry {
        ModEntry {
            id: id.to_string(),
            game_id: game_id.to_string(),
            name: id.to_string(),
            archive_hash: None,
            archive_path: None,
            installed_at: None,
            enabled: true,
            priority,
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
            install_target: InstallTarget::Data,
            notes: None,
        }
    }

    #[test]
    fn vanilla_protection_defaults_to_enabled() {
        assert!(vanilla_protection_enabled(None));
        assert!(vanilla_protection_enabled(Some("invalid")));
        assert!(vanilla_protection_enabled(Some("true")));
        assert!(!vanilla_protection_enabled(Some("false")));
    }

    #[tokio::test]
    async fn later_deploy_retries_orphaned_vanilla_backup() -> Result<()> {
        let temp = tempdir()?;
        let game_root = temp.path().join("game");
        let game_data = game_root.join("Data");
        let cache_root = temp.path().join("cache");
        let backup = temp.path().join("backup.bin");
        std::fs::create_dir_all(&game_data)?;
        std::fs::write(&backup, b"vanilla")?;
        let game = Game {
            id: "game".to_string(),
            title: "Game".to_string(),
            path: game_root,
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: None,
        };
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker
            .save_vanilla_backup(&game.id, "file.bin", "File.bin", &backup)
            .await?;

        let outcome = deploy(&game, &tracker, &cache_root, false).await?;

        assert_eq!(outcome.vanilla_files_restored, 1);
        assert_eq!(std::fs::read(game_data.join("File.bin"))?, b"vanilla");
        assert!(!backup.exists());
        assert!(
            tracker
                .get_vanilla_backup(&game.id, "file.bin")
                .await?
                .is_none()
        );
        Ok(())
    }

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

        let outcome = deploy(&game, &tracker, &cache_root, true).await?;

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

    // @variants: both
    #[tokio::test]
    async fn witcher_2_restores_vanilla_after_multiple_mod_winners() -> Result<()> {
        let temp = tempdir()?;
        let game_root = temp.path().join("Witcher 2");
        let game_data = game_root.join("CookedPC");
        let cache_root = temp.path().join("cache");
        let cache_a = cache_root.join("mod-a/base_scripts.d2a");
        let cache_b = cache_root.join("mod-b/base_scripts.d2a");
        let live_path = game_data.join("Base_Scripts.d2a");
        let backup_path = temp.path().join("legacy-flat-backup");
        std::fs::create_dir_all(&game_data)?;
        std::fs::create_dir_all(cache_a.parent().expect("mod A cache parent"))?;
        std::fs::create_dir_all(cache_b.parent().expect("mod B cache parent"))?;
        std::fs::write(&live_path, b"mod A")?;
        std::fs::write(&cache_a, b"mod A")?;
        std::fs::write(&cache_b, b"mod B")?;
        std::fs::write(&backup_path, b"vanilla")?;

        let game = Game {
            id: "witcher-2".to_string(),
            title: "The Witcher 2: Assassins of Kings".to_string(),
            path: game_root,
            data_subdir: "CookedPC".to_string(),
            engine: GameEngine::REDEngine,
            wine_prefix: None,
        };
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker.insert_mod(&mod_entry("mod-a", &game.id, 1)).await?;
        tracker.insert_mod(&mod_entry("mod-b", &game.id, 2)).await?;
        let file_a = ModFile {
            mod_id: "mod-a".to_string(),
            game_rel_lowercase: "base_scripts.d2a".to_string(),
            game_rel_original: "Base_Scripts.d2a".to_string(),
            cache_path: cache_a.to_string_lossy().into_owned(),
        };
        let file_b = ModFile {
            mod_id: "mod-b".to_string(),
            game_rel_lowercase: "base_scripts.d2a".to_string(),
            game_rel_original: "base_scripts.d2a".to_string(),
            cache_path: cache_b.to_string_lossy().into_owned(),
        };
        tracker.record_files(&[file_a.clone(), file_b]).await?;
        tracker
            .record_deployed_files(&game.id, std::slice::from_ref(&file_a))
            .await?;
        tracker
            .reset_vanilla_snapshot(&game.id, &[("base_scripts.d2a".to_string(), 7, 0)])
            .await?;
        tracker
            .save_vanilla_backup(
                &game.id,
                "base_scripts.d2a",
                "Base_Scripts.d2a",
                &backup_path,
            )
            .await?;

        let switched = deploy(&game, &tracker, &cache_root, true).await?;
        assert_eq!(switched.vanilla_files_restored, 0);
        assert_eq!(std::fs::read(game_data.join("base_scripts.d2a"))?, b"mod B");
        assert!(
            tracker
                .get_vanilla_backup(&game.id, "BASE_SCRIPTS.D2A")
                .await?
                .is_some()
        );

        tracker.delete_mod_files("mod-b").await?;
        tracker.delete_mod("mod-b").await?;
        let switched_back = deploy(&game, &tracker, &cache_root, true).await?;
        assert_eq!(switched_back.vanilla_files_restored, 0);
        assert_eq!(std::fs::read(&live_path)?, b"mod A");

        tracker.delete_mod_files("mod-a").await?;
        tracker.delete_mod("mod-a").await?;
        let restored = deploy(&game, &tracker, &cache_root, false).await?;
        assert_eq!(restored.vanilla_files_restored, 1);
        assert_eq!(std::fs::read(&live_path)?, b"vanilla");
        assert!(
            tracker
                .get_vanilla_backup(&game.id, "base_scripts.d2a")
                .await?
                .is_none()
        );
        assert!(!backup_path.exists());
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn preflight_reports_unprotected_mod_to_mod_replacement() -> Result<()> {
        let temp = tempdir()?;
        let game_root = temp.path().join("game");
        let game_data = game_root.join("CookedPC");
        let cache_root = temp.path().join("cache");
        let cache_a = cache_root.join("a/file.d2a");
        let cache_b = cache_root.join("b/file.d2a");
        std::fs::create_dir_all(&game_data)?;
        std::fs::create_dir_all(cache_a.parent().expect("cache A parent"))?;
        std::fs::create_dir_all(cache_b.parent().expect("cache B parent"))?;
        std::fs::write(game_data.join("file.d2a"), b"a")?;
        std::fs::write(&cache_a, b"a")?;
        std::fs::write(&cache_b, b"b")?;
        let game = Game {
            id: "witcher-2".to_string(),
            title: "Witcher 2".to_string(),
            path: game_root,
            data_subdir: "CookedPC".to_string(),
            engine: GameEngine::REDEngine,
            wine_prefix: None,
        };
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker.insert_mod(&mod_entry("a", &game.id, 1)).await?;
        tracker.insert_mod(&mod_entry("b", &game.id, 2)).await?;
        let deployed = ModFile {
            mod_id: "a".to_string(),
            game_rel_lowercase: "file.d2a".to_string(),
            game_rel_original: "file.d2a".to_string(),
            cache_path: cache_a.to_string_lossy().into_owned(),
        };
        tracker
            .record_files(&[
                deployed.clone(),
                ModFile {
                    mod_id: "b".to_string(),
                    game_rel_lowercase: "file.d2a".to_string(),
                    game_rel_original: "file.d2a".to_string(),
                    cache_path: cache_b.to_string_lossy().into_owned(),
                },
            ])
            .await?;
        tracker.record_deployed_files(&game.id, &[deployed]).await?;
        tracker
            .reset_vanilla_snapshot(&game.id, &[("file.d2a".to_string(), 1, 0)])
            .await?;

        let preflight = deployment_preflight(&game, &tracker).await?;

        assert_eq!(preflight.vanilla_replacements.len(), 1);
        assert_eq!(
            preflight.vanilla_replacements[0].status,
            super::VanillaReplacementStatus::BackupUnavailable
        );
        Ok(())
    }
}
