use std::collections::HashMap;

use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::models::game::GameEngine;
use crate::models::mod_entry::InstallTarget;
use crate::ui::mod_properties_dialog::{
    ModPropertiesDialog, ModPropertiesInit, ModPropertiesOutput,
};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

impl App {
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
            .map(|g| g.engine == GameEngine::Bethesda)
            .unwrap_or(false);
        let is_aurora = self
            .selected_game()
            .map(|g| g.engine == GameEngine::Aurora)
            .unwrap_or(false);
        self.mod_properties_dialog = Some(
            ModPropertiesDialog::builder()
                .transient_for(root)
                .launch(ModPropertiesInit {
                    mod_entry,
                    is_bethesda,
                    is_aurora,
                })
                .forward(sender.input_sender(), move |output| match output {
                    ModPropertiesOutput::Applied {
                        name,
                        notes,
                        install_target,
                        file_targets,
                    } => AppMsg::ModPropertiesApplied {
                        mod_id: mod_id_for_output.clone(),
                        mod_idx: idx,
                        name,
                        notes,
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
        mod_id: String,
        mod_idx: usize,
        name: String,
        notes: String,
        install_target: InstallTarget,
        file_targets: HashMap<String, InstallTarget>,
    ) {
        self.mod_properties_dialog = None;
        let Some(tracker) = self.tracker.clone() else {
            return;
        };

        {
            let mut guard = self.mods.guard();
            if let Some(item) = guard.get_mut(mod_idx)
                && let crate::ui::mod_list::ModListItemKind::Mod(row) = &mut item.kind
            {
                row.mod_entry.name = name.clone();
                row.mod_entry.install_target = install_target.clone();
                row.mod_entry.notes = if notes.is_empty() {
                    None
                } else {
                    Some(notes.clone())
                };
            }
        }

        let mod_id_clone = mod_id.clone();
        let name_clone = name.clone();
        let notes_clone = notes.clone();
        let install_target_clone = install_target.clone();
        tokio::spawn(async move {
            let _ = tracker.update_mod_name(&mod_id_clone, &name_clone).await;
            let _ = tracker.update_mod_notes(&mod_id_clone, &notes_clone).await;
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
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let mod_name = self.mod_name_for_id(&mod_id);
        let (data_subdir, engine) = self
            .selected_game()
            .map(|g| (g.data_subdir.clone(), g.engine.clone()))
            .unwrap_or_else(|| (String::new(), GameEngine::Bethesda));

        sender.oneshot_command(async move {
            let result: Result<String, String> = async {
                let cache_dir =
                    crate::utils::paths::mod_cache_dir(&mod_id).map_err(|e| e.to_string())?;
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
                            crate::core::installer::strip_data_subdir_prefix_str(
                                &raw,
                                &data_subdir,
                            )
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
