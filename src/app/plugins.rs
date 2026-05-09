use std::collections::HashMap;

use adw;
use relm4::prelude::*;

use crate::core::game;
use crate::models::plugin::Plugin;
#[cfg(feature = "loot")]
use crate::models::plugin::PluginDirtyInfo;
use crate::ui::plugin_list::PluginRowInit;

use super::App;
use super::free_fns::load_game_data;
use super::messages::AppCmdMsg;

impl App {
    pub(crate) fn handle_move_plugin_to(
        &mut self,
        from: usize,
        to: usize,
        sender: &ComponentSender<Self>,
    ) {
        let mut guard = self.plugins.guard();
        let len = guard.len();
        if from >= len || to >= len || from == to {
            return;
        }
        if from < to {
            for i in from..to {
                guard.swap(i, i + 1);
            }
        } else {
            for i in (to..from).rev() {
                guard.swap(i, i + 1);
            }
        }
        drop(guard);
        self.needs_deploy = true;
        if self.plugin_selection_active {
            self.plugin_selection_dirty = true;
        }
        self.refresh_plugin_order_labels();
        self.save_plugin_order(sender);
    }

    pub(crate) fn handle_move_selected_plugins_to(
        &mut self,
        selected: Vec<usize>,
        from: usize,
        to: usize,
        sender: &ComponentSender<Self>,
    ) {
        let len = self.plugins.guard().len();
        let n = selected.len();
        if n == 0 || to >= len {
            return;
        }
        let drag_pos = selected.iter().position(|&s| s == from).unwrap_or(0);
        let anchor = to.saturating_sub(drag_pos).min(len.saturating_sub(n));
        if selected.iter().enumerate().all(|(i, &s)| anchor + i == s) {
            return;
        }

        let items: Vec<PluginRowInit> = {
            let guard = self.plugins.guard();
            selected
                .iter()
                .filter_map(|&idx| {
                    guard.get(idx).map(|row| PluginRowInit {
                        plugin: row.plugin.clone(),
                        display_filename: row.display_filename.clone(),
                        mod_name: row.mod_name.clone(),
                        order_label: row.order_label.clone(),
                        missing_masters: row.missing_masters.clone(),
                        mod_enabled: row.mod_enabled,
                        dirty_info: row.dirty_info.clone(),
                        is_vanilla: row.is_vanilla,
                    })
                })
                .collect()
        };
        {
            let mut guard = self.plugins.guard();
            for &idx in selected.iter().rev() {
                guard.remove(idx);
            }
        }
        {
            let mut guard = self.plugins.guard();
            for (i, item) in items.into_iter().enumerate() {
                guard.insert(anchor + i, item);
            }
        }
        self.needs_deploy = true;
        if self.plugin_selection_active {
            self.plugin_selection_dirty = true;
        }
        self.refresh_plugin_order_labels();
        self.save_plugin_order(sender);
    }

    fn refresh_plugin_order_labels(&mut self) {
        let mut count = 0usize;
        let mut guard = self.plugins.guard();
        let len = guard.len();
        for i in 0..len {
            if let Some(row) = guard.get_mut(i)
                && !row.is_vanilla
            {
                count += 1;
                row.order_label = format!("#{count}");
            }
        }
    }

    pub(crate) fn handle_enable_all_plugins(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let profile_id = self
            .profiles
            .get(self.active_profile_idx)
            .map(|p| p.id.clone());
        {
            let mut guard = self.plugins.guard();
            for row in guard.iter_mut() {
                row.plugin.enabled = true;
            }
        }
        self.needs_deploy = true;
        sender.oneshot_command(async move {
            let result = async {
                tracker.set_all_plugins_enabled(&game.id, true).await?;
                if let Some(pid) = &profile_id {
                    tracker.save_to_profile(pid, &game.id).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            AppCmdMsg::PluginOrderSaved(result.await.map_err(|e| e.to_string()))
        });
    }

    pub(crate) fn handle_disable_all_plugins(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let profile_id = self
            .profiles
            .get(self.active_profile_idx)
            .map(|p| p.id.clone());
        {
            let mut guard = self.plugins.guard();
            for row in guard.iter_mut() {
                row.plugin.enabled = false;
            }
        }
        self.needs_deploy = true;
        sender.oneshot_command(async move {
            let result = async {
                tracker.set_all_plugins_enabled(&game.id, false).await?;
                if let Some(pid) = &profile_id {
                    tracker.save_to_profile(pid, &game.id).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            AppCmdMsg::PluginOrderSaved(result.await.map_err(|e| e.to_string()))
        });
    }

    pub(crate) fn handle_toggle_show_vanilla_plugins(&mut self) {
        self.show_vanilla_plugins = !self.show_vanilla_plugins;
        let mut guard = self.plugins.guard();
        if self.show_vanilla_plugins {
            for (i, filename) in self.vanilla_plugin_names.clone().iter().enumerate() {
                #[cfg(feature = "loot")]
                let dirty_info = self.dirty_plugins.get(&filename.to_lowercase()).cloned();
                #[cfg(not(feature = "loot"))]
                let dirty_info: Option<PluginDirtyInfo> = None;

                let mod_name = if self
                    .vanilla_derived_plugins
                    .contains(&filename.to_lowercase())
                {
                    "Vanilla / Modified".to_string()
                } else {
                    "Vanilla / DLC".to_string()
                };
                guard.insert(
                    i,
                    PluginRowInit {
                        plugin: Plugin {
                            id: format!("vanilla:{filename}"),
                            mod_id: String::new(),
                            filename: filename.clone(),
                            load_order: 9999,
                            enabled: true,
                        },
                        display_filename: filename.clone(),
                        mod_name,
                        order_label: String::new(),
                        missing_masters: vec![],
                        mod_enabled: true,
                        dirty_info,
                        is_vanilla: true,
                    },
                );
            }
        } else {
            for _ in 0..self.vanilla_plugin_names.len() {
                guard.remove(0);
            }
        }
    }

    pub(crate) fn handle_sort_with_loot(&mut self, sender: &ComponentSender<Self>) {
        let Some(game) = self.selected_game() else {
            return;
        };
        let game_id = game.id.clone();
        let game_path = game.path.clone();
        let data_subdir = game.data_subdir.clone();
        let local_data_path = game::plugins_txt_paths(game)
            .into_iter()
            .find_map(|p| p.parent().map(|d| d.to_path_buf()));
        let plugin_names: Vec<String> = {
            let guard = self.plugins.guard();
            let mut names: Vec<String> = guard
                .iter()
                .filter(|r| !r.is_vanilla)
                .map(|r| r.plugin.filename.clone())
                .collect();
            names.extend(self.vanilla_plugin_names.iter().cloned());
            names
        };

        #[cfg(feature = "loot")]
        {
            sender.oneshot_command(async move {
                AppCmdMsg::LootSortDone(
                    crate::core::loot_sort::sort_plugins(
                        &game_id,
                        game_path,
                        data_subdir,
                        plugin_names,
                        local_data_path,
                    )
                    .await
                    .map_err(|e| format!("{e:#}")),
                )
            });
        }
        #[cfg(not(feature = "loot"))]
        {
            let _ = (
                game_id,
                game_path,
                data_subdir,
                plugin_names,
                local_data_path,
            );
            self.show_toast("LOOT support is not enabled in this build");
        }
    }
}

// ─── AppCmdMsg handlers ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_cmd_plugin_order_saved(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.push_notification(&format!("Failed to save plugin order: {e}"));
        }
    }

    #[cfg(feature = "loot")]
    pub(crate) fn handle_cmd_loot_sort_done(
        &mut self,
        result: Result<(Vec<String>, HashMap<String, PluginDirtyInfo>), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((sorted_names, dirty)) => {
                let dirty_count = dirty.len();
                self.dirty_plugins = dirty;

                self.needs_deploy = true;
                self.show_toast("Load order sorted by LOOT — deploy to apply");

                if dirty_count > 0 {
                    self.show_toast(&format!(
                        "{dirty_count} plugin{} ha{} dirty edits — clean with xEdit",
                        if dirty_count == 1 { "" } else { "s" },
                        if dirty_count == 1 { "s" } else { "ve" },
                    ));
                }

                let updates: Vec<(String, i32)> = {
                    let guard = self.plugins.guard();
                    let id_map: std::collections::HashMap<String, String> = (0..guard.len())
                        .filter_map(|i| {
                            guard
                                .get(i)
                                .map(|r| (r.plugin.filename.to_lowercase(), r.plugin.id.clone()))
                        })
                        .collect();
                    sorted_names
                        .iter()
                        .enumerate()
                        .filter_map(|(new_idx, name)| {
                            id_map
                                .get(&name.to_lowercase())
                                .map(|id| (id.clone(), new_idx as i32))
                        })
                        .collect()
                };

                if let (Some(tracker), Some(game)) =
                    (self.tracker.clone(), self.selected_game().cloned())
                {
                    sender.oneshot_command(async move {
                        if !updates.is_empty()
                            && let Err(e) = tracker.update_plugin_order(&updates).await
                        {
                            return AppCmdMsg::PluginOrderSaved(Err(e.to_string()));
                        }
                        AppCmdMsg::ModsLoaded(load_game_data(&tracker, &game, false).await, true)
                    });
                }
            }
            Err(e) => {
                self.show_toast(&format!("LOOT sort failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_set_color_scheme(&mut self, idx: u32) {
        self.color_scheme_idx = idx;
        let scheme = match idx {
            1 => adw::ColorScheme::ForceLight,
            2 => adw::ColorScheme::ForceDark,
            _ => adw::ColorScheme::Default,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
        if let Some(tracker) = self.tracker.clone() {
            let val = idx.to_string();
            tokio::spawn(async move {
                let _ = tracker.set_setting("color_scheme", &val).await;
            });
        }
    }

    pub(crate) fn handle_enter_plugin_selection_mode(&mut self) {
        self.plugin_selection_active = true;
        self.plugin_selection_dirty = false;
        self.selected_plugins.clear();
        let mut g = self.plugins.guard();
        for row in g.iter_mut() {
            row.selection_mode = true;
            row.selected = false;
            row.drag_enabled.set(true);
        }
    }

    pub(crate) fn handle_exit_plugin_selection_mode(&mut self) {
        self.plugin_selection_active = false;
        self.plugin_selection_dirty = false;
        self.selected_plugins.clear();
        let mut g = self.plugins.guard();
        for row in g.iter_mut() {
            row.selection_mode = false;
            row.selected = false;
            row.drag_enabled.set(false);
        }
    }

    pub(crate) fn handle_toggle_plugin_row_selected(&mut self, idx: usize) {
        if !self.plugin_selection_active {
            return;
        }
        let mut g = self.plugins.guard();
        let Some(row) = g.get_mut(idx) else { return };
        if row.is_vanilla {
            return;
        }
        row.selected = !row.selected;
        if row.selected {
            self.selected_plugins.insert(idx);
        } else {
            self.selected_plugins.remove(&idx);
        }
    }

    pub(crate) fn handle_set_plugin_row_selected(&mut self, idx: usize, selected: bool) {
        if !self.plugin_selection_active {
            return;
        }
        let mut g = self.plugins.guard();
        let Some(row) = g.get_mut(idx) else { return };
        if row.is_vanilla || row.selected == selected {
            return;
        }
        row.selected = selected;
        if selected {
            self.selected_plugins.insert(idx);
        } else {
            self.selected_plugins.remove(&idx);
        }
    }

    pub(crate) fn handle_enable_selected_plugins(&mut self, sender: &ComponentSender<Self>) {
        if self.selected_plugins.is_empty() {
            return;
        }
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        let profile_id = self.profiles.get(self.active_profile_idx).map(|p| p.id.clone());

        let indices: Vec<usize> = self.selected_plugins.iter().copied().collect();
        let mut plugin_ids: Vec<String> = Vec::new();

        {
            let mut guard = self.plugins.guard();
            for &idx in &indices {
                let Some(row) = guard.get_mut(idx) else { continue };
                row.plugin.enabled = true;
                plugin_ids.push(row.plugin.id.clone());
            }
        }
        self.needs_deploy = true;
        self.plugin_selection_dirty = true;

        let _game_id = game.id.clone();
        sender.oneshot_command(async move {
            let result = async {
                for plugin_id in &plugin_ids {
                    tracker.toggle_plugin(plugin_id, true).await?;
                }
                if let Some(pid) = &profile_id {
                    tracker.save_to_profile(pid, &game.id).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            AppCmdMsg::PluginOrderSaved(result.await.map_err(|e| e.to_string()))
        });

        self.handle_exit_plugin_selection_mode();
    }

    pub(crate) fn handle_disable_selected_plugins(&mut self, sender: &ComponentSender<Self>) {
        if self.selected_plugins.is_empty() {
            return;
        }
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        let profile_id = self.profiles.get(self.active_profile_idx).map(|p| p.id.clone());

        let indices: Vec<usize> = self.selected_plugins.iter().copied().collect();
        let mut plugin_ids: Vec<String> = Vec::new();

        {
            let mut guard = self.plugins.guard();
            for &idx in &indices {
                let Some(row) = guard.get_mut(idx) else { continue };
                row.plugin.enabled = false;
                plugin_ids.push(row.plugin.id.clone());
            }
        }
        self.needs_deploy = true;
        self.plugin_selection_dirty = true;

        sender.oneshot_command(async move {
            let result = async {
                for plugin_id in &plugin_ids {
                    tracker.toggle_plugin(plugin_id, false).await?;
                }
                if let Some(pid) = &profile_id {
                    tracker.save_to_profile(pid, &game.id).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            AppCmdMsg::PluginOrderSaved(result.await.map_err(|e| e.to_string()))
        });

        self.handle_exit_plugin_selection_mode();
    }
}
