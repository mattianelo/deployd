use gtk::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::ui::mod_list::{ModListItemInit, ModListItemKind, ModRowInit};
use crate::utils::paths;

use super::super::App;
use super::super::free_fns::load_game_data;
use super::super::messages::AppCmdMsg;

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
            let nids = init
                .mod_entry
                .nexus_mod_id
                .zip(init.mod_entry.nexus_file_id);
            let name = init.mod_entry.name.clone();
            let hash = init.mod_entry.archive_hash.clone();
            (id, nids, name, hash)
        };

        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let cache_root = self
            .selected_game()
            .and_then(|g| self.cache_root_for(&g.id).ok())
            .unwrap_or_else(|| paths::cache_root().unwrap_or_default());

        self.pending_scroll_restore = Some(self.mod_scroll.vadjustment().value());
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
                let cache = paths::mod_cache_dir_in(&cache_root, &mod_id);
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
        if from >= len || to >= len || from == to {
            return;
        }

        // Find the end of this group's block (next separator or end of list).
        let block_end = (from + 1..len)
            .find(|&i| guard.get(i).map(|r| r.is_separator()).unwrap_or(false))
            .unwrap_or(len);
        let block_size = block_end - from;

        // Drop within own block is a no-op.
        if to >= from && to < block_end {
            return;
        }

        // Extract the whole block (separator + member mods).
        // guard.remove() returns Option<ModListItem>; we move the kind field into
        // a fresh ModListItemInit (both structs share the same ModListItemKind).
        let mut block = Vec::with_capacity(block_size);
        for _ in 0..block_size {
            if let Some(item) = guard.remove(from) {
                block.push(crate::ui::mod_list::ModListItemInit { kind: item.kind, visible: item.visible });
            }
        }

        // After removal, indices > from shift down by block_size.
        let effective_to = if to > from {
            (to - block_size).min(guard.len())
        } else {
            to.min(guard.len())
        };

        for (i, item) in block.into_iter().enumerate() {
            guard.insert(effective_to + i, item);
        }

        drop(guard);
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
        }
        self.needs_deploy = true;
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
                AppCmdMsg::ModsLoaded(load_game_data(&tracker, &game, false).await, true)
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
                AppCmdMsg::ModsLoaded(load_game_data(&tracker, &game, false).await, true)
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
}
