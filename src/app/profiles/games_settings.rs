use std::path::PathBuf;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::models::game::GameEngine;
use crate::ui::game_setup_dialog::{GameSetupDialog, GameSetupOutput};
use crate::ui::settings_dialog::{SettingsDialog, SettingsDialogOutput};

use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::App;

impl App {
    pub(crate) fn handle_settings_clicked(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        self.overflow_menu_btn.popdown();
        let Some(tracker) = self.tracker.clone() else {
            self.toaster.toast("Database not ready yet");
            return;
        };
        self.settings_dialog = Some(
            SettingsDialog::builder()
                .transient_for(root)
                .launch(tracker)
                .forward(sender.input_sender(), |output| match output {
                    SettingsDialogOutput::Closed => AppMsg::SettingsClosed,
                    SettingsDialogOutput::ApiKeyChanged => AppMsg::NexusApiKeyUpdated,
                    SettingsDialogOutput::ManageGames => AppMsg::ManageGamesClicked,
                    SettingsDialogOutput::RescanGames => AppMsg::RescanGames,
                }),
        );
    }

    pub(crate) fn handle_settings_closed(&mut self, sender: &ComponentSender<Self>) {
        if let Some(dialog) = self.settings_dialog.take() {
            dialog.widget().destroy();
        }
        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                let dir = tracker
                    .get_setting("downloads_dir")
                    .await
                    .ok()
                    .flatten()
                    .map(PathBuf::from);
                AppCmdMsg::DownloadsDirUpdated(dir)
            });
        }
    }

    pub(crate) fn handle_nexus_api_key_updated(&mut self) {
        self.toaster.toast("Nexus Mods key updated.");
    }

    pub(crate) fn handle_manage_games_clicked(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let detected: Vec<crate::models::game::Game> = self.games.clone();

        self.game_setup_dialog = Some(
            GameSetupDialog::builder()
                .transient_for(root)
                .launch((detected, vec![]))
                .forward(sender.input_sender(), |output| match output {
                    GameSetupOutput::Confirmed { enabled, hidden_ids } => {
                        AppMsg::GamesConfigured(enabled, hidden_ids)
                    }
                    GameSetupOutput::Closed => AppMsg::Noop,
                }),
        );
    }

    pub(crate) fn handle_games_configured(
        &mut self,
        configs: Vec<crate::app::messages::GameConfig>,
        hidden_ids: Vec<String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(dialog) = self.game_setup_dialog.take() {
            dialog.widget().close();
        }

        if configs.is_empty() && hidden_ids.is_empty() {
            return;
        }

        let n_existing = self.game_model.n_items();
        for _ in 0..n_existing {
            self.game_model.remove(0);
        }
        self.games.clear();

        for cfg in &configs {
            self.game_model.append(&cfg.game.title);
            self.games.push(cfg.game.clone());
        }

        if let Some(tracker) = self.tracker.clone() {
            let configs_for_db = configs.clone();
            let hidden_for_db = hidden_ids;
            sender.oneshot_command(async move {
                for cfg in &configs_for_db {
                    let engine_str = match cfg.game.engine {
                        GameEngine::REDEngine => "redengine",
                        GameEngine::Eclipse => "eclipse",
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
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }

        self.selected_game_idx = 0;
        self.game_dropdown.set_selected(0);
        if let Some(game) = self.games.first() {
            let game_id = game.id.clone();
            if let Some(tracker) = self.tracker.clone() {
                sender.oneshot_command(async move {
                    let _ = tracker.set_setting("last_game_id", &game_id).await;
                    AppCmdMsg::PrioritySaved(Ok(()))
                });
            }
            sender.input(AppMsg::GameSelected(0));
        }
    }

    pub(crate) fn handle_remove_game(&mut self, game_id: String, sender: &ComponentSender<Self>) {
        let Some(idx) = self.games.iter().position(|g| g.id == game_id) else {
            return;
        };

        self.games.remove(idx);
        self.game_model.remove(idx as u32);

        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                let _ = tracker.hide_game(&game_id).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }

        if self.games.is_empty() {
            return;
        }

        let new_idx = idx.min(self.games.len() - 1);
        self.selected_game_idx = new_idx;
        self.game_dropdown.set_selected(new_idx as u32);
        sender.input(AppMsg::GameSelected(new_idx as u32));
    }
}
