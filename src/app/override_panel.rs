use adw::prelude::*;
use relm4::prelude::*;

use super::App;
use super::messages::{AppCmdMsg, AppMsg, ModsCmdMsg, ModsMsg};

#[derive(Debug, Clone, Copy)]
pub(crate) enum OverrideAction {
    Toggle,
    Properties,
    Reinstall,
    Remove,
    RemoveConfirmed,
}

impl App {
    pub(crate) fn sync_game_panels(&mut self) {
        let labels: &[&str] = if self.game_shows_plugins() {
            &["All", "Mod Order", "Plugin Order", "Downloads"]
        } else if self.game_shows_overrides() {
            &["All", "DAZIPs & Tools", "Overrides", "Downloads"]
        } else {
            &["All", "Mod Order", "Downloads"]
        };
        self.ui
            .scope_dropdown
            .set_model(Some(&gtk::StringList::new(labels)));
        self.ui.scope_dropdown.set_selected(0);
        self.shell.search_scope = super::types::SearchScope::All;
        if !self.game_shows_conflicts() {
            self.mods.filter = super::types::ModFilter::All;
        }
        self.rebuild_override_panel();
    }

    pub(crate) fn rebuild_override_panel(&self) {
        let list = &self.ui.override_list;
        while let Some(child) = list.first_child() {
            list.remove(&child);
        }
        if !self.game_shows_overrides() {
            return;
        }
        let entries: Vec<_> = self
            .mods
            .rows
            .iter()
            .filter_map(|row| row.mod_row())
            .filter(|row| self.ui.override_mod_ids.contains(&row.mod_entry.id))
            .collect();
        if entries.is_empty() {
            let row = adw::ActionRow::builder()
                .title("No Override mods")
                .subtitle("Add an archive containing loose Override resources to manage them here.")
                .build();
            list.append(&row);
        }
        let mut matches = 0;
        for entry in &entries {
            let row = adw::ActionRow::builder()
                .title(gtk::glib::markup_escape_text(&entry.mod_entry.name))
                .subtitle(if entry.mod_entry.enabled {
                    "Enabled"
                } else {
                    "Disabled"
                })
                .build();
            for (icon, tooltip, action) in [
                (
                    if entry.mod_entry.enabled {
                        "media-playback-pause-symbolic"
                    } else {
                        "media-playback-start-symbolic"
                    },
                    if entry.mod_entry.enabled {
                        "Disable"
                    } else {
                        "Enable"
                    },
                    OverrideAction::Toggle,
                ),
                (
                    "document-properties-symbolic",
                    "Properties",
                    OverrideAction::Properties,
                ),
                (
                    "view-refresh-symbolic",
                    "Reinstall from archive",
                    OverrideAction::Reinstall,
                ),
                ("user-trash-symbolic", "Remove", OverrideAction::Remove),
            ] {
                let button = gtk::Button::from_icon_name(icon);
                button.set_tooltip_text(Some(tooltip));
                button.set_valign(gtk::Align::Center);
                button.add_css_class("flat");
                button.set_sensitive(!self.is_busy() && self.shell.location_recovery.is_none());
                let id = entry.mod_entry.id.clone();
                let sender = self.ui.notification_sender.clone();
                button.connect_clicked(move |_| {
                    let _ = sender.send(AppMsg::Mods(ModsMsg::OverrideAction(id.clone(), action)));
                });
                row.add_suffix(&button);
            }
            let searches_overrides = matches!(
                self.shell.search_scope,
                super::types::SearchScope::All | super::types::SearchScope::PluginOrder
            );
            row.set_visible(
                !searches_overrides
                    || entry
                        .mod_entry
                        .name
                        .to_lowercase()
                        .contains(&self.shell.search_text.to_lowercase()),
            );
            if row.is_visible() {
                matches += 1;
            }
            list.append(&row);
        }
        if !entries.is_empty() && matches == 0 {
            list.append(
                &adw::ActionRow::builder()
                    .title("No matching overrides")
                    .build(),
            );
        }
    }

    pub(crate) fn set_dao_panel_enabled(
        &mut self,
        enabled: bool,
        selected_only: bool,
        sender: &ComponentSender<Self>,
    ) {
        if self.is_busy() || self.shell.location_recovery.is_some() {
            return;
        }
        let (Some(tracker), Some(game)) =
            (self.session.tracker.clone(), self.selected_game().cloned())
        else {
            return;
        };
        let ids: Vec<_> = self
            .mods
            .rows
            .iter()
            .enumerate()
            .filter(|(index, _)| !selected_only || self.mods.selected.contains(index))
            .filter_map(|(_, row)| row.mod_id())
            .filter(|id| !self.ui.override_mod_ids.contains(*id))
            .map(str::to_owned)
            .collect();
        let profile_id = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|profile| profile.id.clone());
        self.ui.override_saving = true;
        self.rebuild_override_panel();
        self.location_command(sender, async move {
            let result = async {
                tracker
                    .set_eclipse_enabled(&ids, enabled, profile_id.as_deref())
                    .await?;
                super::session::load_game_data(
                    &tracker,
                    &game,
                    super::session::GameLoadMode::Refresh,
                )
                .await
                .map_err(anyhow::Error::msg)
            }
            .await
            .map_err(|error| error.to_string());
            AppCmdMsg::Mods(ModsCmdMsg::OverrideChanged {
                game_id: game.id,
                result: Box::new(result),
            })
        });
    }

    pub(crate) fn handle_override_action(
        &mut self,
        id: String,
        action: OverrideAction,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if !self.game_shows_overrides()
            || self.is_busy()
            || self.shell.location_recovery.is_some()
            || !self.ui.override_mod_ids.contains(&id)
        {
            return;
        }
        let Some(index) = self
            .mods
            .rows
            .iter()
            .position(|row| row.mod_id() == Some(&id))
        else {
            return;
        };
        match action {
            OverrideAction::Properties => {
                self.open_mod_properties_at(index, root, sender);
                return;
            }
            OverrideAction::Reinstall => {
                self.reinstall_mod_at(index, sender);
                return;
            }
            OverrideAction::Remove => {
                let dialog = adw::AlertDialog::builder()
                    .heading("Remove Override Mod?")
                    .body("This removes the cached Override mod. Deploy afterward to update game files. Other packages from the same archive are kept.")
                    .build();
                dialog.add_response("cancel", "Cancel");
                dialog.add_response("remove", "Remove");
                dialog.set_close_response("cancel");
                dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
                let input = sender.input_sender().clone();
                dialog.connect_response(None, move |_, response| {
                    if response == "remove" {
                        let _ = input.send(AppMsg::Mods(ModsMsg::OverrideAction(
                            id.clone(),
                            OverrideAction::RemoveConfirmed,
                        )));
                    }
                });
                dialog.present(Some(root));
                return;
            }
            OverrideAction::Toggle | OverrideAction::RemoveConfirmed => {}
        }
        let (Some(tracker), Some(game)) =
            (self.session.tracker.clone(), self.selected_game().cloned())
        else {
            return;
        };
        let enabled = self
            .mods
            .rows
            .get(index)
            .and_then(|row| row.mod_row())
            .is_some_and(|row| row.mod_entry.enabled);
        let Some(entry) = self
            .mods
            .rows
            .get(index)
            .and_then(|row| row.mod_row())
            .map(|row| row.mod_entry.clone())
        else {
            return;
        };
        if matches!(action, OverrideAction::RemoveConfirmed) {
            let cache = match self.cache_root_for(&game.id) {
                Ok(root) => crate::utils::paths::mod_cache_dir_in(&root, &id),
                Err(error) => {
                    self.push_notification(&format!("Cannot resolve mod cache: {error}"));
                    return;
                }
            };
            self.ui.override_saving = true;
            self.rebuild_override_panel();
            self.location_command(sender, async move {
                let result = async {
                    tracker.remove_eclipse_mod(&id).await?;
                    let warning = tokio::task::spawn_blocking(move || {
                        super::install::cleanup::remove_mod_cache(&cache)
                    })
                    .await?;
                    anyhow::Ok((id, warning.into_iter().collect()))
                }
                .await
                .map_err(|error| error.to_string());
                AppCmdMsg::Mods(ModsCmdMsg::ModRemoved(
                    result,
                    entry.nexus_mod_id.zip(entry.nexus_file_id),
                    entry.name,
                    entry.archive_hash,
                ))
            });
            return;
        }
        let profile_id = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|profile| profile.id.clone());
        self.ui.override_saving = true;
        self.rebuild_override_panel();
        self.location_command(sender, async move {
            let result = async {
                tracker
                    .set_eclipse_enabled(&[id], !enabled, profile_id.as_deref())
                    .await?;
                super::session::load_game_data(
                    &tracker,
                    &game,
                    super::session::GameLoadMode::Refresh,
                )
                .await
                .map_err(anyhow::Error::msg)
            }
            .await
            .map_err(|error| error.to_string());
            AppCmdMsg::Mods(ModsCmdMsg::OverrideChanged {
                game_id: game.id,
                result: Box::new(result),
            })
        });
    }

    pub(crate) fn override_changed(
        &mut self,
        game_id: String,
        result: Result<super::types::LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.ui.override_saving = false;
        match result {
            Ok(data) => {
                self.apply_loaded_data(data, sender);
            }
            Err(error) => {
                self.push_notification(&format!("Could not update Override mod: {error}"));
                if self.selected_game().is_some_and(|game| game.id == game_id) {
                    self.reload_mods(sender);
                }
            }
        }
        self.rebuild_override_panel();
    }
}
