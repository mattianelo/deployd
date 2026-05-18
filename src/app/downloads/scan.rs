use std::collections::{HashMap, HashSet};
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
    let mut removed_ids = Vec::new();
    for entry in &mut all_downloads {
        if entry.is_active() {
            continue;
        }
        let has_invalid_archive = entry
            .archive_path
            .as_ref()
            .is_some_and(|p| !(p.exists() && p.starts_with(&base_dir)));
        if !has_invalid_archive {
            continue;
        }
        if entry.status == DownloadStatus::Installed {
            entry.archive_path = None;
        } else {
            removed_ids.push(entry.id.clone());
        }
    }
    all_downloads.retain(|e| {
        e.is_active()
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

            let mod_name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            if let Some(outcome) =
                reconcile_archive(&mut all_downloads, &path, Some(domain), &nexus_ids)
            {
                changed_ids.extend(outcome.changed_ids);
                removed_ids.extend(outcome.removed_ids);
                continue;
            }
            // Also skip if the same filename is already tracked under a
            // different path (e.g. user moved the downloads folder).
            if !existing.contains(&path)
                && path
                    .file_name()
                    .map(|n| existing_names.contains(n))
                    .unwrap_or(false)
            {
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

            let mod_name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            if let Some(outcome) = reconcile_archive(&mut all_downloads, &path, None, &nexus_ids) {
                changed_ids.extend(outcome.changed_ids);
                removed_ids.extend(outcome.removed_ids);
                continue;
            }
            // Skip if same filename is already tracked (path-change dedup)
            if !existing_after.contains(&path)
                && path
                    .file_name()
                    .map(|n| existing_names.contains(n))
                    .unwrap_or(false)
            {
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

    let sweep = sweep_duplicate_downloads(&mut all_downloads);
    changed_ids.extend(sweep.changed_ids);
    removed_ids.extend(sweep.removed_ids);

    let stale_pathless_ids: Vec<String> = all_downloads
        .iter()
        .filter(|entry| !entry.is_active() && entry.archive_path.is_none())
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
    changed_ids.sort();
    changed_ids.dedup();
    for id in &changed_ids {
        if let Some(entry) = all_downloads.iter().find(|e| &e.id == id) {
            to_persist.push(entry.clone());
        }
    }
    removed_ids.sort();
    removed_ids.dedup();

    Ok(DownloadScanResult {
        entries: all_downloads,
        removed_ids,
        to_persist,
        new_count,
    })
}

#[derive(Default)]
struct ReconcileOutcome {
    changed_ids: Vec<String>,
    removed_ids: Vec<String>,
}

#[derive(PartialEq)]
struct DownloadSnapshot {
    archive_path: Option<PathBuf>,
    game_domain: Option<String>,
    metadata_fetched: bool,
    nexus_file_name: Option<String>,
    archive_hash: Option<String>,
    archive_md5: Option<String>,
    version: Option<String>,
}

fn reconcile_archive(
    all_downloads: &mut Vec<DownloadEntry>,
    path: &std::path::Path,
    domain: Option<&str>,
    scanned_nexus_ids: &Option<NexusIds>,
) -> Option<ReconcileOutcome> {
    let candidates = archive_match_candidates(all_downloads, path, domain, scanned_nexus_ids)?;
    Some(merge_candidates(
        all_downloads,
        candidates,
        Some(path),
        domain,
    ))
}

fn archive_match_candidates(
    all_downloads: &[DownloadEntry],
    path: &std::path::Path,
    domain: Option<&str>,
    scanned_nexus_ids: &Option<NexusIds>,
) -> Option<Vec<usize>> {
    let exact_path: Vec<usize> = all_downloads
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.archive_path.as_ref().is_some_and(|p| p == path))
        .map(|(idx, _)| idx)
        .collect();
    if exact_path.iter().any(|idx| all_downloads[*idx].is_active()) {
        let active_path = exact_path
            .into_iter()
            .filter(|idx| all_downloads[*idx].is_active())
            .collect();
        return Some(active_path);
    }

    let file_name = path.file_name()?.to_string_lossy();
    let normalized_file = normalize_nexus_filename(&file_name);
    let mut candidates = exact_path;

    for (idx, entry) in all_downloads.iter().enumerate() {
        if !candidates.contains(&idx)
            && !entry.is_active()
            && download_domain_matches(entry, domain)
            && exact_nexus_file_matches(entry, scanned_nexus_ids)
        {
            candidates.push(idx);
        }
    }

    let filename_matches: Vec<usize> = all_downloads
        .iter()
        .enumerate()
        .filter(|(idx, entry)| {
            !candidates.contains(idx)
                && !entry.is_active()
                && download_domain_matches(entry, domain)
                && entry
                    .nexus_file_name
                    .as_deref()
                    .is_some_and(|name| normalize_nexus_filename(name) == normalized_file)
        })
        .map(|(idx, _)| idx)
        .collect();
    candidates.extend(filename_matches);

    let mod_matches: Vec<usize> = all_downloads
        .iter()
        .enumerate()
        .filter(|(idx, entry)| {
            !candidates.contains(idx)
                && !entry.is_active()
                && download_domain_matches(entry, domain)
                && download_nexus_mod_matches(entry, scanned_nexus_ids)
        })
        .map(|(idx, _)| idx)
        .collect();
    if mod_matches.len() == 1 {
        candidates.extend(mod_matches);
    }

    (!candidates.is_empty()).then_some(candidates)
}

fn merge_candidates(
    all_downloads: &mut Vec<DownloadEntry>,
    candidate_indices: Vec<usize>,
    path: Option<&std::path::Path>,
    domain: Option<&str>,
) -> ReconcileOutcome {
    let candidates: Vec<String> = candidate_indices
        .into_iter()
        .filter_map(|idx| all_downloads.get(idx))
        .map(|entry| entry.id.clone())
        .collect();
    let scored_candidates: Vec<(String, i32)> = candidates
        .iter()
        .filter_map(|id| {
            all_downloads
                .iter()
                .find(|entry| &entry.id == id)
                .map(|entry| (entry.id.clone(), download_quality(entry)))
        })
        .collect();
    let Some(best_score) = scored_candidates.iter().map(|(_, score)| *score).max() else {
        return ReconcileOutcome::default();
    };
    let best_ids: Vec<String> = scored_candidates
        .iter()
        .filter(|(_, score)| *score == best_score)
        .map(|(id, _)| id.clone())
        .collect();
    let [winner_id] = best_ids.as_slice() else {
        return ReconcileOutcome::default();
    };
    let winner_id = winner_id.clone();

    let mut outcome = ReconcileOutcome::default();
    for candidate_id in candidates {
        if candidate_id == winner_id {
            continue;
        }
        let Some(loser_idx) = all_downloads
            .iter()
            .position(|entry| entry.id == candidate_id)
        else {
            continue;
        };
        if all_downloads[loser_idx].is_active() {
            continue;
        }
        let loser = all_downloads.remove(loser_idx);
        merge_download_metadata(all_downloads, &winner_id, &loser);
        outcome.removed_ids.push(loser.id);
    }

    if let Some(winner) = all_downloads.iter_mut().find(|entry| entry.id == winner_id) {
        let before = snapshot_for_change_detection(winner);
        if let Some(path) = path {
            winner.archive_path = Some(path.to_path_buf());
        }
        if winner.game_domain.is_none() {
            winner.game_domain = domain.map(str::to_string);
        }
        if snapshot_for_change_detection(winner) != before || !outcome.removed_ids.is_empty() {
            outcome.changed_ids.push(winner.id.clone());
        }
    }
    outcome
}

fn sweep_duplicate_downloads(all_downloads: &mut Vec<DownloadEntry>) -> ReconcileOutcome {
    let mut outcome = ReconcileOutcome::default();
    for group in duplicate_groups(all_downloads) {
        let indices: Vec<usize> = group
            .iter()
            .filter_map(|id| all_downloads.iter().position(|entry| &entry.id == id))
            .collect();
        let sweep = merge_candidates(all_downloads, indices, None, None);
        outcome.changed_ids.extend(sweep.changed_ids);
        outcome.removed_ids.extend(sweep.removed_ids);
    }
    outcome
}

fn duplicate_groups(all_downloads: &[DownloadEntry]) -> Vec<Vec<String>> {
    let mut groups = Vec::new();
    collect_duplicate_groups(
        &mut groups,
        group_by_archive_path(all_downloads),
        all_downloads,
    );
    collect_duplicate_groups(
        &mut groups,
        group_by_exact_nexus_file(all_downloads),
        all_downloads,
    );
    collect_duplicate_groups(
        &mut groups,
        group_by_normalized_nexus_file_name(all_downloads),
        all_downloads,
    );
    collect_duplicate_groups(
        &mut groups,
        group_by_safe_nexus_mod(all_downloads),
        all_downloads,
    );
    groups
}

fn collect_duplicate_groups(
    groups: &mut Vec<Vec<String>>,
    candidate_groups: Vec<Vec<usize>>,
    all_downloads: &[DownloadEntry],
) {
    for group in candidate_groups {
        let group_ids: Vec<String> = group
            .iter()
            .filter_map(|idx| all_downloads.get(*idx))
            .map(|entry| entry.id.clone())
            .collect();
        if group.len() > 1 {
            groups.push(group_ids);
        }
    }
}

fn group_by_archive_path(all_downloads: &[DownloadEntry]) -> Vec<Vec<usize>> {
    let mut grouped: HashMap<PathBuf, Vec<usize>> = HashMap::new();
    for (idx, entry) in all_downloads.iter().enumerate() {
        if entry.is_active() {
            continue;
        }
        if let Some(path) = entry.archive_path.clone() {
            grouped.entry(path).or_default().push(idx);
        }
    }
    grouped
        .into_values()
        .filter(|group| group.len() > 1)
        .collect()
}

fn group_by_exact_nexus_file(all_downloads: &[DownloadEntry]) -> Vec<Vec<usize>> {
    let mut grouped: HashMap<(String, i64, i64), Vec<usize>> = HashMap::new();
    for (idx, entry) in all_downloads.iter().enumerate() {
        if entry.is_active() {
            continue;
        }
        let Some(ids) = entry.nexus_ids.as_ref() else {
            continue;
        };
        if ids.file_id == 0 {
            continue;
        }
        grouped
            .entry((ids.domain.clone(), ids.mod_id, ids.file_id))
            .or_default()
            .push(idx);
    }
    grouped
        .into_values()
        .filter(|group| group.len() > 1)
        .collect()
}

fn group_by_normalized_nexus_file_name(all_downloads: &[DownloadEntry]) -> Vec<Vec<usize>> {
    let mut grouped: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (idx, entry) in all_downloads.iter().enumerate() {
        if entry.is_active() {
            continue;
        }
        let Some(domain) = entry_domain(entry) else {
            continue;
        };
        let Some(file_name) = entry.nexus_file_name.as_deref() else {
            continue;
        };
        grouped
            .entry((domain.to_string(), normalize_nexus_filename(file_name)))
            .or_default()
            .push(idx);
    }
    grouped
        .into_values()
        .filter(|group| group.len() > 1)
        .collect()
}

fn group_by_safe_nexus_mod(all_downloads: &[DownloadEntry]) -> Vec<Vec<usize>> {
    let mut grouped: HashMap<(String, i64), Vec<usize>> = HashMap::new();
    for (idx, entry) in all_downloads.iter().enumerate() {
        if entry.is_active() {
            continue;
        }
        let Some(ids) = entry.nexus_ids.as_ref() else {
            continue;
        };
        grouped
            .entry((ids.domain.clone(), ids.mod_id))
            .or_default()
            .push(idx);
    }
    grouped
        .into_values()
        .filter(|group| group.len() > 1 && has_no_conflicting_nexus_file_ids(all_downloads, group))
        .collect()
}

fn has_no_conflicting_nexus_file_ids(all_downloads: &[DownloadEntry], group: &[usize]) -> bool {
    let file_ids: HashSet<i64> = group
        .iter()
        .filter_map(|idx| all_downloads.get(*idx))
        .filter_map(|entry| entry.nexus_ids.as_ref())
        .filter_map(|ids| (ids.file_id != 0).then_some(ids.file_id))
        .collect();
    file_ids.len() <= 1
}

fn merge_download_metadata(
    all_downloads: &mut [DownloadEntry],
    winner_id: &str,
    loser: &DownloadEntry,
) {
    let Some(winner) = all_downloads.iter_mut().find(|entry| entry.id == winner_id) else {
        return;
    };
    if winner.nexus_ids.is_none() {
        winner.nexus_ids = loser.nexus_ids.clone();
    }
    if winner.game_domain.is_none() {
        winner.game_domain = loser.game_domain.clone();
    }
    if winner.nexus_file_name.is_none() {
        winner.nexus_file_name = loser.nexus_file_name.clone();
    }
    if !winner.nexus_is_primary {
        winner.nexus_is_primary = loser.nexus_is_primary;
    }
    if winner.archive_hash.is_none() {
        winner.archive_hash = loser.archive_hash.clone();
    }
    if winner.archive_md5.is_none() {
        winner.archive_md5 = loser.archive_md5.clone();
    }
    if winner.version.is_none() {
        winner.version = loser.version.clone();
    }
    if winner.author.is_none() {
        winner.author = loser.author.clone();
    }
    if winner.archive_path.is_none() {
        winner.archive_path = loser.archive_path.clone();
    }
    winner.metadata_fetched |= loser.metadata_fetched;
    if winner.status != DownloadStatus::Installed && loser.status == DownloadStatus::Installed {
        winner.status = DownloadStatus::Installed;
        winner.status_msg = DownloadStatus::Installed.default_status_msg().to_string();
    }
}

fn download_quality(entry: &DownloadEntry) -> i32 {
    let mut score = 0;
    if entry.status == DownloadStatus::Installed {
        score += 1_000;
    }
    if entry.metadata_fetched {
        score += 100;
    }
    if entry.nexus_ids.as_ref().is_some_and(|ids| ids.file_id != 0) {
        score += 80;
    }
    if entry.nexus_file_name.is_some() {
        score += 40;
    }
    if entry.version.is_some() {
        score += 20;
    }
    if entry.author.is_some() {
        score += 20;
    }
    if entry.archive_hash.is_some() {
        score += 20;
    }
    if entry.archive_md5.is_some() {
        score += 20;
    }
    if entry.archive_path.is_some() {
        score += 5;
    }
    score
}

fn snapshot_for_change_detection(entry: &DownloadEntry) -> DownloadSnapshot {
    DownloadSnapshot {
        archive_path: entry.archive_path.clone(),
        game_domain: entry.game_domain.clone(),
        metadata_fetched: entry.metadata_fetched,
        nexus_file_name: entry.nexus_file_name.clone(),
        archive_hash: entry.archive_hash.clone(),
        archive_md5: entry.archive_md5.clone(),
        version: entry.version.clone(),
    }
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

fn exact_nexus_file_matches(entry: &DownloadEntry, scanned_nexus_ids: &Option<NexusIds>) -> bool {
    match (&entry.nexus_ids, scanned_nexus_ids) {
        (Some(existing), Some(scanned)) => {
            scanned.file_id != 0
                && existing.file_id == scanned.file_id
                && existing.mod_id == scanned.mod_id
                && (scanned.domain.is_empty() || existing.domain == scanned.domain)
        }
        _ => false,
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

fn entry_domain(entry: &DownloadEntry) -> Option<&str> {
    entry
        .game_domain
        .as_deref()
        .or_else(|| entry.nexus_ids.as_ref().map(|ids| ids.domain.as_str()))
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
    fn scan_removes_unmatched_installed_download_inventory() -> Result<()> {
        let temp = TempDir::new()?;
        let installed = download_entry("installed", "Installed Without Archive");

        let scan = scan_downloads(temp.path().to_path_buf(), vec![installed])
            .map_err(anyhow::Error::msg)?;

        assert!(scan.entries.is_empty());
        assert_eq!(scan.removed_ids, vec!["installed".to_string()]);
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

    #[test]
    fn scan_sweeps_duplicate_installed_entries_by_exact_nexus_file() -> Result<()> {
        let temp = TempDir::new()?;
        let archive = temp
            .path()
            .join("Unofficial Fallout 4 Patch-4598-2-1-5-1750000000.7z");
        std::fs::write(&archive, b"archive")?;

        let rich = download_entry("rich", "Unofficial Fallout 4 Patch");
        let mut duplicate = download_entry("duplicate", "UFO4P Duplicate");
        duplicate.archive_path = Some(archive.clone());
        duplicate.metadata_fetched = false;
        duplicate.nexus_file_name = None;
        duplicate.version = None;
        duplicate.author = None;
        duplicate.archive_md5 = None;

        let scan = scan_downloads(temp.path().to_path_buf(), vec![rich, duplicate])
            .map_err(anyhow::Error::msg)?;

        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].id, "rich");
        assert_eq!(
            scan.entries[0].archive_path.as_deref(),
            Some(archive.as_path())
        );
        assert_eq!(scan.removed_ids, vec!["duplicate".to_string()]);
        assert_eq!(scan.to_persist.len(), 1);
        assert_eq!(scan.to_persist[0].id, "rich");
        Ok(())
    }

    #[test]
    fn scan_sweeps_duplicate_entries_by_safe_mod_id_match() -> Result<()> {
        let temp = TempDir::new()?;
        let archive = temp
            .path()
            .join("Unofficial Fallout 4 Patch-4598-2-1-5-1750000000.7z");
        std::fs::write(&archive, b"archive")?;

        let rich = download_entry("rich", "Unofficial Fallout 4 Patch");
        let mut duplicate = download_entry("duplicate", "UFO4P Scanned");
        duplicate.archive_path = Some(archive.clone());
        if let Some(ref mut ids) = duplicate.nexus_ids {
            ids.file_id = 0;
        }
        duplicate.metadata_fetched = false;
        duplicate.nexus_file_name = None;

        let scan = scan_downloads(temp.path().to_path_buf(), vec![rich, duplicate])
            .map_err(anyhow::Error::msg)?;

        assert_eq!(scan.entries.len(), 1);
        assert_eq!(scan.entries[0].id, "rich");
        assert_eq!(
            scan.entries[0].archive_path.as_deref(),
            Some(archive.as_path())
        );
        assert_eq!(scan.removed_ids, vec!["duplicate".to_string()]);
        Ok(())
    }

    #[test]
    fn scan_keeps_duplicate_mod_id_when_file_ids_conflict() -> Result<()> {
        let temp = TempDir::new()?;
        let first_archive = temp.path().join("First File-4598-1-0-1750000000.7z");
        let second_archive = temp.path().join("Second File-4598-2-0-1750000000.7z");
        std::fs::write(&first_archive, b"first")?;
        std::fs::write(&second_archive, b"second")?;

        let mut first = download_entry("first", "First File");
        first.archive_path = Some(first_archive);
        let mut second = download_entry("second", "Second File");
        second.archive_path = Some(second_archive);
        if let Some(ref mut ids) = second.nexus_ids {
            ids.file_id = 456;
        }
        second.nexus_file_name = Some("Second File.7z".to_string());

        let scan = scan_downloads(temp.path().to_path_buf(), vec![first, second])
            .map_err(anyhow::Error::msg)?;

        assert_eq!(scan.entries.len(), 2);
        assert!(scan.removed_ids.is_empty());
        Ok(())
    }

    #[test]
    fn scan_does_not_merge_active_downloads() -> Result<()> {
        let temp = TempDir::new()?;
        let domain_dir = temp.path().join("fallout4");
        std::fs::create_dir_all(&domain_dir)?;
        let archive = domain_dir.join("Unofficial Fallout 4 Patch-4598-2-1-5-1750000000.7z");
        std::fs::write(&archive, b"archive")?;

        let installed = download_entry("installed", "Unofficial Fallout 4 Patch");
        let mut active = download_entry("active", "Active Download");
        active.status = DownloadStatus::Downloading;
        active.status_msg = "Downloading...".to_string();
        active.archive_path = Some(archive);
        active.metadata_fetched = false;

        let scan = scan_downloads(temp.path().to_path_buf(), vec![installed, active])
            .map_err(anyhow::Error::msg)?;

        assert_eq!(scan.entries.len(), 1);
        assert!(scan.entries.iter().any(|entry| entry.id == "active"));
        assert_eq!(scan.removed_ids, vec!["installed".to_string()]);
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
