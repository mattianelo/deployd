mod detection;
pub(crate) mod eclipse;
mod ini;
mod known_games;
mod metadata;
mod tools;
mod wine;

use std::path::PathBuf;

use crate::models::game::{Game, GameEngine};
use self::known_games::{GameStore, KNOWN_GAMES};

pub use detection::detect_games;
pub use ini::{custom_ini_paths, ensure_ini_symlinks, missing_bethesda_reg_key, plugins_txt_paths};
pub use metadata::{all_nexus_domains, detect_save_dir, game_id_for_nexus_domain, has_save_management, nexus_domain};
pub use tools::{archive_mod_dir, detect_tool_path, tool_presets_for};
pub use eclipse::write_addins_xml;
pub use wine::{detect_wine_config, WineConfig};
pub(crate) use wine::linux_path_to_wine_path;

pub struct KnownGameOption {
    pub deployd_id: &'static str,
    pub title: &'static str,
    pub store: &'static str,
    pub data_subdir: &'static str,
    pub engine: &'static GameEngine,
}

/// For Eclipse, returns the Wine user's `Documents/` folder (two levels above `deploy_dir`).
pub fn tool_search_dir(game: &Game) -> Option<PathBuf> {
    let dd = deploy_dir(game);
    if game.engine == GameEngine::Eclipse {
        dd.parent().and_then(|p| p.parent()).map(|p| p.to_path_buf())
    } else {
        Some(dd)
    }
}

/// For Eclipse, resolves inside the Wine prefix user directory instead of the game folder.
pub fn deploy_dir(game: &Game) -> PathBuf {
    if game.engine == GameEngine::Eclipse {
        if let Some(known) = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id) {
            if let Some(user_dir) = wine::find_wine_user_dir(known, game) {
                return user_dir.join(&game.data_subdir);
            }
        }
    }
    game.data_dir()
}

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
        })
        .collect()
}
