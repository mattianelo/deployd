use std::path::PathBuf;

use relm4::prelude::*;

use crate::models::download::{DownloadStatus, NexusIds};
use crate::utils::paths;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::types::{DownloadScanResult, NxmDownloadResult, WorkKind};

impl App {
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
                let _ = tracker.delete_download_entries(&removed_ids).await;
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(Ok(())))
            });
        }

        if !scan.to_persist.is_empty()
            && let Some(tracker) = self.session.tracker.clone()
        {
            let to_persist = scan.to_persist.clone();
            sender.oneshot_command(async move {
                for entry in &to_persist {
                    let _ = tracker.save_download_entry(entry).await;
                }
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(Ok(())))
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
                let _ = tracker.save_download_entry(&entry).await;
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(Ok(())))
            });
        }
    }

    pub(crate) fn handle_cmd_nexus_metadata_fetched(
        &mut self,
        dl_id: Option<String>,
        result: Result<(String, String, String, String, Option<String>), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((mod_id, version, author, nexus_name, nexus_file_name)) => {
                // Propagate the fetched name back to the download entry if it was
                // not already resolved by an earlier mechanism (e.g. NXM auto-fetch).
                if let Some(ref id) = dl_id {
                    self.finish_download_metadata_fetch(id);
                    let needs_update = self
                        .download
                        .all
                        .iter()
                        .find(|e| &e.id == id)
                        .is_some_and(|e| !e.metadata_fetched);
                    if needs_update {
                        if let Some(entry) = self.download.all.iter_mut().find(|e| &e.id == id) {
                            entry.mod_name = nexus_name.clone();
                            if entry.nexus_file_name.is_none() {
                                entry.nexus_file_name = nexus_file_name.clone();
                            }
                            entry.metadata_fetched = true;
                            if !author.is_empty() {
                                entry.author = Some(author.clone());
                            }
                        }
                        {
                            let mut guard = self.download.rows.guard();
                            for i in 0..guard.len() {
                                if let Some(row) = guard.get_mut(i)
                                    && row.entry.id == *id
                                {
                                    row.entry.mod_name = nexus_name.clone();
                                    if row.entry.nexus_file_name.is_none() {
                                        row.entry.nexus_file_name = nexus_file_name.clone();
                                    }
                                    row.entry.metadata_fetched = true;
                                    if !author.is_empty() {
                                        row.entry.author = Some(author.clone());
                                    }
                                    break;
                                }
                            }
                        }
                        if let Some(tracker) = self.session.tracker.clone()
                            && let Some(entry) =
                                self.download.all.iter().find(|e| &e.id == id).cloned()
                        {
                            sender.oneshot_command(async move {
                                let _ = tracker.save_download_entry(&entry).await;
                                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(
                                    Ok(()),
                                ))
                            });
                        }
                    }
                }
                let mut guard = self.mods.rows.guard();
                for i in 0..guard.len() {
                    if let Some(row) = guard.get_mut(i)
                        && let Some(init) = row.mod_row_mut()
                        && init.mod_entry.id == mod_id
                    {
                        init.mod_entry.version = Some(version);
                        if !author.is_empty() {
                            init.mod_entry.author = Some(author);
                        }
                        break;
                    }
                }
            }
            Err(e) => {
                eprintln!("deployd: failed to fetch Nexus metadata: {e}");
                self.push_notification(&format!("Metadata fetch failed: {e}"));
                if let Some(ref id) = dl_id {
                    self.finish_download_metadata_fetch(id);
                }
                // For disk-scanned entries with a known mod_id but unresolved file_id,
                // offer the dialog so the user can at least store the file_id for the next retry.
                if let Some(ref id) = dl_id
                    && let Some(entry) = self.download.all.iter().find(|e| &e.id == id)
                    && let Some(NexusIds {
                        mod_id,
                        file_id: 0,
                        ref domain,
                    }) = entry.nexus_ids
                {
                    let _ = sender.input_sender().send(AppMsg::Downloads(
                        crate::app::messages::DownloadsMsg::ShowFileIdDialog {
                            download_id: id.clone(),
                            mod_id,
                            domain: domain.clone(),
                            partial_name: None,
                        },
                    ));
                }
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
                    entry.nexus_file_name = nxm_result.nexus_file_name.clone();
                    entry.nexus_is_primary = nxm_result.nexus_is_primary;
                    entry.version = nxm_result.version.clone();
                    entry.metadata_fetched = true;
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
                        row.entry.nexus_file_name = nxm_result.nexus_file_name.clone();
                        row.entry.nexus_is_primary = nxm_result.nexus_is_primary;
                        row.entry.version = nxm_result.version.clone();
                        row.entry.metadata_fetched = true;
                        break;
                    }
                }
                drop(guard);
                // When this was the last active download, rebuild the view so the
                // sort order and filter chips (Active/Completed) reflect the new status.
                if !self.download.all.iter().any(|e| e.is_active()) {
                    self.rebuild_downloads_view();
                } else {
                    self.refresh_download_counts();
                }

                // Persist completed download entry
                if let Some(tracker) = self.session.tracker.clone()
                    && let Some(entry) = self
                        .download
                        .all
                        .iter()
                        .find(|e| e.id == nxm_result.download_id)
                {
                    let entry = entry.clone();
                    sender.oneshot_command(async move {
                        let _ = tracker.save_download_entry(&entry).await;
                        AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(Ok(())))
                    });
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

    pub(crate) fn handle_cmd_downloads_dir_updated(&mut self, dir: Option<PathBuf>) {
        if let Some(dir) = dir {
            self.download.directory = dir;
        } else {
            self.download.directory = paths::default_downloads_dir();
        }
    }
}
