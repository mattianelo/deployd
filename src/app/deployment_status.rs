use relm4::prelude::*;

use crate::core::generations::status::{self, Status};

use super::{App, AppCmdMsg};

#[derive(Debug)]
pub(crate) struct Update {
    request: u64,
    game: String,
    profile: String,
    result: Result<(Status, Option<std::time::SystemTime>), String>,
}

impl App {
    pub(crate) fn refresh_deployment_status(&mut self, sender: &ComponentSender<Self>) {
        self.shell.status_request = self.shell.status_request.wrapping_add(1);
        let request = self.shell.status_request;
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        if self.session.profile_game_id.as_deref() != Some(game.id.as_str()) {
            self.shell.deployment_status = None;
            self.session.last_deployed_profile_id = None;
            return;
        }
        let Some(profile) = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|profile| profile.id.clone())
        else {
            return;
        };
        let context = (game.id.clone(), profile.clone());
        if self.shell.status_context.as_ref() != Some(&context) {
            self.shell.deployment_status = None;
            self.session.last_deployed_profile_id = None;
            self.shell.status_context = Some(context);
        }
        if self.is_busy() || self.session.initializing || self.shell.location_recovery.is_some() {
            return;
        }
        let cache = match self.cache_root_for(&game.id) {
            Ok(cache) => cache,
            Err(_) => return,
        };
        self.shell.status_loading = true;
        if self.shell.status_inflight {
            return;
        }
        self.shell.status_inflight = true;
        sender.oneshot_command(async move {
            let _lease = crate::core::location_recovery::activity_lock()
                .read_owned()
                .await;
            let result = async {
                let status = status::read(&tracker, &game, &profile, &cache).await?;
                let game_id = game.id.clone();
                let profile_id = profile.clone();
                let synced = tokio::task::spawn_blocking(move || {
                    crate::core::save_manager::last_save_sync_time(&game_id, &profile_id)
                })
                .await?;
                anyhow::Ok((status, synced))
            }
            .await
            .map_err(|error| format!("{error:#}"));
            AppCmdMsg::DeploymentStatus(Update {
                request,
                game: game.id,
                profile,
                result,
            })
        });
    }

    pub(crate) fn apply_deployment_status(
        &mut self,
        update: Update,
        sender: &ComponentSender<Self>,
    ) {
        self.shell.status_inflight = false;
        if !update.matches(
            self.shell.status_request,
            self.selected_game().map(|game| game.id.as_str()),
            self.session
                .profiles
                .get(self.session.active_profile_idx)
                .map(|profile| profile.id.as_str()),
        ) {
            self.refresh_deployment_status(sender);
            return;
        }
        self.shell.status_loading = false;
        match update.result {
            Ok((status, synced)) => {
                if let Some(profile) = self
                    .session
                    .profiles
                    .get_mut(self.session.active_profile_idx)
                {
                    profile.save_synced_at = synced;
                }
                self.session.last_deployed_profile_id = status.deployed_profile.clone();
                self.shell.deployment_status = Some(status);
                self.shell.deployment_status_error = None;
            }
            Err(error) => {
                self.shell.deployment_status = None;
                if self.shell.deployment_status_error.as_ref() != Some(&error) {
                    self.push_notification(&format!("Cannot check deployment status: {error}"));
                }
                self.shell.deployment_status_error = Some(error);
            }
        }
    }

    pub(crate) fn needs_deploy(&self) -> bool {
        self.shell
            .deployment_status
            .as_ref()
            .is_some_and(|status| status.needs_deploy)
    }
}

pub(crate) fn affects_status(command: &AppCmdMsg) -> bool {
    match command {
        AppCmdMsg::LocationActivityCompleted(_, command) => affects_status(command),
        AppCmdMsg::DeploymentStatus(_) | AppCmdMsg::Downloads(_) => false,
        _ => true,
    }
}

impl Update {
    fn matches(&self, request: u64, game: Option<&str>, profile: Option<&str>) -> bool {
        self.request == request
            && game == Some(self.game.as_str())
            && profile == Some(self.profile.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn stale_requests_games_and_profiles_cannot_replace_current_status() {
        let update = Update {
            request: 4,
            game: "game".into(),
            profile: "profile".into(),
            result: Ok((Status::default(), None)),
        };
        assert!(update.matches(4, Some("game"), Some("profile")));
        assert!(!update.matches(5, Some("game"), Some("profile")));
        assert!(!update.matches(4, Some("other"), Some("profile")));
        assert!(!update.matches(4, Some("game"), Some("other")));
        assert!(!update.matches(4, None, None));
    }
}
