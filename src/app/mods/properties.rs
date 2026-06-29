use std::collections::HashMap;

use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::core::game;
use crate::models::game::GameEngine;
use crate::models::mod_entry::InstallTarget;
use crate::ui::mod_properties_dialog::{
    ModPropertiesDialog, ModPropertiesInit, ModPropertiesOutput,
};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

pub(crate) struct AppliedModProperties {
    pub(crate) mod_id: String,
    pub(crate) mod_idx: usize,
    pub(crate) name: String,
    pub(crate) notes: String,
    pub(crate) nexus_mod_id: Option<i64>,
    pub(crate) nexus_id_changed: bool,
    pub(crate) install_target: InstallTarget,
    pub(crate) file_targets: HashMap<String, InstallTarget>,
}

impl App {
    pub(crate) fn handle_open_mod_properties(
        &mut self,
        index: DynamicIndex,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (
            mod_entry,
            override_files,
            overridden_files,
            conflicting_mod_names,
            conflicted_by_mod_names,
        ) = {
            let guard = self.mods.guard();
            if let Some(item) = guard.get(idx)
                && let crate::ui::mod_list::ModListItemKind::Mod(row) = &item.kind
            {
                (
                    row.mod_entry.clone(),
                    row.override_files.clone(),
                    row.overridden_files.clone(),
                    row.conflicting_mod_names.clone(),
                    row.conflicted_by_mod_names.clone(),
                )
            } else {
                return;
            }
        };
        let mod_id = mod_entry.id.clone();
        let mod_id_for_output = mod_id.clone();
        let is_bethesda = self
            .selected_game()
            .map(|g| g.engine == GameEngine::Bethesda)
            .unwrap_or(false);
        let is_aurora = self
            .selected_game()
            .map(|g| g.engine == GameEngine::Aurora)
            .unwrap_or(false);
        let cache_root = self
            .selected_game()
            .and_then(|g| self.cache_root_for(&g.id).ok())
            .unwrap_or_else(|| crate::utils::paths::cache_root().unwrap_or_default());
        self.mod_properties_dialog = Some(
            ModPropertiesDialog::builder()
                .transient_for(root)
                .launch(ModPropertiesInit {
                    mod_entry,
                    is_bethesda,
                    is_aurora,
                    cache_root,
                    override_files,
                    overridden_files,
                    conflicting_mod_names,
                    conflicted_by_mod_names,
                })
                .forward(sender.input_sender(), move |output| match output {
                    ModPropertiesOutput::Applied {
                        name,
                        notes,
                        nexus_mod_id,
                        nexus_id_changed,
                        install_target,
                        file_targets,
                    } => AppMsg::ModPropertiesApplied {
                        mod_id: mod_id_for_output.clone(),
                        mod_idx: idx,
                        name,
                        notes,
                        nexus_mod_id,
                        nexus_id_changed,
                        install_target,
                        file_targets,
                    },
                    ModPropertiesOutput::Cancelled => AppMsg::ModPropertiesCancelled,
                    ModPropertiesOutput::ScanCache { mod_id } => AppMsg::ScanModFromCache(mod_id),
                }),
        );
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
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
        applied: AppliedModProperties,
        sender: &ComponentSender<Self>,
    ) {
        let AppliedModProperties {
            mod_id,
            mod_idx,
            name,
            notes,
            nexus_mod_id,
            nexus_id_changed,
            install_target,
            file_targets,
        } = applied;
        self.mod_properties_dialog = None;
        let Some(tracker) = self.tracker.clone() else {
            return;
        };

        let nexus_domain = nexus_mod_id
            .and_then(|_| self.selected_game().and_then(game::nexus_domain))
            .map(str::to_string);
        let mut nexus_file_id = None;
        let nexus_update_allowed = nexus_mod_id.is_none() || nexus_domain.is_some();
        {
            let mut guard = self.mods.guard();
            if let Some(item) = guard.get_mut(mod_idx)
                && let crate::ui::mod_list::ModListItemKind::Mod(row) = &mut item.kind
            {
                if nexus_update_allowed {
                    nexus_file_id = if nexus_id_changed {
                        None
                    } else {
                        row.mod_entry.nexus_file_id
                    };
                }
                row.mod_entry.name = name.clone();
                item.search_key = name.to_lowercase();
                row.mod_entry.install_target = install_target.clone();
                row.mod_entry.notes = if notes.is_empty() {
                    None
                } else {
                    Some(notes.clone())
                };
                if nexus_update_allowed {
                    row.mod_entry.nexus_mod_id = nexus_mod_id;
                    row.mod_entry.nexus_domain = nexus_domain.clone();
                    row.mod_entry.nexus_file_id = nexus_file_id;
                }
            }
        }
        if !nexus_update_allowed {
            self.show_toast("Current game has no Nexus domain; Nexus ID was not updated.");
        }

        let mod_id_clone = mod_id.clone();
        let name_clone = name.clone();
        let notes_clone = notes.clone();
        let install_target_clone = install_target.clone();
        sender.oneshot_command(async move {
            let save_result: anyhow::Result<()> = async {
                tracker.update_mod_name(&mod_id_clone, &name_clone).await?;
                tracker
                    .update_mod_notes(&mod_id_clone, &notes_clone)
                    .await?;
                tracker
                    .update_file_targets(&mod_id_clone, &file_targets)
                    .await?;
                tracker
                    .set_mod_install_target_column(&mod_id_clone, &install_target_clone)
                    .await?;
                if nexus_update_allowed {
                    tracker
                        .update_mod_nexus_ids(
                            &mod_id_clone,
                            nexus_mod_id,
                            nexus_file_id,
                            nexus_domain.as_deref(),
                        )
                        .await?;
                }
                Ok(())
            }
            .await;

            if let Err(e) = save_result {
                return AppCmdMsg::ModNexusMetadataRefreshed {
                    mod_id: mod_id_clone,
                    result: Err(format!("Failed to save mod properties: {e}")),
                };
            }

            let Some(nexus_mod_id) = nexus_mod_id else {
                return AppCmdMsg::ModNexusMetadataRefreshed {
                    mod_id: mod_id_clone,
                    result: Ok((String::new(), String::new(), String::new())),
                };
            };
            let Some(domain) = nexus_domain else {
                return AppCmdMsg::ModNexusMetadataRefreshed {
                    mod_id: mod_id_clone,
                    result: Ok((String::new(), String::new(), String::new())),
                };
            };
            let api_key = match tracker.get_setting("nexus_api_key").await {
                Ok(Some(key)) if !key.is_empty() => key,
                Ok(_) => {
                    return AppCmdMsg::ModNexusMetadataRefreshed {
                        mod_id: mod_id_clone,
                        result: Err("No Nexus API key configured. Set it in Settings.".to_string()),
                    };
                }
                Err(e) => {
                    return AppCmdMsg::ModNexusMetadataRefreshed {
                        mod_id: mod_id_clone,
                        result: Err(e.to_string()),
                    };
                }
            };
            let client = crate::core::nexus_api::NexusClient::new(api_key);
            match client.get_mod_info(&domain, nexus_mod_id).await {
                Ok((info, _)) => {
                    let result = tracker
                        .update_mod_nexus_metadata(
                            &mod_id_clone,
                            &info.version,
                            &info.author,
                            info.summary.as_deref().unwrap_or(""),
                        )
                        .await
                        .map(|_| (info.version, info.author, info.name))
                        .map_err(|e| e.to_string());
                    AppCmdMsg::ModNexusMetadataRefreshed {
                        mod_id: mod_id_clone,
                        result,
                    }
                }
                Err(e) => AppCmdMsg::ModNexusMetadataRefreshed {
                    mod_id: mod_id_clone,
                    result: Err(e.to_string()),
                },
            }
        });

        self.needs_deploy = true;
        self.show_toast(&format!("Properties updated for {name}"));
    }

    pub(crate) fn handle_cmd_mod_nexus_metadata_refreshed(
        &mut self,
        mod_id: String,
        result: Result<(String, String, String), String>,
    ) {
        match result {
            Ok((version, author, nexus_name)) => {
                if version.is_empty() && author.is_empty() {
                    return;
                }
                let mut guard = self.mods.guard();
                for i in 0..guard.len() {
                    if let Some(row) = guard.get_mut(i)
                        && let Some(init) = row.mod_row_mut()
                        && init.mod_entry.id == mod_id
                    {
                        init.mod_entry.latest_version = Some(version.clone());
                        init.mod_entry.author = Some(author.clone());
                        break;
                    }
                }
                drop(guard);
                self.show_toast(&format!("Nexus metadata refreshed for {nexus_name}"));
            }
            Err(e) => {
                self.push_notification(&format!("Nexus metadata refresh failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_mod_properties_cancelled(&mut self) {
        self.mod_properties_dialog = None;
    }

    pub(crate) fn handle_scan_mod_from_cache(
        &mut self,
        mod_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let mod_name = self.mod_name_for_id(&mod_id);
        let (data_subdir, engine) = self
            .selected_game()
            .map(|g| (g.data_subdir.clone(), g.engine.clone()))
            .unwrap_or_else(|| (String::new(), GameEngine::Bethesda));
        let cache_root = self
            .selected_game()
            .and_then(|g| self.cache_root_for(&g.id).ok())
            .unwrap_or_else(|| crate::utils::paths::cache_root().unwrap_or_default());

        sender.oneshot_command(async move {
            let result: Result<String, String> = async {
                let cache_dir = crate::utils::paths::mod_cache_dir_in(&cache_root, &mod_id);
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
                        let raw = rel.to_string_lossy().replace('\\', "/");

                        // Strip a redundant data-subdir prefix (e.g. "Data/Override/foo" →
                        // "Override/foo") so that renaming a cache subfolder to "data" does not
                        // produce "Data/data/..." at deploy time.
                        let normalized = if data_subdir.is_empty() {
                            raw
                        } else {
                            crate::core::installer::strip_data_subdir_prefix_str(&raw, &data_subdir)
                        };

                        // For Aurora, cache paths that sit inside game-root sibling dirs
                        // (system/, launcher/, register/) should be recorded with the "../"
                        // prefix so the deployer routes them to the game root, not Data/.
                        let game_rel_original = if engine == GameEngine::Aurora {
                            let lower = normalized.to_lowercase();
                            if lower.starts_with("system/")
                                || lower.starts_with("launcher/")
                                || lower.starts_with("register/")
                            {
                                format!("../{normalized}")
                            } else {
                                normalized
                            }
                        } else {
                            normalized
                        };

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
}
