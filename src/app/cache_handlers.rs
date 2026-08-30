use std::path::PathBuf;

use relm4::prelude::*;

use crate::utils::paths;

use super::App;
use super::messages::AppCmdMsg;
use crate::core::cache;

impl App {
    pub(crate) fn handle_cache_dir_change_requested(
        &mut self,
        game_id: String,
        new_dir: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let game_path = self
            .session
            .games
            .iter()
            .find(|g| g.id == game_id)
            .map(|g| g.path.clone());
        let Some(game_path) = game_path else {
            self.push_notification("Game not found");
            return;
        };
        let old_cache_root = match self.cache_root_for(&game_id) {
            Ok(path) => path,
            Err(error) => {
                self.push_notification(&format!("Cannot resolve the current cache: {error}"));
                return;
            }
        };
        let new_dir_clone = new_dir.clone();

        self.show_toast("Moving cache…");

        sender.oneshot_command(async move {
            let result = cache::move_game_cache(
                &tracker,
                &game_id,
                &game_path,
                &old_cache_root,
                &new_dir_clone,
            )
            .await
            .map_err(|e| e.to_string());
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::CacheDirMoved {
                game_id,
                new_dir,
                result,
            })
        });
    }

    pub(crate) fn handle_cache_dir_reset_requested(
        &mut self,
        game_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let current_cache_root = match self.cache_root_for(&game_id) {
            Ok(path) => path,
            Err(error) => {
                self.push_notification(&format!("Cannot resolve the current cache: {error}"));
                return;
            }
        };
        let default_cache_root = match paths::cache_root() {
            Ok(path) => path,
            Err(error) => {
                self.push_notification(&format!("Cannot resolve the default cache: {error}"));
                return;
            }
        };

        if current_cache_root == default_cache_root {
            return;
        }

        self.show_toast("Resetting cache location…");

        let game_id_clone = game_id.clone();
        sender.oneshot_command(async move {
            let result = cache::reset_game_cache(
                &tracker,
                &game_id_clone,
                &current_cache_root,
                &default_cache_root,
            )
            .await
            .map_err(|e| e.to_string());
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::CacheDirReset {
                game_id: game_id_clone,
                result,
            })
        });
    }

    pub(crate) fn handle_cmd_cache_dir_moved(
        &mut self,
        game_id: String,
        new_dir: PathBuf,
        result: Result<(), String>,
    ) {
        match result {
            Ok(()) => {
                self.session.game_cache_dirs.insert(game_id, new_dir);
                self.show_toast("Cache moved successfully");
            }
            Err(e) => {
                self.push_notification(&format!("Cache move failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_cache_dir_reset(
        &mut self,
        game_id: String,
        result: Result<(), String>,
    ) {
        match result {
            Ok(()) => {
                self.session.game_cache_dirs.remove(&game_id);
                self.show_toast("Cache location reset to default");
            }
            Err(e) => {
                self.push_notification(&format!("Cache reset failed: {e}"));
            }
        }
    }
}
