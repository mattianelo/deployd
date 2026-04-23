use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use anyhow::Result;

use crate::core::installer::auto_detect_install_target;
use crate::core::tracker::Tracker;
use crate::models::game::Game;
use crate::models::mod_entry::InstallTarget;
use crate::utils::plugins_txt;

use super::engine_handler::EngineHandler;
use super::ini;

pub(super) struct BethesdaHandler;

impl EngineHandler for BethesdaHandler {
    fn route_file_list(
        &self,
        _game: &Game,
        _mod_name: &str,
        _stripped_wrapper: Option<&str>,
        file_list: Vec<(PathBuf, PathBuf)>,
        _file_targets: &HashMap<String, InstallTarget>,
    ) -> Vec<(PathBuf, PathBuf)> {
        file_list
    }

    fn deploy_to_root(
        &self,
        file_key: &str,
        file_targets: &HashMap<String, InstallTarget>,
        explicit_root: bool,
    ) -> bool {
        if explicit_root {
            file_targets
                .get(file_key)
                .cloned()
                .unwrap_or(InstallTarget::Root)
                == InstallTarget::Root
        } else {
            // Auto-detect Root for SKSE loaders, ASI plugins, and similar.
            file_targets
                .get(file_key)
                .cloned()
                .unwrap_or_else(|| auto_detect_install_target(file_key))
                == InstallTarget::Root
        }
    }

    fn post_deploy<'a>(
        &'a self,
        game: &'a Game,
        tracker: &'a Tracker,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let plugins = tracker.list_plugins(&game.id).await?;

            let plugins_paths = ini::plugins_txt_paths(game);
            if plugins_paths.is_empty() {
                eprintln!("deployd: WINE prefix not found, skipping Plugins.txt");
            }
            for plugins_path in &plugins_paths {
                plugins_txt::write_plugins_txt(plugins_path, &plugins)?;
            }

            let ini_paths = ini::custom_ini_paths(game);
            if ini_paths.is_empty() {
                eprintln!("deployd: WINE prefix not found, skipping custom INI");
            }
            for ini_path in &ini_paths {
                plugins_txt::ensure_archive_invalidation(ini_path)?;
            }

            Ok(())
        })
    }
}
