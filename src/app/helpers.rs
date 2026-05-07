use std::path::PathBuf;

use anyhow::Result;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game;
use crate::models::download::DownloadStatus;
use crate::models::game::Game;
use crate::models::profile::SaveMode;
use crate::ui::mod_list::ModListItemKind;
use crate::ui::pre_install_dialog::{
    PreInstallDialog, PreInstallDialogInit, PreInstallDialogOutput,
};
use crate::utils::paths;

use super::App;
use super::messages::AppMsg;
use super::types::{DownloadFilter, ModFilter, SearchScope};

impl App {
    pub(crate) fn selected_game(&self) -> Option<&Game> {
        self.games.get(self.selected_game_idx)
    }

    /// Resolve the effective cache root for a game.
    pub(crate) fn cache_root_for(&self, game_id: &str) -> Result<PathBuf> {
        let custom = self.game_cache_dirs.get(game_id).map(PathBuf::as_path);
        paths::game_cache_root(custom)
    }

    /// Find an installed mod that has the given Nexus mod ID.
    /// Returns `(mod_id, mod_name, priority)` if found.
    pub(crate) fn find_installed_mod_by_nexus_id(
        &mut self,
        nexus_mod_id: i64,
        nexus_file_id: i64,
    ) -> Option<(String, String, i32)> {
        // file_id == 0 means the archive was scanned from disk without a known Nexus file ID
        // (the filename only encodes the mod ID). We can't distinguish between different files
        // from the same mod page in that case, so skip the duplicate check to avoid false positives.
        if nexus_file_id == 0 {
            return None;
        }
        let guard = self.mods.guard();
        for item in guard.iter() {
            if let ModListItemKind::Mod(ref init) = item.kind
                && init.mod_entry.nexus_mod_id == Some(nexus_mod_id)
                && init.mod_entry.nexus_file_id == Some(nexus_file_id)
            {
                return Some((
                    init.mod_entry.id.clone(),
                    init.mod_entry.name.clone(),
                    init.mod_entry.priority,
                ));
            }
        }
        None
    }

    /// Find an installed mod whose `archive_hash` matches the given SHA-256 hex string.
    /// Returns `(mod_id, mod_name, priority)` if found.
    pub(crate) fn find_mod_by_archive_hash(&self, hash: &str) -> Option<(String, String, i32)> {
        self.mods.iter().find_map(|item| {
            if let ModListItemKind::Mod(ref init) = item.kind
                && init
                    .mod_entry
                    .archive_hash
                    .as_deref()
                    .is_some_and(|h| h == hash)
            {
                Some((
                    init.mod_entry.id.clone(),
                    init.mod_entry.name.clone(),
                    init.mod_entry.priority,
                ))
            } else {
                None
            }
        })
    }

    /// Open the pre-install name/target dialog for the current `pending_install`.
    pub(crate) fn open_pre_install_dialog(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        // If the install-time fetch found the mod name but could not match a Nexus file
        // entry, ask the user to supply a file ID before opening the pre-install dialog.
        if self.pending_file_id_needed.is_some() {
            self.show_file_id_dialog(root, sender);
            return;
        }
        // If a background Nexus fetch completed during extraction, apply it before
        // reading pending.mod_name so the dialog proposes the real mod name.
        if let Some(fetched) = self.pending_fetched_name.take()
            && let Some(pending) = &mut self.pending_install
        {
            pending.mod_name = fetched;
        }
        let Some(pending) = &self.pending_install else {
            return;
        };
        let mod_name = pending.mod_name.clone();
        let is_fomod = pending.fomod_config.is_some();
        let is_bethesda = pending.game.engine == crate::models::game::GameEngine::Bethesda;
        let is_aurora = pending.game.engine == crate::models::game::GameEngine::Aurora;
        let file_preview = if let Some(ref fl) = pending.file_list {
            let rules = crate::core::rules::rules_for_game(&pending.game.id);
            crate::ui::pre_install_dialog::file_preview_from_list(
                fl,
                &rules,
                pending.game.engine.clone(),
                &pending.game.data_subdir,
            )
        } else {
            vec![]
        };
        let mod_names: Vec<String> = self
            .mods
            .iter()
            .filter_map(|item| {
                if item.is_separator() {
                    None
                } else {
                    Some(item.mod_name().to_owned())
                }
            })
            .collect();
        self.pre_install_dialog = Some(
            PreInstallDialog::builder()
                .transient_for(root)
                .launch(PreInstallDialogInit {
                    mod_name,
                    file_preview,
                    is_fomod,
                    is_bethesda,
                    is_aurora,
                    mod_names,
                })
                .forward(sender.input_sender(), |output| match output {
                    PreInstallDialogOutput::Confirmed(name, targets, excluded) => {
                        AppMsg::PreInstallConfirmed(name, targets, excluded)
                    }
                    PreInstallDialogOutput::Cancelled => AppMsg::PreInstallCancelled,
                }),
        );
    }

    pub(crate) fn has_games(&self) -> bool {
        !self.games.is_empty()
    }

    /// True when the mod list has no mod rows (only separators or empty).
    pub(crate) fn has_no_mods(&self) -> bool {
        !self.mods.iter().any(|item| !item.is_separator())
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.installing || self.deploying || self.proton_setup
    }

    /// True when the selected game supports per-profile save management.
    pub(crate) fn game_has_save_management(&self) -> bool {
        self.selected_game().is_some_and(game::has_save_management)
    }

    /// True when the active profile uses per-profile saves and a manual sync makes sense.
    pub(crate) fn can_sync_saves(&self) -> bool {
        self.game_has_save_management()
            && self
                .profiles
                .get(self.active_profile_idx)
                .is_some_and(|p| p.save_mode == SaveMode::ProfileSpecific)
    }

    /// Label for the save mode toggle button based on the active profile.
    /// For ProfileSpecific profiles, appends the age of the last save snapshot.
    pub(crate) fn save_mode_label(&self) -> String {
        let Some(profile) = self.profiles.get(self.active_profile_idx) else {
            return "Saves: Global".to_string();
        };
        match &profile.save_mode {
            SaveMode::Global => "Saves: Global".to_string(),
            SaveMode::ProfileSpecific => {
                let age = match profile.save_synced_at {
                    None => "never synced".to_string(),
                    Some(t) => {
                        let secs = t.elapsed().unwrap_or_default().as_secs();
                        if secs < 60 {
                            "just now".to_string()
                        } else if secs < 3600 {
                            format!("{}m ago", secs / 60)
                        } else if secs < 86400 {
                            format!("{}h ago", secs / 3600)
                        } else {
                            format!("{}d ago", secs / 86400)
                        }
                    }
                };
                format!("Saves: Profile · {age}")
            }
        }
    }

    /// Return `(mod_id, priority)` for an existing mod whose name matches `name`
    /// (case-insensitive), or `None` if not found.
    pub(crate) fn find_mod_id_and_priority_by_name(&self, name: &str) -> Option<(String, i32)> {
        self.mods.iter().find_map(|item| {
            if let ModListItemKind::Mod(ref m) = item.kind
                && m.mod_entry.name.eq_ignore_ascii_case(name)
            {
                Some((m.mod_entry.id.clone(), m.mod_entry.priority))
            } else {
                None
            }
        })
    }

    /// Return the display name for a mod by its ID, or the ID itself if not found.
    pub(crate) fn mod_name_for_id(&self, mod_id: &str) -> String {
        self.mods
            .iter()
            .find_map(|item| {
                if let ModListItemKind::Mod(ref m) = item.kind
                    && m.mod_entry.id == mod_id
                {
                    return Some(m.mod_entry.name.clone());
                }
                None
            })
            .unwrap_or_else(|| mod_id.to_string())
    }

    /// Number of distinct notification items currently in the popover.
    pub(crate) fn notifications_count(&self) -> usize {
        usize::from(self.external_changes_count > 0)
            + usize::from(self.app_update_version.is_some())
            + self.notification_count
    }

    /// Label for the notifications headerbar button badge.
    pub(crate) fn notifications_badge(&self) -> String {
        let n = self.notifications_count();
        if n > 0 { n.to_string() } else { String::new() }
    }

    pub(crate) fn rate_limit_label(&self) -> String {
        match &self.rate_limit_info {
            Some(rl) => format!(
                "API: {}/{} hourly · {}/{} daily",
                rl.hourly_remaining, rl.hourly_limit, rl.daily_remaining, rl.daily_limit
            ),
            None => String::new(),
        }
    }

    pub(crate) fn rate_limit_css(&self) -> Vec<&'static str> {
        let mut classes = vec!["caption"];
        match &self.rate_limit_info {
            Some(rl) if rl.hourly_remaining < 10 || rl.daily_remaining < 100 => {
                classes.push("warning");
            }
            _ => {
                classes.push("dim-label");
            }
        }
        classes
    }

    pub(crate) fn total_mods_count(&self) -> usize {
        self.mods.iter().filter(|m| !m.is_separator()).count()
    }

    pub(crate) fn enabled_mods_count(&self) -> usize {
        self.mods
            .iter()
            .filter(|m| m.mod_row().is_some_and(|r| r.mod_entry.enabled))
            .count()
    }

    pub(crate) fn issues_mods_count(&self) -> usize {
        self.mods
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
        self.plugins.iter().filter(|p| p.plugin.enabled).count()
    }

    pub(crate) fn active_downloads_count(&self) -> usize {
        self.downloads
            .iter()
            .filter(|d| d.entry.is_active())
            .count()
    }

    pub(crate) fn completed_downloads_count(&self) -> usize {
        self.downloads
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
            self.plugins.len()
        )
    }

    pub(crate) fn apply_search_filter(&mut self) {
        let query = self.search_text.to_lowercase();
        let empty = query.is_empty();

        if self.search_scope == SearchScope::All || self.search_scope == SearchScope::ModOrder {
            let mod_filter = self.mod_filter;
            let no_filter = empty && matches!(mod_filter, ModFilter::All);
            let mut in_collapsed_group = false;
            let mut guard = self.mods.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    if row.is_separator() {
                        in_collapsed_group = row.is_collapsed();
                        // Hide separators when search or filter chip is active.
                        row.visible = no_filter;
                    } else if no_filter {
                        // No active search/filter: respect group collapse state.
                        row.visible = !in_collapsed_group;
                    } else {
                        // Search or filter active: show all matching mods regardless of group.
                        let name_match = empty || row.mod_name().to_lowercase().contains(&query);
                        let filter_match = match mod_filter {
                            ModFilter::All => true,
                            ModFilter::Enabled => {
                                row.mod_row().is_some_and(|r| r.mod_entry.enabled)
                            }
                            ModFilter::Issues => row
                                .mod_row()
                                .is_some_and(|r| r.overrides > 0 || r.overridden_by > 0),
                        };
                        row.visible = name_match && filter_match;
                    }
                }
            }
        }

        if self.search_scope == SearchScope::All || self.search_scope == SearchScope::PluginOrder {
            let mut guard = self.plugins.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    row.visible = empty
                        || row.plugin.filename.to_lowercase().contains(&query)
                        || row.mod_name.to_lowercase().contains(&query);
                }
            }
        }

        if self.search_scope == SearchScope::All || self.search_scope == SearchScope::Downloads {
            let download_filter = self.download_filter;
            let mut guard = self.downloads.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    let name_match = empty || row.entry.mod_name.to_lowercase().contains(&query);
                    let filter_match = match download_filter {
                        DownloadFilter::All => true,
                        DownloadFilter::Active => row.entry.is_active(),
                        DownloadFilter::Completed => matches!(
                            row.entry.status,
                            DownloadStatus::Installed | DownloadStatus::Downloaded
                        ),
                    };
                    row.visible = name_match && filter_match;
                }
            }
        }
    }

    /// Rebuild tool buttons in the headerbar for the current game's tools.
    /// Up to 3 tools are shown as individual buttons; any beyond that are
    /// collected into an overflow `gtk::MenuButton`.
    pub(crate) fn rebuild_tool_buttons(&self, sender: &ComponentSender<Self>) {
        const MAX_VISIBLE: usize = 3;

        // Remove all existing tool buttons
        while let Some(child) = self.tool_buttons_box.first_child() {
            self.tool_buttons_box.remove(&child);
        }

        let busy = self.is_busy();
        let (visible, overflow) = if self.tools.len() > MAX_VISIBLE {
            self.tools.split_at(MAX_VISIBLE)
        } else {
            (&self.tools[..], &[][..])
        };

        for tool in visible {
            let btn = gtk::Button::new();
            btn.set_icon_name(&tool.icon_name);
            btn.set_tooltip_text(Some(&tool.name));
            btn.set_sensitive(!busy);
            btn.add_css_class("flat");

            let tool_id = tool.id.clone();
            let input_sender = sender.input_sender().clone();
            btn.connect_clicked(move |_| {
                input_sender
                    .send(AppMsg::LaunchTool(tool_id.clone()))
                    .unwrap();
            });

            self.tool_buttons_box.append(&btn);
        }

        if !overflow.is_empty() {
            let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
            for tool in overflow {
                let btn = gtk::Button::new();
                btn.set_icon_name(&tool.icon_name);
                btn.set_tooltip_text(Some(&tool.name));
                btn.set_label(&tool.name);
                btn.set_sensitive(!busy);
                btn.add_css_class("flat");

                let tool_id = tool.id.clone();
                let input_sender = sender.input_sender().clone();
                btn.connect_clicked(move |b| {
                    input_sender
                        .send(AppMsg::LaunchTool(tool_id.clone()))
                        .unwrap();
                    if let Some(p) = b
                        .ancestor(gtk::Popover::static_type())
                        .and_downcast::<gtk::Popover>()
                    {
                        p.popdown();
                    }
                });

                popover_box.append(&btn);
            }

            let popover = gtk::Popover::new();
            popover.set_child(Some(&popover_box));

            let overflow_btn = gtk::MenuButton::new();
            overflow_btn.set_icon_name("view-more-symbolic");
            overflow_btn.set_tooltip_text(Some("More tools"));
            overflow_btn.set_sensitive(!busy);
            overflow_btn.add_css_class("flat");
            overflow_btn.set_popover(Some(&popover));

            self.tool_buttons_box.append(&overflow_btn);
        }

        let has_tools = self.has_games() && !self.tools.is_empty();
        self.tool_buttons_box.set_visible(has_tools);

        if has_tools {
            // Append a vertical separator so it sits between the tool buttons
            // and the main action buttons (deploy, search, notifications, etc.).
            let sep = gtk::Separator::builder()
                .orientation(gtk::Orientation::Vertical)
                .margin_top(6)
                .margin_bottom(6)
                .build();
            self.tool_buttons_box.append(&sep);
        }
    }
}
