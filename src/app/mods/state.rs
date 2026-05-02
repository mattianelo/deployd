use std::collections::{HashMap, HashSet};

use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game;
use crate::core::mod_folders;
use crate::core::tracker::OverrideInfo;
use crate::models::group::ModGroup;
use crate::models::mod_entry::ModEntry;
use crate::models::plugin::Plugin;
use crate::models::profile::Profile;
use crate::ui::mod_list::{ModListItemInit, ModListItemKind, ModRowInit};
use crate::ui::plugin_list::PluginRowInit;

use super::super::App;
use super::super::free_fns::load_game_data;
use super::super::messages::AppCmdMsg;
use super::super::types::LoadedData;

impl App {
    /// In-session reload: recomputes conflict overrides and refreshes the mod/plugin
    /// lists from DB without re-syncing Plugins.txt.
    pub(crate) fn reload_mods(&self, sender: &ComponentSender<Self>) {
        self.reload_mods_impl(sender, false, true);
    }

    /// Full reload including a Plugins.txt sync. Use only on initial game select.
    pub(crate) fn reload_mods_full(&self, sender: &ComponentSender<Self>) {
        self.reload_mods_impl(sender, true, false);
    }

    fn reload_mods_impl(&self, sender: &ComponentSender<Self>, sync_txt: bool, preserve_collapsed: bool) {
        if let (Some(tracker), Some(game)) = (self.tracker.clone(), self.selected_game().cloned()) {
            sender.oneshot_command(async move {
                let result = async { load_game_data(&tracker, &game, sync_txt).await };
                AppCmdMsg::ModsLoaded(result.await, preserve_collapsed)
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
        let saved_pos = self
            .pending_scroll_restore
            .take()
            .unwrap_or_else(|| vadj.value());

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
                    visible: true,
                    compact: false,
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
            let reinstall_from_file = m
                .archive_path
                .as_ref()
                .map(|p| !std::path::Path::new(p).starts_with(&self.downloads_dir))
                .unwrap_or(false);
            mod_display_idx += 1;
            guard.push_back(ModListItemInit {
                kind: ModListItemKind::Mod(Box::new(ModRowInit {
                    mod_entry: m,
                    priority_label: format!("#{mod_display_idx}"),
                    overrides: info.map_or(0, |i| i.overrides),
                    overridden_by: info.map_or(0, |i| i.overridden_by),
                    override_files: info.map_or_else(Vec::new, |i| i.override_files.clone()),
                    overridden_files: info.map_or_else(Vec::new, |i| i.overridden_files.clone()),
                    conflicting_mod_names: info
                        .map_or_else(Vec::new, |i| i.conflicting_mod_names.clone()),
                    conflicted_by_mod_names: info
                        .map_or_else(Vec::new, |i| i.conflicted_by_mod_names.clone()),
                    reinstall_from_file,
                })),
                visible,
                compact: self.compact_mod_rows,
            });
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
                visible: true,
                compact: false,
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
        vanilla_plugin_master_counts: &HashMap<String, usize>,
        vanilla_derived: &HashSet<String>,
    ) {
        let managed_lower: HashSet<String> =
            plugins.iter().map(|p| p.filename.to_lowercase()).collect();

        // Map lowercase filename → on-disk filename for all plugins present in the Data dir.
        // Used to resolve display casing for any plugin regardless of what was in the archive.
        let on_disk_name_map: HashMap<String, String> = vanilla_plugins
            .iter()
            .map(|n| (n.to_lowercase(), n.clone()))
            .collect();

        let mut installed = managed_lower.clone();
        installed.extend(vanilla_plugins.iter().map(|n| n.to_lowercase()));

        // Collect truly-unmanaged vanilla plugins (not overwritten by any mod).
        let mut vanilla_sorted: Vec<String> = vanilla_plugins
            .iter()
            .filter(|name| !managed_lower.contains(&name.to_lowercase()))
            .cloned()
            .collect();

        // Also include managed plugins that originally were vanilla game files (e.g. cleaned
        // masters). They are rendered in the vanilla section with a "Vanilla / Modified" label
        // and are non-draggable like ordinary vanilla plugins.
        for p in &plugins {
            let lower = p.filename.to_lowercase();
            if vanilla_derived.contains(&lower) {
                // Prefer the on-disk casing if available, otherwise use the installed filename.
                let display_name = on_disk_name_map
                    .get(&lower)
                    .cloned()
                    .unwrap_or_else(|| p.filename.clone());
                vanilla_sorted.push(display_name);
            }
        }

        // Sort by (extension tier, master count, alphabetical).
        // Fewer declared masters → loads earlier → appears first within each tier.
        // This places root masters (Fallout4.esm, Skyrim.esm) before DLC/CC plugins
        // without relying on hardcoded name lists.
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
            let mc = |n: &str| {
                vanilla_plugin_master_counts
                    .get(n.to_lowercase().as_str())
                    .copied()
                    .unwrap_or(0)
            };
            tier(a)
                .cmp(&tier(b))
                .then_with(|| mc(a).cmp(&mc(b)))
                .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
        });
        self.vanilla_plugin_names = vanilla_sorted;
        self.vanilla_derived_plugins = vanilla_derived.clone();

        let mut guard = self.plugins.guard();
        guard.clear();

        if self.show_vanilla_plugins {
            for filename in &self.vanilla_plugin_names {
                #[cfg(feature = "loot")]
                let dirty_info = self.dirty_plugins.get(&filename.to_lowercase()).cloned();
                #[cfg(not(feature = "loot"))]
                let dirty_info: Option<PluginDirtyInfo> = None;

                let mod_name = if vanilla_derived.contains(&filename.to_lowercase()) {
                    "Vanilla / Modified".to_string()
                } else {
                    "Vanilla / DLC".to_string()
                };
                guard.push_back(PluginRowInit {
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
                    compact: self.compact_plugin_rows,
                });
            }
        }

        let mut managed_display_idx = 0usize;
        for p in plugins {
            // Vanilla-derived plugins are rendered in the vanilla section above; skip them here.
            if vanilla_derived.contains(&p.filename.to_lowercase()) {
                continue;
            }
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

            let display_filename = on_disk_name_map
                .get(&p.filename.to_lowercase())
                .cloned()
                .unwrap_or_else(|| p.filename.clone());
            guard.push_back(PluginRowInit {
                plugin: p,
                display_filename,
                mod_name,
                order_label: format!("#{}", managed_display_idx + 1),
                missing_masters,
                mod_enabled,
                dirty_info,
                is_vanilla: false,
                compact: self.compact_plugin_rows,
            });
            managed_display_idx += 1;
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

    pub(crate) fn reload_order_snapshots(&self, sender: &ComponentSender<Self>) {
        use crate::app::messages::AppCmdMsg;
        use crate::models::order_snapshot::SnapshotKind;
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        sender.oneshot_command(async move {
            let mod_snaps = tracker
                .list_order_snapshots(&game.id, SnapshotKind::Mod)
                .await
                .unwrap_or_default();
            let plugin_snaps = tracker
                .list_order_snapshots(&game.id, SnapshotKind::Plugin)
                .await
                .unwrap_or_default();
            AppCmdMsg::OrderSnapshotsLoaded(mod_snaps, plugin_snaps)
        });
    }

    pub(crate) fn apply_loaded_data(&mut self, data: LoadedData, sender: &ComponentSender<Self>) {
        self.populate_plugins(
            data.plugins,
            &data.mods,
            &data.plugin_masters,
            &data.vanilla_plugins,
            &data.vanilla_plugin_master_counts,
            &data.vanilla_derived_plugins,
        );
        self.populate_mods(data.mods, &data.groups, &data.overrides);
        self.update_profile_list(data.profiles, data.active_profile_idx);
        self.tools = data.tools;
        self.plugin_masters = data.plugin_masters;
        self.rebuild_tool_buttons(sender);
        self.reload_order_snapshots(sender);
        self.apply_search_filter();
    }

    /// Update the `#N` priority labels for all mod rows in-place after a reorder.
    pub(crate) fn refresh_priority_labels(&mut self) {
        let mut count = 0usize;
        let mut guard = self.mods.guard();
        let len = guard.len();
        for i in 0..len {
            if let Some(item) = guard.get_mut(i)
                && let crate::ui::mod_list::ModListItemKind::Mod(ref mut init) = item.kind
            {
                count += 1;
                init.priority_label = format!("#{count}");
            }
        }
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
        let mod_names: HashMap<String, String> = (0..guard.len())
            .filter_map(|i| {
                guard.get(i).and_then(|r| r.mod_row()).map(|r| {
                    (r.mod_entry.id.clone(), r.mod_entry.name.clone())
                })
            })
            .collect();
        drop(guard);

        if let (Some(tracker), Some(game)) = (self.tracker.clone(), self.selected_game()) {
            let game_id = game.id.clone();
            let engine = game.engine.clone();
            let cache_root = self
                .cache_root_for(&game_id)
                .unwrap_or_else(|_| crate::utils::paths::cache_root().unwrap_or_default());
            sender.oneshot_command(async move {
                if let Err(e) = tracker.update_priorities(&updates).await {
                    return AppCmdMsg::OverridesRefreshed(Err(e.to_string()));
                }
                if let Err(e) =
                    mod_folders::refresh_named_mod_folders(&tracker, &game_id, &cache_root).await
                {
                    eprintln!("[deployd] named_mods refresh failed: {e}");
                }
                AppCmdMsg::OverridesRefreshed(
                    tracker
                        .compute_overrides(&game_id, game::handler_for(&engine), &mod_names)
                        .await
                        .map_err(|e| e.to_string()),
                )
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
