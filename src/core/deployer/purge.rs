use std::path::Path;

use anyhow::Result;

use crate::core::game;
use crate::core::mod_folders;
use crate::core::tracker::Tracker;
use crate::models::game::Game;

use super::backup::{bake_modified_plugins, restore_all_vanilla};
use super::filesystem::remove_deployed_file;
use super::report::PurgeOutcome;

pub async fn purge(game: &Game, tracker: &Tracker, cache_root: &Path) -> Result<PurgeOutcome> {
    let game_data = game::deploy_dir(game);
    let mut warnings = Vec::new();
    bake_modified_plugins(game, tracker, &game_data).await?;

    let deployed = tracker.get_deployed_files(&game.id).await?;
    let count = deployed.len();
    for f in &deployed {
        warnings.extend(remove_deployed_file(f, game, &game_data)?);
    }
    tracker.clear_deployed_files(&game.id).await?;

    warnings.extend(restore_all_vanilla(game, tracker, &game_data).await?);

    if let Err(e) = mod_folders::refresh_named_mod_folders(tracker, &game.id, cache_root).await {
        warnings.push(format!("Named mod-folder refresh failed: {e}"));
    }

    Ok(PurgeOutcome {
        files_removed: count,
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

    use super::purge;

    // @variants: both
    #[tokio::test]
    async fn does_not_report_success_after_required_removal_fails() -> Result<()> {
        let temp = tempdir()?;
        let game_data = temp.path().join("Data");
        std::fs::create_dir_all(game_data.join("blocked.esp"))?;
        let game = Game {
            id: "game".to_string(),
            title: "Game".to_string(),
            path: temp.path().to_path_buf(),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: None,
        };
        let deployed = ModFile {
            mod_id: "mod".to_string(),
            game_rel_lowercase: "blocked.esp".to_string(),
            game_rel_original: "blocked.esp".to_string(),
            cache_path: temp
                .path()
                .join("cache/mod/blocked.esp")
                .display()
                .to_string(),
        };
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker
            .record_deployed_files(&game.id, std::slice::from_ref(&deployed))
            .await?;

        let error = purge(&game, &tracker, &temp.path().join("cache"))
            .await
            .expect_err("purge must fail when a tracked file cannot be removed");

        assert!(error.to_string().contains("Failed to remove deployed file"));
        assert_eq!(tracker.get_deployed_files(&game.id).await?.len(), 1);
        Ok(())
    }
}
