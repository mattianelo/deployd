use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::detector;
use crate::ui::absorb_dialog::{AbsorbDialog, AbsorbDialogOutput};
use crate::ui::pre_install_dialog::{
    PreInstallDialog, PreInstallDialogInit, PreInstallDialogOutput,
};

use super::App;
use super::messages::{AppCmdMsg, AppMsg};
use super::types::PendingInstall;

impl App {
    pub(crate) fn handle_discard_external_files(
        &mut self,
        paths: Vec<PathBuf>,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(dialog) = self.absorb_dialog.take() {
            dialog.widget().destroy();
        }
        let mut deleted = 0usize;
        let mut failed = 0usize;
        for path in &paths {
            match std::fs::remove_file(path) {
                Ok(()) => deleted += 1,
                Err(e) => {
                    eprintln!("deployd: discard failed {}: {e}", path.display());
                    failed += 1;
                }
            }
        }
        if failed > 0 {
            self.push_notification(&format!(
                "Discarded {deleted} file(s); {failed} could not be deleted"
            ));
        } else if deleted > 0 {
            self.push_notification(&format!("Discarded {deleted} external file(s)"));
        }
        sender.input(AppMsg::ScanExternalFiles);
    }

    pub(crate) fn handle_create_mod_from_external_cancelled(&mut self) {
        if let Some(dialog) = self.absorb_dialog.take() {
            dialog.widget().destroy();
        }
    }

    pub(crate) fn handle_reset_vanilla_baseline(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.overflow_menu_btn.popdown();
        let dialog = adw::AlertDialog::builder()
            .heading("Reset vanilla baseline?")
            .body("This re-snapshots the current game folder as the new vanilla state. Any files added since the last snapshot will no longer be reported as external changes. Use this after a clean game reinstall.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("reset", "Reset");
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("reset", adw::ResponseAppearance::Destructive);
        let input_sender = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "reset" {
                input_sender
                    .send(AppMsg::ResetVanillaBaselineConfirmed)
                    .unwrap();
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_reset_vanilla_baseline_confirmed(
        &mut self,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        self.push_notification("Resetting vanilla baseline…");
        sender.oneshot_command(async move {
            let result = async {
                let entries = detector::snapshot_game_files(&game);
                tracker
                    .reset_vanilla_snapshot(&game.id, &entries)
                    .await
                    .map_err(|e: anyhow::Error| e.to_string())
            }
            .await;
            AppCmdMsg::VanillaBaselineReset(result)
        });
    }

    pub(crate) fn handle_mark_external_files_as_vanilla(
        &mut self,
        files: Vec<crate::core::detector::ExternalFile>,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        sender.oneshot_command(async move {
            let result = async {
                let mut entries = Vec::with_capacity(files.len());
                for ef in &files {
                    let meta = std::fs::metadata(&ef.abs_path).map_err(|e| e.to_string())?;
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    entries.push((ef.game_rel.clone(), meta.len(), mtime));
                }
                tracker
                    .update_vanilla_entries(&game.id, &entries)
                    .await
                    .map_err(|e: anyhow::Error| e.to_string())?;
                Ok::<usize, String>(entries.len())
            }
            .await;
            AppCmdMsg::VanillaEntriesUpdated(result)
        });
    }

    pub(crate) fn handle_adopt_managed_plugin_changes(
        &mut self,
        files: Vec<crate::core::detector::ExternalFile>,
        sender: &ComponentSender<Self>,
    ) {
        if files.is_empty() {
            return;
        }
        if let Some(dialog) = self.absorb_dialog.take() {
            dialog.widget().destroy();
        }
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        self.push_notification("Adopting cleaned plugin(s)…");
        sender.oneshot_command(async move {
            let result = async {
                let plugin_files = tracker
                    .get_deployed_plugin_files(&game.id)
                    .await
                    .map_err(|e: anyhow::Error| e.to_string())?;
                let cache_map: std::collections::HashMap<String, std::path::PathBuf> = plugin_files
                    .into_iter()
                    .map(|(rel, _, path)| (rel, path))
                    .collect();

                let mut adopted = 0usize;
                for ef in &files {
                    let Some(cache_path) = cache_map.get(&ef.game_rel) else {
                        continue;
                    };
                    if ef.xedit_backup_path.is_some() {
                        let plugin_filename = std::path::Path::new(&ef.game_rel)
                            .file_name()
                            .map(|n| n.to_string_lossy().to_lowercase())
                            .unwrap_or_default();
                        if let Some(dir_name) =
                            crate::core::detector::xedit_backup_dir_name(&game.id)
                        {
                            let backup_base = game.data_dir().join(dir_name);
                            if let Ok(entries) = std::fs::read_dir(&backup_base) {
                                for e in entries.flatten() {
                                    if e.file_name().to_string_lossy().to_lowercase()
                                        == plugin_filename
                                    {
                                        let _ = std::fs::remove_dir_all(e.path());
                                        break;
                                    }
                                }
                            }
                        }
                    } else {
                        std::fs::copy(&ef.abs_path, cache_path).map_err(|e| {
                            format!("Failed to update cache for {}: {e}", ef.game_rel)
                        })?;
                        std::fs::remove_file(&ef.abs_path).map_err(|e| {
                            format!("Failed to remove {} from game folder: {e}", ef.game_rel)
                        })?;
                        std::fs::hard_link(cache_path, &ef.abs_path).map_err(|e| {
                            format!(
                                "Failed to hardlink {} back into game folder: {e}",
                                ef.game_rel
                            )
                        })?;
                    }
                    adopted += 1;
                }
                Ok::<usize, String>(adopted)
            }
            .await;
            AppCmdMsg::ManagedPluginsAdopted(result)
        });
    }

    pub(crate) fn handle_restore_from_xedit_backup(
        &mut self,
        files: Vec<crate::core::detector::ExternalFile>,
        sender: &ComponentSender<Self>,
    ) {
        if files.is_empty() {
            return;
        }
        if let Some(dialog) = self.absorb_dialog.take() {
            dialog.widget().destroy();
        }
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        self.push_notification("Restoring plugin(s) from xEdit backup…");
        sender.oneshot_command(async move {
            let result = async {
                let plugin_files = tracker
                    .get_deployed_plugin_files(&game.id)
                    .await
                    .map_err(|e: anyhow::Error| e.to_string())?;
                let cache_map: std::collections::HashMap<String, std::path::PathBuf> = plugin_files
                    .into_iter()
                    .map(|(rel, _, path)| (rel, path))
                    .collect();

                let mut restored = 0usize;
                for ef in &files {
                    let Some(backup_path) = &ef.xedit_backup_path else {
                        continue;
                    };
                    let Some(cache_path) = cache_map.get(&ef.game_rel) else {
                        continue;
                    };
                    std::fs::copy(backup_path, cache_path).map_err(|e| {
                        format!("Failed to restore {} from backup: {e}", ef.game_rel)
                    })?;
                    let plugin_filename = std::path::Path::new(&ef.game_rel)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    if let Some(dir_name) = crate::core::detector::xedit_backup_dir_name(&game.id) {
                        let backup_base = game.data_dir().join(dir_name);
                        if let Ok(entries) = std::fs::read_dir(&backup_base) {
                            for e in entries.flatten() {
                                if e.file_name().to_string_lossy().to_lowercase() == plugin_filename
                                {
                                    let _ = std::fs::remove_dir_all(e.path());
                                    break;
                                }
                            }
                        }
                    }
                    restored += 1;
                }
                Ok::<usize, String>(restored)
            }
            .await;
            AppCmdMsg::BackupRestored(result)
        });
    }

    pub(crate) fn handle_scan_external_files(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        sender.oneshot_command(async move {
            let result = async {
                let tracked = tracker
                    .get_tracked_rel_paths(&game.id)
                    .await
                    .map_err(|e: anyhow::Error| e.to_string())?;
                let vanilla = tracker
                    .get_vanilla_metadata(&game.id)
                    .await
                    .map_err(|e: anyhow::Error| e.to_string())?;
                let mut files = detector::scan_external_files(&game, &tracked, &vanilla);
                let plugin_files = tracker
                    .get_deployed_plugin_files(&game.id)
                    .await
                    .map_err(|e: anyhow::Error| e.to_string())?;
                let modified_managed =
                    detector::scan_modified_managed_plugins(&game.id, &game, &plugin_files);
                files.extend(modified_managed);
                Ok::<_, String>(files)
            }
            .await;
            AppCmdMsg::ExternalScanDone(result)
        });
    }

    pub(crate) fn handle_absorb_external_files(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let files = self.pending_external_files.clone();
        if files.is_empty() {
            return;
        }
        self.absorb_dialog = Some(
            AbsorbDialog::builder()
                .transient_for(root)
                .launch(files)
                .forward(sender.input_sender(), |output| match output {
                    AbsorbDialogOutput::Selected(file_list) => {
                        AppMsg::AbsorbFilesSelected(file_list)
                    }
                    AbsorbDialogOutput::Discarded(paths) => AppMsg::DiscardExternalFiles(paths),
                    AbsorbDialogOutput::MarkedAsVanilla(files) => {
                        AppMsg::MarkExternalFilesAsVanilla(files)
                    }
                    AbsorbDialogOutput::AdoptManagedChanges(files) => {
                        AppMsg::AdoptManagedPluginChanges(files)
                    }
                    AbsorbDialogOutput::RestoreFromBackup(files) => {
                        AppMsg::RestoreFromXEditBackup(files)
                    }
                    AbsorbDialogOutput::Cancelled => AppMsg::CreateModFromExternalCancelled,
                }),
        );
    }

    pub(crate) fn handle_absorb_files_selected(
        &mut self,
        file_list: Vec<(std::path::PathBuf, std::path::PathBuf)>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if file_list.is_empty() {
            return;
        }
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let mod_name = "External Changes".to_string();
        let rules = crate::core::rules::rules_for_game(&game.id);
        let preview = crate::ui::pre_install_dialog::file_preview_from_list(
            &file_list,
            &rules,
            game.engine.clone(),
            &game.data_subdir,
        );
        let is_bethesda = game.engine == crate::models::game::GameEngine::Bethesda;
        let is_aurora = game.engine == crate::models::game::GameEngine::Aurora;
        self.pending_external_files.clear();
        self.external_changes_count = 0;
        self.pending_install = Some(PendingInstall {
            tmp_dir: tempfile::tempdir().expect("tempdir"),
            mod_name: mod_name.clone(),
            game,
            file_list: Some(file_list),
            stripped_wrapper: None,
            fomod_config_path: None,
            fomod_config: None,
            nexus_ids: None,
            archive_hash: None,
            archive_path: None,
            file_targets: HashMap::new(),
            excluded_files: HashSet::new(),
        });
        let mod_names: Vec<String> = self
            .mods
            .iter()
            .filter_map(|item| {
                if item.is_separator() {
                    None
                } else {
                    Some(item.mod_name().to_owned())
                }
            })
            .collect();
        self.pre_install_dialog = Some(
            PreInstallDialog::builder()
                .transient_for(root)
                .launch(PreInstallDialogInit {
                    mod_name,
                    file_preview: preview,
                    is_fomod: false,
                    is_bethesda,
                    is_aurora,
                    mod_names,
                })
                .forward(sender.input_sender(), |output| match output {
                    PreInstallDialogOutput::Confirmed(name, targets, excluded) => {
                        AppMsg::PreInstallConfirmed(name, targets, excluded)
                    }
                    PreInstallDialogOutput::Cancelled => AppMsg::PreInstallCancelled,
                }),
        );
    }
}

// ─── AppCmdMsg handlers ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_cmd_external_scan_done(
        &mut self,
        result: Result<Vec<crate::core::detector::ExternalFile>, String>,
    ) {
        match result {
            Ok(files) => {
                self.external_changes_count = files.len();
                self.pending_external_files = files;
            }
            Err(e) => {
                eprintln!("deployd: external scan failed: {e}");
            }
        }
    }

    pub(crate) fn handle_cmd_managed_plugins_adopted(
        &mut self,
        result: Result<usize, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(count) => {
                self.push_notification(&format!(
                    "Adopted {count} cleaned plugin{} into deployd",
                    if count == 1 { "" } else { "s" }
                ));
                #[cfg(feature = "loot")]
                if self
                    .selected_game()
                    .map(|g| crate::core::loot_sort::game_has_loot_support(&g.id))
                    .unwrap_or(false)
                {
                    sender.input(AppMsg::SortWithLoot);
                }
                sender.input(AppMsg::ScanExternalFiles);
            }
            Err(e) => {
                self.push_notification(&format!("Failed to adopt plugin: {e}"));
                sender.input(AppMsg::ScanExternalFiles);
            }
        }
    }

    pub(crate) fn handle_cmd_backup_restored(
        &mut self,
        result: Result<usize, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(count) => {
                self.push_notification(&format!(
                    "Restored {count} plugin{} from xEdit backup — plugin{} {} dirty edits",
                    if count == 1 { "" } else { "s" },
                    if count == 1 { "" } else { "s" },
                    if count == 1 { "has" } else { "have" },
                ));
                #[cfg(feature = "loot")]
                if self
                    .selected_game()
                    .map(|g| crate::core::loot_sort::game_has_loot_support(&g.id))
                    .unwrap_or(false)
                {
                    sender.input(AppMsg::SortWithLoot);
                }
                sender.input(AppMsg::ScanExternalFiles);
            }
            Err(e) => {
                self.push_notification(&format!("Failed to restore from backup: {e}"));
                sender.input(AppMsg::ScanExternalFiles);
            }
        }
    }

    pub(crate) fn handle_cmd_vanilla_baseline_reset(
        &mut self,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(()) => {
                self.push_notification("Vanilla baseline reset — rescanning…");
                sender.input(AppMsg::ScanExternalFiles);
            }
            Err(e) => self.push_notification(&format!("Reset failed: {e}")),
        }
    }

    pub(crate) fn handle_cmd_vanilla_entries_updated(
        &mut self,
        result: Result<usize, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(count) => {
                self.push_notification(&format!("Marked {count} file(s) as vanilla"));
                sender.input(AppMsg::ScanExternalFiles);
            }
            Err(e) => self.push_notification(&format!("Mark as vanilla failed: {e}")),
        }
    }
}
