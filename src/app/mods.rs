use std::collections::{HashMap, HashSet};

use gtk::prelude::*;
use gtk::glib;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::core::game;
use crate::core::mod_folders;
use crate::core::tracker::OverrideInfo;
use crate::models::game::Game;
use crate::models::group::ModGroup;
use crate::models::mod_entry::ModEntry;
use crate::models::plugin::Plugin;
use crate::models::profile::Profile;
use crate::ui::mod_list::{ModListItemInit, ModListItemKind, ModRowInit};
use crate::ui::mod_properties_dialog::{
    ModPropertiesDialog, ModPropertiesInit, ModPropertiesOutput,
};
use crate::ui::plugin_list::PluginRowInit;
use crate::utils::paths;

use super::free_fns::load_game_data;
use super::messages::{AppCmdMsg, AppMsg};
use super::types::LoadedData;
use super::App;

// ─── State helpers ───────────────────────────────────────────────────────────

impl App {
    /// In-session reload: recomputes conflict overrides and refreshes the mod/plugin
    /// lists from DB without re-syncing Plugins.txt.
    pub(crate) fn reload_mods(&self, sender: &ComponentSender<Self>) {
        self.reload_mods_impl(sender, false);
    }

    /// Full reload including a Plugins.txt sync. Use only on initial game select.
    pub(crate) fn reload_mods_full(&self, sender: &ComponentSender<Self>) {
        self.reload_mods_impl(sender, true);
    }

    fn reload_mods_impl(&self, sender: &ComponentSender<Self>, sync_txt: bool) {
        if let (Some(tracker), Some(game)) = (self.tracker.clone(), self.selected_game().cloned()) {
            sender.oneshot_command(async move {
                let result = async { load_game_data(&tracker, &game, sync_txt).await };
                AppCmdMsg::ModsLoaded(result.await)
            });
        }
    }

    pub(crate) fn populate_mods(
        &mut self,
        mods: Vec<ModEntry>,
        groups: &[ModGroup],
        overrides: &HashMap<String, OverrideInfo>,
    ) {
        let mut sorted_groups = groups.to_vec();
        sorted_groups.sort_by(|a, b| {
            a.position
                .partial_cmp(&b.position)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let vadj = self.mod_scroll.vadjustment();
        let saved_pos = vadj.value();

        let mut guard = self.mods.guard();
        guard.clear();

        let mut group_idx = 0;
        let mut mod_display_idx = 0usize;

        for (seq, m) in mods.into_iter().enumerate() {
            while group_idx < sorted_groups.len() && sorted_groups[group_idx].position <= seq as f64
            {
                let g = &sorted_groups[group_idx];
                let collapsed = self.collapsed_groups.contains(&g.id);
                guard.push_back(ModListItemInit {
                    kind: ModListItemKind::Separator {
                        group_id: g.id.clone(),
                        name: g.name.clone(),
                        collapsed,
                    },
                });
                group_idx += 1;
            }

            let visible = {
                let guard_len = guard.len();
                let mut in_collapsed = false;
                for back in (0..guard_len).rev() {
                    if let Some(item) = guard.get(back)
                        && item.is_separator()
                    {
                        in_collapsed = item.is_collapsed();
                        break;
                    }
                }
                !in_collapsed
            };

            let info = overrides.get(&m.id);
            mod_display_idx += 1;
            guard.push_back(ModListItemInit {
                kind: ModListItemKind::Mod(ModRowInit {
                    mod_entry: m,
                    priority_label: format!("#{mod_display_idx}"),
                    overrides: info.map_or(0, |i| i.overrides),
                    overridden_by: info.map_or(0, |i| i.overridden_by),
                    override_files: info.map_or_else(Vec::new, |i| i.override_files.clone()),
                    overridden_files: info.map_or_else(Vec::new, |i| i.overridden_files.clone()),
                }),
            });
            let last = guard.len() - 1;
            if let Some(item) = guard.get_mut(last) {
                item.visible = visible;
            }
        }

        while group_idx < sorted_groups.len() {
            let g = &sorted_groups[group_idx];
            let collapsed = self.collapsed_groups.contains(&g.id);
            guard.push_back(ModListItemInit {
                kind: ModListItemKind::Separator {
                    group_id: g.id.clone(),
                    name: g.name.clone(),
                    collapsed,
                },
            });
            group_idx += 1;
        }
        drop(guard);

        glib::idle_add_local_once(move || {
            vadj.set_value(saved_pos);
        });
    }

    pub(crate) fn populate_plugins(
        &mut self,
        plugins: Vec<Plugin>,
        mods: &[ModEntry],
        plugin_masters: &HashMap<String, Vec<String>>,
        vanilla_plugins: &HashSet<String>,
    ) {
        let managed_lower: HashSet<String> =
            plugins.iter().map(|p| p.filename.to_lowercase()).collect();

        let mut installed = managed_lower.clone();
        installed.extend(vanilla_plugins.iter().map(|n| n.to_lowercase()));

        let mut vanilla_sorted: Vec<String> = vanilla_plugins
            .iter()
            .filter(|name| !managed_lower.contains(&name.to_lowercase()))
            .cloned()
            .collect();
        vanilla_sorted.sort_by(|a, b| {
            let tier = |n: &str| {
                let lower = n.to_lowercase();
                if lower.ends_with(".esm") {
                    0u8
                } else if lower.ends_with(".esl") {
                    1
                } else {
                    2
                }
            };
            tier(a)
                .cmp(&tier(b))
                .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
        });
        self.vanilla_plugin_names = vanilla_sorted;

        let mut guard = self.plugins.guard();
        guard.clear();

        if self.show_vanilla_plugins {
            for filename in &self.vanilla_plugin_names {
                #[cfg(feature = "loot")]
                let dirty_info = self.dirty_plugins.get(&filename.to_lowercase()).cloned();
                #[cfg(not(feature = "loot"))]
                let dirty_info: Option<PluginDirtyInfo> = None;

                guard.push_back(PluginRowInit {
                    plugin: Plugin {
                        id: format!("vanilla:{filename}"),
                        mod_id: String::new(),
                        filename: filename.clone(),
                        load_order: 9999,
                        enabled: true,
                    },
                    mod_name: "Vanilla / DLC".to_string(),
                    order_label: String::new(),
                    missing_masters: vec![],
                    mod_enabled: true,
                    dirty_info,
                    is_vanilla: true,
                });
            }
        }

        for (i, p) in plugins.into_iter().enumerate() {
            let mod_name = mods
                .iter()
                .find(|m| m.id == p.mod_id)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| "Unknown".to_string());
            let missing_masters: Vec<String> = plugin_masters
                .get(&p.id)
                .map(|masters| {
                    masters
                        .iter()
                        .filter(|m| !installed.contains(&m.to_lowercase()))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            let mod_enabled = mods
                .iter()
                .find(|m| m.id == p.mod_id)
                .map(|m| m.enabled)
                .unwrap_or(true);
            #[cfg(feature = "loot")]
            let dirty_info = self.dirty_plugins.get(&p.filename.to_lowercase()).cloned();
            #[cfg(not(feature = "loot"))]
            let dirty_info: Option<PluginDirtyInfo> = None;

            guard.push_back(PluginRowInit {
                plugin: p,
                mod_name,
                order_label: format!("#{}", i + 1),
                missing_masters,
                mod_enabled,
                dirty_info,
                is_vanilla: false,
            });
        }

        self.managed_plugins_count = guard.len()
            - if self.show_vanilla_plugins {
                self.vanilla_plugin_names.len()
            } else {
                0
            };
    }

    pub(crate) fn update_profile_list(&mut self, profiles: Vec<Profile>, active_idx: usize) {
        self.updating_profiles = true;
        self.profiles = profiles;
        self.active_profile_idx = active_idx;

        let names: Vec<&str> = self.profiles.iter().map(|p| p.name.as_str()).collect();
        self.profile_model
            .splice(0, self.profile_model.n_items(), &names);
        self.profile_dropdown.set_selected(active_idx as u32);
        if let Some(p) = self.profiles.get(active_idx) {
            self.profile_rename_entry.set_text(&p.name);
        }
        self.updating_profiles = false;
    }

    pub(crate) fn apply_loaded_data(&mut self, data: LoadedData, sender: &ComponentSender<Self>) {
        self.populate_plugins(
            data.plugins,
            &data.mods,
            &data.plugin_masters,
            &data.vanilla_plugins,
        );
        self.populate_mods(data.mods, &data.groups, &data.overrides);
        self.update_profile_list(data.profiles, data.active_profile_idx);
        self.tools = data.tools;
        self.plugin_masters = data.plugin_masters;
        self.rebuild_tool_buttons(sender);
    }

    pub(crate) fn save_group_positions(&mut self) {
        let guard = self.mods.guard();
        let mut updates: Vec<(String, f64)> = Vec::new();
        let mut mod_count = 0usize;
        for i in 0..guard.len() {
            if let Some(item) = guard.get(i) {
                if let ModListItemKind::Separator { group_id, .. } = &item.kind {
                    updates.push((group_id.clone(), mod_count as f64));
                } else {
                    mod_count += 1;
                }
            }
        }
        drop(guard);

        if updates.is_empty() {
            return;
        }
        if let Some(tracker) = self.tracker.clone() {
            tokio::spawn(async move {
                for (group_id, position) in updates {
                    let _ = tracker.move_group(&group_id, position).await;
                }
            });
        }
    }

    pub(crate) fn save_mod_priorities(&mut self, sender: &ComponentSender<Self>) {
        let guard = self.mods.guard();
        let updates: Vec<(String, i32)> = (0..guard.len())
            .filter_map(|i| {
                guard
                    .get(i)
                    .and_then(|row| row.mod_id().map(|id| (id.to_string(), i as i32)))
            })
            .collect();
        drop(guard);

        if let Some(tracker) = self.tracker.clone() {
            let game_id = self.selected_game().map(|g| g.id.clone());
            sender.oneshot_command(async move {
                let result = tracker
                    .update_priorities(&updates)
                    .await
                    .map_err(|e| e.to_string());
                if result.is_ok() {
                    if let Some(ref gid) = game_id {
                        if let Err(e) = mod_folders::refresh_named_mod_folders(&tracker, gid).await {
                            eprintln!("[deployd] named_mods refresh failed: {e}");
                        }
                    }
                }
                AppCmdMsg::PrioritySaved(result)
            });
        }
    }

    pub(crate) fn save_plugin_order(&mut self, sender: &ComponentSender<Self>) {
        let guard = self.plugins.guard();
        let updates: Vec<(String, i32)> = (0..guard.len())
            .filter_map(|i| guard.get(i).map(|row| (row.plugin.id.clone(), i as i32)))
            .collect();
        drop(guard);

        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::PluginOrderSaved(
                    tracker
                        .update_plugin_order(&updates)
                        .await
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn auto_save_profile(&self, sender: &ComponentSender<Self>) {
        if let (Some(tracker), Some(game)) = (self.tracker.clone(), self.selected_game())
            && let Some(profile) = self.profiles.get(self.active_profile_idx)
        {
            let profile_id = profile.id.clone();
            let game_id = game.id.clone();
            sender.oneshot_command(async move {
                let _ = tracker.save_to_profile(&profile_id, &game_id).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
    }
}

// ─── AppMsg handlers ─────────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_game_selected(
        &mut self,
        idx: u32,
        sender: &ComponentSender<Self>,
    ) {
        let new_idx = idx as usize;
        if new_idx == self.selected_game_idx {
            return;
        }
        self.selected_game_idx = new_idx;
        self.pending_external_files.clear();
        self.external_changes_count = 0;
        #[cfg(feature = "loot")]
        self.dirty_plugins.clear();
        if let (Some(tracker), Some(game)) = (self.tracker.clone(), self.games.get(new_idx)) {
            let game_id = game.id.clone();
            sender.oneshot_command(async move {
                let _ = tracker.set_setting("last_game_id", &game_id).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
        self.reload_mods_full(sender);
        self.rebuild_downloads_view();
    }

    pub(crate) fn handle_remove_mod(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (mod_id, removed_nexus_ids, removed_mod_name, removed_archive_hash) = {
            let guard = self.mods.guard();
            let Some(row) = guard.get(idx) else { return };
            let Some(init) = row.mod_row() else { return };
            let id = init.mod_entry.id.clone();
            let nids = init.mod_entry.nexus_mod_id.zip(init.mod_entry.nexus_file_id);
            let name = init.mod_entry.name.clone();
            let hash = init.mod_entry.archive_hash.clone();
            (id, nids, name, hash)
        };

        let Some(tracker) = self.tracker.clone() else { return };

        self.mods.guard().remove(idx);
        self.needs_deploy = true;
        self.save_group_positions();

        sender.oneshot_command(async move {
            let result: Result<String, String> = async {
                tracker
                    .delete_plugins_for_mod(&mod_id)
                    .await
                    .map_err(|e| e.to_string())?;
                tracker
                    .delete_mod_files(&mod_id)
                    .await
                    .map_err(|e| e.to_string())?;
                tracker
                    .delete_mod(&mod_id)
                    .await
                    .map_err(|e| e.to_string())?;
                if let Ok(cache) = paths::mod_cache_dir(&mod_id)
                    && cache.exists()
                {
                    let _ = std::fs::remove_dir_all(&cache);
                }
                Ok(mod_id)
            }
            .await;
            AppCmdMsg::ModRemoved(
                result,
                removed_nexus_ids,
                removed_mod_name,
                removed_archive_hash,
            )
        });
    }

    pub(crate) fn handle_toggle_mod_enabled(
        &mut self,
        index: DynamicIndex,
        enabled: bool,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let mod_id = {
            let guard = self.mods.guard();
            let Some(row) = guard.get(idx) else { return };
            let Some(id) = row.mod_id() else { return };
            id.to_string()
        };

        {
            let mut guard = self.mods.guard();
            if let Some(row) = guard.get_mut(idx)
                && let Some(entry) = row.mod_entry_mut()
            {
                entry.enabled = enabled;
            }
        }

        {
            let mut guard = self.plugins.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i)
                    && row.plugin.mod_id == mod_id
                {
                    row.mod_enabled = enabled;
                }
            }
        }

        self.needs_deploy = true;

        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::PrioritySaved(
                    tracker
                        .toggle_mod(&mod_id, enabled)
                        .await
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn handle_move_mod_to(
        &mut self,
        from: usize,
        to: usize,
        sender: &ComponentSender<Self>,
    ) {
        let mut guard = self.mods.guard();
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
        self.save_group_positions();
        self.save_mod_priorities(sender);
    }

    pub(crate) fn handle_move_group_to(
        &mut self,
        from: usize,
        to: usize,
        sender: &ComponentSender<Self>,
    ) {
        let mut guard = self.mods.guard();
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
        self.save_group_positions();
        self.save_mod_priorities(sender);
    }

    pub(crate) fn handle_move_selected_mods_to(
        &mut self,
        selected: Vec<usize>,
        from: usize,
        to: usize,
        sender: &ComponentSender<Self>,
    ) {
        let len = self.mods.guard().len();
        let n = selected.len();
        if n == 0 || to >= len {
            return;
        }
        let drag_pos = selected.iter().position(|&s| s == from).unwrap_or(0);
        let anchor = to.saturating_sub(drag_pos).min(len.saturating_sub(n));
        if selected.iter().enumerate().all(|(i, &s)| anchor + i == s) {
            return;
        }

        let items: Vec<ModListItemInit> = {
            let guard = self.mods.guard();
            selected
                .iter()
                .filter_map(|&idx| {
                    guard.get(idx).and_then(|row| row.mod_row()).map(|init| {
                        ModListItemInit {
                            kind: ModListItemKind::Mod(ModRowInit {
                                mod_entry: init.mod_entry.clone(),
                                priority_label: init.priority_label.clone(),
                                overrides: init.overrides,
                                overridden_by: init.overridden_by,
                                override_files: init.override_files.clone(),
                                overridden_files: init.overridden_files.clone(),
                            }),
                        }
                    })
                })
                .collect()
        };
        {
            let mut guard = self.mods.guard();
            for &idx in selected.iter().rev() {
                guard.remove(idx);
            }
        }
        {
            let mut guard = self.mods.guard();
            for (i, item) in items.into_iter().enumerate() {
                guard.insert(anchor + i, item);
            }
        }
        self.needs_deploy = true;
        self.save_group_positions();
        self.save_mod_priorities(sender);
    }

    pub(crate) fn handle_rescan_games(&mut self, sender: &ComponentSender<Self>) {
        sender.oneshot_command(async move {
            let games = tokio::task::spawn_blocking(game::detect_games)
                .await
                .unwrap_or_default();
            AppCmdMsg::GamesRescanned(games)
        });
    }

    pub(crate) fn handle_enable_all_mods(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        let profile_id = self
            .profiles
            .get(self.active_profile_idx)
            .map(|p| p.id.clone());
        {
            let mut guard = self.mods.guard();
            for item in guard.iter_mut() {
                if let crate::ui::mod_list::ModListItemKind::Mod(ref mut entry) = item.kind {
                    entry.mod_entry.enabled = true;
                }
            }
        }
        {
            let mut guard = self.plugins.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    row.mod_enabled = true;
                }
            }
        }
        self.needs_deploy = true;
        sender.oneshot_command(async move {
            let result = async {
                tracker.set_all_mods_enabled(&game.id, true).await?;
                if let Some(pid) = &profile_id {
                    tracker.save_to_profile(pid, &game.id).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            AppCmdMsg::PrioritySaved(result.await.map_err(|e| e.to_string()))
        });
    }

    pub(crate) fn handle_disable_all_mods(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        let profile_id = self
            .profiles
            .get(self.active_profile_idx)
            .map(|p| p.id.clone());
        {
            let mut guard = self.mods.guard();
            for item in guard.iter_mut() {
                if let crate::ui::mod_list::ModListItemKind::Mod(ref mut entry) = item.kind {
                    entry.mod_entry.enabled = false;
                }
            }
        }
        {
            let mut guard = self.plugins.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    row.mod_enabled = false;
                }
            }
        }
        self.needs_deploy = true;
        sender.oneshot_command(async move {
            let result = async {
                tracker.set_all_mods_enabled(&game.id, false).await?;
                if let Some(pid) = &profile_id {
                    tracker.save_to_profile(pid, &game.id).await?;
                }
                Ok::<(), anyhow::Error>(())
            };
            AppCmdMsg::PrioritySaved(result.await.map_err(|e| e.to_string()))
        });
    }

    pub(crate) fn handle_rename_mod(
        &mut self,
        index: DynamicIndex,
        new_name: String,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let mod_id = {
            let guard = self.mods.guard();
            let Some(row) = guard.get(idx) else { return };
            let Some(id) = row.mod_id() else { return };
            id.to_string()
        };

        {
            let mut guard = self.mods.guard();
            if let Some(row) = guard.get_mut(idx)
                && let Some(entry) = row.mod_entry_mut()
            {
                entry.name = new_name.clone();
            }
        }

        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::PrioritySaved(
                    tracker
                        .update_mod_name(&mod_id, &new_name)
                        .await
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn handle_toggle_group_collapse(&mut self, index: DynamicIndex) {
        let idx = index.current_index();
        let (group_id, new_collapsed) = {
            let guard = self.mods.guard();
            if let Some(item) = guard.get(idx)
                && let crate::ui::mod_list::ModListItemKind::Separator {
                    group_id, collapsed, ..
                } = &item.kind
            {
                (group_id.clone(), !collapsed)
            } else {
                return;
            }
        };

        if new_collapsed {
            self.collapsed_groups.insert(group_id.clone());
        } else {
            self.collapsed_groups.remove(&group_id);
        }

        {
            let mut guard = self.mods.guard();
            let len = guard.len();
            let mut in_toggled_group = false;
            for i in 0..len {
                if let Some(item) = guard.get_mut(i) {
                    if item.is_separator() {
                        if let crate::ui::mod_list::ModListItemKind::Separator {
                            group_id: ref gid,
                            ref mut collapsed,
                            ..
                        } = item.kind
                        {
                            if gid == &group_id {
                                *collapsed = new_collapsed;
                                in_toggled_group = true;
                            } else {
                                in_toggled_group = false;
                            }
                        }
                    } else if in_toggled_group {
                        item.visible = !new_collapsed;
                    }
                }
            }
        }

        if let Some(tracker) = self.tracker.clone() {
            let gid = group_id.clone();
            tokio::spawn(async move {
                let _ = tracker.set_group_collapsed(&gid, new_collapsed).await;
            });
        }
    }

    pub(crate) fn handle_delete_group(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let group_id = {
            let guard = self.mods.guard();
            if let Some(item) = guard.get(idx)
                && let crate::ui::mod_list::ModListItemKind::Separator { group_id, .. } =
                    &item.kind
            {
                group_id.clone()
            } else {
                return;
            }
        };
        self.collapsed_groups.remove(&group_id);

        if let (Some(tracker), Some(game)) =
            (self.tracker.clone(), self.selected_game().cloned())
        {
            sender.oneshot_command(async move {
                if let Err(e) = tracker.delete_group(&group_id).await {
                    eprintln!("Failed to delete group: {e}");
                }
                AppCmdMsg::ModsLoaded(load_game_data(&tracker, &game, false).await)
            });
        }
    }

    pub(crate) fn handle_create_group(
        &mut self,
        name: String,
        sender: &ComponentSender<Self>,
    ) {
        if let (Some(tracker), Some(game)) =
            (self.tracker.clone(), self.selected_game().cloned())
        {
            let position = {
                let guard = self.mods.guard();
                guard.len() as f64
            };
            sender.oneshot_command(async move {
                if let Err(e) = tracker.create_group(&game.id, &name, position).await {
                    eprintln!("Failed to create group: {e}");
                }
                AppCmdMsg::ModsLoaded(load_game_data(&tracker, &game, false).await)
            });
        }
    }

    pub(crate) fn handle_rename_group(&mut self, index: DynamicIndex, new_name: String) {
        let idx = index.current_index();
        let group_id = {
            let guard = self.mods.guard();
            if let Some(item) = guard.get(idx)
                && let crate::ui::mod_list::ModListItemKind::Separator { group_id, .. } =
                    &item.kind
            {
                group_id.clone()
            } else {
                return;
            }
        };

        {
            let mut guard = self.mods.guard();
            if let Some(item) = guard.get_mut(idx)
                && let crate::ui::mod_list::ModListItemKind::Separator { name, .. } =
                    &mut item.kind
            {
                *name = new_name.clone();
            }
        }

        if let Some(tracker) = self.tracker.clone() {
            tokio::spawn(async move {
                let _ = tracker.rename_group(&group_id, &new_name).await;
            });
        }
    }

    pub(crate) fn handle_open_mod_properties(
        &mut self,
        index: DynamicIndex,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let mod_entry = {
            let guard = self.mods.guard();
            if let Some(item) = guard.get(idx)
                && let crate::ui::mod_list::ModListItemKind::Mod(row) = &item.kind
            {
                row.mod_entry.clone()
            } else {
                return;
            }
        };
        let mod_id = mod_entry.id.clone();
        let mod_id_for_output = mod_id.clone();
        let is_bethesda = self
            .selected_game()
            .map(|g| g.engine == crate::models::game::GameEngine::Bethesda)
            .unwrap_or(true);
        self.mod_properties_dialog = Some(
            ModPropertiesDialog::builder()
                .transient_for(root)
                .launch(ModPropertiesInit {
                    mod_entry,
                    is_bethesda,
                })
                .forward(sender.input_sender(), move |output| match output {
                    ModPropertiesOutput::Applied {
                        name,
                        install_target,
                        file_targets,
                    } => AppMsg::ModPropertiesApplied {
                        mod_id: mod_id_for_output.clone(),
                        mod_idx: idx,
                        name,
                        install_target,
                        file_targets,
                    },
                    ModPropertiesOutput::Cancelled => AppMsg::ModPropertiesCancelled,
                    ModPropertiesOutput::ScanCache { mod_id } => {
                        AppMsg::ScanModFromCache(mod_id)
                    }
                }),
        );
        let Some(tracker) = self.tracker.clone() else { return };
        let mod_id_for_load = mod_id;
        sender.oneshot_command(async move {
            let files = tracker
                .get_mod_files(&mod_id_for_load)
                .await
                .unwrap_or_default();
            AppCmdMsg::ModFilesLoaded(files)
        });
    }

    pub(crate) fn handle_mod_properties_applied(
        &mut self,
        mod_id: String,
        mod_idx: usize,
        name: String,
        install_target: crate::models::mod_entry::InstallTarget,
        file_targets: HashMap<String, crate::models::mod_entry::InstallTarget>,
    ) {
        self.mod_properties_dialog = None;
        let Some(tracker) = self.tracker.clone() else { return };

        {
            let mut guard = self.mods.guard();
            if let Some(item) = guard.get_mut(mod_idx)
                && let crate::ui::mod_list::ModListItemKind::Mod(row) = &mut item.kind
            {
                row.mod_entry.name = name.clone();
                row.mod_entry.install_target = install_target.clone();
            }
        }

        let mod_id_clone = mod_id.clone();
        let name_clone = name.clone();
        let install_target_clone = install_target.clone();
        tokio::spawn(async move {
            let _ = tracker.update_mod_name(&mod_id_clone, &name_clone).await;
            let _ = tracker
                .update_file_targets(&mod_id_clone, &file_targets)
                .await;
            let _ = tracker
                .set_mod_install_target_column(&mod_id_clone, &install_target_clone)
                .await;
        });

        self.needs_deploy = true;
        self.toaster
            .toast(&format!("Properties updated for {name}"));
    }

    pub(crate) fn handle_mod_properties_cancelled(&mut self) {
        self.mod_properties_dialog = None;
    }

    pub(crate) fn handle_scan_mod_from_cache(
        &mut self,
        mod_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else { return };
        let mod_name = self.mod_name_for_id(&mod_id);
        sender.oneshot_command(async move {
            let result: Result<String, String> = async {
                let cache_dir = crate::utils::paths::mod_cache_dir(&mod_id)
                    .map_err(|e| e.to_string())?;
                let mut files = Vec::new();
                if cache_dir.is_dir() {
                    for entry in walkdir::WalkDir::new(&cache_dir).min_depth(1) {
                        let entry = entry.map_err(|e| e.to_string())?;
                        if !entry.file_type().is_file() {
                            continue;
                        }
                        let rel = entry
                            .path()
                            .strip_prefix(&cache_dir)
                            .map_err(|e| e.to_string())?;
                        let game_rel_original = rel.to_string_lossy().replace('\\', "/");
                        let game_rel_lowercase = game_rel_original.to_lowercase();
                        let cache_path = entry.path().to_string_lossy().to_string();
                        files.push(crate::models::manifest::ModFile {
                            mod_id: mod_id.clone(),
                            game_rel_lowercase,
                            game_rel_original,
                            cache_path,
                        });
                    }
                }
                tracker
                    .delete_mod_files(&mod_id)
                    .await
                    .map_err(|e| e.to_string())?;
                tracker
                    .record_files(&files)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(format!("{} — {} file(s) registered", mod_name, files.len()))
            }
            .await;
            AppCmdMsg::ModFilesRescanned(result)
        });
    }

    pub(crate) fn handle_create_empty_mod(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else { return };
        let Some(game) = self.selected_game().cloned() else { return };
        let game_id = game.id.clone();
        sender.oneshot_command(async move {
            let result: Result<(String, std::path::PathBuf), String> = async {
                let mod_id = uuid::Uuid::new_v4().to_string();
                let cache_dir = crate::utils::paths::mod_cache_dir(&mod_id)
                    .map_err(|e| e.to_string())?;
                std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
                let priority = tracker
                    .next_priority(&game_id)
                    .await
                    .map_err(|e| e.to_string())?;
                let now = chrono::Utc::now().to_rfc3339();
                let entry = crate::models::mod_entry::ModEntry {
                    id: mod_id.clone(),
                    game_id,
                    name: "New Mod".to_string(),
                    archive_hash: None,
                    installed_at: Some(now),
                    enabled: true,
                    priority,
                    nexus_mod_id: None,
                    nexus_file_id: None,
                    nexus_domain: None,
                    version: None,
                    author: None,
                    nexus_description: None,
                    latest_version: None,
                    install_target: crate::models::mod_entry::InstallTarget::Data,
                };
                tracker
                    .insert_mod(&entry)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok((mod_id, cache_dir))
            }
            .await;
            AppCmdMsg::EmptyModCreated(result)
        });
    }
}

// ─── AppCmdMsg handlers ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_cmd_mods_loaded(
        &mut self,
        result: Result<LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                self.collapsed_groups = data
                    .groups
                    .iter()
                    .filter(|g| g.collapsed)
                    .map(|g| g.id.clone())
                    .collect();
                self.apply_loaded_data(data, sender);
                sender.input(AppMsg::ScanExternalFiles);
                // Reload last-deployed profile for the newly selected game.
                if let (Some(tracker), Some(game)) =
                    (self.tracker.clone(), self.selected_game().cloned())
                {
                    let key = format!("last_deployed_profile_{}", game.id);
                    sender.oneshot_command(async move {
                        AppCmdMsg::LastDeployedProfileLoaded(
                            tracker.get_setting(&key).await.ok().flatten(),
                        )
                    });
                }
            }
            Err(e) => {
                self.toaster.toast(&format!("Load failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_mod_removed(
        &mut self,
        result: Result<String, String>,
        nexus_ids: Option<(i64, i64)>,
        mod_name: String,
        archive_hash: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(_) => {
                self.toaster
                    .toast("Mod removed. Deploy to update game files");
                let changed = self.reset_installed_download_for_mod(
                    nexus_ids,
                    &mod_name,
                    archive_hash.as_deref(),
                );
                if !changed.is_empty()
                    && let Some(tracker) = self.tracker.clone()
                {
                    tokio::spawn(async move {
                        for entry in &changed {
                            let _ = tracker.save_download_entry(entry).await;
                        }
                    });
                }
                self.auto_save_profile(sender);
                self.reload_mods(sender);
            }
            Err(e) => {
                self.toaster.toast(&format!("Remove failed: {e}"));
                self.reload_mods(sender);
            }
        }
    }

    pub(crate) fn handle_cmd_priority_saved(
        &mut self,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Err(e) = result {
            self.toaster.toast(&format!("Failed to save order: {e}"));
        } else {
            self.reload_mods(sender);
        }
    }

    pub(crate) fn handle_cmd_empty_mod_created(
        &mut self,
        result: Result<(String, std::path::PathBuf), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((_mod_id, cache_dir)) => {
                self.reload_mods(sender);
                self.toaster.toast(
                    "Empty mod created — put files in its cache folder, then use Scan Cache in Properties",
                );
                let _ = open::that(&cache_dir);
            }
            Err(e) => {
                self.toaster.toast(&format!("Failed to create mod: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_mod_files_rescanned(
        &mut self,
        result: Result<String, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(msg) => {
                self.needs_deploy = true;
                self.reload_mods(sender);
                self.toaster.toast(&msg);
            }
            Err(e) => {
                self.toaster.toast(&format!("Rescan failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_games_rescanned(&mut self, found_games: Vec<Game>) {
        let known_ids: HashSet<String> = self.games.iter().map(|g| g.id.clone()).collect();
        let new_games: Vec<Game> = found_games
            .into_iter()
            .filter(|g| !known_ids.contains(&g.id))
            .collect();
        if new_games.is_empty() {
            self.toaster.toast("No new games found");
        } else {
            let count = new_games.len();
            for g in &new_games {
                self.game_model.append(&g.title);
            }
            self.games.extend(new_games);
            self.toaster.toast(&format!("{count} new game(s) detected"));
        }
    }

    pub(crate) fn handle_cmd_mod_files_loaded(
        &mut self,
        files: Vec<crate::models::manifest::ModFile>,
    ) {
        if let Some(ctrl) = &self.mod_properties_dialog {
            ctrl.sender()
                .send(crate::ui::mod_properties_dialog::ModPropertiesMsg::LoadFiles(files))
                .ok();
        }
    }
}
