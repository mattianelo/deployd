use std::collections::HashSet;
use std::path::PathBuf;

use crate::core::game;
use crate::models::download::{DownloadEntry, DownloadStatus, NexusIds};

use super::super::App;
use super::super::free_fns::{normalize_nexus_filename, parse_nexus_mod_id};
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
    let mut removed_ids: Vec<String> = all_downloads
        .iter()
        .filter(|e| {
            !e.is_active()
                && e.status != DownloadStatus::Installed
                && e.archive_path
                    .as_ref()
                    .is_some_and(|p| !(p.exists() && p.starts_with(&base_dir)))
        })
        .map(|e| e.id.clone())
        .collect();
    all_downloads.retain(|e| {
        e.is_active()
            || e.status == DownloadStatus::Installed
            || e.archive_path.is_none()
            || e.archive_path
                .as_ref()
                .is_some_and(|p| p.exists() && p.starts_with(&base_dir))
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
    let mut changed_ids: Vec<String> = Vec::new();

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
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let nexus_ids = parse_nexus_mod_id(&file_name).map(|mod_id| NexusIds {
                mod_id,
                file_id: 0,
                domain: domain.to_string(),
            });

            if existing.contains(&path) {
                if let Some((kept_id, removed_id)) =
                    merge_path_duplicate(&mut all_downloads, &path, Some(domain), &nexus_ids)
                {
                    changed_ids.push(kept_id);
                    removed_ids.push(removed_id);
                    continue;
                }
                // Archive already tracked — ensure game_domain is set
                // (covers entries from before per-game subfolder migration)
                if let Some(dl) = all_downloads
                    .iter_mut()
                    .find(|e| e.archive_path.as_ref() == Some(&path))
                    && dl.game_domain.is_none()
                {
                    dl.game_domain = Some(domain.to_string());
                    changed_ids.push(dl.id.clone());
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

            let mod_name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            if let Some(id) =
                attach_pathless_download(&mut all_downloads, &path, Some(domain), &nexus_ids)
            {
                changed_ids.push(id);
                continue;
            }

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
            let file_name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let nexus_ids = parse_nexus_mod_id(&file_name).map(|mod_id| NexusIds {
                mod_id,
                file_id: 0,
                domain: String::new(),
            });

            if existing_after.contains(&path) {
                if let Some((kept_id, removed_id)) =
                    merge_path_duplicate(&mut all_downloads, &path, None, &nexus_ids)
                {
                    changed_ids.push(kept_id);
                    removed_ids.push(removed_id);
                }
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

            let mod_name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            if let Some(id) = attach_pathless_download(&mut all_downloads, &path, None, &nexus_ids)
            {
                changed_ids.push(id);
                continue;
            }

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

    let stale_pathless_ids: Vec<String> = all_downloads
        .iter()
        .filter(|entry| {
            !entry.is_active()
                && entry.status != DownloadStatus::Installed
                && entry.archive_path.is_none()
        })
        .map(|entry| entry.id.clone())
        .collect();
    if !stale_pathless_ids.is_empty() {
        removed_ids.extend(stale_pathless_ids.iter().cloned());
        all_downloads.retain(|entry| !stale_pathless_ids.contains(&entry.id));
    }

    let mut to_persist: Vec<DownloadEntry> = all_downloads
        .iter()
        .rev()
        .take(new_count)
        .cloned()
        .collect();
    for id in &changed_ids {
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

fn attach_pathless_download(
    all_downloads: &mut [DownloadEntry],
    path: &std::path::Path,
    domain: Option<&str>,
    scanned_nexus_ids: &Option<NexusIds>,
) -> Option<String> {
    let file_name = path.file_name()?.to_string_lossy();
    let normalized_file = normalize_nexus_filename(&file_name);
    let mut nexus_match: Option<usize> = None;
    let mut nexus_match_count = 0usize;

    for (idx, entry) in all_downloads.iter().enumerate() {
        if entry.archive_path.is_some() || !download_domain_matches(entry, domain) {
            continue;
        }
        if let Some(nexus_file_name) = entry.nexus_file_name.as_deref()
            && normalize_nexus_filename(nexus_file_name) == normalized_file
        {
            return attach_archive_path(all_downloads, idx, path, domain);
        }
        if download_nexus_mod_matches(entry, scanned_nexus_ids) {
            nexus_match = Some(idx);
            nexus_match_count += 1;
        }
    }

    if nexus_match_count == 1
        && let Some(idx) = nexus_match
    {
        return attach_archive_path(all_downloads, idx, path, domain);
    }

    None
}

fn merge_path_duplicate(
    all_downloads: &mut Vec<DownloadEntry>,
    path: &std::path::Path,
    domain: Option<&str>,
    scanned_nexus_ids: &Option<NexusIds>,
) -> Option<(String, String)> {
    let path_entry_idx = all_downloads
        .iter()
        .position(|entry| entry.archive_path.as_ref() == Some(&path.to_path_buf()))?;
    let path_entry_id = all_downloads.get(path_entry_idx)?.id.clone();
    let kept_id = attach_pathless_download(all_downloads, path, domain, scanned_nexus_ids)?;
    if kept_id == path_entry_id {
        return None;
    }
    if let Some(remove_idx) = all_downloads
        .iter()
        .position(|entry| entry.id == path_entry_id)
    {
        let removed = all_downloads.remove(remove_idx);
        return Some((kept_id, removed.id));
    }
    None
}

fn attach_archive_path(
    all_downloads: &mut [DownloadEntry],
    idx: usize,
    path: &std::path::Path,
    domain: Option<&str>,
) -> Option<String> {
    let entry = all_downloads.get_mut(idx)?;
    entry.archive_path = Some(path.to_path_buf());
    if entry.game_domain.is_none() {
        entry.game_domain = domain.map(str::to_string);
    }
    Some(entry.id.clone())
}

fn download_domain_matches(entry: &DownloadEntry, domain: Option<&str>) -> bool {
    match domain {
        Some(domain) => {
            entry.game_domain.as_deref() == Some(domain)
                || entry
                    .nexus_ids
                    .as_ref()
                    .is_some_and(|ids| ids.domain == domain)
        }
        None => true,
    }
}

fn download_nexus_mod_matches(entry: &DownloadEntry, scanned_nexus_ids: &Option<NexusIds>) -> bool {
    match (&entry.nexus_ids, scanned_nexus_ids) {
        (Some(existing), Some(scanned)) => {
            existing.mod_id == scanned.mod_id
                && (scanned.domain.is_empty() || existing.domain == scanned.domain)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;
    use tempfile::TempDir;

    #[test]
    fn scan_reattaches_archive_to_imported_metadata_entry() -> Result<()> {
        let temp = TempDir::new()?;
        let domain_dir = temp.path().join("fallout4");
        std::fs::create_dir_all(&domain_dir)?;
        let archive = domain_dir.join("Unofficial Fallout 4 Patch-4598-2-1-5-1750000000.7z");
        std::fs::write(&archive, b"archive")?;

        let imported = download_entry("imported", "Unofficial Fallout 4 Patch");
        let scan = scan_downloads(temp.path().to_path_buf(), vec![imported])
            .map_err(anyhow::Error::msg)?;

        assert_eq!(scan.new_count, 0, "scan should not create a duplicate");
        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].id, "imported");
        assert_eq!(
            scan.entries[0].archive_path.as_deref(),
            Some(archive.as_path())
        );
        assert!(scan.entries[0].metadata_fetched);
        assert_eq!(scan.to_persist.len(), 1);
        assert_eq!(scan.to_persist[0].id, "imported");
        Ok(())
    }

    #[test]
    fn scan_preserves_downloaded_imported_metadata_before_reattach() -> Result<()> {
        let temp = TempDir::new()?;
        let domain_dir = temp.path().join("fallout4");
        std::fs::create_dir_all(&domain_dir)?;
        let archive = domain_dir.join("LooksMenu-12631-1-6-20-1750000000.7z");
        std::fs::write(&archive, b"archive")?;

        let mut imported = download_entry("downloaded", "LooksMenu");
        imported.status = DownloadStatus::Downloaded;
        imported.status_msg = "Ready to install".to_string();
        imported.nexus_ids = Some(NexusIds {
            mod_id: 12631,
            file_id: 456,
            domain: "fallout4".to_string(),
        });
        imported.nexus_file_name = Some("LooksMenu-12631-1-6-20.7z".to_string());

        let scan = scan_downloads(temp.path().to_path_buf(), vec![imported])
            .map_err(anyhow::Error::msg)?;

        assert_eq!(scan.removed_ids.len(), 0);
        assert_eq!(scan.new_count, 0);
        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].id, "downloaded");
        assert_eq!(scan.entries[0].status, DownloadStatus::Downloaded);
        assert_eq!(
            scan.entries[0].archive_path.as_deref(),
            Some(archive.as_path())
        );
        assert!(scan.entries[0].metadata_fetched);
        Ok(())
    }

    #[test]
    fn scan_creates_entry_when_pathless_match_is_ambiguous() -> Result<()> {
        let temp = TempDir::new()?;
        let domain_dir = temp.path().join("fallout4");
        std::fs::create_dir_all(&domain_dir)?;
        let archive = domain_dir.join("Ambiguous Mod-4598-1-0-1750000000.7z");
        std::fs::write(&archive, b"archive")?;

        let mut first = download_entry("first", "First");
        first.status = DownloadStatus::Downloaded;
        first.nexus_file_name = None;
        let mut second = download_entry("second", "Second");
        second.status = DownloadStatus::Downloaded;
        second.nexus_file_name = None;
        let scan = scan_downloads(temp.path().to_path_buf(), vec![first, second])
            .map_err(anyhow::Error::msg)?;

        assert_eq!(scan.new_count, 1);
        assert_eq!(scan.entries.len(), 1);
        assert!(
            scan.entries
                .iter()
                .any(|entry| entry.id != "first" && entry.id != "second")
        );
        assert_eq!(
            scan.removed_ids,
            vec!["first".to_string(), "second".to_string()]
        );
        Ok(())
    }

    #[test]
    fn scan_removes_unmatched_pathless_download_metadata() -> Result<()> {
        let temp = TempDir::new()?;
        let mut imported = download_entry("missing", "Missing Archive");
        imported.status = DownloadStatus::Downloaded;
        imported.status_msg = "Ready to install".to_string();

        let scan = scan_downloads(temp.path().to_path_buf(), vec![imported])
            .map_err(anyhow::Error::msg)?;

        assert!(scan.entries.is_empty());
        assert_eq!(scan.removed_ids, vec!["missing".to_string()]);
        assert!(scan.to_persist.is_empty());
        Ok(())
    }

    #[test]
    fn scan_merges_existing_path_only_duplicate_into_imported_metadata() -> Result<()> {
        let temp = TempDir::new()?;
        let domain_dir = temp.path().join("fallout4");
        std::fs::create_dir_all(&domain_dir)?;
        let archive = domain_dir.join("Unofficial Fallout 4 Patch-4598-2-1-5-1750000000.7z");
        std::fs::write(&archive, b"archive")?;

        let imported = download_entry("imported", "Unofficial Fallout 4 Patch");
        let mut duplicate = download_entry("duplicate", "Unofficial Fallout 4 Patch");
        duplicate.archive_path = Some(archive.clone());
        duplicate.metadata_fetched = false;
        duplicate.nexus_file_name = None;
        duplicate.version = None;
        duplicate.author = None;

        let scan = scan_downloads(temp.path().to_path_buf(), vec![imported, duplicate])
            .map_err(anyhow::Error::msg)?;

        assert_eq!(scan.new_count, 0);
        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].id, "imported");
        assert_eq!(
            scan.entries[0].archive_path.as_deref(),
            Some(archive.as_path())
        );
        assert_eq!(scan.removed_ids, vec!["duplicate".to_string()]);
        assert_eq!(scan.to_persist.len(), 1);
        assert_eq!(scan.to_persist[0].id, "imported");
        Ok(())
    }

    fn download_entry(id: &str, name: &str) -> DownloadEntry {
        DownloadEntry {
            id: id.to_string(),
            mod_name: name.to_string(),
            status: DownloadStatus::Installed,
            progress: 1.0,
            status_msg: "Installed".to_string(),
            error_msg: None,
            nexus_ids: Some(NexusIds {
                mod_id: 4598,
                file_id: 123,
                domain: "fallout4".to_string(),
            }),
            archive_path: None,
            metadata_fetched: true,
            game_domain: Some("fallout4".to_string()),
            nexus_file_name: Some("Unofficial Fallout 4 Patch-4598-2-1-5.7z".to_string()),
            nexus_is_primary: true,
            archive_hash: Some("sha256".to_string()),
            archive_md5: Some("md5".to_string()),
            version: Some("2.1.5".to_string()),
            author: Some("Author".to_string()),
            hidden: false,
        }
    }
}
