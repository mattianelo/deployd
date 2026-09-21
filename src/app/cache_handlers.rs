use std::path::PathBuf;

use adw::prelude::*;
use relm4::prelude::*;

use crate::core::cache;
use crate::utils::paths;

use super::App;
use super::messages::{AppCmdMsg, AppMsg, GamesMsg};

impl App {
    pub(crate) fn handle_cache_dir_change_requested(
        &mut self,
        game_id: String,
        new_dir: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        if self.is_busy() || self.shell.location_recovery.is_some() {
            self.show_toast("Wait for the current operation to finish");
            return;
        }
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

        self.open_cache_move("Moving cache");
        let progress = cache_progress(sender);

        self.location_command(sender, async move {
            let result = cache::move_game_cache_with_progress(
                &tracker,
                &game_id,
                &game_path,
                &old_cache_root,
                &new_dir_clone,
                &progress,
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
        if self.is_busy() || self.shell.location_recovery.is_some() {
            self.show_toast("Wait for the current operation to finish");
            return;
        }
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

        self.open_cache_move("Restoring the default cache location");
        let progress = cache_progress(sender);

        let game_id_clone = game_id.clone();
        self.location_command(sender, async move {
            let result = cache::reset_game_cache_with_progress(
                &tracker,
                &game_id_clone,
                &current_cache_root,
                &default_cache_root,
                &progress,
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
        sender: &ComponentSender<Self>,
    ) {
        self.finish_cache_move(&result);
        match result {
            Ok(()) => {
                self.session
                    .game_cache_dirs
                    .insert(game_id.clone(), new_dir);
                self.refresh_after_cache_move(&game_id, sender);
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
        sender: &ComponentSender<Self>,
    ) {
        self.finish_cache_move(&result);
        match result {
            Ok(()) => {
                self.session.game_cache_dirs.remove(&game_id);
                self.refresh_after_cache_move(&game_id, sender);
                self.show_toast("Cache location reset to default");
            }
            Err(e) => {
                self.push_notification(&format!("Cache reset failed: {e}"));
            }
        }
    }
}

fn cache_progress(
    sender: &ComponentSender<App>,
) -> impl Fn(usize, usize, &'static str) + Send + Sync + 'static {
    let sender = sender.input_sender().clone();
    move |done, total, phase| {
        let _ = sender.send(AppMsg::Games(GamesMsg::CacheMoveProgress {
            done,
            total,
            phase,
        }));
    }
}

pub(crate) struct CacheMove {
    dialog: adw::AlertDialog,
    progress: gtk::ProgressBar,
}

impl App {
    fn open_cache_move(&mut self, heading: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body("Preparing the cache move…")
            .build();
        dialog.set_can_close(false);
        let progress = gtk::ProgressBar::new();
        dialog.set_extra_child(Some(&progress));
        if let Some(setup) = &self.ui.game_setup_dialog {
            dialog.present(Some(setup.widget()));
        } else {
            dialog.present(Some(&self.ui.toast_overlay));
        }
        self.ui.cache_move = Some(CacheMove { dialog, progress });
    }

    pub(crate) fn cache_move_progress(&self, done: usize, total: usize, phase: &str) {
        if let Some(operation) = &self.ui.cache_move {
            operation.dialog.set_body(phase);
            operation
                .progress
                .set_fraction(done as f64 / total.max(1) as f64);
        }
    }

    fn finish_cache_move(&mut self, result: &Result<(), String>) {
        if let Some(operation) = self.ui.cache_move.take() {
            if let Err(error) = result {
                operation.dialog.set_heading(Some("Cache move failed"));
                operation.dialog.set_body(error);
                operation.progress.set_visible(false);
                operation.dialog.add_response("close", "Close");
                operation.dialog.set_close_response("close");
                operation.dialog.set_can_close(true);
            } else {
                operation.dialog.force_close();
            }
        }
        self.update_cache_settings_dialog();
    }

    fn update_cache_settings_dialog(&self) {
        if let Some(setup) = &self.ui.game_setup_dialog {
            setup.emit(
                crate::ui::game_setup_dialog::GameSetupMsg::CacheDirsUpdated(
                    self.session.game_cache_dirs.clone(),
                ),
            );
        }
    }

    fn refresh_after_cache_move(&mut self, game_id: &str, sender: &ComponentSender<Self>) {
        self.update_cache_settings_dialog();
        if self.selected_game().is_some_and(|game| game.id == game_id) {
            self.reload_mods(sender);
            self.refresh_deployment_status(sender);
        }
    }
}
