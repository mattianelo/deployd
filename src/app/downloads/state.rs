use gtk::prelude::*;

use crate::models::download::{DownloadEntry, DownloadStatus};

use super::super::App;

impl App {
    pub(crate) fn refresh_download_counts(&mut self) {
        // Sidebar counts (game-filtered)
        let guard = self.downloads.guard();
        let mut active = 0;
        for i in 0..guard.len() {
            if let Some(row) = guard.get(i)
                && row.entry.is_active()
            {
                active += 1;
            }
        }
        drop(guard);
        self.active_download_count = active;

        // Global count (all games)
        self.global_active_downloads = self.all_downloads.iter().filter(|e| e.is_active()).count();
    }

    /// Rebuild the downloads factory to show only entries for the current game.
    pub(crate) fn rebuild_downloads_view(&mut self) {
        // If a download is in progress, defer the sort until it completes.
        // Clearing and rebuilding the factory during an active download can crash
        // due to widget destruction racing with in-flight progress updates.
        if self.all_downloads.iter().any(|e| e.is_active()) {
            return;
        }
        let current_domain = self
            .selected_game()
            .and_then(crate::core::game::nexus_domain)
            .map(String::from);
        let mut entries: Vec<&DownloadEntry> = self
            .all_downloads
            .iter()
            .filter(|entry| match (&current_domain, &entry.game_domain) {
                (Some(cur), Some(dom)) => cur == dom,
                (_, None) => true,
                (None, _) => true,
            })
            .collect();
        match self.download_sort {
            crate::app::types::DownloadSort::Name => {
                entries.sort_by(|a, b| a.mod_name.to_lowercase().cmp(&b.mod_name.to_lowercase()))
            }
            crate::app::types::DownloadSort::Status => entries
                .sort_by_key(|e| crate::app::types::download_status_sort_key(&e.status)),
            crate::app::types::DownloadSort::Default => {}
        }
        // Save scroll position before rebuilding; restore it on the next GLib iteration
        // so the layout has already been applied when we set the value.
        let vadj = self.downloads_scroll.vadjustment();
        let saved_pos = vadj.value();

        // Single guard acquisition: populate then filter in one locked scope.
        let query = self.search_text.to_lowercase();
        let mut guard = self.downloads.guard();
        guard.clear();
        for entry in entries {
            guard.push_back(entry.clone());
        }
        if !query.is_empty() {
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i) {
                    row.visible = row.entry.mod_name.to_lowercase().contains(&query);
                }
            }
        }
        drop(guard);

        glib::idle_add_local_once(move || {
            vadj.set_value(saved_pos);
        });

        self.refresh_download_counts();
    }

    /// After a mod is removed, reset the matching "Installed" download entry (if any)
    /// back to "Downloaded" so the install button reappears for that specific mod.
    /// Only the download whose nexus IDs (or name, for non-Nexus mods) match the removed
    /// mod is affected — other installed downloads are left untouched.
    pub(crate) fn reset_installed_download_for_mod(
        &mut self,
        nexus_ids: Option<(i64, i64)>,
        mod_name: &str,
        mod_archive_hash: Option<&str>,
    ) -> Vec<DownloadEntry> {
        // When nexus_file_id == 0 (disk-scanned sentinel, file ID unknown), multiple archives
        // from the same Nexus mod page all share (mod_id, 0). When a mod_archive_hash is
        // available we use it as an exact tiebreaker. Without a hash we fall back to
        // counting installed entries: reset only when there is exactly one (unambiguous).
        let fid_zero_ambiguous = if matches!(nexus_ids, Some((_, 0))) && mod_archive_hash.is_none()
        {
            let mid = nexus_ids.unwrap().0;
            self.all_downloads
                .iter()
                .filter(|e| {
                    e.status == DownloadStatus::Installed
                        && e.archive_path.as_ref().map(|p| p.exists()).unwrap_or(false)
                        && matches!(&e.nexus_ids, Some((m, 0, _)) if *m == mid)
                })
                .count()
                > 1
        } else {
            false
        };

        let mut changed_entries = Vec::new();
        for entry in &mut self.all_downloads {
            if entry.status != DownloadStatus::Installed {
                continue;
            }
            if !entry
                .archive_path
                .as_ref()
                .map(|p| p.exists())
                .unwrap_or(false)
            {
                continue;
            }
            let matches = match (nexus_ids, &entry.nexus_ids) {
                // fid == 0 is the disk-scan sentinel (file ID unknown).
                // Prefer an exact archive_hash match; fall back to the count heuristic
                // when no hash is available.
                (Some((mid, 0)), Some((emid, 0, _))) => {
                    mid == *emid
                        && if let Some(mod_hash) = mod_archive_hash {
                            entry.archive_hash.as_deref() == Some(mod_hash)
                        } else {
                            !fid_zero_ambiguous
                        }
                }
                (Some((mid, fid)), Some((emid, efid, _))) => mid == *emid && fid == *efid,
                // For non-Nexus mods match by name (case-insensitive)
                (None, None) => entry.mod_name.to_lowercase() == mod_name.to_lowercase(),
                _ => false,
            };
            if matches {
                entry.status = DownloadStatus::Downloaded;
                entry.status_msg = "Ready to install".to_string();
                changed_entries.push(entry.clone());
            }
        }
        if !changed_entries.is_empty() {
            self.rebuild_downloads_view();
        }
        changed_entries
    }

    /// Find a download entry in the backing store by ID.
    pub(crate) fn find_download_mut(&mut self, id: &str) -> Option<&mut DownloadEntry> {
        self.all_downloads.iter_mut().find(|e| e.id == id)
    }

    pub(crate) fn update_download_status(
        &mut self,
        download_id: &str,
        status: DownloadStatus,
        msg: &str,
    ) {
        let prev_was_active = self
            .all_downloads
            .iter()
            .find(|e| e.id == download_id)
            .map(|e| e.is_active())
            .unwrap_or(false);
        let new_is_active = matches!(
            status,
            DownloadStatus::Downloading | DownloadStatus::Extracting
        );

        // Update backing store
        if let Some(entry) = self.find_download_mut(download_id) {
            entry.error_msg = if status == DownloadStatus::Failed {
                Some(msg.to_string())
            } else {
                None
            };
            entry.status = status.clone();
            entry.status_msg = msg.to_string();
        }
        // Update factory
        let mut guard = self.downloads.guard();
        for i in 0..guard.len() {
            if let Some(row) = guard.get_mut(i)
                && row.entry.id == download_id
            {
                row.entry.error_msg = if status == DownloadStatus::Failed {
                    Some(msg.to_string())
                } else {
                    None
                };
                row.entry.status = status;
                row.entry.status_msg = msg.to_string();
                break;
            }
        }
        drop(guard);

        // When an active download finishes and none others are running,
        // apply any sort order that was deferred during the download.
        if prev_was_active && !new_is_active && !self.all_downloads.iter().any(|e| e.is_active()) {
            self.rebuild_downloads_view();
        } else {
            self.refresh_download_counts();
        }
    }
}
