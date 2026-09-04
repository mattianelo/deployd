use std::collections::HashMap;

use gtk::prelude::WidgetExt;
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

#[derive(Debug)]
pub(crate) struct AppliedModProperties {
    pub(crate) mod_id: String,
    pub(crate) name: String,
    pub(crate) notes: String,
    pub(crate) version: Option<String>,
    pub(crate) nexus_mod_id: Option<i64>,
    pub(crate) nexus_id_changed: bool,
    pub(crate) install_target: InstallTarget,
    pub(crate) file_targets: HashMap<String, InstallTarget>,
    pub(crate) routing_changed: bool,
}

#[derive(Debug)]
pub(crate) struct SavedModProperties {
    pub(crate) applied: AppliedModProperties,
    pub(crate) nexus_file_id: Option<i64>,
    pub(crate) nexus_domain: Option<String>,
    pub(crate) nexus_update_allowed: bool,
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
            let guard = self.mods.rows.guard();
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
        let Some(game) = self.selected_game() else {
            self.push_notification("No game selected");
            return;
        };
        let is_bethesda = game.engine == GameEngine::Bethesda;
        let is_aurora = game.engine == GameEngine::Aurora;
        let cache_root = match self.cache_root_for(&game.id) {
            Ok(path) => path,
            Err(error) => {
                self.push_notification(&format!("Cannot resolve the mod cache: {error}"));
                return;
            }
        };
        self.ui.mod_properties_dialog = Some(
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
                        version,
                        nexus_mod_id,
                        nexus_id_changed,
                        install_target,
                        file_targets,
                        routing_changed,
                    } => AppMsg::Mods(crate::app::messages::ModsMsg::ModPropertiesApplied {
                        mod_id: mod_id_for_output.clone(),
                        name,
                        notes,
                        version,
                        nexus_mod_id,
                        nexus_id_changed,
                        install_target,
                        file_targets,
                        routing_changed,
                    }),
                    ModPropertiesOutput::Cancelled => {
                        AppMsg::Mods(crate::app::messages::ModsMsg::ModPropertiesCancelled)
                    }
                    ModPropertiesOutput::ScanCache { mod_id } => {
                        AppMsg::Mods(crate::app::messages::ModsMsg::ScanModFromCache(mod_id))
                    }
                }),
        );
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let mod_id_for_load = mod_id;
        sender.oneshot_command(async move {
            let files = tracker
                .get_mod_files(&mod_id_for_load)
                .await
                .unwrap_or_default();
            AppCmdMsg::Mods(crate::app::messages::ModsCmdMsg::ModFilesLoaded {
                mod_id: mod_id_for_load,
                files,
            })
        });
    }

    pub(crate) fn handle_mod_properties_applied(
        &mut self,
        applied: AppliedModProperties,
        sender: &ComponentSender<Self>,
    ) {
        let mod_id = applied.mod_id.clone();
        let Some(tracker) = self.session.tracker.clone() else {
            if let Some(controller) = &self.ui.mod_properties_dialog {
                controller
                    .sender()
                    .send(crate::ui::mod_properties_dialog::ModPropertiesMsg::SaveFailed)
                    .ok();
            }
            self.push_notification("Failed to save mod properties: database unavailable");
            return;
        };

        let nexus_domain = applied
            .nexus_mod_id
            .and_then(|_| self.selected_game().and_then(game::nexus_domain))
            .map(str::to_string);
        let nexus_update_allowed = applied.nexus_mod_id.is_none() || nexus_domain.is_some();
        let current_nexus_file_id = {
            let guard = self.mods.rows.guard();
            (0..guard.len()).find_map(|index| {
                guard
                    .get(index)
                    .and_then(|item| item.mod_row())
                    .filter(|row| row.mod_entry.id == mod_id)
                    .and_then(|row| row.mod_entry.nexus_file_id)
            })
        };
        let nexus_file_id = if nexus_update_allowed && !applied.nexus_id_changed {
            current_nexus_file_id
        } else {
            None
        };
        let saved = SavedModProperties {
            applied,
            nexus_file_id,
            nexus_domain,
            nexus_update_allowed,
        };
        sender.oneshot_command(async move {
            let notes = (!saved.applied.notes.is_empty()).then_some(saved.applied.notes.as_str());
            let nexus_identity = saved.nexus_update_allowed.then_some(
                crate::core::tracker::mods::ModNexusIdentityUpdate {
                    mod_id: saved.applied.nexus_mod_id,
                    file_id: saved.nexus_file_id,
                    domain: saved.nexus_domain.as_deref(),
                },
            );
            let result = tracker
                .update_mod_properties(crate::core::tracker::mods::ModPropertiesUpdate {
                    mod_id: &saved.applied.mod_id,
                    name: &saved.applied.name,
                    notes,
                    version: saved.applied.version.as_deref(),
                    install_target: &saved.applied.install_target,
                    file_targets: &saved.applied.file_targets,
                    nexus_identity,
                })
                .await
                .map_err(|error| error.to_string());
            AppCmdMsg::Mods(crate::app::messages::ModsCmdMsg::ModPropertiesSaved {
                saved: Box::new(saved),
                result,
            })
        });
    }

    pub(crate) fn handle_cmd_mod_properties_saved(
        &mut self,
        saved: SavedModProperties,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Err(error) = result {
            if let Some(controller) = &self.ui.mod_properties_dialog {
                controller
                    .sender()
                    .send(crate::ui::mod_properties_dialog::ModPropertiesMsg::SaveFailed)
                    .ok();
            }
            self.push_notification(&format!("Failed to save mod properties: {error}"));
            return;
        }

        if let Some(controller) = &self.ui.mod_properties_dialog {
            controller.widget().set_visible(false);
        }
        self.ui.mod_properties_dialog = None;

        let mod_id = saved.applied.mod_id.clone();
        let name = saved.applied.name.clone();
        let mut guard = self.mods.rows.guard();
        for index in 0..guard.len() {
            if let Some(item) = guard.get_mut(index)
                && let crate::ui::mod_list::ModListItemKind::Mod(row) = &mut item.kind
                && row.mod_entry.id == mod_id
            {
                row.mod_entry.name = saved.applied.name.clone();
                item.search_key = saved.applied.name.to_lowercase();
                row.mod_entry.notes =
                    (!saved.applied.notes.is_empty()).then(|| saved.applied.notes.clone());
                row.mod_entry.version = saved.applied.version.clone();
                row.mod_entry.install_target = saved.applied.install_target.clone();
                if saved.nexus_update_allowed {
                    row.mod_entry.nexus_mod_id = saved.applied.nexus_mod_id;
                    row.mod_entry.nexus_file_id = saved.nexus_file_id;
                    row.mod_entry.nexus_domain = saved.nexus_domain.clone();
                }
                break;
            }
        }
        drop(guard);

        self.shell.needs_deploy |= saved.applied.routing_changed;
        if !saved.nexus_update_allowed {
            self.show_toast("Current game has no Nexus domain; Nexus ID was not updated.");
        }
        self.show_toast(&format!("Properties updated for {name}"));

        if saved.nexus_update_allowed
            && let (Some(nexus_mod_id), Some(domain)) =
                (saved.applied.nexus_mod_id, saved.nexus_domain)
        {
            self.refresh_mod_nexus_metadata(mod_id, nexus_mod_id, domain, sender);
        }
    }

    fn refresh_mod_nexus_metadata(
        &self,
        mod_id: String,
        nexus_mod_id: i64,
        domain: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        sender.oneshot_command(async move {
            let result: Result<(String, String, String), String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .map_err(|error| error.to_string())?
                    .filter(|key| !key.is_empty())
                    .ok_or_else(|| {
                        "No Nexus API key configured. Set it in Settings.".to_string()
                    })?;
                let client = crate::core::nexus_api::NexusClient::new(api_key);
                let (info, _) = client
                    .get_mod_info(&domain, nexus_mod_id)
                    .await
                    .map_err(|error| error.to_string())?;
                tracker
                    .update_mod_nexus_metadata(
                        &mod_id,
                        &info.version,
                        &info.author,
                        info.summary.as_deref().unwrap_or(""),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
                Ok((info.version, info.author, info.name))
            }
            .await;
            AppCmdMsg::Mods(
                crate::app::messages::ModsCmdMsg::ModNexusMetadataRefreshed { mod_id, result },
            )
        });
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
                let mut guard = self.mods.rows.guard();
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
        self.ui.mod_properties_dialog = None;
    }

    pub(crate) fn handle_scan_mod_from_cache(
        &mut self,
        mod_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.session.tracker.clone() else {
            send_rescan_failure(sender, mod_id, "Database unavailable".to_string());
            return;
        };
        let mod_name = self.mod_name_for_id(&mod_id);
        let Some(game) = self.selected_game() else {
            send_rescan_failure(sender, mod_id, "No game selected".to_string());
            return;
        };
        let data_subdir = game.data_subdir.clone();
        let engine = game.engine.clone();
        let cache_root = match self.cache_root_for(&game.id) {
            Ok(path) => path,
            Err(error) => {
                send_rescan_failure(
                    sender,
                    mod_id,
                    format!("Cannot resolve the mod cache: {error}"),
                );
                return;
            }
        };

        let result_mod_id = mod_id.clone();
        sender.oneshot_command(async move {
            let result = async {
                let existing_files = tracker
                    .get_mod_files(&mod_id)
                    .await
                    .map_err(|error| error.to_string())?;
                let cache_dir = crate::utils::paths::mod_cache_dir_in(&cache_root, &mod_id);
                let scan_mod_id = mod_id.clone();
                let files = tokio::task::spawn_blocking(move || {
                    scan_mod_cache(
                        &scan_mod_id,
                        &cache_dir,
                        &data_subdir,
                        &engine,
                        &existing_files,
                    )
                })
                .await
                .map_err(|error| format!("Cache scan task failed: {error}"))??;
                tracker
                    .replace_mod_files(&mod_id, &files)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(crate::app::messages::RescannedModFiles {
                    summary: format!("{} — {} file(s) registered", mod_name, files.len()),
                    files,
                })
            }
            .await;
            AppCmdMsg::Mods(crate::app::messages::ModsCmdMsg::ModFilesRescanned {
                mod_id: result_mod_id,
                result,
            })
        });
    }
}

fn send_rescan_failure(sender: &ComponentSender<App>, mod_id: String, error: String) {
    sender.oneshot_command(async move {
        AppCmdMsg::Mods(crate::app::messages::ModsCmdMsg::ModFilesRescanned {
            mod_id,
            result: Err(error),
        })
    });
}

fn scan_mod_cache(
    mod_id: &str,
    cache_dir: &std::path::Path,
    data_subdir: &str,
    engine: &GameEngine,
    existing_files: &[crate::models::manifest::ModFile],
) -> Result<Vec<crate::models::manifest::ModFile>, String> {
    let saved_targets = saved_rescan_targets(engine, existing_files);
    let mut files = Vec::new();
    if !cache_dir.is_dir() {
        return Ok(files);
    }

    for entry in walkdir::WalkDir::new(cache_dir).min_depth(1) {
        let entry = entry.map_err(|error| error.to_string())?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(cache_dir)
            .map_err(|error| error.to_string())?;
        let raw = relative.to_string_lossy().replace('\\', "/");
        let normalized = if data_subdir.is_empty() {
            raw
        } else {
            crate::core::installer::strip_data_subdir_prefix_str(&raw, data_subdir)
        };
        let path_key = normalized.to_lowercase();
        let target = saved_targets
            .get(&path_key)
            .cloned()
            .unwrap_or_else(|| default_rescan_target(engine, &normalized));
        let game_rel_original = if target == InstallTarget::Root {
            format!("../{normalized}")
        } else {
            normalized
        };
        files.push(crate::models::manifest::ModFile {
            mod_id: mod_id.to_string(),
            game_rel_lowercase: game_rel_original.to_lowercase(),
            game_rel_original,
            cache_path: entry.path().to_string_lossy().to_string(),
        });
    }
    files.sort_by(|left, right| left.game_rel_lowercase.cmp(&right.game_rel_lowercase));
    Ok(files)
}

fn saved_rescan_targets(
    engine: &GameEngine,
    files: &[crate::models::manifest::ModFile],
) -> HashMap<String, InstallTarget> {
    if !matches!(engine, GameEngine::Bethesda | GameEngine::Aurora) {
        return HashMap::new();
    }
    files
        .iter()
        .map(|file| {
            let path = file
                .game_rel_lowercase
                .strip_prefix("../")
                .unwrap_or(&file.game_rel_lowercase)
                .to_string();
            let target = if file.game_rel_lowercase.starts_with("../") {
                InstallTarget::Root
            } else {
                InstallTarget::Data
            };
            (path, target)
        })
        .collect()
}

fn default_rescan_target(engine: &GameEngine, path: &str) -> InstallTarget {
    match engine {
        GameEngine::Bethesda => crate::core::installer::auto_detect_install_target(path),
        GameEngine::Aurora if is_aurora_root_path(path) => InstallTarget::Root,
        _ => InstallTarget::Data,
    }
}

fn is_aurora_root_path(path: &str) -> bool {
    let path = path.to_lowercase();
    path.starts_with("system/") || path.starts_with("launcher/") || path.starts_with("register/")
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;

    use super::*;

    fn existing_file(path: &str) -> crate::models::manifest::ModFile {
        crate::models::manifest::ModFile {
            mod_id: "mod-a".to_string(),
            game_rel_lowercase: path.to_string(),
            game_rel_original: path.to_string(),
            cache_path: String::new(),
        }
    }

    fn write_cache_file(root: &std::path::Path, path: &str) -> Result<()> {
        let path = root.join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, "cached")?;
        Ok(())
    }

    // @variants: both
    #[test]
    fn bethesda_rescan_preserves_saved_targets_and_routes_new_binary() -> Result<()> {
        let cache = tempfile::tempdir()?;
        write_cache_file(cache.path(), "textures/custom.dds")?;
        write_cache_file(cache.path(), "forced-data.dll")?;
        write_cache_file(cache.path(), "new-root.dll")?;
        let existing = vec![
            existing_file("../textures/custom.dds"),
            existing_file("forced-data.dll"),
            existing_file("deleted.txt"),
        ];

        let files = scan_mod_cache(
            "mod-a",
            cache.path(),
            "Data",
            &GameEngine::Bethesda,
            &existing,
        )
        .map_err(anyhow::Error::msg)?;
        let paths: Vec<_> = files
            .iter()
            .map(|file| file.game_rel_lowercase.as_str())
            .collect();

        assert!(paths.contains(&"../textures/custom.dds"));
        assert!(paths.contains(&"forced-data.dll"));
        assert!(paths.contains(&"../new-root.dll"));
        assert!(!paths.contains(&"deleted.txt"));
        Ok(())
    }

    // @variants: both
    #[test]
    fn aurora_rescan_preserves_manual_targets_and_routes_new_sibling() -> Result<()> {
        let cache = tempfile::tempdir()?;
        write_cache_file(cache.path(), "system/forced-data.dll")?;
        write_cache_file(cache.path(), "Override/forced-root.2da")?;
        write_cache_file(cache.path(), "launcher/new.exe")?;
        let existing = vec![
            existing_file("system/forced-data.dll"),
            existing_file("../override/forced-root.2da"),
        ];

        let files = scan_mod_cache(
            "mod-a",
            cache.path(),
            "Data",
            &GameEngine::Aurora,
            &existing,
        )
        .map_err(anyhow::Error::msg)?;
        let paths: Vec<_> = files
            .iter()
            .map(|file| file.game_rel_lowercase.as_str())
            .collect();

        assert!(paths.contains(&"system/forced-data.dll"));
        assert!(paths.contains(&"../override/forced-root.2da"));
        assert!(paths.contains(&"../launcher/new.exe"));
        Ok(())
    }

    // @variants: both
    #[test]
    fn rescan_does_not_reinterpret_other_engine_anchors() -> Result<()> {
        let eclipse_cache = tempfile::tempdir()?;
        write_cache_file(eclipse_cache.path(), "~docs~/BioWare/settings.ini")?;
        let eclipse = scan_mod_cache("mod-a", eclipse_cache.path(), "", &GameEngine::Eclipse, &[])
            .map_err(anyhow::Error::msg)?;
        assert_eq!(eclipse[0].game_rel_lowercase, "~docs~/bioware/settings.ini");

        let red_cache = tempfile::tempdir()?;
        write_cache_file(red_cache.path(), "Mods/example/content/file.reds")?;
        let red = scan_mod_cache("mod-a", red_cache.path(), "", &GameEngine::REDEngine, &[])
            .map_err(anyhow::Error::msg)?;
        assert_eq!(red[0].game_rel_lowercase, "mods/example/content/file.reds");
        Ok(())
    }
}
