use std::collections::HashMap;
use std::path::PathBuf;

use crate::core::installer;
use crate::models::game::Game;
use crate::models::mod_entry::InstallTarget;

use super::engine_handler::EngineHandler;

pub(super) struct AuroraHandler;

impl EngineHandler for AuroraHandler {
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
