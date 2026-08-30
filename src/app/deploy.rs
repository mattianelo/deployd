use std::path::PathBuf;

use adw::prelude::*;
use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::deployer;
use crate::utils::snap::{self, SelectedFolderKind};

use super::App;
use super::messages::{AppCmdMsg, AppMsg};
use super::types::{DeployCompletion, WorkKind};

impl App {
    pub(crate) fn handle_deploy_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        // Validate preconditions before showing any dialog.
        if self.session.tracker.is_none() {
            self.push_notification("Database not ready yet");
            return;
        }
        let Some(game) = self.selected_game() else {
            self.push_notification("No game selected");
            return;
        };
        if !game.path.exists() {
            sender.input(AppMsg::Shell(
                crate::app::messages::ShellMsg::GrantGameFolderAccess,
            ));
            return;
        }

        let current_id = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|p| p.id.clone());
        let mismatch = self
            .session
            .last_deployed_profile_id
            .as_ref()
            .zip(current_id.as_ref())
            .is_some_and(|(last, cur)| last != cur);

        if mismatch {
            let last_name = self
                .session
                .last_deployed_profile_id
                .as_deref()
                .and_then(|id| self.session.profiles.iter().find(|p| p.id == id))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "another profile".to_string());
            let cur_name = self
                .session
                .profiles
                .get(self.session.active_profile_idx)
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
                    let _ = s.send(AppMsg::Shell(
                        crate::app::messages::ShellMsg::DeployConfirmed,
                    ));
                }
            });
            dialog.present(Some(root));
            return;
        }

        self.execute_deploy(sender);
    }

    /// Run the deploy operation directly (after any required confirmation dialog).
    pub(crate) fn execute_deploy(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            self.push_notification("No game selected");
            return;
        };
        let Some(profile_id) = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|profile| profile.id.clone())
        else {
            self.push_notification("No profile selected");
            return;
        };
        if !game.path.exists() {
            sender.input(AppMsg::Shell(
                crate::app::messages::ShellMsg::GrantGameFolderAccess,
            ));
            return;
        }
        let cache_root = match self.cache_root_for(&game.id) {
            Ok(path) => path,
            Err(error) => {
                self.push_notification(&format!("Cannot resolve the mod cache: {error}"));
                return;
            }
        };

        self.shell.deploying = true;
        self.begin_work(WorkKind::Deploying, "Deploying...");

        sender.oneshot_command(async move {
            if let Err(error) = tracker.save_to_profile(&profile_id, &game.id).await {
                return AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::DeployDone(Err(
                    format!("Failed to save the active profile before deployment: {error}"),
                )));
            }
            let timing_start = std::time::Instant::now();
            let game_id = game.id.clone();
            let result = match deployer::deploy(&game, &tracker, &cache_root).await {
                Ok(result) => {
                    crate::app::timing::log_phase("deploy.apply", &game_id, timing_start, None);
                    match tracker.record_deployed_profile(&game_id, &profile_id).await {
                        Ok(()) => Ok(DeployCompletion {
                            outcome: result,
                            profile_id,
                        }),
                        Err(error) => {
                            Err(format!("Failed to record the deployed profile: {error}"))
                        }
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::DeployDone(result))
        });
    }

    pub(crate) fn handle_purge_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.ui.deploy_options_btn.popdown();
        self.ui.overflow_menu_btn.popdown();
        let current_id = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|p| p.id.clone());
        let mismatch = self
            .session
            .last_deployed_profile_id
            .as_ref()
            .zip(current_id.as_ref())
            .is_some_and(|(last, cur)| last != cur);

        let detail = if mismatch {
            let last_name = self
                .session
                .last_deployed_profile_id
                .as_deref()
                .and_then(|id| self.session.profiles.iter().find(|p| p.id == id))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "another profile".to_string());
            let cur_name = self
                .session
                .profiles
                .get(self.session.active_profile_idx)
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
                let _ = input_sender.send(AppMsg::Shell(
                    crate::app::messages::ShellMsg::PurgeConfirmed,
                ));
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_purge_confirmed(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            self.push_notification("No game selected");
            return;
        };
        if !game.path.exists() {
            sender.input(AppMsg::Shell(
                crate::app::messages::ShellMsg::GrantGameFolderAccess,
            ));
            return;
        }
        let cache_root = match self.cache_root_for(&game.id) {
            Ok(path) => path,
            Err(error) => {
                self.push_notification(&format!("Cannot resolve the mod cache: {error}"));
                return;
            }
        };

        self.shell.deploying = true;
        self.begin_work(WorkKind::Purging, "Purging...");

        sender.oneshot_command(async move {
            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PurgeDone(
                deployer::purge(&game, &tracker, &cache_root)
                    .await
                    .map_err(|e| e.to_string()),
            ))
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
                input_sender
                    .send(AppMsg::Shell(
                        crate::app::messages::ShellMsg::GameFolderGranted(path),
                    ))
                    .ok();
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
        let Some(game_id) = self.selected_game().map(|game| game.id.clone()) else {
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        let saved_path = path.clone();
        sender.oneshot_command(async move {
            let result = tracker
                .upsert_game_path(&game_id, &saved_path)
                .await
                .map_err(|error| error.to_string());
            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::GamePathSaved {
                game_id,
                path: saved_path,
                result,
            })
        });
    }

    pub(crate) fn handle_cmd_game_path_saved(
        &mut self,
        game_id: String,
        path: PathBuf,
        result: Result<(), String>,
    ) {
        match result {
            Ok(()) => {
                if let Some(game) = self
                    .session
                    .games
                    .iter_mut()
                    .find(|game| game.id == game_id)
                {
                    game.path = path;
                }
                self.show_toast("Game folder confirmed — you can now deploy");
            }
            Err(error) => self.push_notification(&format!("Could not save game folder: {error}")),
        }
    }
}

// ─── AppCmdMsg handlers ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_cmd_deploy_done(
        &mut self,
        result: Result<DeployCompletion, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.shell.deploying = false;
        self.finish_work(WorkKind::Deploying);
        match result {
            Ok(completion) => {
                self.shell.needs_deploy = false;
                self.session.last_deployed_profile_id = Some(completion.profile_id);
                self.rebuild_tool_buttons(sender);
                let outcome = completion.outcome;
                let added = outcome.files_added;
                let removed = outcome.files_removed;
                let total = outcome.files_total;
                let conflicts = outcome.conflicts_resolved;
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
                for warning in outcome.warnings {
                    self.push_notification(&format!("Deployment cleanup warning: {warning}"));
                }
                sender.input(AppMsg::Mods(
                    crate::app::messages::ModsMsg::ScanExternalFiles,
                ));
            }
            Err(e) => {
                self.push_notification(&format!("Deploy failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_purge_done(
        &mut self,
        result: Result<crate::core::deployer::PurgeOutcome, String>,
    ) {
        self.shell.deploying = false;
        self.finish_work(WorkKind::Purging);
        match result {
            Ok(outcome) => {
                self.shell.needs_deploy = true;
                if outcome.files_removed == 0 {
                    self.push_notification(
                        "No deployed files tracked — the game folder may already be clean, or try redeploying first",
                    );
                } else {
                    self.show_toast(&format!("Purged {} deployed files", outcome.files_removed));
                }
                for warning in outcome.warnings {
                    self.push_notification(&format!("Purge cleanup warning: {warning}"));
                }
            }
            Err(e) => {
                self.push_notification(&format!("Purge failed: {e}"));
            }
        }
    }
}
