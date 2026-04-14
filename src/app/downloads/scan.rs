use std::collections::HashSet;

use crate::core::game;
use crate::models::download::{DownloadEntry, DownloadStatus};

use super::super::App;
use super::super::free_fns::parse_nexus_mod_id;
use super::super::messages::AppCmdMsg;

impl App {
    pub(crate) fn handle_scan_downloads_folder(
        &mut self,
        sender: &relm4::prelude::ComponentSender<Self>,
    ) {
        let base_dir = &self.downloads_dir;
        if !base_dir.exists() {
            if self.initial_scan_done {
                self.toaster.toast("Downloads folder not found");
            }
            self.initial_scan_done = true;
            return;
        }

        // Remove non-active entries whose archive file no longer exists on disk.
        // This cleans up stale entries when the user changes the downloads folder
        // or manually deletes archives.
        // Installed entries are always kept even if the archive was deleted — the
        // status is meaningful and should persist until the mod is explicitly removed.
        let removed_ids: Vec<String> = self
            .all_downloads
            .iter()
            .filter(|e| {
                !e.is_active()
                    && e.status != DownloadStatus::Installed
                    && !e.archive_path.as_ref().map(|p| p.exists()).unwrap_or(false)
            })
            .map(|e| e.id.clone())
            .collect();
        self.all_downloads.retain(|e| {
            e.is_active()
                || e.status == DownloadStatus::Installed
                || e.archive_path.as_ref().map(|p| p.exists()).unwrap_or(false)
        });
        if !removed_ids.is_empty()
            && let Some(tracker) = self.tracker.clone()
        {
            sender.oneshot_command(async move {
                let _ = tracker.delete_download_entries(&removed_ids).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }

        // Collect existing archive paths from backing store to avoid duplicates.
        // Also collect filenames for path-change dedup (same file, different folder).
        let existing: HashSet<std::path::PathBuf> = self
            .all_downloads
            .iter()
            .filter_map(|e| e.archive_path.clone())
            .collect();
        let existing_names: HashSet<std::ffi::OsString> = self
            .all_downloads
            .iter()
            .filter_map(|e| {
                e.archive_path
                    .as_ref()
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_os_string())
            })
            .collect();

        let archive_extensions = ["zip", "7z", "rar"];
        let mut new_count = 0usize;
        let mut domain_updated_ids: Vec<String> = Vec::new();

        // Scan per-game subfolders (e.g. downloads_dir/skyrimspecialedition/)
        for domain in game::all_nexus_domains() {
            let game_dir = base_dir.join(domain);
            let Ok(entries) = std::fs::read_dir(&game_dir) else {
                continue;
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !archive_extensions.contains(&ext.as_str()) {
                    continue;
                }
                if existing.contains(&path) {
                    // Archive already tracked — ensure game_domain is set
                    // (covers entries from before per-game subfolder migration)
                    if let Some(dl) = self
                        .all_downloads
                        .iter_mut()
                        .find(|e| e.archive_path.as_ref() == Some(&path))
                        && dl.game_domain.is_none()
                    {
                        dl.game_domain = Some(domain.to_string());
                        domain_updated_ids.push(dl.id.clone());
                    }
                    continue;
                }
                // Also skip if the same filename is already tracked under a
                // different path (e.g. user moved the downloads folder).
                if path
                    .file_name()
                    .map(|n| existing_names.contains(n))
                    .unwrap_or(false)
                {
                    continue;
                }

                let file_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let mod_name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                let nexus_ids =
                    parse_nexus_mod_id(&file_name).map(|mod_id| (mod_id, 0i64, domain.to_string()));

                let download_id = uuid::Uuid::new_v4().to_string();
                let entry = DownloadEntry {
                    id: download_id,
                    mod_name,
                    status: DownloadStatus::Downloaded,
                    progress: 1.0,
                    status_msg: "Ready to install".to_string(),
                    error_msg: None,
                    nexus_ids,
                    archive_path: Some(path),
                    metadata_fetched: false,
                    game_domain: Some(domain.to_string()),
                    nexus_file_name: None,
                    nexus_is_primary: false,
                    archive_hash: None,
                };
                self.all_downloads.push(entry);
                new_count += 1;
            }
        }

        // Also scan the flat root folder for unrecognized archives
        // (backward compat + manual drops). Re-collect existing paths
        // to include entries just added from subfolders.
        let existing_after: HashSet<std::path::PathBuf> = self
            .all_downloads
            .iter()
            .filter_map(|e| e.archive_path.clone())
            .collect();
        if let Ok(root_entries) = std::fs::read_dir(base_dir) {
            for entry in root_entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !archive_extensions.contains(&ext.as_str()) {
                    continue;
                }
                if existing_after.contains(&path) {
                    continue;
                }
                // Skip if same filename is already tracked (path-change dedup)
                if path
                    .file_name()
                    .map(|n| existing_names.contains(n))
                    .unwrap_or(false)
                {
                    continue;
                }

                let file_name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let mod_name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                let nexus_ids =
                    parse_nexus_mod_id(&file_name).map(|mod_id| (mod_id, 0i64, String::new()));

                let download_id = uuid::Uuid::new_v4().to_string();
                let entry = DownloadEntry {
                    id: download_id,
                    mod_name,
                    status: DownloadStatus::Downloaded,
                    progress: 1.0,
                    status_msg: "Ready to install".to_string(),
                    error_msg: None,
                    nexus_ids,
                    archive_path: Some(path),
                    metadata_fetched: false,
                    game_domain: None,
                    nexus_file_name: None,
                    nexus_is_primary: false,
                    archive_hash: None,
                };
                self.all_downloads.push(entry);
                new_count += 1;
            }
        }

        self.rebuild_downloads_view();

        // Persist newly scanned entries + entries whose game_domain was updated
        let mut to_persist: Vec<DownloadEntry> = self
            .all_downloads
            .iter()
            .rev()
            .take(new_count)
            .cloned()
            .collect();
        for id in &domain_updated_ids {
            if let Some(entry) = self.all_downloads.iter().find(|e| &e.id == id) {
                to_persist.push(entry.clone());
            }
        }
        if !to_persist.is_empty()
            && let Some(tracker) = self.tracker.clone()
        {
            sender.oneshot_command(async move {
                for entry in &to_persist {
                    let _ = tracker.save_download_entry(entry).await;
                }
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }

        if self.initial_scan_done && new_count > 0 {
            self.toaster.toast(&format!("Found {new_count} archive(s)"));
        }
        self.initial_scan_done = true;
    }
}
