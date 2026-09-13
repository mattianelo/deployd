use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::ui::game_setup_dialog::{GameSetupDialog, GameSetupOutput};
use crate::utils;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

impl App {
    pub(crate) fn handle_remove_current_game(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(game) = self.session.games.get(self.session.selected_game_idx) {
            self.confirm_remove_game(game.id.clone(), root, sender);
        }
    }

    pub(crate) fn handle_cmd_games_persisted(
        &mut self,
        result: Result<Vec<crate::models::game::GameConfig>, String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(dialog) = self.ui.mele_setup.take() {
            dialog.close();
        }
        let configs = match result {
            Ok(configs) => configs,
            Err(error) => {
                self.push_notification(&format!("Could not save game settings: {error}"));
                return;
            }
        };
        if let Some(tracker) = self.session.tracker.clone() {
            self.location_command(sender, async move {
                AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::LocationAccessChecked(
                    crate::core::location_recovery::blocked_games(&tracker)
                        .await
                        .map_err(|error| error.to_string()),
                ))
            });
        }
        let count = self.ui.game_model.n_items();
        for _ in 0..count {
            self.ui.game_model.remove(0);
        }
        self.session.games.clear();
        for config in configs {
            self.ui.game_model.append(&config.game.title);
            self.session.games.push(config.game);
        }
        if self.session.games.is_empty() {
            self.session.selected_game_idx = 0;
            return;
        }
        // Force the reload even when the newly persisted game is already index zero.
        self.session.selected_game_idx = usize::MAX;
        sender.input(AppMsg::Games(crate::app::messages::GamesMsg::GameSelected(
            0,
        )));
    }

    pub(crate) fn handle_manage_games_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let detected: Vec<crate::models::game::Game> = self.session.games.clone();
        let cache_dirs = self.session.game_cache_dirs.clone();
        let can_export_for_snap = self.shell.running_as_appimage
            && std::env::var_os("SNAP").is_none()
            && utils::experimental_enabled();

        self.ui.game_setup_dialog = Some(
            GameSetupDialog::builder()
                .transient_for(root)
                .launch((detected, vec![], cache_dirs, can_export_for_snap))
                .forward(sender.input_sender(), |output| match output {
                    GameSetupOutput::RestoreAccessRequested {
                        game_id,
                        wine_prefix,
                    } => AppMsg::Recovery(crate::app::location_recovery::RecoveryMsg::Start(
                        game_id,
                        if wine_prefix {
                            crate::utils::location::FolderRole::Prefix
                        } else {
                            crate::utils::location::FolderRole::Game
                        },
                    )),
                    GameSetupOutput::Confirmed {
                        enabled,
                        hidden_ids,
                    } => AppMsg::Games(crate::app::messages::GamesMsg::GamesConfigured(
                        enabled, hidden_ids,
                    )),
                    GameSetupOutput::Closed => {
                        AppMsg::Games(crate::app::messages::GamesMsg::ManageGamesClosed)
                    }
                    GameSetupOutput::CacheDirChangeRequested { game_id, new_dir } => {
                        AppMsg::Games(crate::app::messages::GamesMsg::CacheDirChangeRequested {
                            game_id,
                            new_dir,
                        })
                    }
                    GameSetupOutput::CacheDirResetRequested { game_id } => {
                        AppMsg::Games(crate::app::messages::GamesMsg::CacheDirResetRequested {
                            game_id,
                        })
                    }
                    GameSetupOutput::ExportForSnapRequested { game_id } => AppMsg::Migration(
                        crate::app::messages::MigrationMsg::ExportGameForSnap(game_id),
                    ),
                }),
        );
    }

    pub(crate) fn handle_games_configured(
        &mut self,
        configs: Vec<crate::models::game::GameConfig>,
        hidden_ids: Vec<String>,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        if self.ui.mele_setup.is_some() {
            return;
        }
        if let Some(dialog) = self.ui.game_setup_dialog.take() {
            dialog.widget().close();
        }

        if configs.is_empty() && hidden_ids.is_empty() {
            return;
        }

        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        let configs_for_db = configs.clone();
        let Ok(lease) = crate::core::location_recovery::activity_lock().try_read_owned() else {
            self.push_notification(
                "Wait for folder access recovery to finish, then save game settings again",
            );
            return;
        };
        if configs
            .iter()
            .any(|config| config.game.engine == crate::models::game::GameEngine::MassEffect)
        {
            self.ui.mele_setup = Some(crate::ui::mele_dialog::SetupProgress::new(root));
        }
        let progress_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result = crate::core::game::mass_effect::baseline::configure(
                &tracker,
                &configs_for_db,
                &hidden_ids,
                std::sync::Arc::new(move |progress| {
                    let _ = progress_sender.send(AppMsg::Games(
                        crate::app::messages::GamesMsg::SetupProgress(progress),
                    ));
                }),
            )
            .await
            .map(|()| configs_for_db)
            .map_err(|error| format!("{error:#}"));
            AppCmdMsg::LocationActivityCompleted(
                lease,
                Box::new(AppCmdMsg::Games(
                    crate::app::messages::GamesCmdMsg::GamesPersisted(result),
                )),
            )
        });

        self.session.pending_new_game_ids.clear();
        self.session.selected_game_idx = 0;
        self.ui.game_dropdown.set_selected(0);
    }

    pub(crate) fn handle_manage_games_closed(&mut self, sender: &ComponentSender<Self>) {
        let ids = std::mem::take(&mut self.session.pending_new_game_ids);
        if ids.is_empty() {
            return;
        }
        // Remove the new (unconfirmed) games from the in-memory list and the dropdown.
        for id in &ids {
            if let Some(idx) = self.session.games.iter().position(|g| &g.id == id) {
                self.session.games.remove(idx);
                self.ui.game_model.remove(idx as u32);
            }
        }
        if let Some(tracker) = self.session.tracker.clone() {
            self.location_command(sender, async move {
                let result: Result<(), String> = async {
                    for id in &ids {
                        tracker
                            .hide_game(id)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    Ok(())
                }
                .await;
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
    }

    pub(crate) fn confirm_remove_game(
        &mut self,
        game_id: String,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading("Stop managing this game?")
            .body("Installed mods stay in the cache and can be kept or permanently deleted.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("remove", "Remove only");
        dialog.add_response("remove-delete", "Remove & delete mods");
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("remove-delete", adw::ResponseAppearance::Destructive);
        let s = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| match response {
            "remove" => {
                let _ = s.send(AppMsg::Games(
                    crate::app::messages::GamesMsg::RemoveGameConfirmed {
                        game_id: game_id.clone(),
                        delete_mods: false,
                    },
                ));
            }
            "remove-delete" => {
                let _ = s.send(AppMsg::Games(
                    crate::app::messages::GamesMsg::RemoveGameConfirmed {
                        game_id: game_id.clone(),
                        delete_mods: true,
                    },
                ));
            }
            _ => {}
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_remove_game(
        &mut self,
        game_id: String,
        delete_mods: bool,
        sender: &ComponentSender<Self>,
    ) {
        if !self.session.games.iter().any(|game| game.id == game_id) {
            return;
        }
        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        let cache_root = if delete_mods {
            match self.cache_root_for(&game_id) {
                Ok(path) => Some(path),
                Err(error) => {
                    self.push_notification(&format!("Cannot resolve the mod cache: {error}"));
                    return;
                }
            }
        } else {
            None
        };
        self.location_command(sender, async move {
            let result: Result<Vec<String>, String> = async {
                let mod_ids = tracker
                    .remove_managed_game(&game_id, delete_mods)
                    .await
                    .map_err(|error| error.to_string())?;
                let mut warnings = Vec::new();
                if let Some(cache_root) = cache_root {
                    for mod_id in mod_ids {
                        let cache = crate::utils::paths::mod_cache_dir_in(&cache_root, &mod_id);
                        if let Err(error) = std::fs::remove_dir_all(&cache)
                            && error.kind() != std::io::ErrorKind::NotFound
                        {
                            warnings.push(format!(
                                "Could not remove cached files for mod '{mod_id}': {error}"
                            ));
                        }
                    }
                }
                Ok(warnings)
            }
            .await;
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::GameRemoved { game_id, result })
        });
    }

    pub(crate) fn handle_cmd_game_removed(
        &mut self,
        game_id: String,
        result: Result<Vec<String>, String>,
        sender: &ComponentSender<Self>,
    ) {
        let warnings = match result {
            Ok(warnings) => warnings,
            Err(error) => {
                self.push_notification(&format!("Could not remove game: {error}"));
                return;
            }
        };
        let Some(index) = self
            .session
            .games
            .iter()
            .position(|game| game.id == game_id)
        else {
            return;
        };
        self.session.games.remove(index);
        self.ui.game_model.remove(index as u32);
        for warning in warnings {
            self.push_notification(&format!("Game removal warning: {warning}"));
        }
        if self.session.games.is_empty() {
            return;
        }
        let new_index = index.min(self.session.games.len() - 1);
        self.session.selected_game_idx = new_index;
        self.ui.game_dropdown.set_selected(new_index as u32);
        sender.input(AppMsg::Games(crate::app::messages::GamesMsg::GameSelected(
            new_index as u32,
        )));
    }
}
