use gtk::prelude::*;

use crate::models::download::{DownloadEntry, DownloadStatus, NexusIds};

use super::super::App;

fn replaced_download_index(
    entries: &[DownloadEntry],
    replacement: &crate::app::state::ReplacementContext,
    installed_download_id: Option<&str>,
) -> Option<usize> {
    entries.iter().position(|entry| {
        if entry.status != DownloadStatus::Installed
            || installed_download_id == Some(entry.id.as_str())
            || entry
                .archive_path
                .as_ref()
                .is_none_or(|path| !path.exists())
        {
            return false;
        }
        if let Some(hash) = replacement.archive_hash.as_deref() {
            return entry.archive_hash.as_deref() == Some(hash);
        }
        match (replacement.nexus_ids, &entry.nexus_ids) {
            (Some((mod_id, file_id)), Some(ids)) => ids.mod_id == mod_id && ids.file_id == file_id,
            (None, None) => entry.mod_name.eq_ignore_ascii_case(&replacement.mod_name),
            _ => false,
        }
    })
}

impl App {
    pub(crate) fn reset_installed_download_after_replacement(
        &mut self,
        replacement: &crate::app::state::ReplacementContext,
        installed_download_id: Option<&str>,
    ) -> Vec<DownloadEntry> {
        let candidate =
            replaced_download_index(&self.download.all, replacement, installed_download_id);
        let Some(index) = candidate else {
            return Vec::new();
        };
        let entry = &mut self.download.all[index];
        entry.status = DownloadStatus::Downloaded;
        entry.status_msg = DownloadStatus::Downloaded.default_status_msg().to_string();
        let changed = vec![entry.clone()];
        self.rebuild_downloads_view();
        changed
    }

    pub(crate) fn downloads_pane_state(&self) -> crate::ui::downloads_pane::DownloadsPaneState {
        crate::ui::downloads_pane::DownloadsPaneState {
            filter: self.download.filter,
            sort: self.download.sort,
            show_hidden: self.download.show_hidden,
            active_count: self.active_downloads_count(),
            completed_count: self.completed_downloads_count(),
            is_empty: self.download.rows.is_empty(),
        }
    }

    pub(crate) fn refresh_download_counts(&mut self) {
        // Sidebar counts (game-filtered)
        let guard = self.download.rows.guard();
        let mut active = 0;
        for i in 0..guard.len() {
            if let Some(row) = guard.get(i)
                && row.entry.is_active()
            {
                active += 1;
            }
        }
        drop(guard);
        self.download.active_count = active;

        // Global count (all games)
        self.download.global_active_count =
            self.download.all.iter().filter(|e| e.is_active()).count();
    }

    /// Rebuild the downloads factory to show only entries for the current game.
    pub(crate) fn rebuild_downloads_view(&mut self) {
        // If a download is in progress, defer the sort until it completes.
        // Clearing and rebuilding the factory during an active download can crash
        // due to widget destruction racing with in-flight progress updates.
        if self.download.all.iter().any(|e| e.is_active()) {
            return;
        }
        let current_domain = self
            .selected_game()
            .and_then(crate::core::game::nexus_domain)
            .map(String::from);
        let show_hidden = self.download.show_hidden;
        let mut entries: Vec<&DownloadEntry> = self
            .download
            .all
            .iter()
            .filter(|entry| {
                if entry.hidden && !show_hidden {
                    return false;
                }
                match (&current_domain, &entry.game_domain) {
                    (Some(cur), Some(dom)) => cur == dom,
                    (_, None) => true,
                    (None, _) => true,
                }
            })
            .collect();
        match self.download.sort {
            crate::models::download::DownloadSort::Name => {
                entries.sort_by_key(|a| a.mod_name.to_lowercase())
            }
            crate::models::download::DownloadSort::Status => {
                entries.sort_by_key(|e| crate::app::types::download_status_sort_key(&e.status))
            }
            crate::models::download::DownloadSort::Default => {}
        }
        // Save scroll position before rebuilding; restore it on the next GLib iteration
        // so the layout has already been applied when we set the value.
        let vadj = self.download.scroll.vadjustment();
        let saved_pos = vadj.value();

        // Single guard acquisition: populate then filter in one locked scope.
        let query = self.shell.search_text.to_lowercase();
        let mut guard = self.download.rows.guard();
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
        let fid_zero_ambiguous = if let Some((mid, 0)) = nexus_ids {
            mod_archive_hash.is_none()
                && self
                    .download.all
                    .iter()
                    .filter(|e| {
                        e.status == DownloadStatus::Installed
                            && e.archive_path.as_ref().map(|p| p.exists()).unwrap_or(false)
                            && matches!(&e.nexus_ids, Some(NexusIds { mod_id: m, file_id: 0, .. }) if *m == mid)
                    })
                    .count()
                    > 1
        } else {
            false
        };

        let mut changed_entries = Vec::new();
        for entry in &mut self.download.all {
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
                (
                    Some((mid, 0)),
                    Some(NexusIds {
                        mod_id: emid,
                        file_id: 0,
                        ..
                    }),
                ) => {
                    mid == *emid
                        && if let Some(mod_hash) = mod_archive_hash {
                            entry.archive_hash.as_deref() == Some(mod_hash)
                        } else {
                            !fid_zero_ambiguous
                        }
                }
                (
                    Some((mid, fid)),
                    Some(NexusIds {
                        mod_id: emid,
                        file_id: efid,
                        ..
                    }),
                ) => mid == *emid && fid == *efid,
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
        self.download.all.iter_mut().find(|e| e.id == id)
    }

    pub(crate) fn update_download_status(
        &mut self,
        download_id: &str,
        status: DownloadStatus,
        msg: &str,
    ) {
        let prev_was_active = self
            .download
            .all
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
        let mut guard = self.download.rows.guard();
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
        if prev_was_active && !new_is_active && !self.download.all.iter().any(|e| e.is_active()) {
            self.rebuild_downloads_view();
        } else {
            self.refresh_download_counts();
        }
    }

    pub(crate) fn begin_download_metadata_fetch(&mut self, download_id: &str) {
        if let Some(entry) = self.download.all.iter().find(|e| e.id == download_id)
            && !entry.is_active()
        {
            self.download
                .metadata_previous_status
                .entry(download_id.to_string())
                .or_insert_with(|| entry.status.clone());
        }
        self.update_download_status(
            download_id,
            DownloadStatus::Extracting,
            "Fetching Nexus metadata...",
        );
    }

    pub(crate) fn finish_download_metadata_fetch(&mut self, download_id: &str) {
        let Some(status) = self.download.metadata_previous_status.remove(download_id) else {
            return;
        };
        let status = DownloadStatus::restored_after_metadata_fetch(&status);
        let msg = status.default_status_msg().to_string();
        self.update_download_status(download_id, status, &msg);
    }
}

#[cfg(test)]
mod tests {
    use super::replaced_download_index;
    use crate::app::state::ReplacementContext;
    use crate::models::download::{DownloadEntry, DownloadStatus, NexusIds};

    fn installed(id: &str, file_id: i64, hash: &str, path: std::path::PathBuf) -> DownloadEntry {
        let mut entry = DownloadEntry::new(
            id.to_string(),
            "Example".to_string(),
            Some(NexusIds {
                mod_id: 42,
                file_id,
                domain: "fallout4".to_string(),
            }),
        );
        entry.status = DownloadStatus::Installed;
        entry.archive_hash = Some(hash.to_string());
        entry.archive_path = Some(path);
        entry
    }

    #[test]
    fn replacement_selects_only_the_previous_archive() {
        let dir = tempfile::tempdir().expect("temporary directory");
        let old_path = dir.path().join("old.zip");
        let new_path = dir.path().join("new.zip");
        std::fs::write(&old_path, []).expect("old archive");
        std::fs::write(&new_path, []).expect("new archive");
        let entries = vec![
            installed("old", 10, "old-hash", old_path),
            installed("new", 11, "new-hash", new_path),
        ];
        let replacement = ReplacementContext {
            mod_id: "old-mod".to_string(),
            priority: 7,
            mod_name: "Example".to_string(),
            nexus_ids: Some((42, 10)),
            archive_hash: Some("old-hash".to_string()),
        };

        assert_eq!(
            replaced_download_index(&entries, &replacement, Some("new")),
            Some(0)
        );
        assert_eq!(replacement.priority, 7);
    }
}
