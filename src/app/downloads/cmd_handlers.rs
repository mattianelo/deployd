use std::path::PathBuf;

use relm4::prelude::*;

use crate::models::download::DownloadStatus;
use crate::utils::paths;

use super::super::App;
use super::super::messages::AppCmdMsg;
use super::super::types::NxmDownloadResult;

impl App {
    pub(crate) fn handle_cmd_nexus_metadata_fetched(
        &mut self,
        dl_id: Option<String>,
        result: Result<(String, String, String, String), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((_mod_id, version, author, nexus_name)) => {
                // Propagate the fetched name back to the download entry if it was
                // not already resolved by an earlier mechanism (e.g. NXM auto-fetch).
                if let Some(ref id) = dl_id {
                    let needs_update = self
                        .all_downloads
                        .iter()
                        .find(|e| &e.id == id)
                        .is_some_and(|e| !e.metadata_fetched);
                    if needs_update {
                        if let Some(entry) = self.all_downloads.iter_mut().find(|e| &e.id == id) {
                            entry.mod_name = nexus_name.clone();
                            entry.metadata_fetched = true;
                        }
                        {
                            let mut guard = self.downloads.guard();
                            for i in 0..guard.len() {
                                if let Some(row) = guard.get_mut(i)
                                    && row.entry.id == *id
                                {
                                    row.entry.mod_name = nexus_name.clone();
                                    row.entry.metadata_fetched = true;
                                    break;
                                }
                            }
                        }
                        if let Some(tracker) = self.tracker.clone()
                            && let Some(entry) =
                                self.all_downloads.iter().find(|e| &e.id == id).cloned()
                        {
                            sender.oneshot_command(async move {
                                let _ = tracker.save_download_entry(&entry).await;
                                AppCmdMsg::PrioritySaved(Ok(()))
                            });
                        }
                    }
                }
                // Reload mods to show updated metadata
                self.reload_mods(sender);
                self.toaster
                    .toast(&format!("{nexus_name} v{version} by {author}"));
            }
            Err(e) => {
                eprintln!("deployd: failed to fetch Nexus metadata: {e}");
                self.toaster.toast(&format!("Metadata fetch failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_updates_checked(
        &mut self,
        result: Result<Vec<(String, String, String)>, String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(updates) => {
                if updates.is_empty() {
                    self.toaster.toast("All mods are up to date");
                } else {
                    let names: Vec<_> = updates.iter().map(|(_, name, _)| name.as_str()).collect();
                    self.toaster.toast(&format!(
                        "{} mod(s) have updates: {}",
                        updates.len(),
                        names.join(", ")
                    ));
                    // Reload to show update indicators
                    self.reload_mods(sender);
                }
            }
            Err(e) => {
                self.toaster.toast(&format!("Update check failed: {e}"));
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
                let new_nexus_ids = Some((
                    nxm_result.mod_id,
                    nxm_result.file_id,
                    nxm_result.domain.clone(),
                ));
                // Update backing store
                if let Some(entry) = self
                    .all_downloads
                    .iter_mut()
                    .find(|e| e.id == nxm_result.download_id)
                {
                    entry.status = DownloadStatus::Downloaded;
                    entry.status_msg = "Download complete".to_string();
                    entry.archive_path = Some(nxm_result.archive_path.clone());
                    entry.nexus_ids = new_nexus_ids.clone();
                    entry.nexus_file_name = nxm_result.nexus_file_name.clone();
                    entry.nexus_is_primary = nxm_result.nexus_is_primary;
                    entry.metadata_fetched = true;
                }
                // Update factory
                let mut guard = self.downloads.guard();
                for i in 0..guard.len() {
                    if let Some(row) = guard.get_mut(i)
                        && row.entry.id == nxm_result.download_id
                    {
                        row.entry.status = DownloadStatus::Downloaded;
                        row.entry.status_msg = "Download complete".to_string();
                        row.entry.archive_path = Some(nxm_result.archive_path.clone());
                        row.entry.nexus_ids = new_nexus_ids;
                        row.entry.nexus_file_name = nxm_result.nexus_file_name.clone();
                        row.entry.nexus_is_primary = nxm_result.nexus_is_primary;
                        row.entry.metadata_fetched = true;
                        break;
                    }
                }
                drop(guard);
                // Only clear active_download_id if it belongs to this download.
                // If a concurrent extraction is in progress it will have overwritten
                // active_download_id with its own ID — don't clobber that.
                if self.active_download_id.as_deref() == Some(nxm_download_id.as_str()) {
                    self.active_download_id = None;
                }
                self.refresh_download_counts();

                // Persist completed download entry
                if let Some(tracker) = self.tracker.clone()
                    && let Some(entry) = self
                        .all_downloads
                        .iter()
                        .find(|e| e.id == nxm_result.download_id)
                {
                    let entry = entry.clone();
                    sender.oneshot_command(async move {
                        let _ = tracker.save_download_entry(&entry).await;
                        AppCmdMsg::PrioritySaved(Ok(()))
                    });
                }

                self.toaster
                    .toast(&format!("Download complete: {}", nxm_result.file_name));
            }
            Err(e) => {
                // Mark the specific NXM download as failed using the id that was
                // captured in the async closure, not active_download_id (which
                // may have been overwritten by a concurrent extraction).
                self.update_download_status(
                    &nxm_download_id,
                    DownloadStatus::Failed,
                    &format!("Failed: {e}"),
                );
                // Only clear active_download_id if it still refers to this download.
                if self.active_download_id.as_deref() == Some(nxm_download_id.as_str()) {
                    self.active_download_id = None;
                }

                self.toaster.toast(&format!("Download failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_downloads_dir_updated(&mut self, dir: Option<PathBuf>) {
        if let Some(dir) = dir {
            self.downloads_dir = dir;
        } else {
            self.downloads_dir = paths::default_downloads_dir();
        }
    }

    pub(crate) fn handle_cmd_app_update_result(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.toaster
                    .toast("Update downloaded. Restart deployd to use the new version.");
            }
            Err(e) => {
                self.toaster.toast(&format!("Update failed: {e}"));
                // For premium-related failures, open the Nexus page as a fallback.
                if e.contains("premium") {
                    let url = self
                        .app_update_url
                        .as_deref()
                        .unwrap_or(crate::core::update_check::NEXUS_PAGE_URL);
                    let _ = open::that(url);
                }
            }
        }
    }
}
