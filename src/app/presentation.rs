use crate::models::download::DownloadStatus;
use crate::ui::bottom_status::BottomStatusState;

use super::App;
use super::types::{WorkKind, WorkStatus};

impl App {
    pub(crate) fn header_state(&self) -> crate::ui::header::HeaderState {
        crate::ui::header::HeaderState {
            nexus_username: self.shell.nexus_username.clone(),
            nexus_is_premium: self.shell.nexus_is_premium,
            has_games: self.has_games(),
            initializing: self.session.initializing,
            profile_count: self.session.profiles.len(),
            save_mode_label: self.save_mode_label(),
            game_has_save_management: self.game_has_save_management(),
            can_sync_saves: self.can_sync_saves(),
            is_busy: self.is_busy(),
            busy_message: self.busy_message(),
            deploying: self.shell.deploying,
            needs_deploy: self.shell.needs_deploy,
            notification_count: self.notifications_count(),
            notification_badge: self.notifications_badge(),
            external_changes_count: self.mods.external_changes_count,
            app_update_version: self.shell.app_update_version.clone(),
            running_as_appimage: self.shell.running_as_appimage,
            global_active_count: self.download.global_active_count,
            downloads_visible: self.download.visible,
            search_active: self.shell.search_active,
        }
    }
    pub(crate) fn has_games(&self) -> bool {
        !self.session.games.is_empty()
    }

    /// True when the mod list has no mod rows (only separators or empty).
    pub(crate) fn has_no_mods(&self) -> bool {
        !self.mods.rows.iter().any(|item| !item.is_separator())
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.install.is_busy()
            || self.shell.deploying
            || self.tools.proton_setup
            || self.shell.work_status.is_some()
    }

    pub(crate) fn begin_work(&mut self, kind: WorkKind, message: impl Into<String>) {
        let message = message.into();
        self.shell.status_msg = Some(message.clone());
        self.shell.work_status = Some(WorkStatus {
            kind,
            message,
            progress: None,
        });
    }

    pub(crate) fn update_work(
        &mut self,
        kind: WorkKind,
        message: impl Into<String>,
        progress: Option<f64>,
    ) {
        let message = message.into();
        self.shell.status_msg = Some(message.clone());
        self.shell.work_status = Some(WorkStatus {
            kind,
            message,
            progress,
        });
    }

    pub(crate) fn finish_work(&mut self, kind: WorkKind) {
        if self
            .shell
            .work_status
            .as_ref()
            .is_some_and(|status| status.kind == kind)
        {
            self.shell.work_status = None;
            self.shell.status_msg = None;
        }
    }

    pub(crate) fn finish_current_work(&mut self) {
        self.shell.work_status = None;
        self.shell.status_msg = None;
    }

    pub(crate) fn busy_message(&self) -> String {
        if let Some(status) = &self.shell.work_status {
            if let Some(progress) = status.progress {
                let pct = (progress.clamp(0.0, 1.0) * 100.0).round() as u8;
                format!("{} ({pct}%)", status.message)
            } else {
                status.message.clone()
            }
        } else {
            self.shell
                .status_msg
                .clone()
                .unwrap_or_else(|| "Working...".to_string())
        }
    }
    /// Number of distinct notification items currently in the popover.
    pub(crate) fn notifications_count(&self) -> usize {
        usize::from(self.mods.external_changes_count > 0)
            + usize::from(self.shell.app_update_version.is_some())
            + self.ui.notification_count
    }

    /// Label for the notifications headerbar button badge.
    pub(crate) fn notifications_badge(&self) -> String {
        let n = self.notifications_count();
        if n > 0 { n.to_string() } else { String::new() }
    }

    pub(crate) fn rate_limit_label(&self) -> String {
        match &self.download.rate_limit {
            Some(rl) => format!(
                "API: {}/{} hourly · {}/{} daily",
                rl.hourly_remaining, rl.hourly_limit, rl.daily_remaining, rl.daily_limit
            ),
            None => String::new(),
        }
    }

    pub(crate) fn total_mods_count(&self) -> usize {
        self.mods.rows.iter().filter(|m| !m.is_separator()).count()
    }

    pub(crate) fn enabled_mods_count(&self) -> usize {
        self.mods
            .rows
            .iter()
            .filter(|m| m.mod_row().is_some_and(|r| r.mod_entry.enabled))
            .count()
    }

    pub(crate) fn issues_mods_count(&self) -> usize {
        self.mods
            .rows
            .iter()
            .filter(|m| {
                m.mod_row()
                    .is_some_and(|r| r.overrides > 0 || r.overridden_by > 0)
            })
            .count()
    }

    pub(crate) fn conflict_count_label(&self) -> String {
        let n = self.issues_mods_count();
        if n == 1 {
            "1 conflict".to_string()
        } else {
            format!("{n} conflicts")
        }
    }

    pub(crate) fn enabled_plugins_count(&self) -> usize {
        self.plugins
            .rows
            .iter()
            .filter(|p| p.plugin.enabled)
            .count()
    }

    pub(crate) fn active_downloads_count(&self) -> usize {
        self.download
            .rows
            .iter()
            .filter(|d| d.entry.is_active())
            .count()
    }

    pub(crate) fn completed_downloads_count(&self) -> usize {
        self.download
            .rows
            .iter()
            .filter(|d| {
                matches!(
                    d.entry.status,
                    DownloadStatus::Installed | DownloadStatus::Downloaded
                )
            })
            .count()
    }

    pub(crate) fn mod_status_label(&self) -> String {
        format!(
            "{} of {} mods",
            self.enabled_mods_count(),
            self.total_mods_count()
        )
    }

    pub(crate) fn plugin_status_label(&self) -> String {
        format!(
            "{} of {} plugins",
            self.enabled_plugins_count(),
            self.plugins.rows.len()
        )
    }

    pub(crate) fn bottom_status_state(&self) -> BottomStatusState {
        let has_conflicts = self.issues_mods_count() > 0;
        let rate_limit_warning = self
            .download
            .rate_limit
            .as_ref()
            .is_some_and(|limit| limit.hourly_remaining < 10 || limit.daily_remaining < 100);
        BottomStatusState {
            initializing: self.session.initializing,
            mod_status: self.mod_status_label(),
            plugin_status: self.plugin_status_label(),
            conflict_status: self.conflict_count_label(),
            has_conflicts,
            rate_limit_status: self.rate_limit_label(),
            rate_limit_visible: self.download.rate_limit.is_some(),
            rate_limit_warning,
            needs_deploy: self.shell.needs_deploy,
            has_games: self.has_games(),
        }
    }
}
