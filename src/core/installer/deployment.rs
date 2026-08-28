use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::core::game::engine_handler::EngineHandler;
use crate::core::rules;
use crate::models::game::{Game, GameEngine};
use crate::models::mod_entry::InstallTarget;
use crate::utils::paths as utils_paths;

use super::filter_excluded_files;
use super::paths::{strip_data_subdir_prefix, strip_data_subdir_prefix_str};

pub(super) enum PlannedAction {
    Skip,
    Directory {
        explicit_root: bool,
    },
    File {
        deploy_to_root: bool,
        plugin_name: Option<String>,
    },
}

pub(super) struct PlannedFile {
    pub source: PathBuf,
    pub lowercase_rel: PathBuf,
    pub original_rel: String,
    pub action: PlannedAction,
}

pub(super) fn route_and_plan(
    file_list: Vec<(PathBuf, PathBuf)>,
    game: &Game,
    mod_name: &str,
    stripped_wrapper: Option<&str>,
    file_targets: &HashMap<String, InstallTarget>,
    excluded_files: &HashSet<String>,
) -> Vec<PlannedFile> {
    let game_rules = rules::rules_for_game(&game.id);
    let file_list = filter_excluded_files(
        file_list,
        &game_rules,
        &game.engine,
        &game.data_subdir,
        excluded_files,
    );
    let handler = crate::core::game::engine_handler::handler_for(&game.engine);
    let file_list =
        handler.route_file_list(game, mod_name, stripped_wrapper, file_list, file_targets);
    plan_files(
        file_list,
        game,
        &game_rules,
        handler,
        file_targets,
        excluded_files,
    )
}

fn plan_files(
    file_list: Vec<(PathBuf, PathBuf)>,
    game: &Game,
    game_rules: &[rules::Rule],
    handler: &dyn EngineHandler,
    file_targets: &HashMap<String, InstallTarget>,
    excluded_files: &HashSet<String>,
) -> Vec<PlannedFile> {
    file_list
        .into_iter()
        .map(|(source, destination)| {
            let ruled_path = rules::apply_rules(game_rules, &destination.to_string_lossy());
            let file_key = ruled_path.replace('\\', "/");
            let explicit_root = file_key.starts_with("../");
            let original_rel = file_key
                .strip_prefix("../")
                .unwrap_or(&file_key)
                .to_string();
            let lowercase_rel = utils_paths::lowercase_path(Path::new(&original_rel));
            let lowercase_rel = strip_data_subdir_prefix(&lowercase_rel, &game.data_subdir);
            let original_rel = strip_data_subdir_prefix_str(&original_rel, &game.data_subdir);

            let action = if source.is_dir() {
                if should_skip_empty_bethesda_sentinel(&game.engine, &lowercase_rel) {
                    PlannedAction::Skip
                } else {
                    PlannedAction::Directory { explicit_root }
                }
            } else if excluded_files.contains(&file_key) {
                PlannedAction::Skip
            } else {
                let deploy_to_root = handler.deploy_to_root(&file_key, file_targets, explicit_root);
                let plugin_name = plugin_name(&lowercase_rel, &original_rel);
                PlannedAction::File {
                    deploy_to_root,
                    plugin_name,
                }
            };

            PlannedFile {
                source,
                lowercase_rel,
                original_rel,
                action,
            }
        })
        .collect()
}

fn should_skip_empty_bethesda_sentinel(engine: &GameEngine, rel: &Path) -> bool {
    *engine == GameEngine::Bethesda && rel.as_os_str().is_empty()
}

fn plugin_name(lowercase_rel: &Path, original_rel: &str) -> Option<String> {
    let lowercase_rel = lowercase_rel.to_string_lossy();
    let is_plugin = [".esp", ".esm", ".esl"]
        .iter()
        .any(|extension| lowercase_rel.ends_with(extension));
    is_plugin
        .then(|| Path::new(original_rel).file_name())
        .flatten()
        .map(|name| name.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::game::engine_handler;

    fn game(engine: GameEngine) -> Game {
        Game {
            id: "test".to_string(),
            title: "Test".to_string(),
            path: PathBuf::from("game"),
            data_subdir: "Data".to_string(),
            engine,
            wine_prefix: None,
        }
    }

    #[test]
    fn plans_root_plugin_without_touching_cache() {
        let game = game(GameEngine::Bethesda);
        let planned = plan_files(
            vec![(PathBuf::from("plugin.esp"), PathBuf::from("../Plugin.esp"))],
            &game,
            &[],
            engine_handler::handler_for(&game.engine),
            &HashMap::new(),
            &HashSet::new(),
        );

        assert_eq!(planned[0].lowercase_rel, PathBuf::from("plugin.esp"));
        assert_eq!(planned[0].original_rel, "Plugin.esp");
        assert!(matches!(
            &planned[0].action,
            PlannedAction::File {
                deploy_to_root: true,
                plugin_name: Some(name),
            } if name == "Plugin.esp"
        ));
    }

    #[test]
    fn preserves_excluded_file_as_progress_step() {
        let game = game(GameEngine::Bethesda);
        let planned = plan_files(
            vec![(PathBuf::from("readme.txt"), PathBuf::from("readme.txt"))],
            &game,
            &[],
            engine_handler::handler_for(&game.engine),
            &HashMap::new(),
            &HashSet::from(["readme.txt".to_string()]),
        );

        assert_eq!(planned.len(), 1);
        assert!(matches!(planned[0].action, PlannedAction::Skip));
    }

    #[test]
    fn skips_only_bethesda_empty_directory_sentinel() {
        assert!(should_skip_empty_bethesda_sentinel(
            &GameEngine::Bethesda,
            Path::new("")
        ));
        assert!(!should_skip_empty_bethesda_sentinel(
            &GameEngine::Bethesda,
            Path::new("EmptyDir")
        ));
        assert!(!should_skip_empty_bethesda_sentinel(
            &GameEngine::Aurora,
            Path::new("")
        ));
    }
}
