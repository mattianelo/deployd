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
        sender: &ComponentSender<Self>,
    ) {
        let page_version = metadata.page_version.clone();
        let summary = metadata.summary.clone();
        self.handle_download_name_resolved(
            download_id.clone(),
            metadata.mod_name,
            Some(metadata.domain),
            metadata.nexus_file_name,
            metadata.nexus_is_primary,
            metadata.file_id,
            metadata.version,
            metadata.author,
            sender,
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
        if let (Some(tracker), Some(installed_mod_id), Some(page_version)) =
            (self.session.tracker.clone(), installed_mod_id, page_version)
        {
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
                    row.mod_entry.latest_version = Some(page_version.clone());
                    if !author.is_empty() {
                        row.mod_entry.author = Some(author.clone());
                    }
                }
            }
            sender.oneshot_command(async move {
                let result = tracker
                    .update_mod_nexus_metadata(
                        &installed_mod_id,
                        &page_version,
                        &author,
                        summary.as_deref().unwrap_or(""),
                    )
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }
    }

    pub(crate) fn handle_cmd_downloads_scanned(
        &mut self,
        result: Result<DownloadScanResult, String>,
        sender: &ComponentSender<Self>,
    ) {
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

        if !scan.removed_ids.is_empty()
            && let Some(tracker) = self.session.tracker.clone()
        {
            let removed_ids = scan.removed_ids.clone();
            sender.oneshot_command(async move {
                let result = tracker
                    .delete_download_entries(&removed_ids)
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }

        if !scan.to_persist.is_empty()
            && let Some(tracker) = self.session.tracker.clone()
        {
            let to_persist = scan.to_persist.clone();
            sender.oneshot_command(async move {
                let result = async {
                    for entry in &to_persist {
                        tracker.save_download_entry(entry).await?;
                    }
                    anyhow::Ok(())
                }
                .await
                .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }

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
            entry.archive_md5 = Some(md5);
        }
        if let Some(tracker) = self.session.tracker.clone()
            && let Some(entry) = self
                .download
                .all
                .iter()
                .find(|entry| entry.id == download_id)
                .cloned()
        {
            sender.oneshot_command(async move {
                let result = tracker
                    .save_download_entry(&entry)
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
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(ManualMetadataResult::Resolved(metadata)) => {
                let toast = metadata.mod_name.clone();
                self.finish_download_metadata_fetch(&download_id);
                self.apply_nexus_download_metadata(download_id.clone(), metadata, sender);
                self.show_toast(&format!("Metadata updated: {toast}"));
            }
            Ok(ManualMetadataResult::NeedsFileId(metadata)) => {
                let mod_id = self
                    .download
                    .all
                    .iter()
                    .find(|entry| entry.id == download_id)
                    .and_then(|entry| entry.nexus_ids.as_ref())
                    .map(|ids| ids.mod_id);
                let domain = metadata.domain.clone();
                let partial_name = metadata.mod_name.clone();
                self.finish_download_metadata_fetch(&download_id);
                self.apply_nexus_download_metadata(download_id.clone(), metadata, sender);
                if let Some(mod_id) = mod_id {
                    let _ = sender.input_sender().send(AppMsg::Downloads(
                        crate::app::messages::DownloadsMsg::ShowFileIdDialog {
                            download_id,
                            mod_id,
                            domain,
                            partial_name: Some(partial_name),
                        },
                    ));
                }
            }
            Err(e) => {
                eprintln!("deployd: failed to fetch Nexus metadata: {e}");
                self.push_notification(&format!("Metadata fetch failed: {e}"));
                self.finish_download_metadata_fetch(&download_id);
            }
        }
    }

    pub(crate) fn handle_cmd_nxm_download_complete(
        &mut self,
        nxm_download_id: String,
        result: Result<NxmDownloadResult, String>,
        sender: &ComponentSender<Self>,
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
                    sender,
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
