use adw::prelude::*;
use relm4::prelude::*;

use crate::core::{game, save_manager};
use crate::models::profile::SaveMode;

use super::super::App;
use super::super::messages::AppCmdMsg;
use super::super::session::{GameLoadMode, load_game_data};

#[derive(Clone, Copy)]
enum ProfileSelectionPhase {
    Ready,
    Updating,
}

#[derive(Clone, Copy)]
struct ProfileSelectionState {
    displayed_idx: usize,
    active_idx: usize,
    profile_count: usize,
    phase: ProfileSelectionPhase,
}

fn resolve_profile_selection(requested_idx: usize, state: ProfileSelectionState) -> Option<usize> {
    if matches!(state.phase, ProfileSelectionPhase::Updating)
        || requested_idx != state.displayed_idx
        || requested_idx == state.active_idx
        || requested_idx >= state.profile_count
    {
        return None;
    }
    Some(requested_idx)
}

impl App {
    pub(crate) fn handle_new_profile_requested(&mut self, sender: &ComponentSender<Self>) {
        self.ui.profile_menu_btn.popdown();
        self.handle_new_profile_clicked(sender);
    }

    pub(crate) fn handle_clone_profile_requested(&mut self, sender: &ComponentSender<Self>) {
        self.ui.profile_menu_btn.popdown();
        self.handle_clone_profile_clicked(sender);
    }

    pub(crate) fn handle_delete_profile_requested(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.ui.profile_menu_btn.popdown();
        let Some(profile) = self.session.profiles.get(self.session.active_profile_idx) else {
            return;
        };
        let body = if profile.save_mode == SaveMode::ProfileSpecific {
            "This permanently deletes the profile configuration, its isolated save bank, and every save backup owned by it. This cannot be undone."
        } else {
            "This permanently deletes the profile configuration. Shared Global saves are not deleted. This cannot be undone."
        };
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Delete ‘{}’?", profile.name))
            .body(body)
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete permanently");
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        let input = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "delete" {
                let _ = input.send(crate::app::messages::AppMsg::Games(
                    crate::app::messages::GamesMsg::DeleteProfileConfirmed,
                ));
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_toggle_profile_save_mode_requested(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.ui.profile_menu_btn.popdown();
        let Some(profile) = self.session.profiles.get(self.session.active_profile_idx) else {
            return;
        };
        let (heading, body) = match profile.save_mode {
            SaveMode::Global => (
                "Use isolated saves for this profile?",
                "Deployd will preserve the shared Global saves and seed this profile from the current live saves. Close the game before continuing.",
            ),
            SaveMode::ProfileSpecific => (
                "Return to Global saves?",
                "Deployd will preserve this profile's saves and replace the live directory with the shared Global state. Close the game before continuing.",
            ),
        };
        let dialog = adw::AlertDialog::builder()
            .heading(heading)
            .body(body)
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("continue", "Continue");
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("continue", adw::ResponseAppearance::Destructive);
        let input = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "continue" {
                let _ = input.send(crate::app::messages::AppMsg::Games(
                    crate::app::messages::GamesMsg::ToggleProfileSaveModeConfirmed,
                ));
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_sync_saves_requested(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.ui.profile_menu_btn.popdown();
        let dialog = adw::AlertDialog::builder()
            .heading("Sync live saves to this profile?")
            .body("The current profile bank will be replaced, including stored saves that were deleted from the live directory. A recovery point will be created first.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("sync", "Sync");
        dialog.set_close_response("cancel");
        let input = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "sync" {
                let _ = input.send(crate::app::messages::AppMsg::Games(
                    crate::app::messages::GamesMsg::SyncSavesConfirmed,
                ));
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_profile_selected(&mut self, idx: u32, sender: &ComponentSender<Self>) {
        let state = ProfileSelectionState {
            displayed_idx: self.ui.profile_dropdown.selected() as usize,
            active_idx: self.session.active_profile_idx,
            profile_count: self.session.profiles.len(),
            phase: if self.session.updating_profiles {
                ProfileSelectionPhase::Updating
            } else {
                ProfileSelectionPhase::Ready
            },
        };
        let Some(idx) = resolve_profile_selection(idx as usize, state) else {
            return;
        };
        self.session.pending_save_profile_idx = Some(idx);

        let target_profile_id = self.session.profiles[idx].id.clone();
        let target_save_mode = self.session.profiles[idx].save_mode.clone();
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        let old_profile = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .cloned();
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
                let transition = if game::has_save_management(&game) {
                    let backup_cap = save_manager::configured_backup_cap_bytes(&tracker).await;
                    let old_id = old_profile_id.as_deref().unwrap_or(&target_profile_id);
                    let source =
                        save_manager::SaveSetId::for_profile(&game.id, old_id, &old_save_mode);
                    let target = save_manager::SaveSetId::for_profile(
                        &game.id,
                        &target_profile_id,
                        &target_save_mode,
                    );
                    Some(
                        save_manager::prepare_transition(
                            &game,
                            &source,
                            &target,
                            save_manager::BackupTrigger::ProfileSwitch,
                            backup_cap,
                        )
                        .await
                        .map_err(|e| e.to_string())?,
                    )
                } else {
                    None
                };
                if let Err(error) = tracker.switch_profile(&game.id, &target_profile_id).await {
                    if let Some(transition) = transition {
                        transition.rollback().await.map_err(|rollback| {
                            format!("{error}; save rollback also failed: {rollback}")
                        })?;
                    }
                    return Err(error.to_string());
                }
                let save_sync = if let Some(transition) = transition {
                    transition.commit().await.map_err(|e| e.to_string())?
                } else {
                    None
                };
                let data = load_game_data(&tracker, &game, GameLoadMode::Refresh).await?;
                Ok::<_, String>((data, save_sync))
            };
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::ProfileSwitched(
                result.await,
            ))
        });
    }

    pub(crate) fn handle_initialize_pending_save_set(&mut self, sender: &ComponentSender<Self>) {
        let Some(idx) = self.session.pending_save_profile_idx else {
            return;
        };
        let Some(profile) = self.session.profiles.get(idx).cloned() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let save_set =
            save_manager::SaveSetId::for_profile(&game.id, &profile.id, &profile.save_mode);
        sender.oneshot_command(async move {
            let result = save_manager::initialize_save_set(&game, &save_set)
                .await
                .map(|_| (idx, None))
                .map_err(|error| error.to_string());
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::PendingSaveSetPrepared(
                result,
            ))
        });
    }

    pub(crate) fn handle_use_global_for_pending_profile(&mut self, sender: &ComponentSender<Self>) {
        let Some(idx) = self.session.pending_save_profile_idx else {
            return;
        };
        let Some(profile) = self.session.profiles.get(idx).cloned() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let global = save_manager::SaveSetId::Global {
            game_id: game.id.clone(),
        };
        sender.oneshot_command(async move {
            let result = async {
                save_manager::initialize_save_set(&game, &global)
                    .await
                    .map_err(|error| error.to_string())?;
                tracker
                    .set_profile_save_mode(&profile.id, SaveMode::Global)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok((idx, Some(SaveMode::Global)))
            };
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::PendingSaveSetPrepared(
                result.await,
            ))
        });
    }

    pub(crate) fn handle_new_profile_clicked(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        let existing_names: std::collections::HashSet<&str> = self
            .session
            .profiles
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        let mut counter = self.session.profiles.len() + 1;
        let new_name = loop {
            let candidate = format!("Profile {counter}");
            if !existing_names.contains(candidate.as_str()) {
                break candidate;
            }
            counter += 1;
        };
        let active_profile_id = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
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
                load_game_data(&tracker, &game, GameLoadMode::Refresh).await
            };
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::ProfileCreated(
                result.await,
            ))
        });
    }

    pub(crate) fn handle_clone_profile_clicked(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let Some(source_profile) = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .cloned()
        else {
            return;
        };

        let new_name = format!("{} (Copy)", source_profile.name);

        sender.oneshot_command(async move {
            let result = async {
                tracker
                    .save_to_profile(&source_profile.id, &game.id)
                    .await
                    .map_err(|e| e.to_string())?;
                if source_profile.save_mode == SaveMode::ProfileSpecific {
                    let backup_cap = save_manager::configured_backup_cap_bytes(&tracker).await;
                    let source_set = save_manager::SaveSetId::for_profile(
                        &game.id,
                        &source_profile.id,
                        &source_profile.save_mode,
                    );
                    save_manager::capture_save_set(
                        &game,
                        &source_set,
                        save_manager::BackupTrigger::Clone,
                        backup_cap,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                }
                let new_id = tracker
                    .clone_profile(&source_profile.id, &new_name, &game.id)
                    .await
                    .map_err(|e| e.to_string())?;
                if source_profile.save_mode == SaveMode::ProfileSpecific
                    && let Err(error) =
                        save_manager::clone_profile_bank(&game.id, &source_profile.id, &new_id)
                            .await
                {
                    let _ = tracker.delete_profile(&new_id).await;
                    let _ = save_manager::delete_profile_save_data(&game.id, &new_id).await;
                    return Err(error.to_string());
                }
                tracker
                    .switch_profile(&game.id, &new_id)
                    .await
                    .map_err(|e| e.to_string())?;
                load_game_data(&tracker, &game, GameLoadMode::Refresh).await
            };
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::ProfileCloned(
                result.await,
            ))
        });
    }

    pub(crate) fn handle_delete_profile_clicked(&mut self, sender: &ComponentSender<Self>) {
        if self.session.profiles.len() <= 1 {
            self.show_toast("Cannot delete the last profile");
            return;
        }

        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let delete_id = self.session.profiles[self.session.active_profile_idx]
            .id
            .clone();
        let source_profile = self.session.profiles[self.session.active_profile_idx].clone();
        let Some(target_profile) = self
            .session
            .profiles
            .iter()
            .find(|profile| profile.id != delete_id)
            .cloned()
        else {
            return;
        };

        sender.oneshot_command(async move {
            let result = async {
                let transition = if game::has_save_management(&game) {
                    let source_set = save_manager::SaveSetId::for_profile(
                        &game.id,
                        &source_profile.id,
                        &source_profile.save_mode,
                    );
                    let target_set = save_manager::SaveSetId::for_profile(
                        &game.id,
                        &target_profile.id,
                        &target_profile.save_mode,
                    );
                    let backup_cap = save_manager::configured_backup_cap_bytes(&tracker).await;
                    Some(
                        save_manager::prepare_transition(
                            &game,
                            &source_set,
                            &target_set,
                            save_manager::BackupTrigger::ProfileSwitch,
                            backup_cap,
                        )
                        .await
                        .map_err(|error| error.to_string())?,
                    )
                } else {
                    None
                };
                if let Err(error) = tracker.switch_profile(&game.id, &target_profile.id).await {
                    if let Some(transition) = transition {
                        transition.rollback().await.map_err(|rollback| {
                            format!("{error}; save rollback also failed: {rollback}")
                        })?;
                    }
                    return Err(error.to_string());
                }
                if let Err(error) = tracker.delete_profile(&delete_id).await {
                    let database_rollback = tracker.switch_profile(&game.id, &delete_id).await;
                    if let Some(transition) = transition {
                        transition.rollback().await.map_err(|rollback| {
                            format!(
                                "{error}; database rollback: {database_rollback:?}; save rollback also failed: {rollback}"
                            )
                        })?;
                    }
                    database_rollback.map_err(|rollback| {
                        format!("{error}; failed to restore the deleted profile selection: {rollback}")
                    })?;
                    return Err(error.to_string());
                }
                if let Some(transition) = transition {
                    transition.commit().await.map_err(|error| error.to_string())?;
                }
                let cleanup_warning = save_manager::delete_profile_save_data(&game.id, &delete_id)
                    .await
                    .err()
                    .map(|error| error.to_string());
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
                let data = load_game_data(&tracker, &game, GameLoadMode::Refresh).await?;
                Ok((data, cleanup_warning))
            };
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::ProfileDeleted(
                result.await,
            ))
        });
    }

    pub(crate) fn handle_rename_profile(
        &mut self,
        new_name: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(profile) = self.session.profiles.get(self.session.active_profile_idx) else {
            return;
        };
        let profile_id = profile.id.clone();

        sender.oneshot_command(async move {
            let result = tracker
                .rename_profile(&profile_id, &new_name)
                .await
                .map_err(|e| e.to_string());
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::ProfileRenamed(result))
        });
    }

    pub(crate) fn handle_toggle_profile_save_mode(&mut self, sender: &ComponentSender<Self>) {
        let Some(profile) = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .cloned()
        else {
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let new_mode = match profile.save_mode {
            SaveMode::Global => SaveMode::ProfileSpecific,
            SaveMode::ProfileSpecific => SaveMode::Global,
        };
        let profile_id = profile.id.clone();
        sender.oneshot_command(async move {
            let result = async {
                if game::has_save_management(&game) {
                    let old_set = save_manager::SaveSetId::for_profile(
                        &game.id,
                        &profile_id,
                        &profile.save_mode,
                    );
                    let new_set =
                        save_manager::SaveSetId::for_profile(&game.id, &profile_id, &new_mode);
                    if new_mode == SaveMode::ProfileSpecific {
                        let backup_cap = save_manager::configured_backup_cap_bytes(&tracker).await;
                        save_manager::capture_save_set(
                            &game,
                            &old_set,
                            save_manager::BackupTrigger::ModeChange,
                            backup_cap,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        save_manager::initialize_save_set(&game, &new_set)
                            .await
                            .map_err(|e| e.to_string())?;
                    } else {
                        let backup_cap = save_manager::configured_backup_cap_bytes(&tracker).await;
                        let transition = save_manager::prepare_transition(
                            &game,
                            &old_set,
                            &new_set,
                            save_manager::BackupTrigger::ModeChange,
                            backup_cap,
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                        if let Err(error) = tracker
                            .set_profile_save_mode(&profile_id, new_mode.clone())
                            .await
                        {
                            transition.rollback().await.map_err(|rollback| {
                                format!("{error}; save rollback also failed: {rollback}")
                            })?;
                            return Err(error.to_string());
                        }
                        transition.commit().await.map_err(|e| e.to_string())?;
                        return Ok(());
                    }
                }
                tracker
                    .set_profile_save_mode(&profile_id, new_mode)
                    .await
                    .map_err(|e| e.to_string())
            };
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::SaveModeToggled(
                result.await,
            ))
        });
    }

    pub(crate) fn handle_initialize_global_and_disable_isolation(
        &mut self,
        sender: &ComponentSender<Self>,
    ) {
        let Some(profile) = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .cloned()
        else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let global = save_manager::SaveSetId::Global {
            game_id: game.id.clone(),
        };
        sender.oneshot_command(async move {
            let result = async {
                save_manager::initialize_save_set(&game, &global)
                    .await
                    .map_err(|error| error.to_string())?;
                tracker
                    .set_profile_save_mode(&profile.id, SaveMode::Global)
                    .await
                    .map_err(|error| error.to_string())
            };
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::SaveModeToggled(
                result.await,
            ))
        });
    }

    pub(crate) fn handle_sync_saves(&mut self, sender: &ComponentSender<Self>) {
        let Some(profile) = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .cloned()
        else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let profile_id = profile.id.clone();
        sender.oneshot_command(async move {
            let save_set =
                save_manager::SaveSetId::for_profile(&game.id, &profile_id, &profile.save_mode);
            let backup_cap = save_manager::configured_backup_cap_bytes(&tracker).await;
            let result = save_manager::sync_save_set(&game, &save_set, backup_cap)
                .await
                .map_err(|e| e.to_string());
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::SavesSynced(result))
        });
    }

    pub(crate) fn handle_manage_save_backups(&mut self, sender: &ComponentSender<Self>) {
        self.ui.profile_menu_btn.popdown();
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        sender.oneshot_command(async move {
            let result = save_manager::list_backups(&game.id)
                .await
                .map_err(|error| error.to_string());
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::SaveBackupsLoaded(result))
        });
    }

    pub(crate) fn handle_create_save_backup(
        &mut self,
        label: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let Some(profile) = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .cloned()
        else {
            return;
        };
        let save_set =
            save_manager::SaveSetId::for_profile(&game.id, &profile.id, &profile.save_mode);
        sender.oneshot_command(async move {
            let result = save_manager::create_manual_backup(&game, &save_set, label)
                .await
                .map(|_| "Save backup created".to_string())
                .map_err(|error| error.to_string());
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::SaveBackupMutation(
                result,
            ))
        });
    }

    pub(crate) fn handle_restore_save_backup_requested(
        &mut self,
        backup_id: String,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading("Restore this save backup?")
            .body("The destination save set will be replaced. If it is active, the live game saves will also be replaced. A recovery point is created first. Close the game before continuing.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("restore", "Restore");
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("restore", adw::ResponseAppearance::Destructive);
        let input = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "restore" {
                let _ = input.send(crate::app::messages::AppMsg::Games(
                    crate::app::messages::GamesMsg::RestoreSaveBackupConfirmed(backup_id.clone()),
                ));
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_restore_save_backup(
        &mut self,
        backup_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let Some(profile) = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .cloned()
        else {
            return;
        };
        let active_set =
            save_manager::SaveSetId::for_profile(&game.id, &profile.id, &profile.save_mode);
        let tracker = self.session.tracker.clone();
        sender.oneshot_command(async move {
            let backup_cap = match tracker {
                Some(tracker) => save_manager::configured_backup_cap_bytes(&tracker).await,
                None => save_manager::DEFAULT_AUTOMATIC_BACKUP_CAP_BYTES,
            };
            let result = save_manager::restore_backup(&game, &backup_id, &active_set, backup_cap)
                .await
                .map(|_| "Save backup restored".to_string())
                .map_err(|error| error.to_string());
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::SaveBackupMutation(
                result,
            ))
        });
    }

    pub(crate) fn handle_delete_save_backup_requested(
        &mut self,
        backup_id: String,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading("Permanently delete this backup?")
            .body("This save backup cannot be recovered after deletion.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete permanently");
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        let input = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "delete" {
                let _ = input.send(crate::app::messages::AppMsg::Games(
                    crate::app::messages::GamesMsg::DeleteSaveBackupConfirmed(backup_id.clone()),
                ));
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_delete_save_backup(
        &mut self,
        backup_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        sender.oneshot_command(async move {
            let result = save_manager::delete_backup(&game.id, &backup_id)
                .await
                .map(|_| "Save backup deleted".to_string())
                .map_err(|error| error.to_string());
            AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::SaveBackupMutation(
                result,
            ))
        });
    }

    pub(crate) fn save_last_profile(&self, sender: &ComponentSender<Self>) {
        if let (Some(tracker), Some(game), Some(profile)) = (
            self.session.tracker.clone(),
            self.selected_game().cloned(),
            self.session.profiles.get(self.session.active_profile_idx),
        ) {
            let key = format!("last_profile_{}", game.id);
            let id = profile.id.clone();
            sender.oneshot_command(async move {
                let result = tracker
                    .set_setting(&key, &id)
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready_state(displayed_idx: usize, active_idx: usize) -> ProfileSelectionState {
        ProfileSelectionState {
            displayed_idx,
            active_idx,
            profile_count: 3,
            phase: ProfileSelectionPhase::Ready,
        }
    }

    // @variants: both
    #[test]
    fn rejects_stale_queued_selection_after_dropdown_moves() {
        let state = ready_state(2, 2);

        assert_eq!(resolve_profile_selection(0, state), None);
    }

    // @variants: both
    #[test]
    fn accepts_current_user_selection() {
        let state = ready_state(2, 0);

        assert_eq!(resolve_profile_selection(2, state), Some(2));
    }

    // @variants: both
    #[test]
    fn rejects_already_active_selection() {
        let state = ready_state(1, 1);

        assert_eq!(resolve_profile_selection(1, state), None);
    }

    // @variants: both
    #[test]
    fn rejects_selection_during_programmatic_update() {
        let state = ProfileSelectionState {
            displayed_idx: 2,
            active_idx: 0,
            profile_count: 3,
            phase: ProfileSelectionPhase::Updating,
        };

        assert_eq!(resolve_profile_selection(2, state), None);
    }
}
