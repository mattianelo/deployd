use gtk::prelude::*;
use relm4::prelude::*;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::session::{GameLoadMode, fetch_avatar_bytes, load_game_data};
use super::super::types::{InitData, LoadedData, WorkKind};

impl App {
    pub(crate) fn handle_cmd_last_deployed_profile_loaded(
        &mut self,
        result: Result<Option<String>, String>,
    ) {
        match result {
            Ok(id) => self.session.last_deployed_profile_id = id,
            Err(error) => self.push_notification(&format!(
                "Failed to load the last deployed profile: {error}"
            )),
        }
    }

    pub(crate) fn handle_cmd_nexus_avatar_loaded(&mut self, bytes: Option<Vec<u8>>) {
        crate::dlog!(
            "[avatar] NexusAvatarLoaded: {:?}",
            bytes.as_ref().map(Vec::len)
        );
        if let Some(bytes) = bytes {
            match gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(bytes)) {
                Ok(texture) => {
                    crate::dlog!("[avatar] texture created, setting custom image");
                    self.ui.nexus_avatar_widget.set_custom_image(Some(&texture));
                }
                Err(error) => {
                    crate::dlog!("[avatar] Texture::from_bytes failed: {error}");
                }
            }
        }
    }

    pub(crate) fn handle_cmd_nexus_user_refreshed(
        &mut self,
        username: Option<String>,
        avatar_url: Option<String>,
        is_premium: bool,
        sender: &ComponentSender<Self>,
    ) {
        crate::dlog!(
            "[avatar] NexusUserRefreshed: username={:?} avatar_url={:?}",
            username,
            avatar_url,
        );
        self.shell.nexus_username = username.clone();
        self.shell.nexus_avatar_url = avatar_url.clone();
        self.shell.nexus_is_premium = is_premium;
        self.ui.nexus_avatar_widget.set_text(username.as_deref());
        self.ui
            .nexus_avatar_widget
            .set_custom_image(None::<&gtk::gdk::Texture>);
        if let Some(url) = avatar_url {
            sender.oneshot_command(async move {
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::NexusAvatarLoaded(
                    fetch_avatar_bytes(&url).await,
                ))
            });
        } else {
            crate::dlog!("[avatar] NexusUserRefreshed: no avatar URL, showing initials");
        }
    }

    pub(crate) fn handle_cmd_initialized(
        &mut self,
        result: Result<InitData, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                self.session.tracker = Some(data.tracker);

                for game in &mut self.session.games {
                    if let Some(persisted) = data.persisted_games.iter().find(|p| p.id == game.id) {
                        game.path = persisted.path.clone();
                        if persisted.wine_prefix.is_some() {
                            game.wine_prefix = persisted.wine_prefix.clone();
                        }
                    }
                }

                let known_ids: std::collections::HashSet<String> =
                    self.session.games.iter().map(|g| g.id.clone()).collect();
                for persisted in data.persisted_games.iter().filter(|p| p.custom) {
                    if !known_ids.contains(&persisted.id) {
                        let engine = match persisted.engine.as_str() {
                            "redengine" => crate::models::game::GameEngine::REDEngine,
                            "eclipse" => crate::models::game::GameEngine::Eclipse,
                            "aurora" => crate::models::game::GameEngine::Aurora,
                            _ => crate::models::game::GameEngine::Bethesda,
                        };
                        self.ui.game_model.append(&persisted.title);
                        self.session.games.push(crate::models::game::Game {
                            id: persisted.id.clone(),
                            title: persisted.title.clone(),
                            path: persisted.path.clone(),
                            data_subdir: crate::core::game::known_data_subdir(&persisted.id)
                                .map(|s| s.to_string())
                                .unwrap_or_else(|| persisted.data_subdir.clone()),
                            engine,
                            wine_prefix: persisted.wine_prefix.clone(),
                        });
                    }
                }

                // One-time migration: persist any corrected data_subdir values to DB
                // so subsequent loads get the right value directly.
                if let Some(tracker) = self.session.tracker.clone() {
                    let migrations: Vec<_> = self
                        .session
                        .games
                        .iter()
                        .filter_map(|g| {
                            let canonical = crate::core::game::known_data_subdir(&g.id)?;
                            let persisted_val = data
                                .persisted_games
                                .iter()
                                .find(|p| p.id == g.id)
                                .map(|p| p.data_subdir.as_str())
                                .unwrap_or("");
                            if persisted_val != canonical {
                                Some((
                                    g.id.clone(),
                                    g.title.clone(),
                                    g.path.clone(),
                                    canonical.to_string(),
                                    g.engine.clone(),
                                    g.wine_prefix.clone(),
                                ))
                            } else {
                                None
                            }
                        })
                        .collect();
                    if !migrations.is_empty() {
                        sender.oneshot_command(async move {
                            let result = async {
                                for (id, title, path, data_subdir, engine, wine_prefix) in
                                    migrations
                                {
                                    let engine_str = match engine {
                                        crate::models::game::GameEngine::REDEngine => "redengine",
                                        crate::models::game::GameEngine::Eclipse => "eclipse",
                                        crate::models::game::GameEngine::Aurora => "aurora",
                                        _ => "bethesda",
                                    };
                                    tracker
                                        .upsert_game(
                                            &id,
                                            &title,
                                            &path,
                                            &data_subdir,
                                            engine_str,
                                            wine_prefix.as_deref(),
                                            true,
                                        )
                                        .await?;
                                }
                                anyhow::Ok(())
                            }
                            .await
                            .map_err(|error| error.to_string());
                            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(
                                result,
                            ))
                        });
                    }
                }

                if !data.hidden_game_ids.is_empty() {
                    let hidden: std::collections::HashSet<&str> =
                        data.hidden_game_ids.iter().map(String::as_str).collect();
                    let n = self.ui.game_model.n_items();
                    for _ in 0..n {
                        self.ui.game_model.remove(0);
                    }
                    self.session
                        .games
                        .retain(|g| !hidden.contains(g.id.as_str()));
                    for g in &self.session.games {
                        self.ui.game_model.append(&g.title);
                    }
                }

                // Resolve the correct game index now that self.session.games is fully merged
                // and pruned. Computing this from persisted_games order (as init.rs did)
                // was wrong because self.session.games uses the detected-games order.
                let target_idx = data
                    .init_game_id
                    .as_deref()
                    .and_then(|id| self.session.games.iter().position(|g| g.id == id))
                    .unwrap_or(0);
                self.session.selected_game_idx = target_idx;
                self.ui.game_dropdown.set_selected(target_idx as u32);

                self.shell.nexus_username = data.nexus_username.clone();
                self.shell.nexus_avatar_url = data.nexus_avatar_url.clone();
                self.shell.nexus_is_premium = data.nexus_is_premium;
                crate::dlog!(
                    "[avatar] init: username={:?} avatar_url={:?}",
                    self.shell.nexus_username,
                    self.shell.nexus_avatar_url,
                );
                if let Some(username) = data.nexus_username.as_deref() {
                    self.ui.nexus_avatar_widget.set_text(Some(username));
                }
                if let Some(url) = data.nexus_avatar_url.clone() {
                    sender.oneshot_command(async move {
                        AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::NexusAvatarLoaded(
                            fetch_avatar_bytes(&url).await,
                        ))
                    });
                } else {
                    crate::dlog!("[avatar] init: no avatar URL, showing initials");
                }

                if data.first_launch {
                    self.session.initializing = false;
                    sender.input(AppMsg::Games(
                        crate::app::messages::GamesMsg::ShowWelcomeWizard,
                    ));
                    return;
                }

                self.mods.collapsed_groups = data
                    .groups
                    .iter()
                    .filter(|g| g.collapsed)
                    .map(|g| g.id.clone())
                    .collect();
                let loaded = LoadedData {
                    game_id: data.init_game_id.clone().unwrap_or_default(),
                    mods: data.mods,
                    plugins: data.plugins,
                    plugin_masters: data.plugin_masters,
                    overrides: data.overrides,
                    profiles: data.profiles,
                    active_profile_idx: data.active_profile_idx,
                    tools: data.tools,
                    vanilla_plugins: data.vanilla_plugins,
                    groups: data.groups,
                    vanilla_plugin_master_counts: data.vanilla_plugin_master_counts,
                    vanilla_derived_plugins: data.vanilla_derived_plugins,
                    access_warnings: data.access_warnings,
                    plugin_scan_complete: data.plugin_scan_complete,
                };
                self.apply_loaded_data(loaded, sender);
                self.session.last_deployed_profile_id = data.last_deployed_profile_id;

                if let Some(dir) = data.downloads_dir {
                    self.download.directory = dir;
                }

                self.session.game_cache_dirs = data.game_cache_dirs;
                self.download.all = data.download_entries;
                self.rebuild_downloads_view();

                self.download.rate_limit = data.rate_limit_info;

                self.apply_color_scheme(data.color_scheme_idx);

                if let Some(nxm) = self.download.pending_nxm.take() {
                    sender.input(AppMsg::Downloads(
                        crate::app::messages::DownloadsMsg::NxmLinkReceived(nxm),
                    ));
                }

                self.session.initializing = false;

                if data.restored_from_backup {
                    self.push_notification(
                        "Database restored from backup. \
                         Re-install your mods from archives to restore a deployable state.",
                    );
                }

                sender.input(AppMsg::Downloads(
                    crate::app::messages::DownloadsMsg::ScanDownloadsFolder,
                ));
                sender.input(AppMsg::Mods(
                    crate::app::messages::ModsMsg::ScanExternalFiles,
                ));

                let input = sender.input_sender().clone();
                let tracker_for_update = self.session.tracker.clone();
                relm4::spawn(async move {
                    let api_key = if let Some(ref t) = tracker_for_update {
                        match t.get_setting("nexus_api_key").await {
                            Ok(key) => key,
                            Err(error) => {
                                let _ = input.send(AppMsg::Shell(
                                    crate::app::messages::ShellMsg::ShowToast(format!(
                                        "Could not read update settings: {error}"
                                    )),
                                ));
                                None
                            }
                        }
                    } else {
                        None
                    };
                    if let Some(info) =
                        crate::core::update_check::check_for_app_update(api_key).await
                    {
                        let _ = input.send(AppMsg::Shell(
                            crate::app::messages::ShellMsg::AppUpdateAvailable(
                                info.version,
                                info.url,
                            ),
                        ));
                    }
                });
            }
            Err(e) => {
                self.session.initializing = false;
                self.push_notification(&format!("Init failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_profile_switched(
        &mut self,
        result: Result<
            (
                LoadedData,
                Option<crate::core::save_manager::SaveSyncResult>,
            ),
            String,
        >,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((data, save_sync)) => {
                self.shell.needs_deploy = true;
                self.apply_loaded_data(data, sender);
                self.save_last_profile(sender);
                if let Some(sync) = save_sync {
                    self.show_toast(&format!("Profile switched — {}", sync.to_toast()));
                } else {
                    self.show_toast("Profile switched");
                }
            }
            Err(e) => {
                self.push_notification(&format!("Profile switch failed: {e}"));
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
                self.shell.needs_deploy = true;
                self.save_last_profile(sender);
                self.show_toast("Empty profile created — Deploy to purge game folder");
            }
            Err(e) => {
                self.push_notification(&format!("Profile creation failed: {e}"));
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
                    .session
                    .profiles
                    .get(self.session.active_profile_idx)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                self.show_toast(&format!("Cloned as '{name}'"));
            }
            Err(e) => {
                self.push_notification(&format!("Profile clone failed: {e}"));
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
                self.shell.needs_deploy = true;
                self.apply_loaded_data(data, sender);
                self.show_toast("Profile deleted");
            }
            Err(e) => {
                self.push_notification(&format!("Profile delete failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_profile_renamed(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                let new_name = self.ui.profile_rename_entry.text().to_string();
                if let Some(p) = self
                    .session
                    .profiles
                    .get_mut(self.session.active_profile_idx)
                {
                    p.name = new_name;
                }
                let names: Vec<&str> = self
                    .session
                    .profiles
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect();
                self.session.updating_profiles = true;
                self.ui
                    .profile_model
                    .splice(0, self.ui.profile_model.n_items(), &names);
                self.ui
                    .profile_dropdown
                    .set_selected(self.session.active_profile_idx as u32);
                self.session.updating_profiles = false;
                self.show_toast("Profile renamed");
            }
            Err(e) => {
                self.push_notification(&format!("Rename failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_tool_saved(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.push_notification(&format!("Failed to save tool: {e}"));
        }
    }

    pub(crate) fn handle_cmd_tool_deleted(&mut self, result: Result<String, String>) {
        if let Err(e) = result {
            self.push_notification(&format!("Failed to delete tool: {e}"));
        }
    }

    pub(crate) fn handle_cmd_tool_working_dir_saved(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.push_notification(&format!("Failed to save working directory: {e}"));
        }
    }

    pub(crate) fn handle_cmd_tool_launched(&mut self, result: Result<String, String>) {
        match result {
            Ok(name) => {
                self.show_toast(&format!("Launched {name}"));
                if let Some(session) = self.tools.launch_session.as_ref() {
                    self.update_work(
                        WorkKind::LaunchingTool,
                        format!("{} is running", session.tool_name),
                        None,
                    );
                } else {
                    self.close_tool_launch_dialog();
                    self.finish_work(WorkKind::LaunchingTool);
                }
                if self.tools.proton_setup {
                    self.begin_work(WorkKind::SettingUpRuntime, "Finishing Proton GE setup...");
                }
            }
            Err(e) => {
                self.close_tool_launch_dialog();
                self.tools.launch_session = None;
                self.finish_work(WorkKind::LaunchingTool);
                self.tools.proton_setup = false;
                self.finish_work(WorkKind::SettingUpRuntime);
                crate::dlog!("deployd: tool launch error: {e}");
                self.push_notification(&format!("Launch failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_tool_launch_cancelled(&mut self, _name: String) {
        self.close_tool_launch_dialog();
        self.tools.launch_session = None;
        self.tools.proton_setup = false;
        self.finish_work(WorkKind::LaunchingTool);
        self.finish_work(WorkKind::SettingUpRuntime);
    }

    pub(crate) fn handle_cmd_save_mode_toggled(
        &mut self,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(()) => {
                if let Some(tracker) = self.session.tracker.clone()
                    && let Some(game) = self.selected_game().cloned()
                {
                    sender.oneshot_command(async move {
                        AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::ModsLoaded(
                            load_game_data(&tracker, &game, GameLoadMode::Refresh).await,
                            true,
                        ))
                    });
                }
            }
            Err(e) => {
                self.push_notification(&format!("Failed to change save mode: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_saves_synced(
        &mut self,
        result: Result<crate::core::save_manager::SaveSyncResult, String>,
    ) {
        match result {
            Ok(sync) => {
                if let Some(p) = self
                    .session
                    .profiles
                    .get_mut(self.session.active_profile_idx)
                {
                    p.save_synced_at = Some(std::time::SystemTime::now());
                }
                self.show_toast(&sync.to_toast());
            }
            Err(e) => {
                self.push_notification(&format!("Save sync failed: {e}"));
            }
        }
    }
}
