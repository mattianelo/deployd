use std::collections::HashMap;

use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game;
use crate::models::plugin::Plugin;
#[cfg(feature = "loot")]
use crate::models::plugin::PluginDirtyInfo;
use crate::ui::plugin_list::PluginRowInit;

use super::App;
use super::messages::AppCmdMsg;
use super::session::{GameLoadMode, load_game_data};
use super::types::PostLootAction;

impl App {
    pub(crate) fn handle_move_plugin_to(
        &mut self,
        from: usize,
        to: usize,
        sender: &ComponentSender<Self>,
    ) {
        let mut guard = self.plugins.rows.guard();
        let len = guard.len();
        if from >= len || to >= len || from == to {
            return;
        }
        if self.plugins.selection_active
            && self.plugins.selected.contains(&from)
            && self.plugins.selected.len() > 1
        {
            drop(guard);
            let mut selected: Vec<usize> = self.plugins.selected.iter().copied().collect();
            selected.sort_unstable();
            self.handle_move_selected_plugins_to(selected, from, to, sender);
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
        self.shell.needs_deploy = true;
        if self.plugins.selection_active {
            self.plugins.selection_dirty = true;
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
        let len = self.plugins.rows.guard().len();
        let selected: Vec<usize> = {
            let guard = self.plugins.rows.guard();
            selected
                .into_iter()
                .filter(|&idx| guard.get(idx).is_some_and(|row| !row.is_vanilla))
                .collect()
        };
        let n = selected.len();
        if n == 0 || to >= len {
            return;
        }
        let drag_pos = selected.iter().position(|&s| s == from).unwrap_or(0);
        let anchor = to.saturating_sub(drag_pos).min(len.saturating_sub(n));
        if selected.iter().enumerate().all(|(i, &s)| anchor + i == s) {
            return;
        }
        let vadj = self.plugins.scroll.vadjustment();
        let saved_pos = vadj.value();

        let items: Vec<PluginRowInit> = {
            let guard = self.plugins.rows.guard();
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
            let mut guard = self.plugins.rows.guard();
            for &idx in selected.iter().rev() {
                guard.remove(idx);
            }
        }
        {
            let mut guard = self.plugins.rows.guard();
            for (i, item) in items.into_iter().enumerate() {
                guard.insert(anchor + i, item);
            }
        }
        self.plugins.selected.clear();
        {
            let mut guard = self.plugins.rows.guard();
            for i in 0..guard.len() {
                let selected = i >= anchor && i < anchor + n;
                if let Some(row) = guard.get_mut(i) {
                    row.selected = selected;
                    row.selection_mode = self.plugins.selection_active;
                    row.drag_enabled.set(self.plugins.selection_active);
                }
                if selected {
                    self.plugins.selected.insert(i);
                }
            }
        }
        glib::idle_add_local_once(move || {
            vadj.set_value(saved_pos);
        });
        self.shell.needs_deploy = true;
        if self.plugins.selection_active {
            self.plugins.selection_dirty = true;
        }
        self.refresh_plugin_order_labels();
        self.save_plugin_order(sender);
    }

    fn refresh_plugin_order_labels(&mut self) {
        let mut count = 0usize;
        let mut guard = self.plugins.rows.guard();
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
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let profile_id = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|p| p.id.clone());
        {
            let mut guard = self.plugins.rows.guard();
            for row in guard.iter_mut() {
                row.plugin.enabled = true;
            }
        }
        self.shell.needs_deploy = true;
        sender.oneshot_command(async move {
            let result = async {
                tracker.set_all_plugins_enabled(&game.id, true).await?;
                if let Some(pid) = &profile_id {
                    tracker.save_to_profile(pid, &game.id).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            AppCmdMsg::Plugins(crate::app::messages::PluginsCmdMsg::PluginOrderSaved(
                result.await.map_err(|e| e.to_string()),
            ))
        });
    }

    pub(crate) fn handle_disable_all_plugins(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let profile_id = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|p| p.id.clone());
        {
            let mut guard = self.plugins.rows.guard();
            for row in guard.iter_mut() {
                row.plugin.enabled = false;
            }
        }
        self.shell.needs_deploy = true;
        sender.oneshot_command(async move {
            let result = async {
                tracker.set_all_plugins_enabled(&game.id, false).await?;
                if let Some(pid) = &profile_id {
                    tracker.save_to_profile(pid, &game.id).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            AppCmdMsg::Plugins(crate::app::messages::PluginsCmdMsg::PluginOrderSaved(
                result.await.map_err(|e| e.to_string()),
            ))
        });
    }

    pub(crate) fn handle_toggle_show_vanilla_plugins(&mut self) {
        self.plugins.show_vanilla = !self.plugins.show_vanilla;
        let mut guard = self.plugins.rows.guard();
        if self.plugins.show_vanilla {
            for (i, filename) in self.plugins.vanilla_names.clone().iter().enumerate() {
                #[cfg(feature = "loot")]
                let dirty_info = self.plugins.dirty.get(&filename.to_lowercase()).cloned();
                #[cfg(not(feature = "loot"))]
                let dirty_info: Option<PluginDirtyInfo> = None;

                let mod_name = if self
                    .plugins
                    .vanilla_derived
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
            for _ in 0..self.plugins.vanilla_names.len() {
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
            let guard = self.plugins.rows.guard();
            let mut names: Vec<String> = guard
                .iter()
                .filter(|r| !r.is_vanilla)
                .map(|r| r.plugin.filename.clone())
                .collect();
            names.extend(self.plugins.vanilla_names.iter().cloned());
            names
        };

        #[cfg(feature = "loot")]
        {
            let result_game_id = game_id.clone();
            sender.oneshot_command(async move {
                AppCmdMsg::Plugins(crate::app::messages::PluginsCmdMsg::LootSortDone(
                    result_game_id,
                    crate::core::loot_sort::sort_plugins(
                        &game_id,
                        game_path,
                        data_subdir,
                        plugin_names,
                        local_data_path,
                    )
                    .await
                    .map_err(|e| format!("{e:#}")),
                ))
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
        game_id: String,
        result: Result<(Vec<String>, HashMap<String, PluginDirtyInfo>), String>,
        sender: &ComponentSender<Self>,
    ) {
        if self.selected_game().map(|game| game.id.as_str()) != Some(game_id.as_str()) {
            self.plugins.pending_post_loot_action = PostLootAction::None;
            self.push_notification(
                "LOOT finished after the selected game changed; its result was not applied",
            );
            return;
        }
        match result {
            Ok((sorted_names, dirty)) => {
                let dirty_count = dirty.len();
                self.plugins.dirty = dirty;
                let post_action = std::mem::take(&mut self.plugins.pending_post_loot_action);

                self.shell.needs_deploy = true;
                if post_action == PostLootAction::Deploy {
                    self.show_toast("Load order sorted by LOOT — deploying…");
                } else {
                    self.show_toast("Load order sorted by LOOT — deploy to apply");
                }

                if dirty_count > 0 {
                    self.show_toast(&format!(
                        "{dirty_count} plugin{} ha{} dirty edits — clean with xEdit",
                        if dirty_count == 1 { "" } else { "s" },
                        if dirty_count == 1 { "s" } else { "ve" },
                    ));
                }

                let updates: Vec<(String, i32)> = {
                    let guard = self.plugins.rows.guard();
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
                    (self.session.tracker.clone(), self.selected_game().cloned())
                {
                    sender.oneshot_command(async move {
                        let result = async {
                            if !updates.is_empty() {
                                tracker
                                    .update_plugin_order(&updates)
                                    .await
                                    .map_err(|error| error.to_string())?;
                            }
                            load_game_data(&tracker, &game, GameLoadMode::Refresh).await
                        }
                        .await;
                        AppCmdMsg::Plugins(crate::app::messages::PluginsCmdMsg::LootOrderApplied(
                            Box::new(result),
                            post_action,
                        ))
                    });
                } else {
                    self.push_notification(
                        "LOOT sorted the load order, but the game could not be reloaded",
                    );
                }
            }
            Err(e) => {
                self.plugins.pending_post_loot_action = PostLootAction::None;
                self.show_toast(&format!("LOOT sort failed: {e}"));
            }
        }
    }

    #[cfg(feature = "loot")]
    pub(crate) fn handle_cmd_loot_order_applied(
        &mut self,
        result: Result<super::types::LoadedData, String>,
        post_action: PostLootAction,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                self.apply_loaded_data(data, sender);
                if post_action == PostLootAction::Deploy {
                    self.execute_deploy(sender);
                }
            }
            Err(error) => {
                self.push_notification(&format!("Failed to apply LOOT order: {error}"));
            }
        }
    }

    pub(crate) fn handle_enter_plugin_selection_mode(&mut self) {
        self.plugins.selection_active = true;
        self.plugins.selection_dirty = false;
        self.plugins.selected.clear();
        let mut g = self.plugins.rows.guard();
        for row in g.iter_mut() {
            row.selection_mode = true;
            row.selected = false;
            row.drag_enabled.set(true);
        }
    }

    pub(crate) fn handle_exit_plugin_selection_mode(&mut self) {
        self.plugins.selection_active = false;
        self.plugins.selection_dirty = false;
        self.plugins.selected.clear();
        let mut g = self.plugins.rows.guard();
        for row in g.iter_mut() {
            row.selection_mode = false;
            row.selected = false;
            row.drag_enabled.set(false);
        }
    }

    pub(crate) fn handle_toggle_plugin_row_selected(&mut self, idx: usize) {
        if !self.plugins.selection_active {
            return;
        }
        let mut g = self.plugins.rows.guard();
        let Some(row) = g.get_mut(idx) else { return };
        if row.is_vanilla {
            return;
        }
        row.selected = !row.selected;
        if row.selected {
            self.plugins.selected.insert(idx);
        } else {
            self.plugins.selected.remove(&idx);
        }
    }

    pub(crate) fn handle_set_plugin_row_selected(&mut self, idx: usize, selected: bool) {
        if !self.plugins.selection_active {
            return;
        }
        let mut g = self.plugins.rows.guard();
        let Some(row) = g.get_mut(idx) else { return };
        if row.is_vanilla || row.selected == selected {
            return;
        }
        row.selected = selected;
        if selected {
            self.plugins.selected.insert(idx);
        } else {
            self.plugins.selected.remove(&idx);
        }
    }

    pub(crate) fn handle_enable_selected_plugins(&mut self, sender: &ComponentSender<Self>) {
        if self.plugins.selected.is_empty() {
            return;
        }
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let profile_id = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|p| p.id.clone());

        let indices: Vec<usize> = self.plugins.selected.iter().copied().collect();
        let mut plugin_ids: Vec<String> = Vec::new();

        {
            let mut guard = self.plugins.rows.guard();
            for &idx in &indices {
                let Some(row) = guard.get_mut(idx) else {
                    continue;
                };
                row.plugin.enabled = true;
                plugin_ids.push(row.plugin.id.clone());
            }
        }
        self.shell.needs_deploy = true;
        self.plugins.selection_dirty = true;

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
            AppCmdMsg::Plugins(crate::app::messages::PluginsCmdMsg::PluginOrderSaved(
                result.await.map_err(|e| e.to_string()),
            ))
        });

        self.handle_exit_plugin_selection_mode();
    }

    pub(crate) fn handle_disable_selected_plugins(&mut self, sender: &ComponentSender<Self>) {
        if self.plugins.selected.is_empty() {
            return;
        }
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let profile_id = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|p| p.id.clone());

        let indices: Vec<usize> = self.plugins.selected.iter().copied().collect();
        let mut plugin_ids: Vec<String> = Vec::new();

        {
            let mut guard = self.plugins.rows.guard();
            for &idx in &indices {
                let Some(row) = guard.get_mut(idx) else {
                    continue;
                };
                row.plugin.enabled = false;
                plugin_ids.push(row.plugin.id.clone());
            }
        }
        self.shell.needs_deploy = true;
        self.plugins.selection_dirty = true;

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
            AppCmdMsg::Plugins(crate::app::messages::PluginsCmdMsg::PluginOrderSaved(
                result.await.map_err(|e| e.to_string()),
            ))
        });

        self.handle_exit_plugin_selection_mode();
    }
}
