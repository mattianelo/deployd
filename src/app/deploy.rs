use std::path::PathBuf;

use adw::prelude::*;
use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::deployer;
use crate::utils::paths;
use crate::utils::snap::{self, SelectedFolderKind};

use super::App;
use super::messages::{AppCmdMsg, AppMsg};
use super::types::WorkKind;

impl App {
    pub(crate) fn handle_deploy_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        // Validate preconditions before showing any dialog.
        if self.tracker.is_none() {
            self.push_notification("Database not ready yet");
            return;
        }
        let Some(game) = self.selected_game() else {
            self.push_notification("No game selected");
            return;
        };
        if !game.path.exists() {
            sender.input(AppMsg::GrantGameFolderAccess);
            return;
        }

        let current_id = self
            .profiles
            .get(self.active_profile_idx)
            .map(|p| p.id.clone());
        let mismatch = self
            .last_deployed_profile_id
            .as_ref()
            .zip(current_id.as_ref())
            .is_some_and(|(last, cur)| last != cur);

        if mismatch {
            let last_name = self
                .last_deployed_profile_id
                .as_deref()
                .and_then(|id| self.profiles.iter().find(|p| p.id == id))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "another profile".to_string());
            let cur_name = self
                .profiles
                .get(self.active_profile_idx)
                .map(|p| p.name.clone())
                .unwrap_or_default();

            let body = format!(
                "The game folder was last deployed with \"{last_name}\". \
                 You are now on \"{cur_name}\". Deploying will overwrite \
                 the game folder with this profile's mods."
            );
            let dialog = adw::AlertDialog::builder()
                .heading("Deploy with different profile?")
                .body(&body)
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("deploy", "Deploy");
            dialog.set_default_response(Some("deploy"));
            dialog.set_close_response("cancel");
            dialog.set_response_appearance("deploy", adw::ResponseAppearance::Suggested);
            let s = sender.input_sender().clone();
            dialog.connect_response(None, move |_, response| {
                if response == "deploy" {
                    let _ = s.send(AppMsg::DeployConfirmed);
                }
            });
            dialog.present(Some(root));
            return;
        }

        self.execute_deploy(sender);
    }

    /// Run the deploy operation directly (after any required confirmation dialog).
    pub(crate) fn execute_deploy(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            self.push_notification("No game selected");
            return;
        };
        if !game.path.exists() {
            sender.input(AppMsg::GrantGameFolderAccess);
            return;
        }
        let cache_root = self
            .cache_root_for(&game.id)
            .unwrap_or_else(|_| paths::cache_root().unwrap_or_default());

        self.deploying = true;
        self.begin_work(WorkKind::Deploying, "Deploying...");
        self.auto_save_profile(sender);

        sender.oneshot_command(async move {
            let timing_start = std::time::Instant::now();
            let game_id = game.id.clone();
            AppCmdMsg::DeployDone(
                deployer::deploy(&game, &tracker, &cache_root)
                    .await
                    .map_err(|e| e.to_string())
                    .inspect(|_| {
                        crate::app::timing::log_phase("deploy.apply", &game_id, timing_start, None);
                    }),
            )
        });
    }

    pub(crate) fn handle_purge_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.deploy_options_btn.popdown();
        self.overflow_menu_btn.popdown();
        let current_id = self
            .profiles
            .get(self.active_profile_idx)
            .map(|p| p.id.clone());
        let mismatch = self
            .last_deployed_profile_id
            .as_ref()
            .zip(current_id.as_ref())
            .is_some_and(|(last, cur)| last != cur);

        let detail = if mismatch {
            let last_name = self
                .last_deployed_profile_id
                .as_deref()
                .and_then(|id| self.profiles.iter().find(|p| p.id == id))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "another profile".to_string());
            let cur_name = self
                .profiles
                .get(self.active_profile_idx)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            format!(
                "The game folder was last deployed with \"{last_name}\" but you are now on \
                 \"{cur_name}\". This will remove all deployed mod files from the game folder."
            )
        } else {
            "This will remove all deployed mod files from the game folder. You can redeploy at any time."
                .to_string()
        };

        let dialog = adw::AlertDialog::builder()
            .heading("Purge deployed files?")
            .body(&detail)
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("purge", "Purge");
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("purge", adw::ResponseAppearance::Destructive);

        let input_sender = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "purge" {
                input_sender.send(AppMsg::PurgeConfirmed).unwrap();
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_purge_confirmed(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            self.push_notification("No game selected");
            return;
        };
        if !game.path.exists() {
            sender.input(AppMsg::GrantGameFolderAccess);
            return;
        }
        let cache_root = self
            .cache_root_for(&game.id)
            .unwrap_or_else(|_| paths::cache_root().unwrap_or_default());

        self.deploying = true;
        self.begin_work(WorkKind::Purging, "Purging...");

        sender.oneshot_command(async move {
            AppCmdMsg::PurgeDone(
                deployer::purge(&game, &tracker, &cache_root)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }

    pub(crate) fn handle_grant_game_folder_access(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let dialog = gtk::FileDialog::builder()
            .title(format!("Confirm {} Game Folder", game.title))
            .modal(true)
            .build();
        dialog.set_initial_folder(Some(&gio::File::for_path(&game.path)));
        let input_sender = sender.input_sender().clone();
        dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                input_sender.send(AppMsg::GameFolderGranted(path)).ok();
            }
        });
    }

    pub(crate) fn handle_game_folder_granted(
        &mut self,
        path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        if let Err(message) = snap::validate_selected_folder(&path, SelectedFolderKind::GameFolder)
        {
            self.push_notification(&message);
            return;
        }
        let idx = self.selected_game_idx;
        let Some(game) = self.games.get_mut(idx) else {
            return;
        };
        game.path = path.clone();
        if let Some(tracker) = self.tracker.clone() {
            let game_id = game.id.clone();
            sender.oneshot_command(async move {
                tracker.upsert_game_path(&game_id, &path).await.ok();
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
        self.push_notification("Game folder confirmed — you can now deploy");
    }
}

// ─── AppCmdMsg handlers ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_cmd_deploy_done(
        &mut self,
        result: Result<crate::core::deployer::DeployResult, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.deploying = false;
        self.finish_work(WorkKind::Deploying);
        match result {
            Ok(deploy_result) => {
                self.needs_deploy = false;
                // Track which profile this deploy was performed for.
                if let (Some(tracker), Some(game), Some(profile)) = (
                    self.tracker.clone(),
                    self.selected_game().cloned(),
                    self.profiles.get(self.active_profile_idx),
                ) {
                    let id = profile.id.clone();
                    self.last_deployed_profile_id = Some(id.clone());
                    let key = format!("last_deployed_profile_{}", game.id);
                    sender.oneshot_command(async move {
                        let _ = tracker.set_setting(&key, &id).await;
                        AppCmdMsg::PrioritySaved(Ok(()))
                    });
                }
                self.rebuild_tool_buttons(sender);
                let added = deploy_result.files_added;
                let removed = deploy_result.files_removed;
                let total = deploy_result.files_total;
                let conflicts = deploy_result.conflicts_resolved;
                let mut msg = if added == 0 && removed == 0 {
                    format!("Nothing changed ({total} files deployed)")
                } else {
                    let mut parts: Vec<String> = Vec::new();
                    if added > 0 {
                        parts.push(format!("+{added}"));
                    }
                    if removed > 0 {
                        parts.push(format!("-{removed}"));
                    }
                    format!("Deployed {} ({total} total)", parts.join(", "))
                };
                if conflicts > 0 {
                    msg.push_str(&format!(", {conflicts} conflict(s) resolved"));
                }
                self.show_toast(&msg);
                sender.input(AppMsg::ScanExternalFiles);
            }
            Err(e) => {
                self.push_notification(&format!("Deploy failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_purge_done(&mut self, result: Result<usize, String>) {
        self.deploying = false;
        self.finish_work(WorkKind::Purging);
        match result {
            Ok(count) => {
                self.needs_deploy = true;
                if count == 0 {
                    self.push_notification(
                        "No deployed files tracked — the game folder may already be clean, or try redeploying first",
                    );
                } else {
                    self.push_notification(&format!("Purged {count} deployed files"));
                }
            }
            Err(e) => {
                self.push_notification(&format!("Purge failed: {e}"));
            }
        }
    }
}
