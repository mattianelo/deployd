use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::installer;
use crate::models::game::Game;
use crate::models::mod_entry::InstallTarget;

use super::engine_handler::EngineHandler;

pub(super) struct AuroraHandler;

impl EngineHandler for AuroraHandler {
    fn conflict_key<'a>(&self, game_rel_lowercase: &'a str) -> &'a str {
        if game_rel_lowercase.starts_with("override/") {
            // Aurora loads all Override/ files by filename only, regardless of
            // subfolder depth — override/ModA/path/mod.xml and override/mod.xml
            // both surface as mod.xml to the engine.
            game_rel_lowercase
                .rsplit('/')
                .next()
                .unwrap_or(game_rel_lowercase)
        } else {
            game_rel_lowercase
        }
    }

    fn route_file_list(
        &self,
        game: &Game,
        _mod_name: &str,
        _stripped_wrapper: Option<&str>,
        file_list: Vec<(PathBuf, PathBuf)>,
        file_targets: &HashMap<String, InstallTarget>,
    ) -> Vec<(PathBuf, PathBuf)> {
        installer::route_aurora_paths(file_list, &game.data_subdir, file_targets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str) -> &str {
        AuroraHandler.conflict_key(path)
    }

    #[test]
    fn override_root_file_returns_filename() {
        assert_eq!(key("override/foo.xml"), "foo.xml");
    }

    #[test]
    fn override_nested_file_returns_filename() {
        assert_eq!(key("override/mod/sub/foo.xml"), "foo.xml");
    }

    #[test]
    fn non_override_path_is_unchanged() {
        assert_eq!(key("modules/foo.mod"), "modules/foo.mod");
    }

    #[test]
    fn system_path_is_unchanged() {
        assert_eq!(key("../system/foo.key"), "../system/foo.key");
    }

    #[test]
    fn override_paths_with_same_filename_share_conflict_key() {
        assert_eq!(
            key("override/moda/path/items.xml"),
            key("override/items.xml")
        );
    }
}
