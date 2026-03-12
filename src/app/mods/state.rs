use std::collections::{HashMap, HashSet};

use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::mod_folders;
use crate::core::tracker::OverrideInfo;
use crate::models::group::ModGroup;
use crate::models::mod_entry::ModEntry;
use crate::models::plugin::Plugin;
use crate::models::profile::Profile;
use crate::ui::mod_list::{ModListItemInit, ModListItemKind, ModRowInit};
use crate::ui::plugin_list::PluginRowInit;

use super::super::free_fns::load_game_data;
use super::super::messages::AppCmdMsg;
use super::super::types::LoadedData;
use super::super::App;

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
