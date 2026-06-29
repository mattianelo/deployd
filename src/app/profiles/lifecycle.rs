use relm4::prelude::*;

use crate::core::{game, save_manager};
use crate::models::profile::SaveMode;

use super::super::App;
use super::super::free_fns::{GameLoadMode, load_game_data};
use super::super::messages::AppCmdMsg;

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
    pub(crate) fn handle_profile_selected(&mut self, idx: u32, sender: &ComponentSender<Self>) {
        let state = ProfileSelectionState {
            displayed_idx: self.profile_dropdown.selected() as usize,
            active_idx: self.active_profile_idx,
            profile_count: self.profiles.len(),
            phase: if self.updating_profiles {
                ProfileSelectionPhase::Updating
            } else {
                ProfileSelectionPhase::Ready
            },
        };
        let Some(idx) = resolve_profile_selection(idx as usize, state) else {
            return;
        };

        let target_profile_id = self.profiles[idx].id.clone();
        let target_save_mode = self.profiles[idx].save_mode.clone();
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

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
                let data = load_game_data(&tracker, &game, GameLoadMode::Refresh).await?;
                Ok::<_, String>((data, save_sync))
            };
            AppCmdMsg::ProfileSwitched(result.await)
        });
    }

    pub(crate) fn handle_new_profile_clicked(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

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
                load_game_data(&tracker, &game, GameLoadMode::Refresh).await
            };
            AppCmdMsg::ProfileCreated(result.await)
        });
    }

    pub(crate) fn handle_clone_profile_clicked(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
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
                load_game_data(&tracker, &game, GameLoadMode::Refresh).await
            };
            AppCmdMsg::ProfileCloned(result.await)
        });
    }

    pub(crate) fn handle_delete_profile_clicked(&mut self, sender: &ComponentSender<Self>) {
        if self.profiles.len() <= 1 {
            self.show_toast("Cannot delete the last profile");
            return;
        }

        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
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
                load_game_data(&tracker, &game, GameLoadMode::Refresh).await
            };
            AppCmdMsg::ProfileDeleted(result.await)
        });
    }

    pub(crate) fn handle_rename_profile(
        &mut self,
        new_name: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(profile) = self.profiles.get(self.active_profile_idx) else {
            return;
        };
        let profile_id = profile.id.clone();

        sender.oneshot_command(async move {
            let result = tracker
                .rename_profile(&profile_id, &new_name)
                .await
                .map_err(|e| e.to_string());
            AppCmdMsg::ProfileRenamed(result)
        });
    }

    pub(crate) fn handle_toggle_profile_save_mode(&mut self, sender: &ComponentSender<Self>) {
        let Some(profile) = self.profiles.get(self.active_profile_idx).cloned() else {
            return;
        };
        let Some(tracker) = self.tracker.clone() else {
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
        let Some(profile) = self.profiles.get(self.active_profile_idx).cloned() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let profile_id = profile.id.clone();
        sender.oneshot_command(async move {
            let result = save_manager::sync_profile_saves(&game, &profile_id)
                .await
                .map_err(|e| e.to_string());
            AppCmdMsg::SavesSynced(result)
        });
    }

    pub(crate) fn save_last_profile(&self, sender: &ComponentSender<Self>) {
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
