use std::path::PathBuf;

use relm4::prelude::*;

use crate::models::download::{DownloadStatus, NexusIds};
use crate::utils::paths;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::types::{
    DownloadScanResult, ManualMetadataResult, NexusDownloadMetadata, NxmDownloadResult, WorkKind,
};

impl App {
    pub(crate) fn apply_nexus_download_metadata(
        &mut self,
        download_id: String,
        metadata: NexusDownloadMetadata,
    ) {
        let latest_version = metadata.latest_version.clone();
        self.handle_download_name_resolved(
            download_id.clone(),
            metadata.mod_name,
            Some(metadata.domain),
            metadata.nexus_file_name,
            metadata.nexus_is_primary,
            metadata.file_id,
            metadata.version,
            metadata.author,
        );

        let installed_mod_id = self
            .download
            .all
            .iter()
            .find(|entry| entry.id == download_id)
            .and_then(|download| {
                let ids = download.nexus_ids.as_ref()?;
                let guard = self.mods.rows.guard();
                guard
                    .iter()
                    .filter_map(|row| row.mod_row())
                    .find(|row| {
                        row.mod_entry.nexus_mod_id == Some(ids.mod_id)
                            && row.mod_entry.nexus_file_id == Some(ids.file_id)
                    })
                    .map(|row| row.mod_entry.id.clone())
            });
        if let Some(installed_mod_id) = installed_mod_id {
            let author = self
                .download
                .all
                .iter()
                .find(|entry| entry.id == download_id)
                .and_then(|entry| entry.author.clone())
                .unwrap_or_default();
            {
                let mut guard = self.mods.rows.guard();
                if let Some(row) = guard
                    .iter_mut()
                    .filter_map(|row| row.mod_row_mut())
                    .find(|row| row.mod_entry.id == installed_mod_id)
                {
                    row.mod_entry.latest_version = latest_version.clone();
                    if !author.is_empty() {
                        row.mod_entry.author = Some(author.clone());
                    }
                }
            }
        }
    }

    pub(crate) fn handle_cmd_downloads_scanned(
        &mut self,
        result: Result<DownloadScanResult, String>,
    ) {
        self.download.scan_in_progress = false;
        self.finish_work(WorkKind::ScanningDownloads);

        let scan = match result {
            Ok(scan) => scan,
            Err(error) => {
                self.download.initial_scan_done = true;
                self.push_notification(&format!("Downloads scan failed: {error}"));
                return;
            }
        };

        self.download.all = scan.entries;
        self.rebuild_downloads_view();

        if self.download.initial_scan_done && scan.new_count > 0 {
            self.show_toast(&format!("Found {} archive(s)", scan.new_count));
        }
        self.download.initial_scan_done = true;
    }

    pub(crate) fn handle_archive_md5_computed(
        &mut self,
        download_id: String,
        md5: String,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(entry) = self
            .download
            .all
            .iter_mut()
            .find(|entry| entry.id == download_id)
        {
            entry.archive_md5 = Some(md5.clone());
        }
        if let Some(tracker) = self.session.tracker.clone() {
            sender.oneshot_command(async move {
                let result = tracker
                    .update_download_archive_md5(&download_id, &md5)
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
    }

    pub(crate) fn handle_cmd_nexus_metadata_fetched(
        &mut self,
        download_id: String,
        result: Result<ManualMetadataResult, String>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(ManualMetadataResult::NeedsIdentity(candidates)) => {
                self.finish_download_metadata_fetch(&download_id);
                self.show_download_identity_dialog(download_id, candidates, root, sender);
            }
            Ok(result) => {
                let (metadata, needs_file_id) = match result {
                    ManualMetadataResult::Resolved(metadata) => (metadata, false),
                    ManualMetadataResult::NeedsFileId(metadata) => (metadata, true),
                    ManualMetadataResult::NeedsIdentity(_) => return,
                };
                let Some(tracker) = self.session.tracker.clone() else {
                    self.finish_download_metadata_fetch(&download_id);
                    self.push_notification(
                        "Metadata update could not be saved: database unavailable",
                    );
                    return;
                };
                let Some(mut entry) = self
                    .download
                    .all
                    .iter()
                    .find(|entry| entry.id == download_id)
                    .cloned()
                else {
                    self.finish_download_metadata_fetch(&download_id);
                    return;
                };
                // The busy status is transient; only the stable state belongs in the database.
                entry.status = self
                    .download
                    .metadata_previous_status
                    .get(&download_id)
                    .cloned()
                    .unwrap_or(crate::models::download::DownloadStatus::Downloaded);
                entry.status =
                    crate::models::download::DownloadStatus::restored_after_metadata_fetch(
                        &entry.status,
                    );
                entry.status_msg = entry.status.default_status_msg().to_string();
                entry.error_msg = None;
                let directory = self.download.directory.clone();
                sender.oneshot_command(async move {
                    let result = super::metadata::persist_manual_metadata(
                        &tracker, entry, &metadata, directory,
                    )
                    .await;
                    AppCmdMsg::Downloads(
                        crate::app::messages::DownloadsCmdMsg::NexusMetadataPersisted {
                            download_id,
                            metadata,
                            needs_file_id,
                            result: result.map(Box::new),
                        },
                    )
                });
            }
            Err(e) => {
                eprintln!("deployd: failed to fetch Nexus metadata: {e}");
                self.push_notification(&format!("Metadata fetch failed: {e}"));
                self.finish_download_metadata_fetch(&download_id);
            }
        }
    }

    pub(crate) fn handle_cmd_nexus_identity_persisted(
        &mut self,
        download_id: String,
        nexus_ids: NexusIds,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        self.finish_download_metadata_fetch(&download_id);
        match result {
            Ok(()) => {
                if let Some(entry) = self
                    .download
                    .all
                    .iter_mut()
                    .find(|entry| entry.id == download_id)
                {
                    set_confirmed_identity(entry, &nexus_ids);
                }
                let mut guard = self.download.rows.guard();
                for index in 0..guard.len() {
                    if let Some(row) = guard.get_mut(index)
                        && row.entry.id == download_id
                    {
                        set_confirmed_identity(&mut row.entry, &nexus_ids);
                        break;
                    }
                }
                drop(guard);
                self.start_nexus_metadata_fetch(download_id, sender, true);
            }
            Err(error) => {
                eprintln!("deployd: failed to persist Nexus identity: {error}");
                self.push_notification(&format!("Nexus identity could not be saved: {error}"));
            }
        }
    }

    pub(crate) fn handle_cmd_nexus_metadata_persisted(
        &mut self,
        download_id: String,
        metadata: NexusDownloadMetadata,
        needs_file_id: bool,
        result: Result<Box<crate::models::download::DownloadEntry>, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.finish_download_metadata_fetch(&download_id);
        match result {
            Ok(entry) => {
                let identity = entry.nexus_ids.clone();
                let name = metadata.mod_name.clone();
                if let Some(current) = self
                    .download
                    .all
                    .iter_mut()
                    .find(|current| current.id == download_id)
                {
                    *current = *entry;
                }
                self.apply_nexus_download_metadata(download_id.clone(), metadata);
                self.rebuild_downloads_view();
                if needs_file_id {
                    if let Some(ids) = identity {
                        let _ = sender.input_sender().send(AppMsg::Downloads(
                            crate::app::messages::DownloadsMsg::ShowFileIdDialog {
                                download_id,
                                mod_id: ids.mod_id,
                                domain: ids.domain,
                                partial_name: Some(name),
                            },
                        ));
                    }
                } else {
                    self.show_toast(&format!("Metadata updated: {name}"));
                }
            }
            Err(error) => {
                self.push_notification(&format!("Metadata update could not be saved: {error}"))
            }
        }
    }

    pub(crate) fn handle_cmd_nxm_download_complete(
        &mut self,
        nxm_download_id: String,
        result: Result<NxmDownloadResult, String>,
        _sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(nxm_result) => {
                let new_nexus_ids = Some(NexusIds {
                    mod_id: nxm_result.mod_id,
                    file_id: nxm_result.file_id,
                    domain: nxm_result.domain.clone(),
                });
                // Update backing store
                if let Some(entry) = self
                    .download
                    .all
                    .iter_mut()
                    .find(|e| e.id == nxm_result.download_id)
                {
                    entry.status = DownloadStatus::Downloaded;
                    entry.progress = 1.0;
                    entry.status_msg = "Download complete".to_string();
                    entry.archive_path = Some(nxm_result.archive_path.clone());
                    entry.nexus_ids = new_nexus_ids.clone();
                }
                // Update factory
                let mut guard = self.download.rows.guard();
                for i in 0..guard.len() {
                    if let Some(row) = guard.get_mut(i)
                        && row.entry.id == nxm_result.download_id
                    {
                        row.entry.status = DownloadStatus::Downloaded;
                        row.entry.progress = 1.0;
                        row.entry.status_msg = "Download complete".to_string();
                        row.entry.archive_path = Some(nxm_result.archive_path.clone());
                        row.entry.nexus_ids = new_nexus_ids;
                        break;
                    }
                }
                drop(guard);
                self.apply_nexus_download_metadata(
                    nxm_result.download_id.clone(),
                    nxm_result.metadata,
                );
                // When this was the last active download, rebuild the view so the
                // sort order and filter chips (Active/Completed) reflect the new status.
                if !self.download.all.iter().any(|e| e.is_active()) {
                    self.rebuild_downloads_view();
                } else {
                    self.refresh_download_counts();
                }

                self.show_toast(&format!("Download complete: {}", nxm_result.file_name));
            }
            Err(e) => {
                // Mark the specific NXM download as failed using the id that was
                // captured in the async closure.
                self.update_download_status(
                    &nxm_download_id,
                    DownloadStatus::Failed,
                    &format!("Failed: {e}"),
                );

                self.push_notification(&format!("Download failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_downloads_dir_updated(
        &mut self,
        result: Result<Option<PathBuf>, String>,
    ) {
        match result {
            Ok(Some(dir)) => self.download.directory = dir,
            Ok(None) => self.download.directory = paths::default_downloads_dir(),
            Err(error) => self.push_notification(&format!(
                "Failed to refresh the downloads directory: {error}"
            )),
        }
    }
}

fn set_confirmed_identity(entry: &mut crate::models::download::DownloadEntry, ids: &NexusIds) {
    if entry.nexus_ids.as_ref() != Some(ids) {
        entry.metadata_fetched = false;
        entry.nexus_file_name = None;
        entry.nexus_is_primary = false;
        entry.version = None;
        entry.author = None;
    }
    entry.nexus_ids = Some(ids.clone());
    entry.nexus_identity_source = crate::models::download::NexusIdentitySource::Confirmed;
}
