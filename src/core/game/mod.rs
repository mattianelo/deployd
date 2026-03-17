mod detection;
mod ini;
mod known_games;
mod metadata;
mod tools;
mod wine;

use crate::models::game::GameEngine;
use self::known_games::{GameStore, KNOWN_GAMES};

pub use detection::detect_games;
pub use ini::{custom_ini_paths, ensure_ini_symlinks, missing_bethesda_reg_key, plugins_txt_paths};
pub use metadata::{all_nexus_domains, detect_save_dir, game_id_for_nexus_domain, has_save_management, nexus_domain};
pub use tools::{archive_mod_dir, detect_tool_path, tool_presets_for};
pub use wine::{detect_wine_config, WineConfig};
pub(crate) use wine::linux_path_to_wine_path;

/// A lightweight descriptor of a supported game type, used to populate the
/// "Add Custom Game" dropdown in the game setup dialog.
pub struct KnownGameOption {
    pub deployd_id: &'static str,
    pub title: &'static str,
    pub store: &'static str,
    pub data_subdir: &'static str,
    pub engine: &'static GameEngine,
    pub experimental: bool,
}

/// Return all supported game types for the "Add Custom Game" dropdown.
pub fn known_game_options() -> Vec<KnownGameOption> {
    KNOWN_GAMES
        .iter()
        .map(|k| KnownGameOption {
            deployd_id: k.deployd_id,
            title: k.title,
            store: match k.store {
                GameStore::Gog => "GOG",
                GameStore::Steam => "Steam",
            },
            data_subdir: k.data_subdir,
            engine: &k.engine,
            experimental: k.experimental,
        })
        .collect()
}
