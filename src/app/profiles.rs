use std::path::PathBuf;

use gtk::prelude::*;
use gtk::gio;
use relm4::prelude::*;

use crate::core::{game, save_manager, tool_launcher};
use crate::models::profile::SaveMode;
use crate::models::game::GameEngine;
use crate::ui::game_setup_dialog::{GameSetupDialog, GameSetupOutput};
use crate::ui::settings_dialog::{SettingsDialog, SettingsDialogOutput};
use crate::ui::tool_manager::{ToolManager, ToolManagerOutput};

use super::free_fns::load_game_data;
use super::messages::{AppCmdMsg, AppMsg};
use super::types::{InitData, LoadedData};
use super::App;

// ─── AppMsg handlers ─────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_profile_selected(
        &mut self,
        idx: u32,
        sender: &ComponentSender<Self>,
    ) {
        if self.updating_profiles {
            return;
        }
        let idx = idx as usize;
        if idx == self.active_profile_idx || idx >= self.profiles.len() {
            return;
        }

        let target_profile_id = self.profiles[idx].id.clone();
        let target_save_mode = self.profiles[idx].save_mode.clone();
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };

        let old_profile = self.profiles.get(self.active_profile_idx).cloned();
        let old_profile_id = old_profile.as_ref().map(|p| p.id.clone());
        let old_save_mode = old_profile.map(|p| p.save_mode).unwrap_or(SaveMode::Global);

        sender.oneshot_command(async move {
            let result = async {
                if let Some(old_id) = &old_profile_id {
                    tracker
                        .save_to_profile(old_id, &game.id)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                tracker
                    .switch_profile(&game.id, &target_profile_id)
                    .await
                    .map_err(|e| e.to_string())?;
                let save_sync = if game::has_save_management(&game) {
                    save_manager::swap_saves(
                        &game,
                        old_profile_id.as_deref(),
                        &old_save_mode,
                        &target_profile_id,
                        &target_save_mode,
                    )
                    .await
                    .map_err(|e| e.to_string())?
                } else {
                    None
                };
                let data = load_game_data(&tracker, &game, false).await?;
                Ok::<_, String>((data, save_sync))
            };
            AppCmdMsg::ProfileSwitched(result.await)
        });
    }

    pub(crate) fn handle_new_profile_clicked(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };

        let existing_names: std::collections::HashSet<&str> =
            self.profiles.iter().map(|p| p.name.as_str()).collect();
        let mut counter = self.profiles.len() + 1;
        let new_name = loop {
            let candidate = format!("Profile {counter}");
            if !existing_names.contains(candidate.as_str()) {
                break candidate;
            }
            counter += 1;
        };
        let active_profile_id = self
            .profiles
            .get(self.active_profile_idx)
            .map(|p| p.id.clone());

        sender.oneshot_command(async move {
            let result = async {
                if let Some(active_id) = &active_profile_id {
                    tracker
                        .save_to_profile(active_id, &game.id)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                let new_id = tracker
                    .create_clean_profile(&game.id, &new_name)
                    .await
                    .map_err(|e| e.to_string())?;
                tracker
                    .switch_profile(&game.id, &new_id)
                    .await
                    .map_err(|e| e.to_string())?;
                load_game_data(&tracker, &game, false).await
            };
            AppCmdMsg::ProfileCreated(result.await)
        });
    }

    pub(crate) fn handle_clone_profile_clicked(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        let Some(source_profile) = self.profiles.get(self.active_profile_idx).cloned() else {
            return;
        };

        let new_name = format!("{} (Copy)", source_profile.name);

        sender.oneshot_command(async move {
            let result = async {
                tracker
                    .save_to_profile(&source_profile.id, &game.id)
                    .await
                    .map_err(|e| e.to_string())?;
                let new_id = tracker
                    .clone_profile(&source_profile.id, &new_name, &game.id)
                    .await
                    .map_err(|e| e.to_string())?;
                tracker
                    .switch_profile(&game.id, &new_id)
                    .await
                    .map_err(|e| e.to_string())?;
                load_game_data(&tracker, &game, false).await
            };
            AppCmdMsg::ProfileCloned(result.await)
        });
    }

    pub(crate) fn handle_delete_profile_clicked(&mut self, sender: &ComponentSender<Self>) {
        if self.profiles.len() <= 1 {
            self.toaster.toast("Cannot delete the last profile");
            return;
        }

        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        let delete_id = self.profiles[self.active_profile_idx].id.clone();

        sender.oneshot_command(async move {
            let result = async {
                tracker
                    .delete_profile(&delete_id)
                    .await
                    .map_err(|e| e.to_string())?;
                tracker
                    .ensure_default_profile(&game.id)
                    .await
                    .map_err(|e| e.to_string())?;
                let profiles: Vec<crate::models::profile::Profile> = tracker
                    .list_profiles(&game.id)
                    .await
                    .map_err(|e| e.to_string())?;
                if let Some(active) = profiles.iter().find(|p| p.is_active) {
                    tracker
                        .switch_profile(&game.id, &active.id)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                load_game_data(&tracker, &game, false).await
            };
            AppCmdMsg::ProfileDeleted(result.await)
        });
    }

    pub(crate) fn handle_rename_profile(
        &mut self,
        new_name: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(profile) = self.profiles.get(self.active_profile_idx) else { return };
        let profile_id = profile.id.clone();

        sender.oneshot_command(async move {
            let result = tracker
                .rename_profile(&profile_id, &new_name)
                .await
                .map_err(|e| e.to_string());
            AppCmdMsg::ProfileRenamed(result)
        });
    }

    pub(crate) fn handle_export_profile_clicked(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(profile) = self.profiles.get(self.active_profile_idx).cloned() else { return };
        let initial_name =
            format!("{}.deployd-profile.json", profile.name.replace(' ', "_"));
        let dialog = gtk::FileDialog::builder()
            .title("Export Profile")
            .initial_name(&initial_name)
            .build();
        let input_sender = sender.input_sender().clone();
        dialog.save(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let tracker = tracker.clone();
                let profile_id = profile.id.clone();
                gtk::glib::spawn_future_local(async move {
                    let result = async {
                        let export = tracker
                            .export_profile(&profile_id)
                            .await
                            .map_err(|e| e.to_string())?;
                        let json = serde_json::to_string_pretty(&export)
                            .map_err(|e| e.to_string())?;
                        std::fs::write(&path, json).map_err(|e| e.to_string())?;
                        Ok(())
                    }
                    .await;
                    let _ = input_sender.send(AppMsg::ProfileExported(result));
                });
            }
        });
    }

    pub(crate) fn handle_profile_exported(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => self.toaster.toast("Profile exported"),
            Err(e) => self.toaster.toast(&format!("Export failed: {e}")),
        }
    }

    pub(crate) fn handle_import_profile_clicked(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Deployd Profile (*.json)"));
        filter.add_pattern("*.json");
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Import Profile")
            .filters(&filters)
            .build();
        let input_sender = sender.input_sender().clone();
        dialog.open(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let _ = input_sender.send(AppMsg::ImportProfileFileChosen(path));
            }
        });
    }

    pub(crate) fn handle_import_profile_file_chosen(
        &mut self,
        path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        sender.oneshot_command(async move {
            AppCmdMsg::ProfileImported(
                async {
                    let json = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                    let export: crate::models::profile_export::ProfileExport =
                        serde_json::from_str(&json).map_err(|e| e.to_string())?;
                    let new_profile_id = tracker
                        .import_profile(&game.id, &export)
                        .await
                        .map_err(|e| e.to_string())?;
                    tracker
                        .switch_profile(&game.id, &new_profile_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    load_game_data(&tracker, &game, false).await
                }
                .await,
            )
        });
    }

    pub(crate) fn handle_launch_tool(
        &mut self,
        tool_id: String,
        sender: &ComponentSender<Self>,
    ) {
        if self.needs_deploy {
            self.toaster
                .toast("Deploy your mods before launching tools");
            return;
        }

        let Some(tool) = self.tools.iter().find(|t| t.id == tool_id).cloned() else {
            self.toaster.toast("Tool not found");
            return;
        };
        let Some(game) = self.selected_game().cloned() else { return };

        let tool_name = tool.name.clone();
        let exit_sender = sender.input_sender().clone();
        let exit_tool_name = tool_name.clone();
        sender.oneshot_command(async move {
            let result: Result<String, String> = (|| {
                let wine_config = game::detect_wine_config(&game).ok_or_else(|| {
                    "Could not detect Wine configuration for this game".to_string()
                })?;
                tool_launcher::launch_tool(
                    &tool,
                    &game,
                    &wine_config,
                    Some(Box::new(move || {
                        let _ = exit_sender.send(AppMsg::ToolExited(exit_tool_name));
                    })),
                )
                .map_err(|e| e.to_string())?;
                Ok(tool_name)
            })();
            AppCmdMsg::ToolLaunched(result)
        });
    }

    pub(crate) fn handle_tool_exited(&mut self, tool_name: String, sender: &ComponentSender<Self>) {
        self.toaster
            .toast(&format!("{tool_name} closed — scanning for changes…"));
        sender.input(AppMsg::ScanExternalFiles);
        #[cfg(feature = "loot")]
        if self
            .selected_game()
            .map(|g| crate::core::loot_sort::game_has_loot_support(&g.id))
            .unwrap_or(false)
        {
            sender.input(AppMsg::SortWithLoot);
        }
    }

    pub(crate) fn handle_manage_tools_clicked(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let Some(game) = self.selected_game() else { return };
        let game_id = game.id.clone();
        let game_path = game.path.clone();
        let game_engine = game.engine.clone();
        let wine_prefix = game::detect_wine_config(game).map(|wc| wc.prefix);
        let tools = self.tools.clone();

        self.tool_manager_dialog = Some(
            ToolManager::builder()
                .transient_for(root)
                .launch((game_id, tools, game_path, wine_prefix, game_engine))
                .forward(sender.input_sender(), |output| match output {
                    ToolManagerOutput::ToolAdded(tool) => AppMsg::ToolAdded(tool),
                    ToolManagerOutput::ToolRemoved(id) => AppMsg::ToolRemoved(id),
                    ToolManagerOutput::ToolWorkingDirChanged(id, dir) => {
                        AppMsg::ToolWorkingDirChanged(id, dir)
                    }
                    ToolManagerOutput::Closed => AppMsg::ToolManagerClosed,
                }),
        );
    }

    pub(crate) fn handle_tool_added(
        &mut self,
        tool: crate::models::tool::Tool,
        sender: &ComponentSender<Self>,
    ) {
        let tool_clone = tool.clone();
        self.tools.push(tool);
        self.rebuild_tool_buttons(sender);

        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::ToolSaved(
                    tracker
                        .insert_tool(&tool_clone)
                        .await
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn handle_tool_removed(
        &mut self,
        tool_id: String,
        sender: &ComponentSender<Self>,
    ) {
        self.tools.retain(|t| t.id != tool_id);
        self.rebuild_tool_buttons(sender);

        if let Some(tracker) = self.tracker.clone() {
            let id = tool_id.clone();
            sender.oneshot_command(async move {
                AppCmdMsg::ToolDeleted(
                    tracker
                        .delete_tool(&id)
                        .await
                        .map(|_| id)
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn handle_tool_working_dir_changed(
        &mut self,
        tool_id: String,
        new_dir: String,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == tool_id) {
            tool.working_dir = new_dir.clone();
        }

        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::ToolWorkingDirSaved(
                    tracker
                        .update_tool_working_dir(&tool_id, &new_dir)
                        .await
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn handle_tool_manager_closed(&mut self) {
        if let Some(dialog) = self.tool_manager_dialog.take() {
            dialog.widget().destroy();
        }
    }

    pub(crate) fn handle_settings_clicked(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
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
                    SettingsDialogOutput::ApiKeyChanged(_) => AppMsg::NexusApiKeyUpdated,
                    SettingsDialogOutput::ManageGames => AppMsg::ManageGamesClicked,
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
        // Build the list of auto-detected games (exclude any already-custom entries).
        let detected: Vec<crate::models::game::Game> = self.games.clone();

        // Separate persisted custom games from the current game list for the dialog's init.
        // We pass an empty custom list here; existing custom entries are already in detected.
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

        // Rebuild self.games and the dropdown model from the confirmed configs.
        let n_existing = self.game_model.n_items();
        for _ in 0..n_existing {
            self.game_model.remove(0);
        }
        self.games.clear();

        for cfg in &configs {
            self.game_model.append(&cfg.game.title);
            self.games.push(cfg.game.clone());
        }

        // Persist enabled games and hide unchecked ones.
        if let Some(tracker) = self.tracker.clone() {
            let configs_for_db = configs.clone();
            let hidden_for_db = hidden_ids;
            sender.oneshot_command(async move {
                for cfg in &configs_for_db {
                    let engine_str = match cfg.game.engine {
                        GameEngine::REDEngine => "redengine",
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

        // Select the first game and load its data.
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

        // Persist: hide so it is not re-added on next rescan.
        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                let _ = tracker.hide_game(&game_id).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }

        if self.games.is_empty() {
            // No games left — nothing to select; the empty-state UI handles this.
            return;
        }

        let new_idx = idx.min(self.games.len() - 1);
        self.selected_game_idx = new_idx;
        self.game_dropdown.set_selected(new_idx as u32);
        sender.input(AppMsg::GameSelected(new_idx as u32));
    }

    pub(crate) fn handle_toggle_profile_save_mode(&mut self, sender: &ComponentSender<Self>) {
        let Some(profile) = self.profiles.get(self.active_profile_idx).cloned() else { return };
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        let new_mode = match profile.save_mode {
            SaveMode::Global => SaveMode::ProfileSpecific,
            SaveMode::ProfileSpecific => SaveMode::Global,
        };
        let profile_id = profile.id.clone();
        sender.oneshot_command(async move {
            let result = async {
                if new_mode == SaveMode::ProfileSpecific && game::has_save_management(&game) {
                    save_manager::initialize_profile_saves(&game, &profile_id)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                tracker
                    .set_profile_save_mode(&profile_id, new_mode)
                    .await
                    .map_err(|e| e.to_string())
            };
            AppCmdMsg::SaveModeToggled(result.await)
        });
    }

    pub(crate) fn handle_sync_saves(&mut self, sender: &ComponentSender<Self>) {
        let Some(profile) = self.profiles.get(self.active_profile_idx).cloned() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        let profile_id = profile.id.clone();
        sender.oneshot_command(async move {
            let result = save_manager::sync_profile_saves(&game, &profile_id)
                .await
                .map_err(|e| e.to_string());
            AppCmdMsg::SavesSynced(result)
        });
    }

    pub(crate) fn handle_rate_limit_updated(
        &mut self,
        info: crate::core::nexus_api::RateLimitInfo,
    ) {
        self.rate_limit_info = Some(info.clone());
        if let Some(tracker) = self.tracker.clone() {
            tokio::spawn(async move {
                let _ = tracker.save_rate_limits(&info).await;
            });
        }
    }

    pub(crate) fn handle_close_requested(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        if self.global_active_downloads > 0 {
            let dialog = gtk::AlertDialog::builder()
                .message("Downloads in Progress")
                .detail(format!(
                    "{} download(s) are still in progress. Close anyway?",
                    self.global_active_downloads
                ))
                .buttons(["Cancel", "Close"])
                .cancel_button(0)
                .default_button(1)
                .modal(true)
                .build();

            let sender = sender.input_sender().clone();
            dialog.choose(Some(root), None::<&gio::Cancellable>, move |result| {
                if result == Ok(1) {
                    sender.send(AppMsg::ConfirmClose).unwrap();
                }
            });
        } else {
            root.destroy();
        }
    }

    pub(crate) fn handle_confirm_close(&mut self, root: &adw::Window) {
        root.destroy();
    }

    pub(crate) fn handle_search_toggled(&mut self, active: bool) {
        self.search_active = active;
        if !active {
            self.search_text.clear();
            self.apply_search_filter();
        }
    }

    pub(crate) fn handle_search_changed(&mut self, text: String) {
        self.search_text = text;
        self.apply_search_filter();
    }

    pub(crate) fn handle_search_scope_changed(&mut self, idx: u32) {
        self.search_scope = match idx {
            1 => super::types::SearchScope::ModOrder,
            2 => super::types::SearchScope::PluginOrder,
            3 => super::types::SearchScope::Downloads,
            _ => super::types::SearchScope::All,
        };
        self.apply_search_filter();
    }

    pub(crate) fn handle_show_toast(&mut self, msg: String) {
        self.toaster.toast(&msg);
    }

    /// Persist the current active profile ID as `last_profile_{game_id}` in DB settings
    /// so that `ensure_default_profile` can restore it on next startup.
    fn save_last_profile(&self, sender: &ComponentSender<Self>) {
        if let (Some(tracker), Some(game), Some(profile)) = (
            self.tracker.clone(),
            self.selected_game().cloned(),
            self.profiles.get(self.active_profile_idx),
        ) {
            let key = format!("last_profile_{}", game.id);
            let id = profile.id.clone();
            sender.oneshot_command(async move {
                let _ = tracker.set_setting(&key, &id).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
    }
}

// ─── AppCmdMsg handlers ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_cmd_initialized(
        &mut self,
        result: Result<InitData, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                self.tracker = Some(data.tracker);
                self.selected_game_idx = data.selected_game_idx;
                self.game_dropdown
                    .set_selected(data.selected_game_idx as u32);

                // Apply persisted path and wine_prefix overrides to auto-detected games.
                for game in &mut self.games {
                    if let Some(persisted) = data.persisted_games.iter().find(|p| p.id == game.id)
                    {
                        game.path = persisted.path.clone();
                        if persisted.wine_prefix.is_some() {
                            game.wine_prefix = persisted.wine_prefix.clone();
                        }
                    }
                }

                // Append custom games (manually added) that are not in the auto-detected list.
                let known_ids: std::collections::HashSet<String> =
                    self.games.iter().map(|g| g.id.clone()).collect();
                for persisted in data.persisted_games.iter().filter(|p| p.custom) {
                    if !known_ids.contains(&persisted.id) {
                        let engine = if persisted.engine == "redengine" {
                            crate::models::game::GameEngine::REDEngine
                        } else {
                            crate::models::game::GameEngine::Bethesda
                        };
                        self.game_model.append(&persisted.title);
                        self.games.push(crate::models::game::Game {
                            id: persisted.id.clone(),
                            title: persisted.title.clone(),
                            path: persisted.path.clone(),
                            data_subdir: persisted.data_subdir.clone(),
                            engine,
                            wine_prefix: persisted.wine_prefix.clone(),
                        });
                    }
                }

                // Remove any games the user has explicitly hidden.
                if !data.hidden_game_ids.is_empty() {
                    let hidden: std::collections::HashSet<&str> =
                        data.hidden_game_ids.iter().map(String::as_str).collect();
                    // Rebuild game_model from scratch after filtering.
                    let n = self.game_model.n_items();
                    for _ in 0..n {
                        self.game_model.remove(0);
                    }
                    self.games.retain(|g| !hidden.contains(g.id.as_str()));
                    for g in &self.games {
                        self.game_model.append(&g.title);
                    }
                }

                // Show the game setup dialog on first launch (no persisted config yet).
                if data.persisted_games.is_empty() && data.hidden_game_ids.is_empty() {
                    sender.input(AppMsg::ManageGamesClicked);
                }
                self.collapsed_groups = data
                    .groups
                    .iter()
                    .filter(|g| g.collapsed)
                    .map(|g| g.id.clone())
                    .collect();
                let loaded = LoadedData {
                    mods: data.mods,
                    plugins: data.plugins,
                    plugin_masters: data.plugin_masters,
                    overrides: data.overrides,
                    profiles: data.profiles,
                    active_profile_idx: data.active_profile_idx,
                    tools: data.tools,
                    vanilla_plugins: data.vanilla_plugins,
                    groups: data.groups,
                };
                self.apply_loaded_data(loaded, sender);
                self.last_deployed_profile_id = data.last_deployed_profile_id;

                if let Some(dir) = data.downloads_dir {
                    self.downloads_dir = dir;
                }

                self.all_downloads = data.download_entries;
                self.rebuild_downloads_view();

                self.rate_limit_info = data.rate_limit_info;

                if let Some(nxm) = self.pending_nxm.take() {
                    sender.input(AppMsg::NxmLinkReceived(nxm));
                }

                sender.input(AppMsg::ScanDownloadsFolder);
                sender.input(AppMsg::ScanExternalFiles);

                let input = sender.input_sender().clone();
                let tracker_for_update = self.tracker.clone();
                relm4::spawn(async move {
                    let api_key = if let Some(ref t) = tracker_for_update {
                        t.get_setting("nexus_api_key").await.ok().flatten()
                    } else {
                        None
                    };
                    if let Some(info) =
                        crate::core::update_check::check_for_app_update(api_key).await
                    {
                        let _ = input.send(AppMsg::AppUpdateAvailable(info.version, info.url));
                    }
                });
            }
            Err(e) => {
                self.toaster.toast(&format!("Init failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_profile_switched(
        &mut self,
        result: Result<(LoadedData, Option<crate::core::save_manager::SaveSyncResult>), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((data, save_sync)) => {
                self.needs_deploy = true;
                self.apply_loaded_data(data, sender);
                self.save_last_profile(sender);
                if let Some(sync) = save_sync {
                    self.toaster
                        .toast(&format!("Profile switched — {}", sync.to_toast()));
                } else {
                    self.toaster.toast("Profile switched");
                }
            }
            Err(e) => {
                self.toaster.toast(&format!("Profile switch failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_profile_created(
        &mut self,
        result: Result<LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                self.apply_loaded_data(data, sender);
                self.needs_deploy = true;
                self.save_last_profile(sender);
                self.toaster
                    .toast("Empty profile created — Deploy to purge game folder");
            }
            Err(e) => {
                self.toaster.toast(&format!("Profile creation failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_profile_cloned(
        &mut self,
        result: Result<LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                self.apply_loaded_data(data, sender);
                self.save_last_profile(sender);
                let name = self
                    .profiles
                    .get(self.active_profile_idx)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                self.toaster.toast(&format!("Cloned as '{name}'"));
            }
            Err(e) => {
                self.toaster.toast(&format!("Profile clone failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_profile_deleted(
        &mut self,
        result: Result<LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                self.needs_deploy = true;
                self.apply_loaded_data(data, sender);
                self.toaster.toast("Profile deleted");
            }
            Err(e) => {
                self.toaster.toast(&format!("Profile delete failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_profile_renamed(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                let new_name = self.profile_rename_entry.text().to_string();
                if let Some(p) = self.profiles.get_mut(self.active_profile_idx) {
                    p.name = new_name;
                }
                let names: Vec<&str> = self.profiles.iter().map(|p| p.name.as_str()).collect();
                self.updating_profiles = true;
                self.profile_model
                    .splice(0, self.profile_model.n_items(), &names);
                self.profile_dropdown
                    .set_selected(self.active_profile_idx as u32);
                self.updating_profiles = false;
                self.toaster.toast("Profile renamed");
            }
            Err(e) => {
                self.toaster.toast(&format!("Rename failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_profile_imported(
        &mut self,
        result: Result<LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                self.apply_loaded_data(data, sender);
                self.needs_deploy = true;
                self.save_last_profile(sender);
                let name = self
                    .profiles
                    .get(self.active_profile_idx)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                self.toaster
                    .toast(&format!("Imported as '{name}' — Deploy to apply"));
            }
            Err(e) => {
                self.toaster.toast(&format!("Import failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_tool_saved(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.toaster.toast(&format!("Failed to save tool: {e}"));
        }
    }

    pub(crate) fn handle_cmd_tool_deleted(&mut self, result: Result<String, String>) {
        if let Err(e) = result {
            self.toaster.toast(&format!("Failed to delete tool: {e}"));
        }
    }

    pub(crate) fn handle_cmd_tool_working_dir_saved(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.toaster
                .toast(&format!("Failed to save working directory: {e}"));
        }
    }

    pub(crate) fn handle_cmd_tool_launched(&mut self, result: Result<String, String>) {
        match result {
            Ok(name) => {
                self.toaster.toast(&format!("Launched {name}"));
            }
            Err(e) => {
                eprintln!("deployd: tool launch error: {e}");
                self.toaster.toast(&format!("Launch failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_save_mode_toggled(
        &mut self,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(()) => {
                if let Some(tracker) = self.tracker.clone()
                    && let Some(game) = self.selected_game().cloned()
                {
                    sender.oneshot_command(async move {
                        AppCmdMsg::ModsLoaded(load_game_data(&tracker, &game, false).await)
                    });
                }
            }
            Err(e) => {
                self.toaster
                    .toast(&format!("Failed to change save mode: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_saves_synced(
        &mut self,
        result: Result<crate::core::save_manager::SaveSyncResult, String>,
    ) {
        match result {
            Ok(sync) => {
                if let Some(p) = self.profiles.get_mut(self.active_profile_idx) {
                    p.save_synced_at = Some(std::time::SystemTime::now());
                }
                self.toaster.toast(&sync.to_toast());
            }
            Err(e) => {
                self.toaster.toast(&format!("Save sync failed: {e}"));
            }
        }
    }
}
