use std::collections::HashSet;
use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::core::{game, installer};
use crate::core::installer::PrepareResult;
use crate::models::download::{DownloadEntry, DownloadStatus};
use crate::ui::mod_list::ModListItemKind;
use crate::utils::paths;

use super::free_fns::parse_nexus_mod_id;
use super::messages::{AppCmdMsg, AppMsg, PrepareResultMsg};
use super::types::{DownloadSort, NxmDownloadResult, download_status_sort_key};
use super::App;

impl App {
    // ── State helpers ──────────────────────────────────────────────────────────

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
            .and_then(game::nexus_domain)
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
            DownloadSort::Name => {
                entries.sort_by(|a, b| a.mod_name.to_lowercase().cmp(&b.mod_name.to_lowercase()))
            }
            DownloadSort::Status => entries.sort_by_key(|e| download_status_sort_key(&e.status)),
            DownloadSort::Default => {}
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

    // ── AppMsg handlers ────────────────────────────────────────────────────────

    pub(crate) fn handle_toggle_downloads(&mut self) {
        self.downloads_visible = !self.downloads_visible;
    }

    pub(crate) fn handle_download_sort_changed(
        &mut self,
        idx: u32,
    ) {
        let new_sort = match idx {
            1 => DownloadSort::Name,
            2 => DownloadSort::Status,
            _ => DownloadSort::Default,
        };
        // GTK may emit notify::selected even when the value didn't change (e.g. during
        // a #[watch] set_selected call). Skip the rebuild to avoid spurious redraws and
        // to prevent the factory from being cleared while a download is in progress.
        if new_sort == self.download_sort {
            return;
        }
        self.download_sort = new_sort;
        self.rebuild_downloads_view();
    }

    pub(crate) fn handle_clear_download_metadata(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let download_id = {
            let guard = self.downloads.guard();
            guard.get(idx).map(|r| r.entry.id.clone())
        };
        let Some(download_id) = download_id else {
            return;
        };
        // Reset metadata in backing store
        if let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == download_id) {
            // Revert name to archive filename
            if let Some(ref path) = entry.archive_path {
                entry.mod_name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
            }
            entry.metadata_fetched = false;
            entry.nexus_file_name = None;
            entry.nexus_is_primary = false;
            // If the archive is still present, allow reinstall
            if entry.status == DownloadStatus::Installed && entry.archive_path.is_some() {
                entry.status = DownloadStatus::Downloaded;
                entry.status_msg = String::new();
            }
        }
        // Update factory
        {
            let mut guard = self.downloads.guard();
            if let Some(row) = guard.get_mut(idx) {
                if let Some(ref path) = row.entry.archive_path {
                    row.entry.mod_name = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string();
                }
                row.entry.metadata_fetched = false;
                row.entry.nexus_file_name = None;
                row.entry.nexus_is_primary = false;
                if row.entry.status == DownloadStatus::Installed
                    && row.entry.archive_path.is_some()
                {
                    row.entry.status = DownloadStatus::Downloaded;
                    row.entry.status_msg = String::new();
                }
            }
        }
        // Persist
        if let Some(tracker) = self.tracker.clone()
            && let Some(entry) = self.all_downloads.iter().find(|e| e.id == download_id)
        {
            let entry = entry.clone();
            sender.oneshot_command(async move {
                let _ = tracker.save_download_entry(&entry).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
    }

    pub(crate) fn handle_rename_download(
        &mut self,
        index: DynamicIndex,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (download_id, current_name) = {
            let guard = self.downloads.guard();
            let Some(row) = guard.get(idx) else {
                return;
            };
            (row.entry.id.clone(), row.entry.mod_name.clone())
        };

        let entry = gtk::Entry::builder()
            .text(&current_name)
            .hexpand(true)
            .activates_default(true)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        let dialog =
            adw::MessageDialog::new(Some(root), Some("Rename Download"), None::<&str>);
        dialog.set_extra_child(Some(&entry));
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("apply", "Apply");
        dialog.set_default_response(Some("apply"));
        dialog.set_close_response("cancel");

        let input_sender = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "apply" {
                let name = entry.text().to_string();
                if !name.is_empty() {
                    input_sender
                        .send(AppMsg::ConfirmDownloadRename(download_id.clone(), name))
                        .ok();
                }
            }
        });
        dialog.present();
    }

    pub(crate) fn handle_confirm_download_rename(
        &mut self,
        download_id: String,
        new_name: String,
        sender: &ComponentSender<Self>,
    ) {
        // Update backing store
        if let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == download_id) {
            entry.mod_name = new_name.clone();
        }
        // Update factory in-place (no full rebuild needed)
        {
            let mut guard = self.downloads.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i)
                    && row.entry.id == download_id
                {
                    row.entry.mod_name = new_name.clone();
                    break;
                }
            }
        }
        // Persist
        if let Some(tracker) = self.tracker.clone()
            && let Some(entry) = self.all_downloads.iter().find(|e| e.id == download_id)
        {
            let entry = entry.clone();
            sender.oneshot_command(async move {
                let _ = tracker.save_download_entry(&entry).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
    }

    pub(crate) fn handle_install_download(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (archive_path, nexus_ids, download_id, suggested_name) = {
            let guard = self.downloads.guard();
            let Some(row) = guard.get(idx) else {
                return;
            };
            let Some(path) = row.entry.archive_path.clone() else {
                return;
            };
            // Build suggested name: prefer Nexus mod name + always append the per-file
            // label so multiple files from the same mod page get distinct mod names.
            let suggested = if row.entry.metadata_fetched {
                let mut name = row.entry.mod_name.clone();
                if let Some(ref file_name) = row.entry.nexus_file_name
                    && file_name != &name
                {
                    // If the file name already contains the mod name, use the file name
                    // directly to avoid "Mod Name - Abbrev - Mod Name" duplication.
                    if file_name.to_lowercase().contains(&name.to_lowercase()) {
                        name = file_name.clone();
                    } else {
                        name = format!("{name} - {file_name}");
                    }
                }
                name
            } else {
                path.file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            };
            (
                path,
                row.entry.nexus_ids.clone(),
                row.entry.id.clone(),
                suggested,
            )
        };

        // Switch to correct game based on nexus domain
        if let Some((_, _, ref domain)) = nexus_ids
            && let Some(target_game_id) = game::game_id_for_nexus_domain(domain)
            && let Some(game_idx) = self.games.iter().position(|g| g.id == target_game_id)
            && game_idx != self.selected_game_idx
        {
            self.selected_game_idx = game_idx;
            self.game_dropdown.set_selected(game_idx as u32);
        }

        // Store nexus_ids for the PendingInstall handoff
        self.pending_nexus_ids = nexus_ids;
        self.active_download_id = Some(download_id.clone());

        // Update status to Extracting
        self.update_download_status(
            &download_id,
            DownloadStatus::Extracting,
            "Extracting...",
        );

        // Feed into existing install pipeline
        self.installing = true;
        self.status_msg = Some("Extracting...".to_string());

        let extract_sender = sender.input_sender().clone();
        let on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
            Some(Box::new(move |done, total| {
                let frac = done as f64 / total as f64;
                let _ = extract_sender.send(AppMsg::InstallProgress(
                    frac,
                    format!("Extracting file {done}/{total}"),
                ));
            }));

        sender.oneshot_command(async move {
            let result: Result<PrepareResultMsg, String> = async {
                let hash_path = archive_path.clone();
                let archive_hash = tokio::task::spawn_blocking(move || {
                    crate::utils::archive::hash_archive_file(&hash_path).ok()
                })
                .await
                .unwrap_or(None);

                let prepare = installer::prepare_mod(&archive_path, on_extract_progress)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                let mod_name = suggested_name;
                match prepare {
                    PrepareResult::Normal {
                        file_list,
                        stripped_wrapper,
                        tmp_dir,
                    } => Ok(PrepareResultMsg::Normal {
                        file_list,
                        stripped_wrapper,
                        tmp_dir,
                        mod_name,
                        archive_hash,
                    }),
                    PrepareResult::Fomod {
                        config,
                        config_path,
                        tmp_dir,
                    } => Ok(PrepareResultMsg::Fomod {
                        config,
                        config_path,
                        tmp_dir,
                        mod_name,
                        archive_hash,
                    }),
                }
            }
            .await;
            AppCmdMsg::ModPrepared(result)
        });
    }

    pub(crate) fn handle_download_progress(
        &mut self,
        download_id: String,
        fraction: f64,
        msg: String,
    ) {
        // Update backing store
        if let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == download_id) {
            entry.progress = fraction;
            entry.status_msg = msg.clone();
        }
        // Update factory
        let mut guard = self.downloads.guard();
        for i in 0..guard.len() {
            if let Some(row) = guard.get_mut(i)
                && row.entry.id == download_id
            {
                row.entry.progress = fraction;
                row.entry.status_msg = msg;
                break;
            }
        }
    }

    pub(crate) fn handle_download_name_resolved(
        &mut self,
        download_id: String,
        name: String,
        game_domain: Option<String>,
        nexus_file_name: Option<String>,
        nexus_is_primary: bool,
        sender: &ComponentSender<Self>,
    ) {
        // Capture old domain before mutation to detect filtering changes
        let old_domain = self
            .all_downloads
            .iter()
            .find(|e| e.id == download_id)
            .and_then(|e| e.game_domain.clone());
        // Update backing store
        if let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == download_id) {
            entry.mod_name = name.clone();
            entry.metadata_fetched = true;
            entry.nexus_file_name = nexus_file_name.clone();
            entry.nexus_is_primary = nexus_is_primary;
            if let Some(ref domain) = game_domain {
                entry.game_domain = Some(domain.clone());
                if let Some((_, _, ref mut dom)) = entry.nexus_ids {
                    *dom = domain.clone();
                }
                // Auto-move archive from flat root to per-game subfolder
                if let Some(ref archive_path) = entry.archive_path
                    && archive_path.parent() == Some(self.downloads_dir.as_path())
                {
                    let target_dir = self.downloads_dir.join(domain);
                    if let Some(file_name) = archive_path.file_name() {
                        let _ = std::fs::create_dir_all(&target_dir);
                        let target = target_dir.join(file_name);
                        if std::fs::rename(archive_path, &target).is_ok() {
                            entry.archive_path = Some(target);
                        }
                    }
                }
            }
        }
        // Persist updated entry
        if let Some(tracker) = self.tracker.clone()
            && let Some(entry) = self.all_downloads.iter().find(|e| e.id == download_id)
        {
            let entry = entry.clone();
            sender.oneshot_command(async move {
                let _ = tracker.save_download_entry(&entry).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
        // Update factory
        {
            let mut guard = self.downloads.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i)
                    && row.entry.id == download_id
                {
                    row.entry.mod_name = name;
                    row.entry.metadata_fetched = true;
                    row.entry.nexus_file_name = nexus_file_name;
                    row.entry.nexus_is_primary = nexus_is_primary;
                    if let Some(ref domain) = game_domain {
                        row.entry.game_domain = Some(domain.clone());
                        if let Some((_, _, ref mut dom)) = row.entry.nexus_ids {
                            *dom = domain.clone();
                        }
                    }
                    break;
                }
            }
        }
        // Rebuild views only if game_domain actually changed (affects filtering)
        if game_domain.is_some() && old_domain != game_domain {
            self.rebuild_downloads_view();
        }
    }

    pub(crate) fn handle_fetch_download_metadata(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (download_id, nexus_mod_id, nexus_file_id, stored_domain, archive_filename) = {
            let guard = self.downloads.guard();
            let Some(row) = guard.get(idx) else { return };
            let Some((nexus_mod_id, nexus_file_id, ref domain)) = row.entry.nexus_ids else {
                drop(guard);
                self.toaster
                    .toast("No Nexus info available for this download");
                return;
            };
            // Extract archive filename for disk-scanned entries (file_id == 0)
            // so we can match it against NexusFileEntry.file_name.
            let archive_filename = row
                .entry
                .archive_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            (
                row.entry.id.clone(),
                nexus_mod_id,
                nexus_file_id,
                domain.clone(),
                archive_filename,
            )
        };
        // Use stored domain if non-empty, otherwise fall back to current game
        let domain = if stored_domain.is_empty() {
            self.selected_game()
                .and_then(game::nexus_domain)
                .unwrap_or("skyrimspecialedition")
                .to_string()
        } else {
            stored_domain
        };

        let Some(tracker) = self.tracker.clone() else {
            return;
        };

        // Find the installed mod that corresponds to this download entry (if any),
        // so the manual fetch can mirror the NXM auto-path and write metadata to
        // the mods table too (fixes disparity between manual vs. NXM metadata fetch).
        let installed_mod_id: Option<String> = {
            let guard = self.mods.guard();
            guard
                .iter()
                .filter_map(|item| match &item.kind {
                    ModListItemKind::Mod(init) => Some(&init.mod_entry),
                    _ => None,
                })
                .find(|e| {
                    e.nexus_mod_id == Some(nexus_mod_id)
                        && (nexus_file_id == 0 || e.nexus_file_id == Some(nexus_file_id))
                })
                .map(|e| e.id.clone())
        };

        let input_sender = sender.input_sender().clone();
        self.toaster.toast("Fetching metadata...");
        sender.oneshot_command(async move {
            let result: Result<(String, String, String), String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .map_err(|e| e.to_string())?
                    .filter(|k| !k.is_empty())
                    .ok_or("No API key configured. Set it in Settings.")?;
                let client = crate::core::nexus_api::NexusClient::new(api_key);
                let (info, rate_limits) = client
                    .get_mod_info(&domain, nexus_mod_id)
                    .await
                    .map_err(|e| e.to_string())?;
                if let Some(rl) = rate_limits {
                    let _ = input_sender.send(AppMsg::RateLimitUpdated(rl));
                }
                // Fetch file list to resolve the per-file display name.
                // • file_id != 0 (NXM download): match by exact file_id.
                // • file_id == 0 (disk-scanned): match by archive filename so
                //   manually downloaded files also get their proper Nexus name.
                let file_info = if nexus_file_id != 0 || archive_filename.is_some() {
                    match client.get_mod_files(&domain, nexus_mod_id).await {
                        Ok((files, rate_limits)) => {
                            if let Some(rl) = rate_limits {
                                let _ = input_sender.send(AppMsg::RateLimitUpdated(rl));
                            }
                            if nexus_file_id != 0 {
                                // NXM path: exact match by file ID
                                files.files.into_iter().find(|f| f.file_id == nexus_file_id)
                            } else {
                                // Disk-scan path: match by archive filename
                                let fname = archive_filename.as_deref().unwrap_or("");
                                files.files.into_iter().find(|f| f.file_name == fname)
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "deployd: get_mod_files({domain}/{nexus_mod_id}) \
                                 failed (non-fatal): {e}"
                            );
                            let _ = input_sender.send(AppMsg::ShowToast(format!(
                                "Mod name fetched, but file list unavailable: {e}"
                            )));
                            None
                        }
                    }
                } else {
                    None
                };
                let nexus_file_name = file_info.as_ref().map(|f| f.name.clone());
                let file_version = file_info.as_ref().and_then(|f| f.version.clone());
                let nexus_is_primary = file_info.map(|f| f.is_primary).unwrap_or(false);
                // Capture before DownloadNameResolved moves info.name
                let mod_version = info.version.clone();
                let mod_author = info.author.clone();
                let _ = input_sender.send(AppMsg::DownloadNameResolved(
                    download_id,
                    info.name.clone(),
                    Some(domain),
                    nexus_file_name,
                    nexus_is_primary,
                ));
                // Mirror NXM auto-path: write metadata back to the installed mod row.
                if let Some(ref mod_id) = installed_mod_id {
                    tracker
                        .update_mod_nexus_metadata(
                            mod_id,
                            &mod_version,
                            &mod_author,
                            info.summary.as_deref().unwrap_or(""),
                        )
                        .await
                        .map_err(|e| e.to_string())?;
                    let installed_version =
                        file_version.unwrap_or_else(|| mod_version.clone());
                    tracker
                        .set_mod_installed_version(mod_id, &installed_version)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Ok((info.name, mod_version, mod_author))
            }
            .await;
            match result {
                Ok((name, version, author)) => AppCmdMsg::NexusMetadataFetched(Ok((
                    String::new(),
                    version,
                    author,
                    name,
                ))),
                Err(e) => AppCmdMsg::NexusMetadataFetched(Err(e)),
            }
        });
    }

    pub(crate) fn handle_nxm_link_received(
        &mut self,
        uri: String,
        sender: &ComponentSender<Self>,
    ) {
        use crate::core::nexus_api::NexusClient;
        use crate::core::nxm::NxmLink;

        let Some(tracker) = self.tracker.clone() else {
            // DB not initialized yet, store for processing after init completes
            self.pending_nxm = Some(uri);
            return;
        };

        let link = match NxmLink::parse(&uri) {
            Ok(l) => l,
            Err(e) => {
                self.toaster.toast(&format!("Invalid NXM link: {e}"));
                return;
            }
        };

        // Check game domain is supported
        if game::game_id_for_nexus_domain(&link.domain).is_none() {
            self.toaster
                .toast(&format!("Unsupported game: {}", link.domain));
            return;
        }

        // Create download entry and add to sidebar
        let download_id = uuid::Uuid::new_v4().to_string();
        let mod_name = format!("Mod {} (file {})", link.mod_id, link.file_id);
        let nexus_ids = Some((link.mod_id, link.file_id, link.domain.clone()));
        let entry = DownloadEntry::new(download_id.clone(), mod_name, nexus_ids);
        self.all_downloads.push(entry.clone());
        // Push directly to the factory instead of calling rebuild_downloads_view().
        // rebuild_downloads_view() has an early-return guard for active downloads,
        // so it would skip adding this entry — leaving it invisible until the next
        // unguarded rebuild (e.g. sort or scan). Pushing directly avoids that.
        {
            let mut guard = self.downloads.guard();
            guard.push_back(entry);
        }
        self.refresh_download_counts();
        self.downloads_visible = true;
        self.active_download_id = Some(download_id.clone());

        let input_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result: Result<NxmDownloadResult, String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| {
                        "No Nexus API key configured. Set it in Settings.".to_string()
                    })?;

                // Read configured downloads dir (fallback to default), with per-game subfolder
                let download_dir = {
                    let base =
                        match tracker.get_setting("downloads_dir").await.ok().flatten() {
                            Some(dir) => PathBuf::from(dir),
                            None => paths::default_downloads_dir(),
                        };
                    base.join(&link.domain)
                };

                let client = NexusClient::new(api_key);

                // Fetch mod info to get the real name
                if let Ok((mod_info, rate_limits)) =
                    client.get_mod_info(&link.domain, link.mod_id).await
                {
                    if let Some(rl) = rate_limits {
                        let _ = input_sender.send(AppMsg::RateLimitUpdated(rl));
                    }
                    let _ = input_sender.send(AppMsg::DownloadNameResolved(
                        download_id.clone(),
                        mod_info.name,
                        Some(link.domain.clone()),
                        None,
                        false,
                    ));
                }

                // Get download links
                let (links, rate_limits) = client
                    .get_download_links(
                        &link.domain,
                        link.mod_id,
                        link.file_id,
                        link.key.as_deref(),
                        link.expires.as_deref(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                if let Some(rl) = rate_limits {
                    let _ = input_sender.send(AppMsg::RateLimitUpdated(rl));
                }

                let download_url = links
                    .first()
                    .map(|l| l.uri.clone())
                    .ok_or_else(|| "No download links returned".to_string())?;

                // Get mod files to find the filename
                let (files, rate_limits) = client
                    .get_mod_files(&link.domain, link.mod_id)
                    .await
                    .map_err(|e| e.to_string())?;
                if let Some(rl) = rate_limits {
                    let _ = input_sender.send(AppMsg::RateLimitUpdated(rl));
                }

                let nexus_file = files.files.iter().find(|f| f.file_id == link.file_id);
                let file_name = nexus_file
                    .map(|f| {
                        let raw = f.file_name.clone();
                        // If another file in this mod shares the same file_name,
                        // disambiguate by injecting file_id before the extension.
                        let has_duplicate =
                            files.files.iter().filter(|e| e.file_name == raw).count() > 1;
                        if has_duplicate {
                            let p = std::path::Path::new(&raw);
                            match (p.file_stem(), p.extension()) {
                                (Some(stem), Some(ext)) => format!(
                                    "{}-{}.{}",
                                    stem.to_string_lossy(),
                                    f.file_id,
                                    ext.to_string_lossy()
                                ),
                                _ => format!("{}-{}", raw, f.file_id),
                            }
                        } else {
                            raw
                        }
                    })
                    .unwrap_or_else(|| {
                        format!("nexus_{}_{}.zip", link.mod_id, link.file_id)
                    });
                let nexus_file_name = nexus_file.map(|f| f.name.clone());
                let nexus_is_primary = nexus_file.map(|f| f.is_primary).unwrap_or(false);

                // Download to configured downloads folder
                std::fs::create_dir_all(&download_dir).map_err(|e| e.to_string())?;
                let dest = download_dir.join(&file_name);

                let dl_id = download_id.clone();
                let progress_sender = input_sender.clone();
                client
                    .download_file(&download_url, &dest, move |downloaded, total| {
                        if total > 0 {
                            let frac = downloaded as f64 / total as f64;
                            let mb_done = downloaded as f64 / 1_048_576.0;
                            let mb_total = total as f64 / 1_048_576.0;
                            let _ = progress_sender.send(AppMsg::DownloadProgress(
                                dl_id.clone(),
                                frac,
                                format!("Downloading {mb_done:.1}/{mb_total:.1} MB"),
                            ));
                        }
                    })
                    .await
                    .map_err(|e| e.to_string())?;

                Ok(NxmDownloadResult {
                    download_id: download_id.clone(),
                    archive_path: dest,
                    mod_id: link.mod_id,
                    file_id: link.file_id,
                    domain: link.domain,
                    file_name,
                    nexus_file_name,
                    nexus_is_primary,
                })
            }
            .await;
            AppCmdMsg::NxmDownloadComplete(download_id, result)
        });
    }

    pub(crate) fn handle_check_updates(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        self.toaster.toast("Checking for updates...");

        let input_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result: Result<Vec<(String, String, String)>, String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| {
                        "No Nexus API key configured. Set it in Settings.".to_string()
                    })?;

                let client = crate::core::nexus_api::NexusClient::new(api_key);
                let mods = tracker
                    .mods_with_nexus_ids(&game.id)
                    .await
                    .map_err(|e| e.to_string())?;

                let Some(domain) = game::nexus_domain(&game) else {
                    return Err("Unsupported game for Nexus".to_string());
                };

                let mut updates = Vec::new();
                for m in &mods {
                    let nexus_mod_id = m.nexus_mod_id.unwrap();
                    match client.get_mod_files(domain, nexus_mod_id).await {
                        Ok((files_resp, rate_limits)) => {
                            if let Some(rl) = rate_limits {
                                let _ = input_sender.send(AppMsg::RateLimitUpdated(rl));
                            }
                            // Find the main file with highest file_id
                            if let Some(latest_file) = files_resp
                                .files
                                .iter()
                                .filter(|f| f.is_primary)
                                .max_by_key(|f| f.file_id)
                            {
                                let latest_ver =
                                    latest_file.version.as_deref().unwrap_or("");
                                let current_ver = m.version.as_deref().unwrap_or("");
                                // Skip if installed version is unknown — we can't
                                // compare and would always report a false update.
                                if !latest_ver.is_empty()
                                    && !current_ver.is_empty()
                                    && latest_ver != current_ver
                                {
                                    tracker
                                        .set_latest_version(&m.id, latest_ver)
                                        .await
                                        .ok();
                                    updates.push((
                                        m.id.clone(),
                                        m.name.clone(),
                                        latest_ver.to_string(),
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("deployd: update check failed for {}: {e}", m.name);
                        }
                    }
                }

                Ok(updates)
            }
            .await;
            AppCmdMsg::UpdatesChecked(result)
        });
    }

    pub(crate) fn handle_scan_downloads_folder(&mut self, sender: &ComponentSender<Self>) {
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
        self.all_downloads.retain(|e| {
            e.is_active()
                || e.status == DownloadStatus::Installed
                || e.archive_path.as_ref().map(|p| p.exists()).unwrap_or(false)
        });

        // Collect existing archive paths from backing store to avoid duplicates.
        // Also collect filenames for path-change dedup (same file, different folder).
        let existing: HashSet<PathBuf> = self
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

                let nexus_ids = parse_nexus_mod_id(&file_name)
                    .map(|mod_id| (mod_id, 0i64, domain.to_string()));

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
        let existing_after: HashSet<PathBuf> = self
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

                let nexus_ids = parse_nexus_mod_id(&file_name)
                    .map(|mod_id| (mod_id, 0i64, String::new()));

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

    // ── AppCmdMsg handlers ─────────────────────────────────────────────────────

    pub(crate) fn handle_cmd_nexus_metadata_fetched(
        &mut self,
        result: Result<(String, String, String, String), String>,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok((_mod_id, version, author, nexus_name)) => {
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
                    let names: Vec<_> =
                        updates.iter().map(|(_, name, _)| name.as_str()).collect();
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

    /// Download the latest deployd AppImage from Nexus and replace the currently running one.
    /// Only reachable when APPIMAGE env var is set (i.e. running as an AppImage).
    pub(crate) fn handle_self_update_download(&mut self, sender: &ComponentSender<Self>) {
        use crate::core::nexus_api::NexusClient;
        use crate::core::update_check::{NEXUS_MOD_ID, NEXUS_DOMAIN};

        let Some(tracker) = self.tracker.clone() else { return };
        let Ok(appimage_path) = std::env::var("APPIMAGE") else { return };

        self.toaster.toast("Downloading update...");

        sender.oneshot_command(async move {
            let result: Result<(), String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .ok()
                    .flatten()
                    .filter(|k| !k.is_empty())
                    .ok_or_else(|| "No Nexus API key — configure it in Settings.".to_string())?;

                let client = NexusClient::new(api_key);

                let (files_resp, _) = client
                    .get_mod_files(NEXUS_DOMAIN, NEXUS_MOD_ID)
                    .await
                    .map_err(|e| e.to_string())?;

                let file = files_resp
                    .files
                    .into_iter()
                    .find(|f| f.is_primary || f.file_name.ends_with(".AppImage"))
                    .ok_or_else(|| "AppImage file not found on the Nexus mod page.".to_string())?;

                let (links, _) = client
                    .get_download_links(NEXUS_DOMAIN, NEXUS_MOD_ID, file.file_id, None, None)
                    .await
                    .map_err(|e| e.to_string())?;

                let url = links
                    .into_iter()
                    .next()
                    .map(|l| l.uri)
                    .ok_or_else(|| "No download link returned by Nexus.".to_string())?;

                let temp_path = format!("{appimage_path}.new");
                let dest = std::path::Path::new(&temp_path);

                client
                    .download_file(&url, dest, |_, _| {})
                    .await
                    .map_err(|e| e.to_string())?;

                // Make the downloaded file executable before replacing the running one.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o755);
                    std::fs::set_permissions(&temp_path, perms).map_err(|e| e.to_string())?;
                }

                std::fs::rename(&temp_path, &appimage_path).map_err(|e| e.to_string())?;

                Ok(())
            }
            .await;
            AppCmdMsg::AppUpdateResult(result)
        });
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
                        .update_url
                        .as_deref()
                        .unwrap_or(crate::core::update_check::NEXUS_PAGE_URL);
                    let _ = open::that(url);
                }
            }
        }
    }
}
