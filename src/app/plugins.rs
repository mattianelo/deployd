use std::collections::HashMap;

use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::core::game;
use crate::models::plugin::Plugin;
#[cfg(feature = "loot")]
use crate::models::plugin::PluginDirtyInfo;
use crate::ui::plugin_list::PluginRowInit;

use super::free_fns::{check_order_violates_masters, load_game_data};
use super::messages::AppCmdMsg;
use super::App;

impl App {
    pub(crate) fn handle_move_plugin_to(
        &mut self,
        from: usize,
        to: usize,
        sender: &ComponentSender<Self>,
    ) {
        let order: Vec<(String, String)> = {
            let guard = self.plugins.guard();
            let len = guard.len();
            if from >= len || to >= len || from == to {
                return;
            }
            (0..len)
                .filter_map(|i| {
                    guard
                        .get(i)
                        .map(|row| (row.plugin.id.clone(), row.plugin.filename.clone()))
                })
                .collect()
        };

        let mut new_order = order.clone();
        let p = new_order.remove(from);
        new_order.insert(to, p);

        if let Some(master) = check_order_violates_masters(&new_order, &self.plugin_masters) {
            self.toaster
                .toast(&format!("Cannot move plugin: '{}' must load first", master));
            return;
        }

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

        {
            let order: Vec<(String, String)> = {
                let guard = self.plugins.guard();
                (0..guard.len())
                    .filter_map(|i| {
                        guard
                            .get(i)
                            .map(|row| (row.plugin.id.clone(), row.plugin.filename.clone()))
                    })
                    .collect()
            };
            let selected_set: std::collections::HashSet<usize> =
                selected.iter().copied().collect();
            let mut new_order: Vec<(String, String)> = order
                .iter()
                .enumerate()
                .filter(|(i, _)| !selected_set.contains(i))
                .map(|(_, v)| v.clone())
                .collect();
            for (i, &idx) in selected.iter().enumerate() {
                if idx < order.len() {
                    new_order.insert(anchor + i, order[idx].clone());
                }
            }
            if let Some(master) =
                check_order_violates_masters(&new_order, &self.plugin_masters)
            {
                self.toaster
                    .toast(&format!("Cannot move plugin: '{}' must load first", master));
                return;
            }
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
        self.save_plugin_order(sender);
    }

    pub(crate) fn handle_toggle_plugin_enabled(
        &mut self,
        index: DynamicIndex,
        enabled: bool,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let plugin_id = {
            let guard = self.plugins.guard();
            let Some(row) = guard.get(idx) else { return };
            row.plugin.id.clone()
        };

        {
            let mut guard = self.plugins.guard();
            if let Some(row) = guard.get_mut(idx) {
                row.plugin.enabled = enabled;
            }
        }

        self.needs_deploy = true;

        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::PluginOrderSaved(
                    tracker
                        .toggle_plugin(&plugin_id, enabled)
                        .await
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn handle_enable_all_plugins(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
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
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
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
                        mod_name: "Vanilla / DLC".to_string(),
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
        let Some(game) = self.selected_game() else { return };
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
            let _ = (game_id, game_path, data_subdir, plugin_names, local_data_path);
            self.toaster
                .toast("LOOT support is not enabled in this build");
        }
    }
}

// ─── AppCmdMsg handlers ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_cmd_plugin_order_saved(&mut self, result: Result<(), String>) {
        if let Err(e) = result {
            self.toaster
                .toast(&format!("Failed to save plugin order: {e}"));
        }
    }

    #[cfg(feature = "loot")]
    pub(crate) fn handle_cmd_loot_sort_done(
        &mut self,
        result: Result<
            (Vec<String>, HashMap<String, PluginDirtyInfo>),
            String,
        >,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((sorted_names, dirty)) => {
                let dirty_count = dirty.len();
                self.dirty_plugins = dirty;

                if dirty_count > 0 {
                    self.toaster.toast(&format!(
                        "{dirty_count} plugin{} ha{} dirty edits — clean with xEdit",
                        if dirty_count == 1 { "" } else { "s" },
                        if dirty_count == 1 { "s" } else { "ve" },
                    ));
                }

                let updates: Vec<(String, i32)> = {
                    let guard = self.plugins.guard();
                    let id_map: std::collections::HashMap<String, String> = (0..guard.len())
                        .filter_map(|i| {
                            guard.get(i).map(|r| {
                                (r.plugin.filename.to_lowercase(), r.plugin.id.clone())
                            })
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
                        AppCmdMsg::ModsLoaded(load_game_data(&tracker, &game, false).await)
                    });
                }
            }
            Err(e) => {
                self.toaster.toast(&format!("LOOT sort failed: {e}"));
            }
        }
    }
}
