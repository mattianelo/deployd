use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::models::game::GameEngine;
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

    pub(crate) fn handle_cmd_games_persisted(&mut self, sender: &ComponentSender<Self>) {
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
    ) {
        if let Some(dialog) = self.ui.game_setup_dialog.take() {
            dialog.widget().close();
        }

        if configs.is_empty() && hidden_ids.is_empty() {
            return;
        }

        let n_existing = self.ui.game_model.n_items();
        for _ in 0..n_existing {
            self.ui.game_model.remove(0);
        }
        self.session.games.clear();

        for cfg in &configs {
            self.ui.game_model.append(&cfg.game.title);
            self.session.games.push(cfg.game.clone());
        }

        if let Some(tracker) = self.session.tracker.clone() {
            let configs_for_db = configs.clone();
            let hidden_for_db = hidden_ids;
            let first_game_id = self.session.games.first().map(|g| g.id.clone());
            sender.oneshot_command(async move {
                for cfg in &configs_for_db {
                    let engine_str = match cfg.game.engine {
                        GameEngine::REDEngine => "redengine",
                        GameEngine::Eclipse => "eclipse",
                        GameEngine::Aurora => "aurora",
                        GameEngine::Bethesda => "bethesda",
                    };
                    let _ = tracker
                        .upsert_game(
                            &cfg.game.id,
                            &cfg.game.title,
                            &cfg.game.path,
                            &cfg.game.data_subdir,
                            engine_str,
                            cfg.game.wine_prefix.as_deref(),
                            cfg.custom,
                        )
                        .await;
                }
                for id in &hidden_for_db {
                    let _ = tracker.hide_game(id).await;
                }
                if let Some(id) = first_game_id {
                    let _ = tracker.set_setting("last_game_id", &id).await;
                }
                AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::GamesPersisted)
            });
        }

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
            sender.oneshot_command(async move {
                for id in &ids {
                    let _ = tracker.hide_game(id).await;
                }
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(Ok(())))
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
        let Some(idx) = self.session.games.iter().position(|g| g.id == game_id) else {
            return;
        };

        self.session.games.remove(idx);
        self.ui.game_model.remove(idx as u32);

        if let Some(tracker) = self.session.tracker.clone() {
            let cache_root = self
                .cache_root_for(&game_id)
                .unwrap_or_else(|_| crate::utils::paths::cache_root().unwrap_or_default());
            sender.oneshot_command(async move {
                let _ = tracker.hide_game(&game_id).await;
                if delete_mods && let Ok(mods) = tracker.list_mods(&game_id).await {
                    for m in mods {
                        let _ = tracker.delete_plugins_for_mod(&m.id).await;
                        let _ = tracker.delete_mod_files(&m.id).await;
                        let _ = tracker.delete_mod(&m.id).await;
                        let cache = crate::utils::paths::mod_cache_dir_in(&cache_root, &m.id);
                        if cache.exists() {
                            let _ = std::fs::remove_dir_all(&cache);
                        }
                    }
                }
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(Ok(())))
            });
        }

        if self.session.games.is_empty() {
            return;
        }

        let new_idx = idx.min(self.session.games.len() - 1);
        self.session.selected_game_idx = new_idx;
        self.ui.game_dropdown.set_selected(new_idx as u32);
        sender.input(AppMsg::Games(crate::app::messages::GamesMsg::GameSelected(
            new_idx as u32,
        )));
    }
}
