use std::fs;
use std::path::Path;

use crate::core::tracker::Tracker;
use crate::models::game::Game;
use crate::utils::paths;

use super::filesystem::resolve_deploy_path;

/// Preserve externally modified plugins before the deployment delta is applied.
pub(super) async fn bake_modified_plugins(game: &Game, tracker: &Tracker, game_data: &Path) {
    use std::os::unix::fs::MetadataExt;
    let Ok(plugin_files) = tracker.get_deployed_plugin_files(&game.id).await else {
        return;
    };
    for (_, ref game_rel_orig, ref cache_path) in plugin_files {
        let Ok(disk_path) = resolve_deploy_path(game_rel_orig, &game.path, game_data) else {
            continue;
        };
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

pub(super) async fn restore_all_vanilla(game: &Game, tracker: &Tracker, game_data: &Path) {
    let Ok(backups) = tracker.get_all_vanilla_backups(&game.id).await else {
        return;
    };
    for (relative, backup_path) in backups {
        restore_vanilla_file(game, tracker, game_data, &relative, &backup_path).await;
    }
}

pub(super) async fn restore_vanilla_for_paths(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
    relative_paths: &[String],
) {
    for relative in relative_paths {
        if let Ok(Some(backup_path)) = tracker.get_vanilla_backup(&game.id, relative).await {
            restore_vanilla_file(game, tracker, game_data, relative, &backup_path).await;
        }
    }
}

async fn restore_vanilla_file(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
    relative: &str,
    backup_path: &Path,
) {
    let Ok(deploy_path) = resolve_deploy_path(relative, &game.path, game_data) else {
        return;
    };
    if deploy_path.exists() {
        let _ = tracker.delete_vanilla_backup(&game.id, relative).await;
        return;
    }
    if let Some(parent) = deploy_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::copy(backup_path, &deploy_path).is_ok() {
        let _ = tracker.delete_vanilla_backup(&game.id, relative).await;
    }
}

pub(super) async fn backup_vanilla_file(
    game: &Game,
    tracker: &Tracker,
    relative: &str,
    source: &Path,
) {
    let Ok(backup_dir) = paths::vanilla_backup_dir(&game.id) else {
        return;
    };
    let _ = fs::create_dir_all(&backup_dir);
    let backup_path = backup_dir.join(relative.to_lowercase().replace('/', "__"));
    if !backup_path.exists() && fs::copy(source, &backup_path).is_ok() {
        let _ = tracker
            .save_vanilla_backup(&game.id, relative, &backup_path)
            .await;
    }
}
