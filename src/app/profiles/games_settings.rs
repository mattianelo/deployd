use std::path::PathBuf;

use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::models::game::GameEngine;
use crate::ui::game_setup_dialog::{GameSetupDialog, GameSetupOutput};
use crate::ui::settings_dialog::{SettingsDialog, SettingsDialogOutput};
use crate::ui::welcome_wizard::{WelcomeWizard, WelcomeWizardOutput};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

impl App {
    pub(crate) fn handle_settings_clicked(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        self.overflow_menu_btn.popdown();
        let Some(tracker) = self.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        self.settings_dialog = Some(
            SettingsDialog::builder()
                .transient_for(root)
                .launch((
                    tracker,
                    self.nexus_username.is_some(),
                    self.compact_plugin_rows,
                    self.color_scheme_idx,
                ))
                .forward(sender.input_sender(), |output| match output {
                    SettingsDialogOutput::Closed => AppMsg::SettingsClosed,
                    SettingsDialogOutput::ApiKeyChanged => AppMsg::NexusApiKeyUpdated,
                    SettingsDialogOutput::ManageGames => AppMsg::ManageGamesClicked,
                    SettingsDialogOutput::SetCompactPluginRows(compact) => {
                        AppMsg::SetCompactPluginRows(compact)
                    }
                    SettingsDialogOutput::ColorSchemeChanged(idx) => AppMsg::SetColorScheme(idx),
                    SettingsDialogOutput::CreateFullBackup => AppMsg::CreateFullBackupClicked,
                    SettingsDialogOutput::RestoreFromBackup => AppMsg::RestoreFromBackupClicked,
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

    pub(crate) fn handle_nexus_api_key_updated(&mut self, sender: &ComponentSender<Self>) {
        self.push_notification("Nexus Mods key updated.");
        // Re-validate to refresh username and avatar displayed in the headerbar.
        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .ok()
                    .flatten()
                    .filter(|k| !k.is_empty());
                match api_key {
                    Some(key) => {
                        let client = crate::core::nexus_api::NexusClient::new(key);
                        match client.validate_key().await {
                            Ok((user, _)) => {
                                let _ = tracker.save_nexus_user(&user).await;
                                AppCmdMsg::NexusUserRefreshed(
                                    Some(user.name),
                                    user.profile_url,
                                    user.is_premium,
                                )
                            }
                            Err(_) => AppCmdMsg::NexusUserRefreshed(None, None, false),
                        }
                    }
                    None => AppCmdMsg::NexusUserRefreshed(None, None, false),
                }
            });
        }
    }

    pub(crate) fn handle_nexus_login_clicked(&mut self, sender: &ComponentSender<Self>) {
        self.nexus_user_btn.popdown();
        let Some(tracker) = self.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        let input = sender.input_sender().clone();
        relm4::spawn(async move {
            match crate::core::nexus_api::sso_login().await {
                Ok(api_key) => {
                    if let Err(e) = tracker.set_setting("nexus_api_key", &api_key).await {
                        let _ = input.send(AppMsg::ShowToast(format!("Login error: {e}")));
                        return;
                    }
                    if let Err(e) = tracker.set_setting("nexus_login_source", "sso").await {
                        let _ = input.send(AppMsg::ShowToast(format!("Login error: {e}")));
                        return;
                    }
                    let _ = input.send(AppMsg::NexusApiKeyUpdated);
                }
                Err(e) => {
                    let _ = input.send(AppMsg::ShowToast(format!("Nexus login failed: {e}")));
                }
            }
        });
    }

    pub(crate) fn handle_nexus_logout_clicked(&mut self, sender: &ComponentSender<Self>) {
        self.nexus_user_btn.popdown();
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        sender.oneshot_command(async move {
            let _ = tracker.clear_nexus_user().await;
            AppCmdMsg::NexusUserRefreshed(None, None, false)
        });
        self.push_notification("Logged out of Nexus Mods");
    }

    pub(crate) fn handle_manage_games_clicked(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let detected: Vec<crate::models::game::Game> = self.games.clone();
        let cache_dirs = self.game_cache_dirs.clone();

        self.game_setup_dialog = Some(
            GameSetupDialog::builder()
                .transient_for(root)
                .launch((detected, vec![], cache_dirs))
                .forward(sender.input_sender(), |output| match output {
                    GameSetupOutput::Confirmed {
                        enabled,
                        hidden_ids,
                    } => AppMsg::GamesConfigured(enabled, hidden_ids),
                    GameSetupOutput::Closed => AppMsg::ManageGamesClosed,
                    GameSetupOutput::CacheDirChangeRequested { game_id, new_dir } => {
                        AppMsg::CacheDirChangeRequested { game_id, new_dir }
                    }
                    GameSetupOutput::CacheDirResetRequested { game_id } => {
                        AppMsg::CacheDirResetRequested { game_id }
                    }
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
            let first_game_id = self.games.first().map(|g| g.id.clone());
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
                AppCmdMsg::GamesPersisted
            });
        }

        self.pending_new_game_ids.clear();
        self.selected_game_idx = 0;
        self.game_dropdown.set_selected(0);
    }

    pub(crate) fn handle_manage_games_closed(&mut self, sender: &ComponentSender<Self>) {
        let ids = std::mem::take(&mut self.pending_new_game_ids);
        if ids.is_empty() {
            return;
        }
        // Remove the new (unconfirmed) games from the in-memory list and the dropdown.
        for id in &ids {
            if let Some(idx) = self.games.iter().position(|g| &g.id == id) {
                self.games.remove(idx);
                self.game_model.remove(idx as u32);
            }
        }
        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                for id in &ids {
                    let _ = tracker.hide_game(id).await;
                }
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
    }

    pub(crate) fn handle_show_welcome_wizard(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        self.welcome_wizard = Some(
            WelcomeWizard::builder()
                .transient_for(root)
                .launch(())
                .forward(sender.input_sender(), |out| match out {
                    WelcomeWizardOutput::Confirmed {
                        enabled,
                        hidden_ids,
                    } => AppMsg::WelcomeWizardConfirmed(enabled, hidden_ids),
                    WelcomeWizardOutput::Skipped => AppMsg::WelcomeWizardSkipped,
                }),
        );
    }

    pub(crate) fn handle_welcome_wizard_confirmed(
        &mut self,
        configs: Vec<crate::app::messages::GameConfig>,
        hidden_ids: Vec<String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(w) = self.welcome_wizard.take() {
            w.widget().close();
        }
        // Persist the wizard-shown marker so we don't show it again.
        if let Some(ref tracker) = self.tracker {
            let t = tracker.clone();
            relm4::spawn(async move {
                let _ = t.set_setting("welcome_wizard_shown", "1").await;
            });
        }
        // Reuse the existing game-configure flow.
        self.handle_games_configured(configs, hidden_ids, sender);
        // Kick off the downloads scan that was deferred while waiting for the wizard.
        // External-file scan is deliberately omitted here: it must run *after* GameSelected
        // loads the game data and creates the vanilla snapshot, otherwise every game file
        // would be flagged as a new external change.
        sender.input(AppMsg::ScanDownloadsFolder);
    }

    pub(crate) fn confirm_remove_game(
        &mut self,
        game_id: String,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = gtk::AlertDialog::builder()
            .message("Stop managing this game?")
            .detail("Installed mods stay in the cache and can be kept or permanently deleted.")
            .buttons(["Cancel", "Remove only", "Remove & delete mods"])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let s = sender.input_sender().clone();
        dialog.choose(Some(root), None::<&gio::Cancellable>, move |r| match r {
            Ok(1) => {
                let _ = s.send(AppMsg::RemoveGameConfirmed {
                    game_id,
                    delete_mods: false,
                });
            }
            Ok(2) => {
                let _ = s.send(AppMsg::RemoveGameConfirmed {
                    game_id,
                    delete_mods: true,
                });
            }
            _ => {}
        });
    }

    pub(crate) fn handle_remove_game(
        &mut self,
        game_id: String,
        delete_mods: bool,
        sender: &ComponentSender<Self>,
    ) {
        let Some(idx) = self.games.iter().position(|g| g.id == game_id) else {
            return;
        };

        self.games.remove(idx);
        self.game_model.remove(idx as u32);

        if let Some(tracker) = self.tracker.clone() {
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
