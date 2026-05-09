mod aurora;
mod bethesda;
mod detection;
pub(crate) mod eclipse;
pub(crate) mod engine_handler;
mod ini;
mod known_games;
mod metadata;
mod redengine;
mod tools;
mod wine;

use std::path::PathBuf;

use self::known_games::KNOWN_GAMES;
use crate::models::game::{Game, GameEngine};

pub(crate) use engine_handler::handler_for;

pub use detection::detect_games;
pub use ini::{ensure_ini_symlinks, missing_bethesda_reg_key, plugins_txt_paths};
pub use metadata::{
    all_nexus_domains, detect_save_dir, game_id_for_nexus_domain, has_save_management,
    known_data_subdir, nexus_domain,
};
pub use tools::{archive_mod_dir, detect_tool_path, tool_presets_for};
pub(crate) use wine::linux_path_to_wine_path;
pub use wine::{
    MissingSnapWineContent, SnapWineStatus, WineConfig, WineLauncher, detect_wine_config, is_snap,
    missing_snap_wine_message, missing_snap_wine_plugs, proton_runtime_available,
    snap_wine_available, snap_wine_status, umu_folders_path,
};

pub struct KnownGameOption {
    pub deployd_id: &'static str,
    pub title: &'static str,
    pub data_subdir: &'static str,
    pub engine: &'static GameEngine,
}

pub fn tool_search_dir(game: &Game) -> Option<PathBuf> {
    engine_handler::handler_for(&game.engine).tool_search_dir(game)
}

pub fn deploy_dir(game: &Game) -> PathBuf {
    engine_handler::handler_for(&game.engine).deploy_dir(game)
}

pub fn known_game_options() -> Vec<KnownGameOption> {
    KNOWN_GAMES
        .iter()
        .map(|k| KnownGameOption {
            deployd_id: k.deployd_id,
            title: k.title,
            data_subdir: k.data_subdir,
            engine: &k.engine,
        })
        .collect()
}
