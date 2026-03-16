use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::core::{game, installer};
use crate::core::installer::PrepareResult;
use crate::models::download::DownloadStatus;
use crate::ui::mod_list::ModListItemKind;

use super::super::messages::{AppCmdMsg, AppMsg, PrepareResultMsg};
use super::super::App;

impl App {
    pub(crate) fn handle_toggle_downloads(&mut self) {
        self.downloads_visible = !self.downloads_visible;
    }

    pub(crate) fn handle_set_downloads_visible(&mut self, visible: bool) {
        self.downloads_visible = visible;
    }

    pub(crate) fn handle_download_sort_changed(
        &mut self,
        idx: u32,
    ) {
        let new_sort = match idx {
            1 => crate::app::types::DownloadSort::Name,
            2 => crate::app::types::DownloadSort::Status,
            _ => crate::app::types::DownloadSort::Default,
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

    pub(crate) fn handle_reinstall_download(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        self.reinstall_mode = true;
        self.handle_install_download(index, sender);
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
}
