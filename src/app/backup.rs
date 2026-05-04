use std::path::PathBuf;

use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use super::App;
use super::free_fns::load_game_data;
use super::messages::{AppCmdMsg, AppMsg};

impl App {
    pub(crate) fn handle_create_full_backup_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            self.push_notification("Database not ready");
            return;
        };
        let dialog = gtk::FileDialog::builder()
            .title("Save Backup")
            .initial_name("deployd-backup.deployd-backup")
            .build();
        let input_sender = sender.input_sender().clone();
        dialog.save(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let tracker = tracker.clone();
                gtk::glib::spawn_future_local(async move {
                    let result = crate::core::backup::create_full_backup(&path, &tracker)
                        .await
                        .map_err(|e| e.to_string());
                    let _ = input_sender.send(AppMsg::FullBackupCreated(result));
                });
            }
        });
    }

    pub(crate) fn handle_full_backup_created(
        &mut self,
        result: Result<crate::models::backup::BackupManifest, String>,
    ) {
        match result {
            Ok(manifest) => {
                let game_count = manifest.games.len();
                let profile_count: usize = manifest.games.iter().map(|g| g.profile_count).sum();
                self.show_toast(&format!(
                    "Backup created — {game_count} game(s), {profile_count} profile(s)"
                ));
            }
            Err(e) => self.push_notification(&format!("Backup failed: {e}")),
        }
    }

    pub(crate) fn handle_restore_from_backup_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Deployd Backup (*.deployd-backup)"));
        filter.add_pattern("*.deployd-backup");
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Open Backup")
            .filters(&filters)
            .build();
        let input_sender = sender.input_sender().clone();
        dialog.open(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let _ = input_sender.send(AppMsg::RestoreBackupFileChosen(path));
            }
        });
    }

    pub(crate) fn handle_restore_backup_file_chosen(
        &mut self,
        path: PathBuf,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let manifest = match crate::core::backup::read_backup_manifest(&path) {
            Ok(m) => m,
            Err(e) => {
                self.push_notification(&format!("Cannot read backup: {e}"));
                return;
            }
        };

        let game_lines: String = manifest
            .games
            .iter()
            .map(|g| {
                format!(
                    "• {} — {} mod(s), {} profile(s)",
                    g.title, g.mod_count, g.profile_count
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        let detail = format!(
            "Created: {}\nDeployd version: {}\n\n{game_lines}\n\n\
             \"Replace Database\" overwrites all current data and requires a restart.\n\
             \"Import Profiles\" merges profiles for the current game only.",
            manifest.created_at, manifest.deployd_version,
        );

        let dialog = gtk::AlertDialog::builder()
            .message("Restore from Backup")
            .detail(detail)
            .buttons(["Cancel", "Import Profiles", "Replace Database"])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();

        let stage_path = path.clone();
        let import_path = path;
        let s = sender.input_sender().clone();
        dialog.choose(Some(root), None::<&gio::Cancellable>, move |r| match r {
            Ok(1) => {
                let _ = s.send(AppMsg::ImportProfilesFromBackup(import_path));
            }
            Ok(2) => {
                let _ = s.send(AppMsg::StageFullRestore(stage_path));
            }
            _ => {}
        });
    }

    pub(crate) fn handle_stage_full_restore(
        &mut self,
        path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        sender.oneshot_command(async move {
            AppCmdMsg::FullRestoreStaged(
                tokio::task::spawn_blocking(move || {
                    crate::core::backup::stage_full_restore(&path).map_err(|e| e.to_string())
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string())),
            )
        });
    }

    pub(crate) fn handle_cmd_full_restore_staged(
        &mut self,
        result: Result<crate::models::backup::BackupManifest, String>,
        root: &adw::ApplicationWindow,
    ) {
        match result {
            Ok(_) => {
                let dialog = gtk::AlertDialog::builder()
                    .message("Restart Required")
                    .detail(
                        "The backup will be applied on the next launch.\n\n\
                         Note: mod files are not included in the backup. \
                         After restarting, re-install your mods from archives \
                         to restore a deployable state.",
                    )
                    .buttons(["OK"])
                    .default_button(0)
                    .modal(true)
                    .build();
                dialog.choose(Some(root), None::<&gio::Cancellable>, |_| {});
            }
            Err(e) => self.push_notification(&format!("Restore staging failed: {e}")),
        }
    }

    pub(crate) fn handle_import_profiles_from_backup(
        &mut self,
        path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            self.push_notification("Database not ready");
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            self.push_notification("No game selected");
            return;
        };
        sender.oneshot_command(async move {
            AppCmdMsg::ProfilesImportedFromBackup(
                async {
                    let summary =
                        crate::core::backup::import_profiles_from_backup(&path, &game.id, &tracker)
                            .await
                            .map_err(|e| e.to_string())?;
                    let data = load_game_data(&tracker, &game, false)
                        .await
                        .map_err(|e| e.to_string())?;
                    Ok((summary.imported.len(), summary.skipped_mods, data))
                }
                .await,
            )
        });
    }

    pub(crate) fn handle_cmd_profiles_imported_from_backup(
        &mut self,
        result: Result<(usize, usize, crate::app::types::LoadedData), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((imported, skipped_mods, data)) => {
                self.apply_loaded_data(data, sender);
                if skipped_mods > 0 {
                    self.push_notification(&format!(
                        "Imported {imported} profile(s) from backup — \
                         {skipped_mods} mod(s) not matched (install them first, then re-import)"
                    ));
                } else {
                    self.push_notification(&format!("Imported {imported} profile(s) from backup"));
                }
            }
            Err(e) => self.push_notification(&format!("Profile import failed: {e}")),
        }
    }
}
