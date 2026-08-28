use gtk::prelude::*;
use relm4::prelude::*;

use super::super::App;
use super::super::free_fns::{GameLoadMode, fetch_avatar_bytes, load_game_data};
use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::types::{InitData, LoadedData, WorkKind};

impl App {
    pub(crate) fn handle_cmd_last_deployed_profile_loaded(&mut self, id: Option<String>) {
        self.last_deployed_profile_id = id;
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
                    self.nexus_avatar_widget.set_custom_image(Some(&texture));
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
        self.nexus_username = username.clone();
        self.nexus_avatar_url = avatar_url.clone();
        self.nexus_is_premium = is_premium;
        self.nexus_avatar_widget.set_text(username.as_deref());
        self.nexus_avatar_widget
            .set_custom_image(None::<&gtk::gdk::Texture>);
        if let Some(url) = avatar_url {
            sender.oneshot_command(async move {
                AppCmdMsg::NexusAvatarLoaded(fetch_avatar_bytes(&url).await)
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
                self.tracker = Some(data.tracker);

                for game in &mut self.games {
                    if let Some(persisted) = data.persisted_games.iter().find(|p| p.id == game.id) {
                        game.path = persisted.path.clone();
                        if persisted.wine_prefix.is_some() {
                            game.wine_prefix = persisted.wine_prefix.clone();
                        }
                    }
                }

                let known_ids: std::collections::HashSet<String> =
                    self.games.iter().map(|g| g.id.clone()).collect();
                for persisted in data.persisted_games.iter().filter(|p| p.custom) {
                    if !known_ids.contains(&persisted.id) {
                        let engine = match persisted.engine.as_str() {
                            "redengine" => crate::models::game::GameEngine::REDEngine,
                            "eclipse" => crate::models::game::GameEngine::Eclipse,
                            "aurora" => crate::models::game::GameEngine::Aurora,
                            _ => crate::models::game::GameEngine::Bethesda,
                        };
                        self.game_model.append(&persisted.title);
                        self.games.push(crate::models::game::Game {
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
                if let Some(tracker) = self.tracker.clone() {
                    let migrations: Vec<_> = self
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
                        relm4::spawn(async move {
                            for (id, title, path, data_subdir, engine, wine_prefix) in migrations {
                                let engine_str = match engine {
                                    crate::models::game::GameEngine::REDEngine => "redengine",
                                    crate::models::game::GameEngine::Eclipse => "eclipse",
                                    crate::models::game::GameEngine::Aurora => "aurora",
                                    _ => "bethesda",
                                };
                                let _ = tracker
                                    .upsert_game(
                                        &id,
                                        &title,
                                        &path,
                                        &data_subdir,
                                        engine_str,
                                        wine_prefix.as_deref(),
                                        true,
                                    )
                                    .await;
                            }
                        });
                    }
                }

                if !data.hidden_game_ids.is_empty() {
                    let hidden: std::collections::HashSet<&str> =
                        data.hidden_game_ids.iter().map(String::as_str).collect();
                    let n = self.game_model.n_items();
                    for _ in 0..n {
                        self.game_model.remove(0);
                    }
                    self.games.retain(|g| !hidden.contains(g.id.as_str()));
                    for g in &self.games {
                        self.game_model.append(&g.title);
                    }
                }

                // Resolve the correct game index now that self.games is fully merged
                // and pruned. Computing this from persisted_games order (as init.rs did)
                // was wrong because self.games uses the detected-games order.
                let target_idx = data
                    .init_game_id
                    .as_deref()
                    .and_then(|id| self.games.iter().position(|g| g.id == id))
                    .unwrap_or(0);
                self.selected_game_idx = target_idx;
                self.game_dropdown.set_selected(target_idx as u32);

                self.nexus_username = data.nexus_username.clone();
                self.nexus_avatar_url = data.nexus_avatar_url.clone();
                self.nexus_is_premium = data.nexus_is_premium;
                crate::dlog!(
                    "[avatar] init: username={:?} avatar_url={:?}",
                    self.nexus_username,
                    self.nexus_avatar_url,
                );
                if let Some(username) = data.nexus_username.as_deref() {
                    self.nexus_avatar_widget.set_text(Some(username));
                }
                if let Some(url) = data.nexus_avatar_url.clone() {
                    sender.oneshot_command(async move {
                        AppCmdMsg::NexusAvatarLoaded(fetch_avatar_bytes(&url).await)
                    });
                } else {
                    crate::dlog!("[avatar] init: no avatar URL, showing initials");
                }

                if data.first_launch {
                    self.initializing = false;
                    sender.input(AppMsg::ShowWelcomeWizard);
                    return;
                }

                self.collapsed_groups = data
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
                self.last_deployed_profile_id = data.last_deployed_profile_id;

                if let Some(dir) = data.downloads_dir {
                    self.downloads_dir = dir;
                }

                self.game_cache_dirs = data.game_cache_dirs;
                self.all_downloads = data.download_entries;
                self.rebuild_downloads_view();

                self.rate_limit_info = data.rate_limit_info;

                self.handle_set_color_scheme(data.color_scheme_idx);

                if let Some(nxm) = self.pending_nxm.take() {
                    sender.input(AppMsg::NxmLinkReceived(nxm));
                }

                self.initializing = false;

                if data.restored_from_backup {
                    self.push_notification(
                        "Database restored from backup. \
                         Re-install your mods from archives to restore a deployable state.",
                    );
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
                self.initializing = false;
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
                self.needs_deploy = true;
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
                self.needs_deploy = true;
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
                    .profiles
                    .get(self.active_profile_idx)
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
                self.needs_deploy = true;
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
                if let Some(session) = self.tool_launch_session.as_ref() {
                    self.update_work(
                        WorkKind::LaunchingTool,
                        format!("{} is running", session.tool_name),
                        None,
                    );
                } else {
                    self.close_tool_launch_dialog();
                    self.finish_work(WorkKind::LaunchingTool);
                }
                if self.proton_setup {
                    self.begin_work(WorkKind::SettingUpRuntime, "Finishing Proton GE setup...");
                }
            }
            Err(e) => {
                self.close_tool_launch_dialog();
                self.tool_launch_session = None;
                self.finish_work(WorkKind::LaunchingTool);
                self.proton_setup = false;
                self.finish_work(WorkKind::SettingUpRuntime);
                crate::dlog!("deployd: tool launch error: {e}");
                self.push_notification(&format!("Launch failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_tool_launch_cancelled(&mut self, _name: String) {
        self.close_tool_launch_dialog();
        self.tool_launch_session = None;
        self.proton_setup = false;
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
                if let Some(tracker) = self.tracker.clone()
                    && let Some(game) = self.selected_game().cloned()
                {
                    sender.oneshot_command(async move {
                        AppCmdMsg::ModsLoaded(
                            load_game_data(&tracker, &game, GameLoadMode::Refresh).await,
                            true,
                        )
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
                if let Some(p) = self.profiles.get_mut(self.active_profile_idx) {
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
