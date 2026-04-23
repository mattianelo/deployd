use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::installer;
use crate::models::game::Game;
use crate::models::mod_entry::InstallTarget;

use super::engine_handler::EngineHandler;

pub(super) struct REDEngineHandler;

impl EngineHandler for REDEngineHandler {
    fn route_file_list(
        &self,
        game: &Game,
        mod_name: &str,
        stripped_wrapper: Option<&str>,
        file_list: Vec<(PathBuf, PathBuf)>,
        _file_targets: &HashMap<String, InstallTarget>,
    ) -> Vec<(PathBuf, PathBuf)> {
        installer::apply_redengine_path_fixups(game, mod_name, stripped_wrapper, file_list)
    }
}
