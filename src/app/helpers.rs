use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game;
use crate::models::game::Game;
use crate::models::profile::SaveMode;
use crate::ui::mod_list::ModListItemKind;
use crate::ui::pre_install_dialog::{PreInstallDialog, PreInstallDialogInit, PreInstallDialogOutput};

use super::messages::AppMsg;
use super::types::SearchScope;
use super::App;

impl App {
    pub(crate) fn selected_game(&self) -> Option<&Game> {
        self.games.get(self.selected_game_idx)
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
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let Some(pending) = &self.pending_install else {
            return;
        };
        let mod_name = pending.mod_name.clone();
        let is_fomod = pending.fomod_config.is_some();
        let is_bethesda =
            pending.game.engine == crate::models::game::GameEngine::Bethesda;
        let file_preview = if let Some(ref fl) = pending.file_list {
            let rules = crate::core::rules::rules_for_game(&pending.game.id);
            crate::ui::pre_install_dialog::file_preview_from_list(fl, &rules)
        } else {
            vec![]
        };
        self.pre_install_dialog = Some(
            PreInstallDialog::builder()
                .transient_for(root)
                .launch(PreInstallDialogInit {
                    mod_name,
                    file_preview,
                    is_fomod,
                    is_bethesda,
                })
                .forward(sender.input_sender(), |output| match output {
                    PreInstallDialogOutput::Confirmed(name, targets) => {
                        AppMsg::PreInstallConfirmed(name, targets)
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
        self.installing || self.deploying
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

    /// Return the mod ID of an existing mod whose name matches `name` (case-insensitive),
    /// or `None` if no such mod exists in the current game's list.
    pub(crate) fn find_mod_id_by_name(&self, name: &str) -> Option<String> {
        self.mods.iter().find_map(|item| {
            if let ModListItemKind::Mod(ref m) = item.kind
                && m.mod_entry.name.eq_ignore_ascii_case(name)
            {
                Some(m.mod_entry.id.clone())
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

    pub(crate) fn apply_search_filter(&mut self) {
        let query = self.search_text.to_lowercase();
        let empty = query.is_empty();

        if self.search_scope == SearchScope::All || self.search_scope == SearchScope::ModOrder {
            let mut guard = self.mods.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    // Separator rows stay visible unless searching (hide them during search).
                    if row.is_separator() {
                        row.visible = empty;
                    } else {
                        row.visible = empty || row.mod_name().to_lowercase().contains(&query);
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
            let mut guard = self.downloads.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    row.visible = empty || row.entry.mod_name.to_lowercase().contains(&query);
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

        self.tool_buttons_box
            .set_visible(self.has_games() && !self.tools.is_empty());
    }
}
