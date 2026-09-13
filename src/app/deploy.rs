use std::path::PathBuf;

use adw::prelude::*;
use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::deployer;
use crate::ui::header::DEPLOY_SELECTION_TOOLTIP;
use crate::utils::snap::{self, SelectedFolderKind};

use super::App;
use super::messages::{AppCmdMsg, AppMsg};
use super::types::{DeployCompletion, WorkKind};

const VANILLA_REPLACEMENT_DISPLAY_LIMIT: usize = 50;

#[derive(Debug, PartialEq, Eq)]
struct VanillaDeployAction {
    label: &'static str,
    destructive: bool,
}

fn vanilla_deploy_action(protect: bool) -> VanillaDeployAction {
    if protect {
        VanillaDeployAction {
            label: "Back Up and Deploy",
            destructive: false,
        }
    } else {
        VanillaDeployAction {
            label: "Deploy Without Backup",
            destructive: true,
        }
    }
}

fn present_vanilla_replacement_dialog(
    root: &adw::ApplicationWindow,
    sender: &ComponentSender<App>,
    preflight: crate::core::deployer::DeploymentPreflight,
) {
    use crate::core::deployer::VanillaReplacementStatus;

    let count = preflight.vanilla_replacements.len();
    let unavailable = preflight
        .vanilla_replacements
        .iter()
        .filter(|replacement| replacement.status == VanillaReplacementStatus::BackupUnavailable)
        .count();
    let body = if unavailable == 0 {
        format!(
            "This deployment affects {count} vanilla game file(s). With protection enabled, Deployd keeps verified backups and restores them when no enabled mod uses those paths."
        )
    } else {
        format!(
            "This deployment affects {count} vanilla game file(s). {unavailable} original file(s) are already missing or were overwritten without a backup and may require game-platform verification."
        )
    };
    let dialog = adw::AlertDialog::builder()
        .heading("Replace vanilla game files?")
        .body(&body)
        .build();
    dialog.add_response("cancel", "Cancel");
    let initial_action = vanilla_deploy_action(preflight.protect_vanilla_files);
    dialog.add_response("deploy", initial_action.label);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.set_response_appearance(
        "deploy",
        if initial_action.destructive {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        },
    );

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    for replacement in preflight
        .vanilla_replacements
        .into_iter()
        .take(VANILLA_REPLACEMENT_DISPLAY_LIMIT)
    {
        let row = adw::ActionRow::new();
        row.set_title(&gtk::glib::markup_escape_text(&replacement.path));
        row.set_title_lines(1);
        row.set_subtitle(match replacement.status {
            VanillaReplacementStatus::ReadyToBackUp => "Will be backed up before deployment",
            VanillaReplacementStatus::Protected => "A verified backup is already available",
            VanillaReplacementStatus::BackupUnavailable => {
                "Original backup unavailable; platform verification may be required"
            }
        });
        row.set_tooltip_text(Some(&replacement.path));
        list.append(&row);
    }
    if count > VANILLA_REPLACEMENT_DISPLAY_LIMIT {
        let row = adw::ActionRow::new();
        row.set_title(&format!(
            "… and {} more file(s)",
            count - VANILLA_REPLACEMENT_DISPLAY_LIMIT
        ));
        list.append(&row);
    }
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(80)
        .max_content_height(220)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    content.append(&scrolled);

    let protection = adw::SwitchRow::builder()
        .title("Back up and restore vanilla files automatically")
        .subtitle("Applies to all managed games")
        .active(preflight.protect_vanilla_files)
        .build();
    let group = adw::PreferencesGroup::new();
    group.add(&protection);
    content.append(&group);
    dialog.set_extra_child(Some(&content));

    let dynamic_dialog = dialog.clone();
    protection.connect_active_notify(move |row| {
        let action = vanilla_deploy_action(row.is_active());
        dynamic_dialog.set_response_label("deploy", action.label);
        dynamic_dialog.set_response_appearance(
            "deploy",
            if action.destructive {
                adw::ResponseAppearance::Destructive
            } else {
                adw::ResponseAppearance::Suggested
            },
        );
    });

    let input = sender.input_sender().clone();
    dialog.connect_response(None, move |_, response| {
        let message = if response == "deploy" {
            crate::app::messages::ShellMsg::DeployVanillaConfirmed(protection.is_active())
        } else {
            crate::app::messages::ShellMsg::DeployPreflightCancelled
        };
        let _ = input.send(AppMsg::Shell(message));
    });
    dialog.present(Some(root));
}

#[cfg(test)]
mod tests {
    use super::{VanillaDeployAction, vanilla_deploy_action};

    #[test]
    fn vanilla_warning_action_tracks_protection_switch() {
        assert_eq!(
            vanilla_deploy_action(true),
            VanillaDeployAction {
                label: "Back Up and Deploy",
                destructive: false,
            }
        );
        assert_eq!(
            vanilla_deploy_action(false),
            VanillaDeployAction {
                label: "Deploy Without Backup",
                destructive: true,
            }
        );
    }
}

impl App {
    pub(crate) fn handle_deploy_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if self.selection_mode_active() {
            self.show_toast(DEPLOY_SELECTION_TOOLTIP);
            return;
        }
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

        self.prepare_deploy(sender);
    }

    pub(crate) fn prepare_deploy(&mut self, sender: &ComponentSender<Self>) {
        if self.selection_mode_active() {
            self.show_toast(DEPLOY_SELECTION_TOOLTIP);
            return;
        }
        if self
            .selected_game()
            .is_some_and(|game| game.engine == crate::models::game::GameEngine::MassEffect)
        {
            sender.input(AppMsg::Mele(super::mele::Msg::Setup(false)));
            return;
        }
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

        self.shell.deploying = true;
        self.begin_work(WorkKind::Deploying, "Checking deployment...");
        self.location_command(sender, async move {
            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::DeployPreflightDone(
                deployer::deployment_preflight(&game, &tracker)
                    .await
                    .map_err(|error| error.to_string()),
            ))
        });
    }

    pub(crate) fn handle_cmd_deploy_preflight_done(
        &mut self,
        result: Result<crate::core::deployer::DeploymentPreflight, String>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.finish_work(WorkKind::Deploying);
        let preflight = match result {
            Ok(preflight) => preflight,
            Err(error) => {
                self.shell.deploying = false;
                self.push_notification(&format!("Could not inspect deployment: {error}"));
                return;
            }
        };
        if preflight.vanilla_replacements.is_empty() {
            self.shell.deploying = false;
            self.execute_deploy(preflight.protect_vanilla_files, sender);
            return;
        }
        present_vanilla_replacement_dialog(root, sender, preflight);
    }

    pub(crate) fn handle_vanilla_deploy_confirmed(
        &mut self,
        protect: bool,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.session.tracker.clone() else {
            self.shell.deploying = false;
            self.push_notification("Database not ready yet");
            return;
        };
        self.shell.deploying = true;
        self.begin_work(WorkKind::Deploying, "Saving protection preference...");
        self.location_command(sender, async move {
            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::VanillaProtectionSaved {
                protect,
                result: tracker
                    .set_setting("protect_vanilla_files", &protect.to_string())
                    .await
                    .map_err(|error| error.to_string()),
            })
        });
    }

    pub(crate) fn handle_cmd_vanilla_protection_saved(
        &mut self,
        protect: bool,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        self.shell.deploying = false;
        self.finish_work(WorkKind::Deploying);
        match result {
            Ok(()) => self.execute_deploy(protect, sender),
            Err(error) => self.push_notification(&format!(
                "Deployment cancelled because the vanilla protection preference could not be saved: {error}"
            )),
        }
    }

    pub(crate) fn handle_deploy_preflight_cancelled(&mut self) {
        self.shell.deploying = false;
        self.finish_work(WorkKind::Deploying);
    }

    /// Run the deploy operation directly after all required confirmations.
    pub(crate) fn execute_deploy(
        &mut self,
        protect_vanilla_files: bool,
        sender: &ComponentSender<Self>,
    ) {
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

        self.location_command(sender, async move {
            if let Err(error) = tracker.save_to_profile(&profile_id, &game.id).await {
                return AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::DeployDone(Err(
                    format!("Failed to save the active profile before deployment: {error}"),
                )));
            }
            let timing_start = std::time::Instant::now();
            let game_id = game.id.clone();
            let result =
                match deployer::deploy(&game, &tracker, &cache_root, protect_vanilla_files).await {
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
        if self
            .selected_game()
            .is_some_and(|game| game.engine == crate::models::game::GameEngine::MassEffect)
        {
            sender.input(AppMsg::Mele(super::mele::Msg::Setup(true)));
            return;
        }
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

        self.location_command(sender, async move {
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
        if snap::is_snap() {
            self.handle_recovery(
                crate::app::location_recovery::RecoveryMsg::Start(
                    game.id,
                    crate::utils::location::FolderRole::Game,
                ),
                sender,
                root,
            );
            return;
        }
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
            self.push_notification(&message.to_string());
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
        self.location_command(sender, async move {
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
                if outcome.vanilla_files_backed_up > 0 {
                    msg.push_str(&format!(
                        ", {} vanilla file(s) backed up",
                        outcome.vanilla_files_backed_up
                    ));
                }
                if outcome.vanilla_files_restored > 0 {
                    msg.push_str(&format!(
                        ", {} vanilla file(s) restored",
                        outcome.vanilla_files_restored
                    ));
                }
                self.show_toast(&msg);
                for warning in outcome.warnings {
                    self.push_notification(&format!("Deployment warning: {warning}"));
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
                    let mut message = format!("Purged {} deployed files", outcome.files_removed);
                    if outcome.vanilla_files_restored > 0 {
                        message.push_str(&format!(
                            ", restored {} vanilla file(s)",
                            outcome.vanilla_files_restored
                        ));
                    }
                    self.show_toast(&message);
                }
                for warning in outcome.warnings {
                    self.push_notification(&format!("Purge warning: {warning}"));
                }
            }
            Err(e) => {
                self.push_notification(&format!("Purge failed: {e}"));
            }
        }
    }
}
