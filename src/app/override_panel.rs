use adw::prelude::*;
use relm4::prelude::*;

use crate::models::game::GameEngine;

use super::App;
use super::messages::{AppCmdMsg, AppMsg, ModsCmdMsg, ModsMsg};

pub(super) fn is_override(engine: &GameEngine, path: &str) -> bool {
    *engine == GameEngine::Eclipse
        && path
            .to_ascii_lowercase()
            .starts_with("packages/core/override/")
}

fn reordered_priorities(ids: &[String], first: &str, second: &str) -> Option<Vec<(String, i32)>> {
    let a = ids.iter().position(|id| id == first)?;
    let b = ids.iter().position(|id| id == second)?;
    let mut reordered = ids.to_vec();
    reordered.swap(a, b);
    Some(
        reordered
            .into_iter()
            .enumerate()
            .map(|(index, id)| (id, index as i32))
            .collect(),
    )
}

impl App {
    pub(crate) fn sync_game_panels(&mut self) {
        let labels: &[&str] = if self.game_shows_plugins() {
            &["All", "Mod Order", "Plugin Order", "Downloads"]
        } else if self.game_shows_overrides() {
            &["All", "Mod Order", "Overrides", "Downloads"]
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
                .title("No override packages")
                .subtitle("Installed mods containing override files appear here.")
                .build();
            list.append(&row);
        }
        let mut matches = 0;
        for (index, entry) in entries.iter().enumerate() {
            let row = adw::ActionRow::builder()
                .title(gtk::glib::markup_escape_text(&entry.mod_entry.name))
                .subtitle(if entry.mod_entry.enabled {
                    "Enabled"
                } else {
                    "Disabled"
                })
                .build();
            for (icon, tooltip, adjacent) in [
                ("go-up-symbolic", "Move earlier", index.checked_sub(1)),
                (
                    "go-down-symbolic",
                    "Move later",
                    (index + 1 < entries.len()).then_some(index + 1),
                ),
            ] {
                let button = gtk::Button::from_icon_name(icon);
                button.set_tooltip_text(Some(tooltip));
                button.set_valign(gtk::Align::Center);
                button.add_css_class("flat");
                button.set_sensitive(adjacent.is_some() && !self.ui.override_order_saving);
                if let Some(adjacent) = adjacent {
                    let first = entry.mod_entry.id.clone();
                    let second = entries[adjacent].mod_entry.id.clone();
                    let sender = self.ui.notification_sender.clone();
                    button.connect_clicked(move |_| {
                        let _ = sender.send(AppMsg::Mods(ModsMsg::MoveOverride(
                            first.clone(),
                            second.clone(),
                        )));
                    });
                }
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

    pub(crate) fn move_override(
        &mut self,
        first: String,
        second: String,
        sender: &ComponentSender<Self>,
    ) {
        if !self.game_shows_overrides()
            || self.is_busy()
            || self.shell.location_recovery.is_some()
            || self.ui.override_order_saving
            || !self.ui.override_mod_ids.contains(&first)
            || !self.ui.override_mod_ids.contains(&second)
        {
            return;
        }
        let (Some(tracker), Some(game)) =
            (self.session.tracker.clone(), self.selected_game().cloned())
        else {
            return;
        };
        let ids: Vec<String> = self
            .mods
            .rows
            .iter()
            .filter_map(|row| row.mod_id().map(str::to_owned))
            .collect();
        let Some(updates) = reordered_priorities(&ids, &first, &second) else {
            return;
        };
        let cache_root = match self.cache_root_for(&game.id) {
            Ok(root) => root,
            Err(error) => {
                self.push_notification(&format!("Cannot resolve the mod cache: {error}"));
                return;
            }
        };
        self.ui.override_order_saving = true;
        self.rebuild_override_panel();
        self.location_command(sender, async move {
            let result = async {
                tracker.update_priorities(&updates).await?;
                crate::core::mod_folders::refresh_named_mod_folders(
                    &tracker,
                    &game.id,
                    &cache_root,
                )
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
            AppCmdMsg::Mods(ModsCmdMsg::OverrideOrderSaved {
                game_id: game.id,
                result: Box::new(result),
            })
        });
    }

    pub(crate) fn override_order_saved(
        &mut self,
        game_id: String,
        result: Result<super::types::LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.ui.override_order_saving = false;
        match result {
            Ok(data) => {
                self.apply_loaded_data(data, sender);
            }
            Err(error) => {
                self.push_notification(&format!("Could not refresh override order: {error}"));
                if self.selected_game().is_some_and(|game| game.id == game_id) {
                    self.reload_mods(sender);
                }
            }
        }
        self.rebuild_override_panel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn only_eclipse_override_files_populate_the_panel() {
        for engine in [
            GameEngine::Bethesda,
            GameEngine::Aurora,
            GameEngine::REDEngine,
            GameEngine::MassEffect,
        ] {
            assert!(!is_override(&engine, "packages/core/override/mod/file.gda"));
        }
        assert!(is_override(
            &GameEngine::Eclipse,
            "Packages/Core/Override/file.gda"
        ));
        for path in [
            "addins/mod/file",
            "~docs~/file",
            "../system/file",
            "../launcher/file",
            "../register/file",
            "../file",
            "packages/core/override-other/file",
        ] {
            assert!(!is_override(&GameEngine::Eclipse, path));
        }
    }

    // @variants: both
    #[test]
    fn reordering_overrides_preserves_other_mod_slots() {
        let ids: Vec<String> = ["first", "addin", "second"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            reordered_priorities(&ids, "first", "second"),
            Some(vec![
                ("second".into(), 0),
                ("addin".into(), 1),
                ("first".into(), 2)
            ])
        );
        assert!(reordered_priorities(&ids, "missing", "second").is_none());
    }
}
