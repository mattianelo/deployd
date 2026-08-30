use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::core::tracker::Tracker;
use crate::models::game::Game;
use crate::utils::paths;

use super::filesystem::resolve_deploy_path;

/// Preserve externally modified plugins before the deployment delta is applied.
pub(super) async fn bake_modified_plugins(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let plugin_files = tracker
        .get_deployed_plugin_files(&game.id)
        .await
        .context("Failed to load deployed plugins before synchronization")?;
    for (_, ref game_rel_orig, ref cache_path) in plugin_files {
        let disk_path = resolve_deploy_path(game_rel_orig, &game.path, game_data)
            .with_context(|| format!("Invalid deployed plugin path '{game_rel_orig}'"))?;
        if !disk_path
            .try_exists()
            .with_context(|| format!("Failed to inspect deployed plugin '{game_rel_orig}'"))?
            || !cache_path
                .try_exists()
                .with_context(|| format!("Failed to inspect cached plugin '{game_rel_orig}'"))?
        {
            continue;
        }
        let disk_ino = fs::metadata(&disk_path)
            .with_context(|| format!("Failed to inspect deployed plugin '{game_rel_orig}'"))?
            .ino();
        let cache_ino = fs::metadata(cache_path)
            .with_context(|| format!("Failed to inspect cached plugin '{game_rel_orig}'"))?
            .ino();
        if disk_ino != cache_ino {
            fs::copy(&disk_path, cache_path).with_context(|| {
                format!("Could not preserve modified plugin '{game_rel_orig}' in the mod cache")
            })?;
        }
    }
    Ok(())
}

pub(super) async fn restore_all_vanilla(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
) -> Result<Vec<String>> {
    let backups = tracker
        .get_all_vanilla_backups(&game.id)
        .await
        .context("Failed to load vanilla backup records")?;
    let mut warnings = Vec::new();
    for (relative, backup_path) in backups {
        if let Some(warning) =
            restore_vanilla_file(game, tracker, game_data, &relative, &backup_path).await
        {
            warnings.push(warning);
        }
    }
    Ok(warnings)
}

pub(super) async fn restore_vanilla_for_paths(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
    relative_paths: &[String],
) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    for relative in relative_paths {
        let backup_path = tracker
            .get_vanilla_backup(&game.id, relative)
            .await
            .with_context(|| format!("Failed to load vanilla backup record for '{relative}'"))?;
        if let Some(backup_path) = backup_path
            && let Some(warning) =
                restore_vanilla_file(game, tracker, game_data, relative, &backup_path).await
        {
            warnings.push(warning);
        }
    }
    Ok(warnings)
}

async fn restore_vanilla_file(
    game: &Game,
    tracker: &Tracker,
    game_data: &Path,
    relative: &str,
    backup_path: &Path,
) -> Option<String> {
    let deploy_path = match resolve_deploy_path(relative, &game.path, game_data) {
        Ok(path) => path,
        Err(error) => {
            return Some(format!(
                "Could not restore vanilla file '{relative}': invalid deployment path: {error}"
            ));
        }
    };
    if deploy_path.exists() {
        return tracker
            .delete_vanilla_backup(&game.id, relative)
            .await
            .err()
            .map(|error| format!("Could not clear restored backup record '{relative}': {error}"));
    }
    if let Some(parent) = deploy_path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return Some(format!(
            "Could not recreate '{}' while restoring '{relative}': {error}",
            parent.display()
        ));
    }
    if let Err(error) = copy_backup_file(backup_path, &deploy_path) {
        return Some(format!(
            "Could not restore vanilla file '{relative}': {error:#}"
        ));
    }
    tracker
        .delete_vanilla_backup(&game.id, relative)
        .await
        .err()
        .map(|error| format!("Could not clear restored backup record '{relative}': {error}"))
}

pub(super) async fn backup_vanilla_file(
    game: &Game,
    tracker: &Tracker,
    relative: &str,
    source: &Path,
) -> Result<()> {
    let backup_dir = paths::vanilla_backup_dir(&game.id)
        .context("Failed to resolve the vanilla backup directory")?;
    fs::create_dir_all(&backup_dir).with_context(|| {
        format!(
            "Failed to create backup directory '{}'",
            backup_dir.display()
        )
    })?;
    let backup_path = backup_dir.join(relative.to_lowercase().replace('/', "__"));
    if !backup_path.exists() {
        fs::copy(source, &backup_path).with_context(|| {
            format!(
                "Failed to back up vanilla file '{}' to '{}'",
                source.display(),
                backup_path.display()
            )
        })?;
    }
    tracker
        .save_vanilla_backup(&game.id, relative, &backup_path)
        .await
        .with_context(|| format!("Failed to record vanilla backup for '{relative}'"))?;
    Ok(())
}

fn copy_backup_file(backup_path: &Path, deploy_path: &Path) -> Result<()> {
    fs::copy(backup_path, deploy_path).with_context(|| {
        format!(
            "copy from '{}' to '{}' failed",
            backup_path.display(),
            deploy_path.display()
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::tempdir;

    use super::copy_backup_file;

    #[test]
    fn reports_failed_backup_restoration() -> Result<()> {
        let temp = tempdir()?;
        let missing_backup = temp.path().join("missing.backup");
        let destination = temp.path().join("restored.file");

        let error = copy_backup_file(&missing_backup, &destination)
            .expect_err("a missing backup must produce a restoration failure");

        assert!(error.to_string().contains("copy from"));
        assert!(!destination.exists());
        Ok(())
    }
}
