use std::collections::HashMap;

use relm4::prelude::*;

use crate::core::game;
use crate::core::tracker::OverrideInfo;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::types::LoadedData;

impl App {
    pub(crate) fn handle_cmd_mods_loaded(
        &mut self,
        result: Result<LoadedData, String>,
        preserve_collapsed: bool,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                if !preserve_collapsed {
                    self.collapsed_groups = data
                        .groups
                        .iter()
                        .filter(|g| g.collapsed)
                        .map(|g| g.id.clone())
                        .collect();
                }
                self.apply_loaded_data(data, sender);
                sender.input(AppMsg::ScanExternalFiles);
                // Reload last-deployed profile for the newly selected game.
                if let (Some(tracker), Some(game)) =
                    (self.tracker.clone(), self.selected_game().cloned())
                {
                    let key = format!("last_deployed_profile_{}", game.id);
                    sender.oneshot_command(async move {
                        AppCmdMsg::LastDeployedProfileLoaded(
                            tracker.get_setting(&key).await.ok().flatten(),
                        )
                    });
                }
            }
            Err(e) => {
                self.push_notification(&format!("Load failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_mod_removed(
        &mut self,
        result: Result<String, String>,
        nexus_ids: Option<(i64, i64)>,
        mod_name: String,
        archive_hash: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(_) => {
                self.push_notification("Mod removed. Deploy to update game files");
                let changed = self.reset_installed_download_for_mod(
                    nexus_ids,
                    &mod_name,
                    archive_hash.as_deref(),
                );
                if !changed.is_empty()
                    && let Some(tracker) = self.tracker.clone()
                {
                    tokio::spawn(async move {
                        for entry in &changed {
                            let _ = tracker.save_download_entry(entry).await;
                        }
                    });
                }
                self.auto_save_profile(sender);
                if let (Some(tracker), Some(game)) =
                    (self.tracker.clone(), self.selected_game().cloned())
                {
                    let game_id = game.id.clone();
                    let engine = game.engine.clone();
                    let mod_names: HashMap<String, String> = {
                        let guard = self.mods.guard();
                        (0..guard.len())
                            .filter_map(|i| {
                                guard.get(i).and_then(|r| r.mod_row()).map(|r| {
                                    (r.mod_entry.id.clone(), r.mod_entry.name.clone())
                                })
                            })
                            .collect()
                    };
                    sender.oneshot_command(async move {
                        AppCmdMsg::OverridesRefreshed(
                            tracker
                                .compute_overrides(
                                    &game_id,
                                    game::handler_for(&engine),
                                    &mod_names,
                                )
                                .await
                                .map_err(|e| e.to_string()),
                        )
                    });
                }
            }
            Err(e) => {
                self.push_notification(&format!("Remove failed: {e}"));
                self.reload_mods(sender);
            }
        }
    }

    pub(crate) fn handle_cmd_priority_saved(
        &mut self,
        result: Result<(), String>,
        _sender: &ComponentSender<Self>,
    ) {
        if let Err(e) = result {
            self.push_notification(&format!("Failed to save: {e}"));
        }
    }

    pub(crate) fn handle_cmd_overrides_refreshed(
        &mut self,
        result: Result<HashMap<String, OverrideInfo>, String>,
        _sender: &ComponentSender<Self>,
    ) {
        let Ok(overrides) = result else {
            return;
        };
        let mut guard = self.mods.guard();
        let len = guard.len();
        for i in 0..len {
            let needs_update = guard
                .get(i)
                .and_then(|r| r.mod_row())
                .is_some_and(|r| {
                    let info = overrides.get(&r.mod_entry.id);
                    r.overrides != info.map_or(0, |i| i.overrides)
                        || r.overridden_by != info.map_or(0, |i| i.overridden_by)
                });
            if needs_update {
                if let Some(row) = guard.get_mut(i)
                    && let Some(init) = row.mod_row_mut()
                {
                    let id = init.mod_entry.id.clone();
                    let info = overrides.get(&id);
                    init.overrides = info.map_or(0, |i| i.overrides);
                    init.overridden_by = info.map_or(0, |i| i.overridden_by);
                    init.override_files =
                        info.map_or_else(Vec::new, |i| i.override_files.clone());
                    init.overridden_files =
                        info.map_or_else(Vec::new, |i| i.overridden_files.clone());
                    init.conflicting_mod_names =
                        info.map_or_else(Vec::new, |i| i.conflicting_mod_names.clone());
                    init.conflicted_by_mod_names =
                        info.map_or_else(Vec::new, |i| i.conflicted_by_mod_names.clone());
                }
            }
        }
    }

    pub(crate) fn handle_cmd_empty_mod_created(
        &mut self,
        result: Result<(String, std::path::PathBuf), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((_mod_id, cache_dir)) => {
                self.reload_mods(sender);
                self.push_notification(
                    "Empty mod created — put files in its cache folder, then use Scan Cache in Properties",
                );
                let _ = open::that(&cache_dir);
            }
            Err(e) => {
                self.push_notification(&format!("Failed to create mod: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_mod_files_rescanned(
        &mut self,
        result: Result<String, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(msg) => {
                self.needs_deploy = true;
                self.reload_mods(sender);
                self.push_notification(&msg);
            }
            Err(e) => {
                self.push_notification(&format!("Rescan failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_mod_files_loaded(
        &mut self,
        files: Vec<crate::models::manifest::ModFile>,
    ) {
        if let Some(ctrl) = &self.mod_properties_dialog {
            ctrl.sender()
                .send(crate::ui::mod_properties_dialog::ModPropertiesMsg::LoadFiles(files))
                .ok();
        }
    }
}
