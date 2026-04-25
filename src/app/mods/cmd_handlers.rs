use relm4::prelude::*;

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
                self.toaster.toast(&format!("Load failed: {e}"));
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
                self.toaster
                    .toast("Mod removed. Deploy to update game files");
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
                self.reload_mods(sender);
            }
            Err(e) => {
                self.toaster.toast(&format!("Remove failed: {e}"));
                self.reload_mods(sender);
            }
        }
    }

    pub(crate) fn handle_cmd_priority_saved(
        &mut self,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(()) => self.reload_mods(sender),
            Err(e) => self.toaster.toast(&format!("Failed to save order: {e}")),
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
                self.toaster.toast(
                    "Empty mod created — put files in its cache folder, then use Scan Cache in Properties",
                );
                let _ = open::that(&cache_dir);
            }
            Err(e) => {
                self.toaster.toast(&format!("Failed to create mod: {e}"));
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
                self.toaster.toast(&msg);
            }
            Err(e) => {
                self.toaster.toast(&format!("Rescan failed: {e}"));
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
