use std::collections::HashSet;
use std::path::PathBuf;

use crate::core::game;
use crate::models::download::{DownloadEntry, DownloadStatus, NexusIds};

use super::super::App;
use super::super::free_fns::parse_nexus_mod_id;
use super::super::messages::AppCmdMsg;
use super::super::types::{DownloadScanResult, WorkKind};

impl App {
    pub(crate) fn handle_scan_downloads_folder(
        &mut self,
        sender: &relm4::prelude::ComponentSender<Self>,
    ) {
        let base_dir = self.downloads_dir.clone();
        if !base_dir.exists() {
            if self.initial_scan_done {
                self.push_notification("Downloads folder not found");
            }
            self.initial_scan_done = true;
            return;
        }

        self.begin_work(WorkKind::ScanningDownloads, "Scanning downloads...");
        let selected_game_id = self
            .selected_game()
            .map(|game| game.id.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let entries = self.all_downloads.clone();
        sender.oneshot_command(async move {
            let timing_start = std::time::Instant::now();
            let result = tokio::task::spawn_blocking(move || scan_downloads(base_dir, entries))
                .await
                .map_err(|e| e.to_string())
                .and_then(|result| result);
            if let Ok(scan) = &result {
                crate::app::timing::log_phase(
                    "downloads.scan",
                    &selected_game_id,
                    timing_start,
                    Some(scan.entries.len()),
                );
            }
            AppCmdMsg::DownloadsScanned(result)
        });
    }

    pub(crate) fn handle_cmd_downloads_scanned(
        &mut self,
        result: Result<DownloadScanResult, String>,
        sender: &relm4::prelude::ComponentSender<Self>,
    ) {
        self.finish_work(WorkKind::ScanningDownloads);

        let scan = match result {
            Ok(scan) => scan,
            Err(e) => {
                self.initial_scan_done = true;
                self.push_notification(&format!("Downloads scan failed: {e}"));
                return;
            }
        };

        self.all_downloads = scan.entries;
        self.rebuild_downloads_view();

        if !scan.removed_ids.is_empty()
            && let Some(tracker) = self.tracker.clone()
        {
            let removed_ids = scan.removed_ids.clone();
            sender.oneshot_command(async move {
                let _ = tracker.delete_download_entries(&removed_ids).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }

        if !scan.to_persist.is_empty()
            && let Some(tracker) = self.tracker.clone()
        {
            let to_persist = scan.to_persist.clone();
            sender.oneshot_command(async move {
                for entry in &to_persist {
                    let _ = tracker.save_download_entry(entry).await;
                }
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }

        if self.initial_scan_done && scan.new_count > 0 {
            self.show_toast(&format!("Found {} archive(s)", scan.new_count));
        }
        self.initial_scan_done = true;
    }
}

fn scan_downloads(
    base_dir: PathBuf,
    mut all_downloads: Vec<DownloadEntry>,
) -> Result<DownloadScanResult, String> {
    let removed_ids: Vec<String> = all_downloads
        .iter()
        .filter(|e| {
            !e.is_active()
                && e.status != DownloadStatus::Installed
                && !e
                    .archive_path
                    .as_ref()
                    .map(|p| p.exists() && p.starts_with(&base_dir))
                    .unwrap_or(false)
        })
        .map(|e| e.id.clone())
        .collect();
    all_downloads.retain(|e| {
        e.is_active()
            || e.status == DownloadStatus::Installed
            || e.archive_path
                .as_ref()
                .map(|p| p.exists() && p.starts_with(&base_dir))
                .unwrap_or(false)
    });

    let existing: HashSet<std::path::PathBuf> = all_downloads
        .iter()
        .filter_map(|e| e.archive_path.clone())
        .collect();
    let existing_names: HashSet<std::ffi::OsString> = all_downloads
        .iter()
        .filter_map(|e| {
            e.archive_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_os_string())
        })
        .collect();

    let archive_extensions = ["zip", "7z", "rar", "dazip"];
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
                if let Some(dl) = all_downloads
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

            let nexus_ids = parse_nexus_mod_id(&file_name).map(|mod_id| NexusIds {
                mod_id,
                file_id: 0,
                domain: domain.to_string(),
            });

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
                archive_md5: None,
                version: None,
                author: None,
                hidden: false,
            };
            all_downloads.push(entry);
            new_count += 1;
        }
    }

    // Also scan the flat root folder for unrecognized archives
    // (backward compat + manual drops). Re-collect existing paths
    // to include entries just added from subfolders.
    let existing_after: HashSet<std::path::PathBuf> = all_downloads
        .iter()
        .filter_map(|e| e.archive_path.clone())
        .collect();
    if let Ok(root_entries) = std::fs::read_dir(&base_dir) {
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

            let nexus_ids = parse_nexus_mod_id(&file_name).map(|mod_id| NexusIds {
                mod_id,
                file_id: 0,
                domain: String::new(),
            });

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
                archive_md5: None,
                version: None,
                author: None,
                hidden: false,
            };
            all_downloads.push(entry);
            new_count += 1;
        }
    }

    let mut to_persist: Vec<DownloadEntry> = all_downloads
        .iter()
        .rev()
        .take(new_count)
        .cloned()
        .collect();
    for id in &domain_updated_ids {
        if let Some(entry) = all_downloads.iter().find(|e| &e.id == id) {
            to_persist.push(entry.clone());
        }
    }

    Ok(DownloadScanResult {
        entries: all_downloads,
        removed_ids,
        to_persist,
        new_count,
    })
}
