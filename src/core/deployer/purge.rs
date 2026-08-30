use std::path::Path;

use anyhow::Result;

use crate::core::game;
use crate::core::mod_folders;
use crate::core::tracker::Tracker;
use crate::models::game::Game;

use super::backup::{bake_modified_plugins, restore_all_vanilla};
use super::filesystem::remove_deployed_file;

pub async fn purge(game: &Game, tracker: &Tracker, cache_root: &Path) -> Result<usize> {
    let game_data = game::deploy_dir(game);
    bake_modified_plugins(game, tracker, &game_data).await;

    let deployed = tracker.get_deployed_files(&game.id).await?;
    let count = deployed.len();
    for f in &deployed {
        remove_deployed_file(f, game, &game_data);
    }
    tracker.clear_deployed_files(&game.id).await?;

    restore_all_vanilla(game, tracker, &game_data).await;

    if let Err(e) = mod_folders::refresh_named_mod_folders(tracker, &game.id, cache_root).await {
        eprintln!("[deployd] named_mods refresh failed: {e}");
    }

    Ok(count)
}
