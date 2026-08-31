use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg, PrepareResultMsg};
use super::super::progress::throttled_download_install_progress;
use super::super::types::WorkKind;
use crate::core::installer::PrepareResult;
use crate::core::{game, installer};
use crate::models::download::{DownloadEntry, DownloadStatus, NexusIds};

fn metadata_identifies_file(file_name: Option<&str>, file_id: Option<i64>) -> bool {
    file_name.is_some_and(|name| !name.trim().is_empty()) || file_id.is_some_and(|id| id > 0)
}

fn reset_download_metadata(entry: &mut DownloadEntry) {
    let previous_domain = entry
        .game_domain
        .clone()
        .or_else(|| entry.nexus_ids.as_ref().map(|ids| ids.domain.clone()))
        .unwrap_or_default();
    entry.nexus_ids = entry.archive_path.as_ref().and_then(|path| {
        let file_name = path.file_name()?.to_string_lossy();
        crate::core::nexus_identity::parse_nexus_mod_id(&file_name).map(|mod_id| NexusIds {
            mod_id,
            file_id: 0,
            domain: previous_domain,
        })
    });
    if let Some(path) = entry.archive_path.as_ref() {
        entry.mod_name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
    }
    entry.metadata_fetched = false;
    entry.nexus_file_name = None;
    entry.nexus_is_primary = false;
    entry.version = None;
    entry.author = None;
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::models::download::{DownloadEntry, NexusIds};

    use super::{metadata_identifies_file, reset_download_metadata};

    #[test]
    fn partial_mod_metadata_does_not_complete_file_metadata() {
        assert!(!metadata_identifies_file(None, None));
        assert!(!metadata_identifies_file(Some("  "), Some(0)));
    }

    #[test]
    fn exact_file_metadata_completes_metadata() {
        assert!(metadata_identifies_file(Some("Archived file"), None));
        assert!(metadata_identifies_file(None, Some(42)));
    }

    #[test]
    fn clear_metadata_rebuilds_identity_from_current_nexus_filename() {
        let mut entry = DownloadEntry::new(
            "id".to_string(),
            "Wrong page name".to_string(),
            Some(NexusIds {
                mod_id: 8,
                file_id: 0,
                domain: "fallout4".to_string(),
            }),
        );
        entry.archive_path = Some(PathBuf::from(
            "Dynamic Grass 108480 1.3.0 2026-08-31T12-00Z Gpr9A6gVu.zip",
        ));
        entry.metadata_fetched = true;
        entry.nexus_file_name = Some("Wrong file".to_string());
        entry.version = Some("9.9".to_string());

        reset_download_metadata(&mut entry);

        assert_eq!(
            entry.nexus_ids,
            Some(NexusIds {
                mod_id: 108_480,
                file_id: 0,
                domain: "fallout4".to_string(),
            })
        );
        assert_eq!(
            entry.mod_name,
            "Dynamic Grass 108480 1.3.0 2026-08-31T12-00Z Gpr9A6gVu"
        );
        assert!(!entry.metadata_fetched);
        assert_eq!(entry.nexus_file_name, None);
        assert_eq!(entry.version, None);
    }
}

impl App {
    pub(crate) fn handle_set_downloads_visible(&mut self, visible: bool) {
        self.download.visible = visible;
    }

    pub(crate) fn handle_download_sort_changed(&mut self, idx: u32) {
        let new_sort = match idx {
            1 => crate::models::download::DownloadSort::Name,
            2 => crate::models::download::DownloadSort::Status,
            _ => crate::models::download::DownloadSort::Default,
        };
        // GTK may emit notify::selected even when the value didn't change (e.g. during
        // a #[watch] set_selected call). Skip the rebuild to avoid spurious redraws and
        // to prevent the factory from being cleared while a download is in progress.
        if new_sort == self.download.sort {
            return;
        }
        self.download.sort = new_sort;
        self.rebuild_downloads_view();
    }

    pub(crate) fn handle_clear_download_metadata(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let download_id = {
            let guard = self.download.rows.guard();
            guard.get(idx).map(|r| r.entry.id.clone())
        };
        let Some(download_id) = download_id else {
            return;
        };
        // Reset metadata in backing store
        if let Some(entry) = self.download.all.iter_mut().find(|e| e.id == download_id) {
            reset_download_metadata(entry);
            // If the archive is still present, allow reinstall
            if entry.status == DownloadStatus::Installed && entry.archive_path.is_some() {
                entry.status = DownloadStatus::Downloaded;
                entry.status_msg = String::new();
            }
        }
        // Update factory
        {
            let mut guard = self.download.rows.guard();
            if let Some(row) = guard.get_mut(idx) {
                reset_download_metadata(&mut row.entry);
                if row.entry.status == DownloadStatus::Installed && row.entry.archive_path.is_some()
                {
                    row.entry.status = DownloadStatus::Downloaded;
                    row.entry.status_msg = String::new();
                }
            }
        }
        // Persist
        if let Some(tracker) = self.session.tracker.clone()
            && let Some(entry) = self.download.all.iter().find(|e| e.id == download_id)
        {
            let entry = entry.clone();
            sender.oneshot_command(async move {
                let result = tracker
                    .save_download_entry(&entry)
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
    }

    pub(crate) fn handle_rename_download(
        &mut self,
        index: DynamicIndex,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (download_id, current_name) = {
            let guard = self.download.rows.guard();
            let Some(row) = guard.get(idx) else {
                return;
            };
            (row.entry.id.clone(), row.entry.mod_name.clone())
        };

        let entry = gtk::Entry::builder()
            .text(&current_name)
            .hexpand(true)
            .activates_default(true)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        let dialog = adw::AlertDialog::builder()
            .heading("Rename Download")
            .build();
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("apply", "Apply");
        dialog.set_default_response(Some("apply"));
        dialog.set_close_response("cancel");

        let input_sender = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "apply" {
                let name = entry.text().to_string();
                if !name.is_empty() {
                    input_sender
                        .send(AppMsg::Downloads(
                            crate::app::messages::DownloadsMsg::ConfirmDownloadRename(
                                download_id.clone(),
                                name,
                            ),
                        ))
                        .ok();
                }
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_confirm_download_rename(
        &mut self,
        download_id: String,
        new_name: String,
        sender: &ComponentSender<Self>,
    ) {
        // Update backing store
        if let Some(entry) = self.download.all.iter_mut().find(|e| e.id == download_id) {
            entry.mod_name = new_name.clone();
        }
        // Update factory in-place (no full rebuild needed)
        {
            let mut guard = self.download.rows.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i)
                    && row.entry.id == download_id
                {
                    row.entry.mod_name = new_name.clone();
                    break;
                }
            }
        }
        // Persist
        if let Some(tracker) = self.session.tracker.clone()
            && let Some(entry) = self.download.all.iter().find(|e| e.id == download_id)
        {
            let entry = entry.clone();
            sender.oneshot_command(async move {
                let result = tracker
                    .save_download_entry(&entry)
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
    }

    pub(crate) fn handle_reinstall_download(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        self.install.reinstalling = true;
        self.handle_install_download(index, sender);
    }

    pub(crate) fn handle_install_download(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (archive_path, nexus_ids, download_id, suggested_name) = {
            let guard = self.download.rows.guard();
            let Some(row) = guard.get(idx) else {
                return;
            };
            let Some(path) = row.entry.archive_path.clone() else {
                return;
            };
            // Build suggested name: prefer Nexus mod name + always append the per-file
            // label so multiple files from the same mod page get distinct mod names.
            let suggested = if row.entry.metadata_fetched {
                let mut name = row.entry.mod_name.clone();
                if let Some(ref file_name) = row.entry.nexus_file_name
                    && file_name != &name
                {
                    // If the file name already contains the mod name, use the file name
                    // directly to avoid "Mod Name - Abbrev - Mod Name" duplication.
                    if file_name.to_lowercase().contains(&name.to_lowercase()) {
                        name = file_name.clone();
                    } else {
                        name = format!("{name} - {file_name}");
                    }
                }
                name
            } else {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            };
            (
                path,
                row.entry.nexus_ids.clone(),
                row.entry.id.clone(),
                suggested,
            )
        };

        // Switch to correct game based on nexus domain
        if let Some(NexusIds { ref domain, .. }) = nexus_ids
            && let Some(target_game_id) = game::game_id_for_nexus_domain(domain)
            && let Some(game_idx) = self
                .session
                .games
                .iter()
                .position(|g| g.id == target_game_id)
            && game_idx != self.session.selected_game_idx
        {
            self.session.selected_game_idx = game_idx;
            self.ui.game_dropdown.set_selected(game_idx as u32);
        }

        // Store nexus_ids for the PendingInstall handoff
        self.install.nexus_ids = nexus_ids.clone();
        self.install.active_download_id = Some(download_id.clone());
        let game_id = self
            .selected_game()
            .map(|game| game.id.clone())
            .unwrap_or_default();
        let identity = self.install.begin(game_id, Some(download_id.clone()));

        self.update_download_status(
            &download_id,
            DownloadStatus::Extracting,
            "Hashing archive...",
        );

        // Feed into existing install pipeline
        self.install
            .set_stage(crate::app::state::InstallStage::PreparingArchive);
        self.begin_work(WorkKind::PreparingArchive, "Hashing archive...");

        let extract_sender = sender.input_sender().clone();
        let on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
            Some(throttled_download_install_progress(
                extract_sender,
                identity.clone(),
                download_id.clone(),
                "Extracting archive...",
            ));
        let processing_sender = sender.input_sender().clone();
        let processing_download_id = download_id.clone();
        let processing_identity = identity.clone();
        let on_processing: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
            let _ = processing_sender.send(AppMsg::Install(
                crate::app::messages::InstallMsg::InstallProgress(
                    processing_identity,
                    1.0,
                    "Reading FOMOD...".to_string(),
                ),
            ));
            let _ = processing_sender.send(AppMsg::Downloads(
                crate::app::messages::DownloadsMsg::DownloadProgress(
                    processing_download_id,
                    1.0,
                    "Reading FOMOD...".to_string(),
                ),
            ));
        }));

        sender.oneshot_command(async move {
            let result: Result<PrepareResultMsg, crate::app::messages::PrepareFailure> = async {
                let timing_start = std::time::Instant::now();
                let hash_path = archive_path.clone();
                let archive_path_str = Some(archive_path.to_string_lossy().to_string());
                let archive_hash = tokio::task::spawn_blocking(move || {
                    crate::core::archive::hash_archive_file(&hash_path).ok()
                })
                .await
                .unwrap_or(None);
                crate::app::timing::log_phase(
                    "install.hash_archive",
                    "download",
                    timing_start,
                    Some(1),
                );

                let timing_start = std::time::Instant::now();
                let archive_label = archive_path.display().to_string();
                let prepare =
                    installer::prepare_mod(&archive_path, on_extract_progress, on_processing)
                        .await
                        .map_err(|error| {
                            crate::app::messages::PrepareFailure::notification(format!(
                                "{error:#}\nArchive: {archive_label}"
                            ))
                        })?;
                crate::app::timing::log_phase(
                    "install.prepare_archive",
                    "download",
                    timing_start,
                    None,
                );
                let mod_name = suggested_name;
                match prepare {
                    PrepareResult::Normal {
                        file_list,
                        stripped_wrapper,
                        tmp_dir,
                    } => Ok(PrepareResultMsg::Normal {
                        file_list,
                        stripped_wrapper,
                        tmp_dir,
                        mod_name,
                        archive_hash,
                        archive_path: archive_path_str,
                    }),
                    PrepareResult::Fomod {
                        config,
                        config_path,
                        tmp_dir,
                    } => Ok(PrepareResultMsg::Fomod {
                        config,
                        config_path,
                        tmp_dir,
                        mod_name,
                        archive_hash,
                        archive_path: archive_path_str,
                    }),
                }
            }
            .await;
            AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::ModPrepared(
                identity,
                Box::new(result),
            ))
        });
    }

    pub(crate) fn handle_download_progress(
        &mut self,
        download_id: String,
        fraction: f64,
        msg: String,
    ) {
        let Some(entry) = self.download.all.iter_mut().find(|e| e.id == download_id) else {
            return;
        };
        if !entry.is_active() {
            return;
        }
        entry.progress = fraction;
        entry.status_msg = msg.clone();

        // Update factory
        let mut guard = self.download.rows.guard();
        for i in 0..guard.len() {
            if let Some(row) = guard.get_mut(i)
                && row.entry.id == download_id
            {
                row.entry.progress = fraction;
                row.entry.status_msg = msg;
                break;
            }
        }
    }

    // All arguments come from a single NxmResolved message variant; a wrapper struct
    // would add indirection without improving clarity.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn handle_download_name_resolved(
        &mut self,
        download_id: String,
        name: String,
        game_domain: Option<String>,
        nexus_file_name: Option<String>,
        nexus_is_primary: bool,
        resolved_file_id: Option<i64>,
        version: Option<String>,
        author: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        let metadata_fetched =
            metadata_identifies_file(nexus_file_name.as_deref(), resolved_file_id);
        // Capture old domain before mutation to detect filtering changes
        let old_domain = self
            .download
            .all
            .iter()
            .find(|e| e.id == download_id)
            .and_then(|e| e.game_domain.clone());
        // Update backing store
        if let Some(entry) = self.download.all.iter_mut().find(|e| e.id == download_id) {
            entry.mod_name = name.clone();
            entry.metadata_fetched = metadata_fetched;
            entry.nexus_file_name = nexus_file_name.clone();
            entry.nexus_is_primary = nexus_is_primary;
            if version.is_some() {
                entry.version = version.clone();
            }
            if author.is_some() {
                entry.author = author.clone();
            }
            if let Some(fid) = resolved_file_id
                && let Some(NexusIds {
                    file_id: ref mut stored_fid,
                    ..
                }) = entry.nexus_ids
                && *stored_fid == 0
            {
                *stored_fid = fid;
            }
            if let Some(ref domain) = game_domain {
                entry.game_domain = Some(domain.clone());
                if let Some(NexusIds {
                    domain: ref mut dom,
                    ..
                }) = entry.nexus_ids
                {
                    *dom = domain.clone();
                }
                // Auto-move archive from flat root to per-game subfolder
                if let Some(ref archive_path) = entry.archive_path
                    && archive_path.parent() == Some(self.download.directory.as_path())
                {
                    let target_dir = self.download.directory.join(domain);
                    if let Some(file_name) = archive_path.file_name() {
                        let _ = std::fs::create_dir_all(&target_dir);
                        let target = target_dir.join(file_name);
                        if std::fs::rename(archive_path, &target).is_ok() {
                            entry.archive_path = Some(target);
                        }
                    }
                }
            }
        }
        // Persist updated entry
        if let Some(tracker) = self.session.tracker.clone()
            && let Some(entry) = self.download.all.iter().find(|e| e.id == download_id)
        {
            let entry = entry.clone();
            sender.oneshot_command(async move {
                let result = tracker
                    .save_download_entry(&entry)
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
        // Update factory
        {
            let mut guard = self.download.rows.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i)
                    && row.entry.id == download_id
                {
                    row.entry.mod_name = name;
                    row.entry.metadata_fetched = metadata_fetched;
                    row.entry.nexus_file_name = nexus_file_name;
                    row.entry.nexus_is_primary = nexus_is_primary;
                    if version.is_some() {
                        row.entry.version = version.clone();
                    }
                    if author.is_some() {
                        row.entry.author = author.clone();
                    }
                    if let Some(fid) = resolved_file_id
                        && let Some(NexusIds {
                            file_id: ref mut stored_fid,
                            ..
                        }) = row.entry.nexus_ids
                        && *stored_fid == 0
                    {
                        *stored_fid = fid;
                    }
                    if let Some(ref domain) = game_domain {
                        row.entry.game_domain = Some(domain.clone());
                        if let Some(NexusIds {
                            domain: ref mut dom,
                            ..
                        }) = row.entry.nexus_ids
                        {
                            *dom = domain.clone();
                        }
                    }
                    break;
                }
            }
        }
        // Propagate resolved version and author to the installed mod row so the Mod Order panel shows them.
        if let Some(ref v) = version
            && let Some(entry) = self.download.all.iter().find(|e| e.id == download_id)
            && let Some(NexusIds {
                mod_id: nxs_mod_id,
                file_id: nxs_file_id,
                ..
            }) = entry.nexus_ids
            && nxs_file_id != 0
            && let Some(tracker) = self.session.tracker.clone()
            && let Some(game) = self.selected_game().cloned()
        {
            let version_db = v.clone();
            let version_ui = v.clone();
            let author_db = author.clone();
            let author_ui = author.clone();
            let game_id = game.id.clone();
            sender.oneshot_command(async move {
                let result = if let Some(ref a) = author_db {
                    tracker
                        .update_mod_version_author_by_nexus_ids(
                            &game_id,
                            nxs_mod_id,
                            nxs_file_id,
                            &version_db,
                            a,
                        )
                        .await
                        .map(|_| ())
                } else {
                    tracker
                        .update_mod_version_by_nexus_ids(
                            &game_id,
                            nxs_mod_id,
                            nxs_file_id,
                            &version_db,
                        )
                        .await
                        .map(|_| ())
                }
                .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
            // Surgically update the matching factory row so the subtitle appears without a reload.
            let mut guard = self.mods.rows.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i)
                    && let Some(init) = row.mod_row_mut()
                    && init.mod_entry.nexus_mod_id == Some(nxs_mod_id)
                    && init.mod_entry.nexus_file_id == Some(nxs_file_id)
                {
                    init.mod_entry.version = Some(version_ui.clone());
                    if let Some(a) = author_ui {
                        init.mod_entry.author = Some(a);
                    }
                    break;
                }
            }
        }
        if let Some(entry) = self.download.all.iter().find(|e| e.id == download_id)
            && let Some(NexusIds {
                mod_id: nxs_mod_id,
                file_id: nxs_file_id,
                ..
            }) = entry.nexus_ids
            && nxs_file_id != 0
            && let Some(tracker) = self.session.tracker.clone()
            && let Some(game) = self.selected_game().cloned()
        {
            let game_id = game.id.clone();
            let source_file_name = entry.nexus_file_name.clone();
            let source_is_primary = entry.nexus_is_primary;
            let source_md5 = entry.archive_md5.clone();
            sender.oneshot_command(async move {
                let result = tracker
                    .update_mod_source_metadata_by_nexus_ids(
                        &game_id,
                        nxs_mod_id,
                        nxs_file_id,
                        source_file_name.as_deref(),
                        source_is_primary,
                        source_md5.as_deref(),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
        // Rebuild views only if game_domain actually changed (affects filtering)
        if game_domain.is_some() && old_domain != game_domain {
            self.rebuild_downloads_view();
        }
    }

    pub(crate) fn handle_pause_download(&mut self, index: DynamicIndex) {
        let idx = index.current_index();
        let download_id = {
            let guard = self.download.rows.guard();
            guard.get(idx).map(|r| r.entry.id.clone())
        };
        let Some(download_id) = download_id else {
            return;
        };
        self.update_download_status(&download_id, DownloadStatus::Paused, "Paused");
    }

    pub(crate) fn handle_resume_download(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (download_id, nexus_ids) = {
            let guard = self.download.rows.guard();
            let Some(row) = guard.get(idx) else { return };
            (row.entry.id.clone(), row.entry.nexus_ids.clone())
        };
        // Only resume if actually paused
        let is_paused = self
            .download
            .all
            .iter()
            .any(|e| e.id == download_id && e.status == DownloadStatus::Paused);
        if !is_paused {
            return;
        }
        // Re-request the NXM download URL from Nexus and re-enqueue
        if let Some(NexusIds {
            mod_id,
            file_id,
            ref domain,
        }) = nexus_ids
        {
            let uri = format!("nxm://{domain}/mods/{mod_id}/files/{file_id}");
            sender.input(AppMsg::Downloads(
                crate::app::messages::DownloadsMsg::NxmLinkReceived(uri),
            ));
        }
    }

    pub(crate) fn handle_delete_download(
        &mut self,
        index: DynamicIndex,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (download_id, mod_name, archive_path) = {
            let guard = self.download.rows.guard();
            let Some(row) = guard.get(idx) else { return };
            (
                row.entry.id.clone(),
                row.entry.mod_name.clone(),
                row.entry.archive_path.clone(),
            )
        };

        let dialog = adw::AlertDialog::builder()
            .heading("Move Download to Trash")
            .body(format!(
                "Move \"{}\" and its archive file to Trash?",
                mod_name
            ))
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("trash", "Move to Trash");
        dialog.set_response_appearance("trash", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let input_sender = sender.input_sender().clone();
        let command_sender = sender.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "trash" {
                if let Some(path) = archive_path.clone() {
                    let download_id = download_id.clone();
                    command_sender.oneshot_command(async move {
                        AppCmdMsg::Downloads(
                            crate::app::messages::DownloadsCmdMsg::DownloadArchiveTrashed {
                                download_id,
                                result: crate::utils::portal::trash_file(path)
                                    .await
                                    .map_err(|e| e.to_string()),
                            },
                        )
                    });
                } else {
                    input_sender
                        .send(AppMsg::Downloads(
                            crate::app::messages::DownloadsMsg::ConfirmDeleteDownload(
                                download_id.clone(),
                            ),
                        ))
                        .ok();
                }
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_download_archive_trashed(
        &mut self,
        download_id: String,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(()) => {
                self.show_toast("Download moved to Trash");
                self.handle_confirm_delete_download(download_id, sender);
            }
            Err(e) => {
                self.push_notification(&format!("Could not move download archive to Trash: {e}"));
            }
        }
    }

    pub(crate) fn handle_confirm_delete_download(
        &mut self,
        download_id: String,
        sender: &ComponentSender<Self>,
    ) {
        self.download.all.retain(|e| e.id != download_id);
        {
            let mut guard = self.download.rows.guard();
            for i in (0..guard.len()).rev() {
                if guard.get(i).is_some_and(|r| r.entry.id == download_id) {
                    guard.remove(i);
                    break;
                }
            }
        }
        if let Some(tracker) = self.session.tracker.clone() {
            let id = download_id.clone();
            sender.oneshot_command(async move {
                let result = tracker
                    .delete_download_entries(&[id])
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
        self.refresh_download_counts();
    }

    pub(crate) fn handle_hide_download(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let download_id = {
            let guard = self.download.rows.guard();
            guard.get(idx).map(|r| r.entry.id.clone())
        };
        let Some(download_id) = download_id else {
            return;
        };

        if let Some(entry) = self.download.all.iter_mut().find(|e| e.id == download_id) {
            entry.hidden = !entry.hidden;
        }
        if let Some(tracker) = self.session.tracker.clone()
            && let Some(entry) = self.download.all.iter().find(|e| e.id == download_id)
        {
            let entry = entry.clone();
            sender.oneshot_command(async move {
                let result = tracker
                    .save_download_entry(&entry)
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
        self.rebuild_downloads_view();
    }

    pub(crate) fn handle_set_show_hidden_downloads(&mut self, show: bool) {
        self.download.show_hidden = show;
        self.rebuild_downloads_view();
    }
}
