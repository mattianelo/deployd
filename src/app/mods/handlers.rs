use std::collections::{HashMap, HashSet};

use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::core::game;
use crate::ui::mod_list::{ModListItemInit, ModListItemKind, ModRowInit};
use crate::utils::paths;

use super::super::App;
use super::super::free_fns::load_game_data;
use super::super::messages::AppCmdMsg;
use super::super::messages::AppMsg;

impl App {
    pub(crate) fn handle_game_selected(&mut self, idx: u32, sender: &ComponentSender<Self>) {
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

    pub(crate) fn handle_move_mod_to(
        &mut self,
        from: usize,
        to: usize,
        sender: &ComponentSender<Self>,
    ) {
        if self.mod_selection_active
            && self.selected_mods.len() > 1
            && self.selected_mods.contains(&from)
        {
            let mut selected: Vec<usize> = self.selected_mods.iter().copied().collect();
            selected.sort_unstable();
            self.handle_move_selected_mods_to(selected, from, to, sender);
            return;
        }

        let mut guard = self.mods.guard();
        let len = guard.len();
        if from >= len || to > len {
            return;
        }
        let to = to.min(len.saturating_sub(1));
        if from == to {
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
        if self.mod_selection_active {
            self.mod_selection_dirty = true;
        }
        self.refresh_priority_labels();
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
        if from >= len || to > len {
            return;
        }

        let collapsed = guard.get(from).map(|r| r.is_collapsed()).unwrap_or(false);

        if collapsed {
            // Collapsed group looks like a single unit to the user, so move it as a block.
            let block_end = (from + 1..len)
                .find(|&i| guard.get(i).map(|r| r.is_separator()).unwrap_or(false))
                .unwrap_or(len);
            let block_size = block_end - from;

            // Drop landing inside the block's own span is a no-op.
            if to > from && to <= block_end {
                return;
            }

            let mut block = Vec::with_capacity(block_size);
            for _ in 0..block_size {
                if let Some(item) = guard.remove(from) {
                    block.push(ModListItemInit {
                        kind: item.kind,
                        visible: item.visible,
                    });
                }
            }

            let effective_to = if to > from {
                (to - block_size).min(guard.len())
            } else {
                to.min(guard.len())
            };

            for (i, item) in block.into_iter().enumerate() {
                guard.insert(effective_to + i, item);
            }
        } else {
            // Expanded group: move just the separator so grouping shifts dynamically.
            let to = to.min(len.saturating_sub(1));
            if from == to {
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
        }

        drop(guard);
        self.needs_deploy = true;
        if self.mod_selection_active {
            self.mod_selection_dirty = true;
        }
        self.refresh_priority_labels();
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
        if n == 0 || to > len {
            return;
        }
        let to = to.min(len.saturating_sub(1));
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
                    guard
                        .get(idx)
                        .and_then(|row| row.mod_row())
                        .map(|init| ModListItemInit {
                            kind: ModListItemKind::Mod(Box::new(ModRowInit {
                                mod_entry: init.mod_entry.clone(),
                                priority_label: init.priority_label.clone(),
                                overrides: init.overrides,
                                overridden_by: init.overridden_by,
                                override_files: init.override_files.clone(),
                                overridden_files: init.overridden_files.clone(),
                                conflicting_mod_names: init.conflicting_mod_names.clone(),
                                conflicted_by_mod_names: init.conflicted_by_mod_names.clone(),
                                reinstall_from_file: init.reinstall_from_file,
                            })),
                            visible: true,
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

            self.selected_mods.clear();
            for i in anchor..anchor + n {
                self.selected_mods.insert(i);
                if let Some(item) = guard.get_mut(i) {
                    item.selected = true;
                    item.selection_mode = true;
                    item.drag_enabled.set(true);
                }
            }
        }
        self.needs_deploy = true;
        if self.mod_selection_active {
            self.mod_selection_dirty = true;
        }
        self.refresh_priority_labels();
        self.save_group_positions();
        self.save_mod_priorities(sender);
    }

    pub(crate) fn handle_enable_all_mods(&mut self, sender: &ComponentSender<Self>) {
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
        let game_id = game.id.clone();
        let engine = game.engine.clone();
        let mod_names: HashMap<String, String> = {
            let guard = self.mods.guard();
            (0..guard.len())
                .filter_map(|i| {
                    guard
                        .get(i)
                        .and_then(|r| r.mod_row())
                        .map(|r| (r.mod_entry.id.clone(), r.mod_entry.name.clone()))
                })
                .collect()
        };
        sender.oneshot_command(async move {
            if let Err(e) = tracker.set_all_mods_enabled(&game_id, true).await {
                return AppCmdMsg::OverridesRefreshed(Err(e.to_string()));
            }
            if let Some(pid) = &profile_id
                && let Err(e) = tracker.save_to_profile(pid, &game_id).await
            {
                return AppCmdMsg::OverridesRefreshed(Err(e.to_string()));
            }
            AppCmdMsg::OverridesRefreshed(
                tracker
                    .compute_overrides(&game_id, game::handler_for(&engine), &mod_names)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }

    pub(crate) fn handle_disable_all_mods(&mut self, sender: &ComponentSender<Self>) {
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
        let game_id = game.id.clone();
        let engine = game.engine.clone();
        let mod_names: HashMap<String, String> = {
            let guard = self.mods.guard();
            (0..guard.len())
                .filter_map(|i| {
                    guard
                        .get(i)
                        .and_then(|r| r.mod_row())
                        .map(|r| (r.mod_entry.id.clone(), r.mod_entry.name.clone()))
                })
                .collect()
        };
        sender.oneshot_command(async move {
            if let Err(e) = tracker.set_all_mods_enabled(&game_id, false).await {
                return AppCmdMsg::OverridesRefreshed(Err(e.to_string()));
            }
            if let Some(pid) = &profile_id
                && let Err(e) = tracker.save_to_profile(pid, &game_id).await
            {
                return AppCmdMsg::OverridesRefreshed(Err(e.to_string()));
            }
            AppCmdMsg::OverridesRefreshed(
                tracker
                    .compute_overrides(&game_id, game::handler_for(&engine), &mod_names)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }

    pub(crate) fn handle_toggle_group_collapse(&mut self, index: DynamicIndex) {
        let idx = index.current_index();
        let (group_id, new_collapsed) = {
            let guard = self.mods.guard();
            if let Some(item) = guard.get(idx)
                && let crate::ui::mod_list::ModListItemKind::Separator {
                    group_id,
                    collapsed,
                    ..
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
                && let crate::ui::mod_list::ModListItemKind::Separator { group_id, .. } = &item.kind
            {
                group_id.clone()
            } else {
                return;
            }
        };
        self.collapsed_groups.remove(&group_id);

        if let (Some(tracker), Some(game)) = (self.tracker.clone(), self.selected_game().cloned()) {
            sender.oneshot_command(async move {
                if let Err(e) = tracker.delete_group(&group_id).await {
                    eprintln!("Failed to delete group: {e}");
                }
                AppCmdMsg::ModsLoaded(
                    load_game_data(&tracker, &game, crate::app::free_fns::GameLoadMode::Refresh)
                        .await,
                    true,
                )
            });
        }
    }

    pub(crate) fn handle_create_group(&mut self, name: String, sender: &ComponentSender<Self>) {
        if let (Some(tracker), Some(game)) = (self.tracker.clone(), self.selected_game().cloned()) {
            let position = {
                let guard = self.mods.guard();
                guard.len() as f64
            };
            sender.oneshot_command(async move {
                if let Err(e) = tracker.create_group(&game.id, &name, position).await {
                    eprintln!("Failed to create group: {e}");
                }
                AppCmdMsg::ModsLoaded(
                    load_game_data(&tracker, &game, crate::app::free_fns::GameLoadMode::Refresh)
                        .await,
                    true,
                )
            });
        }
    }

    pub(crate) fn handle_rename_group(&mut self, index: DynamicIndex, new_name: String) {
        let idx = index.current_index();
        let group_id = {
            let guard = self.mods.guard();
            if let Some(item) = guard.get(idx)
                && let crate::ui::mod_list::ModListItemKind::Separator { group_id, .. } = &item.kind
            {
                group_id.clone()
            } else {
                return;
            }
        };

        {
            let mut guard = self.mods.guard();
            if let Some(item) = guard.get_mut(idx)
                && let crate::ui::mod_list::ModListItemKind::Separator { name, .. } = &mut item.kind
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

    pub(crate) fn handle_set_group_color(
        &mut self,
        index: DynamicIndex,
        color: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let group_id = {
            let guard = self.mods.guard();
            if let Some(item) = guard.get(idx)
                && let crate::ui::mod_list::ModListItemKind::Separator { group_id, .. } = &item.kind
            {
                group_id.clone()
            } else {
                return;
            }
        };

        {
            let mut guard = self.mods.guard();
            if let Some(item) = guard.get_mut(idx)
                && let crate::ui::mod_list::ModListItemKind::Separator { color: c, .. } =
                    &mut item.kind
            {
                *c = color.clone();
            }
        }

        if let Some(tracker) = self.tracker.clone() {
            let color_ref = color.as_deref().map(String::from);
            sender.oneshot_command(async move {
                let _ = tracker
                    .set_group_color(&group_id, color_ref.as_deref())
                    .await;
                crate::app::messages::AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
    }

    pub(crate) fn handle_create_empty_mod(&mut self, sender: &ComponentSender<Self>) {
        self.overflow_menu_btn.popdown();
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let game_id = game.id.clone();
        let cache_root = self
            .cache_root_for(&game.id)
            .unwrap_or_else(|_| paths::cache_root().unwrap_or_default());
        sender.oneshot_command(async move {
            let result: Result<(String, std::path::PathBuf), String> = async {
                let mod_id = uuid::Uuid::new_v4().to_string();
                let cache_dir = crate::utils::paths::mod_cache_dir_in(&cache_root, &mod_id);
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
                    archive_path: None,
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
                    nexus_file_name: None,
                    nexus_is_primary: false,
                    archive_md5: None,
                    install_target: crate::models::mod_entry::InstallTarget::Data,
                    notes: None,
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

    pub(crate) fn handle_reinstall_mod(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let archive_path = {
            let guard = self.mods.guard();
            let Some(row) = guard.get(idx) else { return };
            let Some(init) = row.mod_row() else { return };
            init.mod_entry.archive_path.clone()
        };
        let Some(path_str) = archive_path else {
            return;
        };
        let path = std::path::PathBuf::from(&path_str);
        if !path.exists() {
            self.push_notification(&format!(
                "Archive not found: {}",
                path.file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or(path_str)
            ));
            return;
        }
        self.reinstall_mode = true;
        sender.input(AppMsg::FileChosen(path));
    }

    pub(crate) fn handle_enter_mod_selection_mode(&mut self) {
        self.mod_selection_active = true;
        self.mod_selection_dirty = false;
        self.selected_mods.clear();
        let mut g = self.mods.guard();
        for item in g.iter_mut() {
            item.selection_mode = true;
            item.selected = false;
            item.drag_enabled.set(true);
        }
    }

    pub(crate) fn handle_exit_mod_selection_mode(&mut self) {
        self.mod_selection_active = false;
        self.mod_selection_dirty = false;
        self.selected_mods.clear();
        let mut g = self.mods.guard();
        for item in g.iter_mut() {
            item.selection_mode = false;
            item.selected = false;
            item.drag_enabled.set(false);
        }
    }

    pub(crate) fn handle_toggle_mod_row_selected(&mut self, idx: usize) {
        if !self.mod_selection_active {
            return;
        }
        let mut g = self.mods.guard();
        let Some(item) = g.get_mut(idx) else { return };
        if item.is_separator() {
            return;
        }
        item.selected = !item.selected;
        if item.selected {
            self.selected_mods.insert(idx);
        } else {
            self.selected_mods.remove(&idx);
        }
    }

    pub(crate) fn handle_set_mod_row_selected(&mut self, idx: usize, selected: bool) {
        if !self.mod_selection_active {
            return;
        }
        let mut g = self.mods.guard();
        let Some(item) = g.get_mut(idx) else { return };
        if item.is_separator() || item.selected == selected {
            return;
        }
        item.selected = selected;
        if selected {
            self.selected_mods.insert(idx);
        } else {
            self.selected_mods.remove(&idx);
        }
    }

    pub(crate) fn handle_enable_selected_mods(&mut self, sender: &ComponentSender<Self>) {
        if self.selected_mods.is_empty() {
            return;
        }
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        let indices: Vec<usize> = self.selected_mods.iter().copied().collect();
        let mut mod_ids: Vec<String> = Vec::new();

        {
            let mut guard = self.mods.guard();
            for &idx in &indices {
                let Some(item) = guard.get_mut(idx) else {
                    continue;
                };
                let Some(entry) = item.mod_entry_mut() else {
                    continue;
                };
                entry.enabled = true;
                mod_ids.push(entry.id.clone());
            }
        }
        {
            let mod_id_set: HashSet<String> = mod_ids.iter().cloned().collect();
            let mut guard = self.plugins.guard();
            for i in 0..guard.len() {
                let matches = guard
                    .get(i)
                    .is_some_and(|r| mod_id_set.contains(&r.plugin.mod_id));
                if matches && let Some(row) = guard.get_mut(i) {
                    row.mod_enabled = true;
                }
            }
        }
        self.needs_deploy = true;
        self.mod_selection_dirty = true;

        let game_id = game.id.clone();
        let engine = game.engine.clone();
        let mod_names: HashMap<String, String> = {
            let guard = self.mods.guard();
            (0..guard.len())
                .filter_map(|i| {
                    guard
                        .get(i)
                        .and_then(|r| r.mod_row())
                        .map(|r| (r.mod_entry.id.clone(), r.mod_entry.name.clone()))
                })
                .collect()
        };
        sender.oneshot_command(async move {
            for mod_id in &mod_ids {
                if let Err(e) = tracker.toggle_mod(mod_id, true).await {
                    return AppCmdMsg::OverridesRefreshed(Err(e.to_string()));
                }
            }
            AppCmdMsg::OverridesRefreshed(
                tracker
                    .compute_overrides(&game_id, game::handler_for(&engine), &mod_names)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });

        self.handle_exit_mod_selection_mode();
    }

    pub(crate) fn handle_disable_selected_mods(&mut self, sender: &ComponentSender<Self>) {
        if self.selected_mods.is_empty() {
            return;
        }
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        let indices: Vec<usize> = self.selected_mods.iter().copied().collect();
        let mut mod_ids: Vec<String> = Vec::new();

        {
            let mut guard = self.mods.guard();
            for &idx in &indices {
                let Some(item) = guard.get_mut(idx) else {
                    continue;
                };
                let Some(entry) = item.mod_entry_mut() else {
                    continue;
                };
                entry.enabled = false;
                mod_ids.push(entry.id.clone());
            }
        }
        {
            let mod_id_set: HashSet<String> = mod_ids.iter().cloned().collect();
            let mut guard = self.plugins.guard();
            for i in 0..guard.len() {
                let matches = guard
                    .get(i)
                    .is_some_and(|r| mod_id_set.contains(&r.plugin.mod_id));
                if matches && let Some(row) = guard.get_mut(i) {
                    row.mod_enabled = false;
                }
            }
        }
        self.needs_deploy = true;
        self.mod_selection_dirty = true;

        let game_id = game.id.clone();
        let engine = game.engine.clone();
        let mod_names: HashMap<String, String> = {
            let guard = self.mods.guard();
            (0..guard.len())
                .filter_map(|i| {
                    guard
                        .get(i)
                        .and_then(|r| r.mod_row())
                        .map(|r| (r.mod_entry.id.clone(), r.mod_entry.name.clone()))
                })
                .collect()
        };
        sender.oneshot_command(async move {
            for mod_id in &mod_ids {
                if let Err(e) = tracker.toggle_mod(mod_id, false).await {
                    return AppCmdMsg::OverridesRefreshed(Err(e.to_string()));
                }
            }
            AppCmdMsg::OverridesRefreshed(
                tracker
                    .compute_overrides(&game_id, game::handler_for(&engine), &mod_names)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });

        self.handle_exit_mod_selection_mode();
    }

    pub(crate) fn handle_remove_selected_mods(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let n = self.selected_mods.len();
        if n == 0 {
            return;
        }
        let dialog = adw::AlertDialog::builder()
            .heading(format!(
                "Remove {} Mod{}?",
                n,
                if n == 1 { "" } else { "s" }
            ))
            .body("This will delete the selected mods and cannot be undone.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("remove", &format!("Remove {n}"));
        dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let s = sender.input_sender().clone();
        dialog.connect_response(None, move |_, id| {
            if id == "remove" {
                let _ = s.send(AppMsg::ConfirmRemoveSelectedMods);
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_confirm_remove_selected_mods(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let cache_root = self
            .selected_game()
            .and_then(|g| self.cache_root_for(&g.id).ok())
            .unwrap_or_else(|| paths::cache_root().unwrap_or_default());

        let vadj = self.mod_scroll.vadjustment();
        self.pending_scroll_restore = Some(vadj.value());

        let mut indices: Vec<usize> = self.selected_mods.iter().copied().collect();
        indices.sort_unstable_by(|a, b| b.cmp(a));

        for &idx in &indices {
            let (mod_id, removed_nexus_ids, removed_mod_name, removed_archive_hash) = {
                let guard = self.mods.guard();
                let Some(row) = guard.get(idx) else { continue };
                let Some(init) = row.mod_row() else { continue };
                (
                    init.mod_entry.id.clone(),
                    init.mod_entry
                        .nexus_mod_id
                        .zip(init.mod_entry.nexus_file_id),
                    init.mod_entry.name.clone(),
                    init.mod_entry.archive_hash.clone(),
                )
            };

            self.mods.guard().remove(idx);
            {
                let mut guard = self.plugins.guard();
                let to_remove: Vec<usize> = (0..guard.len())
                    .filter(|&i| guard.get(i).is_some_and(|row| row.plugin.mod_id == mod_id))
                    .collect();
                for i in to_remove.into_iter().rev() {
                    guard.remove(i);
                }
            }

            let tracker_clone = tracker.clone();
            let cache_root_clone = cache_root.clone();
            sender.oneshot_command(async move {
                let result: Result<String, String> = async {
                    tracker_clone
                        .delete_plugins_for_mod(&mod_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    tracker_clone
                        .delete_mod_files(&mod_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    tracker_clone
                        .delete_mod(&mod_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    let cache = paths::mod_cache_dir_in(&cache_root_clone, &mod_id);
                    if cache.exists() {
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

        self.needs_deploy = true;
        self.mod_selection_dirty = true;
        self.save_group_positions();
        self.handle_exit_mod_selection_mode();
    }
}
