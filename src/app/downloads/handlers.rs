use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::core::installer::PrepareResult;
use crate::core::{game, installer};
use crate::models::download::{DownloadStatus, NexusIds};
use crate::ui::mod_list::ModListItemKind;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg, PrepareResultMsg};
use super::super::progress::throttled_download_install_progress;
use super::super::types::WorkKind;

impl App {
    pub(crate) fn handle_toggle_downloads(&mut self) {
        self.downloads_visible = !self.downloads_visible;
    }

    pub(crate) fn handle_set_downloads_visible(&mut self, visible: bool) {
        self.downloads_visible = visible;
    }

    pub(crate) fn handle_download_sort_changed(&mut self, idx: u32) {
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
                if row.entry.status == DownloadStatus::Installed && row.entry.archive_path.is_some()
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
        root: &adw::ApplicationWindow,
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

        let dialog = adw::AlertDialog::builder()
            .heading("Rename Download")
            .build();
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
        dialog.present(Some(root));
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
        let (archive_path, nexus_ids, download_id, suggested_name, metadata_fetched) = {
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
                row.entry.metadata_fetched,
            )
        };

        // Switch to correct game based on nexus domain
        if let Some(NexusIds { ref domain, .. }) = nexus_ids
            && let Some(target_game_id) = game::game_id_for_nexus_domain(domain)
            && let Some(game_idx) = self.games.iter().position(|g| g.id == target_game_id)
            && game_idx != self.selected_game_idx
        {
            self.selected_game_idx = game_idx;
            self.game_dropdown.set_selected(game_idx as u32);
        }

        // Store nexus_ids for the PendingInstall handoff
        self.pending_nexus_ids = nexus_ids.clone();
        self.active_install_download_id = Some(download_id.clone());
        self.pending_fetched_name = None;

        // If nexus IDs are known but metadata hasn't been fetched yet, fetch the
        // mod name (and file name) from Nexus in parallel with archive extraction
        // so the pre-install dialog can propose the real name.
        if !metadata_fetched
            && let Some(NexusIds {
                mod_id: nexus_mod_id,
                file_id: nexus_file_id,
                ref domain,
            }) = nexus_ids
            && let Some(tracker) = self.tracker.clone()
        {
            let domain = domain.clone();
            // Derive archive filename for disk-scan entries (file_id == 0).
            let archive_filename: Option<String> = archive_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned());
            let download_id_for_cmd = download_id.clone();
            sender.oneshot_command(async move {
                let Some(api_key) = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .ok()
                    .flatten()
                    .filter(|k| !k.is_empty())
                else {
                    return AppCmdMsg::PrioritySaved(Ok(()));
                };
                let client = crate::core::nexus_api::NexusClient::new(api_key);
                let Ok((info, _)) = client.get_mod_info(&domain, nexus_mod_id).await else {
                    return AppCmdMsg::PrioritySaved(Ok(()));
                };
                let mod_name = info.name;

                // Fetch file list when we have a file_id (NXM path) or an archive
                // filename (disk-scan path). Mirror start_nexus_metadata_fetch logic.
                if nexus_file_id != 0 || archive_filename.is_some() {
                    let file_entry = client
                        .get_mod_files(&domain, nexus_mod_id)
                        .await
                        .ok()
                        .and_then(|(resp, _)| {
                            if nexus_file_id != 0 {
                                resp.files.into_iter().find(|f| f.file_id == nexus_file_id)
                            } else {
                                let raw = archive_filename.as_deref().unwrap_or("");
                                let fname_norm =
                                    crate::app::free_fns::normalize_nexus_filename(raw);
                                let local_ts = crate::app::free_fns::extract_nexus_timestamp(raw);
                                let candidates: Vec<_> = resp
                                    .files
                                    .into_iter()
                                    .filter(|f| {
                                        crate::app::free_fns::normalize_nexus_filename(&f.file_name)
                                            == fname_norm
                                    })
                                    .collect();
                                local_ts
                                    .and_then(|ts| {
                                        candidates
                                            .iter()
                                            .find(|f| f.uploaded_timestamp == Some(ts))
                                            .cloned()
                                    })
                                    .or_else(|| candidates.into_iter().next())
                            }
                        });
                    if let Some(ref entry) = file_entry {
                        let fname = &entry.name;
                        let combined = if fname.to_lowercase().contains(&mod_name.to_lowercase()) {
                            fname.clone()
                        } else {
                            format!("{mod_name} - {fname}")
                        };
                        AppCmdMsg::PendingMetadataFetched(combined)
                    } else if nexus_file_id == 0 {
                        // Archive filename didn't match any Nexus file — ask the user
                        // to supply a file ID so the label can be completed.
                        AppCmdMsg::PendingFileNameUnresolved {
                            partial_name: mod_name,
                            download_id: download_id_for_cmd,
                            mod_id: nexus_mod_id,
                            domain,
                        }
                    } else {
                        // Had a file_id but Nexus returned no matching entry.
                        AppCmdMsg::PendingMetadataFetched(mod_name)
                    }
                } else {
                    AppCmdMsg::PendingMetadataFetched(mod_name)
                }
            });
        }

        self.update_download_status(
            &download_id,
            DownloadStatus::Extracting,
            "Hashing archive...",
        );

        // Feed into existing install pipeline
        self.installing = true;
        self.begin_work(WorkKind::PreparingArchive, "Hashing archive...");

        let extract_sender = sender.input_sender().clone();
        let on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
            Some(throttled_download_install_progress(
                extract_sender,
                download_id.clone(),
                "Extracting archive...",
            ));
        let processing_sender = sender.input_sender().clone();
        let processing_download_id = download_id.clone();
        let on_processing: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
            let _ = processing_sender
                .send(AppMsg::InstallProgress(1.0, "Reading FOMOD...".to_string()));
            let _ = processing_sender.send(AppMsg::DownloadProgress(
                processing_download_id,
                1.0,
                "Reading FOMOD...".to_string(),
            ));
        }));

        sender.oneshot_command(async move {
            let result: Result<PrepareResultMsg, String> = async {
                let timing_start = std::time::Instant::now();
                let hash_path = archive_path.clone();
                let archive_path_str = Some(archive_path.to_string_lossy().to_string());
                let archive_hash = tokio::task::spawn_blocking(move || {
                    crate::utils::archive::hash_archive_file(&hash_path).ok()
                })
                .await
                .unwrap_or(None);
                crate::app::timing::log_phase(
                    "install.hash_archive",
                    "download",
                    timing_start,
                    Some(1),
                );

                let timing_start = std::time::Instant::now();
                let prepare =
                    installer::prepare_mod(&archive_path, on_extract_progress, on_processing)
                        .await
                        .map_err(|e| format!("{e:#}"))?;
                crate::app::timing::log_phase(
                    "install.prepare_archive",
                    "download",
                    timing_start,
                    None,
                );
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
                        archive_path: archive_path_str,
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
                        archive_path: archive_path_str,
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
        let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == download_id) else {
            return;
        };
        if !entry.is_active() {
            return;
        }
        entry.progress = fraction;
        entry.status_msg = msg.clone();

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

    // All arguments come from a single NxmResolved message variant; a wrapper struct
    // would add indirection without improving clarity.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn handle_download_name_resolved(
        &mut self,
        download_id: String,
        name: String,
        game_domain: Option<String>,
        nexus_file_name: Option<String>,
        nexus_is_primary: bool,
        resolved_file_id: Option<i64>,
        version: Option<String>,
        author: Option<String>,
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
            if version.is_some() {
                entry.version = version.clone();
            }
            if author.is_some() {
                entry.author = author.clone();
            }
            if let Some(fid) = resolved_file_id
                && let Some(NexusIds {
                    file_id: ref mut stored_fid,
                    ..
                }) = entry.nexus_ids
                && *stored_fid == 0
            {
                *stored_fid = fid;
            }
            if let Some(ref domain) = game_domain {
                entry.game_domain = Some(domain.clone());
                if let Some(NexusIds {
                    domain: ref mut dom,
                    ..
                }) = entry.nexus_ids
                {
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
                    if version.is_some() {
                        row.entry.version = version.clone();
                    }
                    if author.is_some() {
                        row.entry.author = author.clone();
                    }
                    if let Some(fid) = resolved_file_id
                        && let Some(NexusIds {
                            file_id: ref mut stored_fid,
                            ..
                        }) = row.entry.nexus_ids
                        && *stored_fid == 0
                    {
                        *stored_fid = fid;
                    }
                    if let Some(ref domain) = game_domain {
                        row.entry.game_domain = Some(domain.clone());
                        if let Some(NexusIds {
                            domain: ref mut dom,
                            ..
                        }) = row.entry.nexus_ids
                        {
                            *dom = domain.clone();
                        }
                    }
                    break;
                }
            }
        }
        // Propagate resolved version and author to the installed mod row so the Mod Order panel shows them.
        if let Some(ref v) = version
            && let Some(entry) = self.all_downloads.iter().find(|e| e.id == download_id)
            && let Some(NexusIds {
                mod_id: nxs_mod_id,
                file_id: nxs_file_id,
                ..
            }) = entry.nexus_ids
            && nxs_file_id != 0
            && let Some(tracker) = self.tracker.clone()
            && let Some(game) = self.selected_game().cloned()
        {
            let version_db = v.clone();
            let version_ui = v.clone();
            let author_db = author.clone();
            let author_ui = author.clone();
            let game_id = game.id.clone();
            sender.oneshot_command(async move {
                if let Some(ref a) = author_db {
                    tracker
                        .update_mod_version_author_by_nexus_ids(
                            &game_id,
                            nxs_mod_id,
                            nxs_file_id,
                            &version_db,
                            a,
                        )
                        .await
                        .ok();
                } else {
                    tracker
                        .update_mod_version_by_nexus_ids(
                            &game_id,
                            nxs_mod_id,
                            nxs_file_id,
                            &version_db,
                        )
                        .await
                        .ok();
                }
                AppCmdMsg::PrioritySaved(Ok(()))
            });
            // Surgically update the matching factory row so the subtitle appears without a reload.
            let mut guard = self.mods.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i)
                    && let Some(init) = row.mod_row_mut()
                    && init.mod_entry.nexus_mod_id == Some(nxs_mod_id)
                    && init.mod_entry.nexus_file_id == Some(nxs_file_id)
                {
                    init.mod_entry.version = Some(version_ui.clone());
                    if let Some(a) = author_ui {
                        init.mod_entry.author = Some(a);
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
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        // If the entry has no nexus_ids yet, ask the user for a Nexus URL or mod ID.
        {
            let no_nexus_ids = {
                let guard = self.downloads.guard();
                let Some(row) = guard.get(idx) else { return };
                if row.entry.nexus_ids.is_some() {
                    None
                } else {
                    Some((row.entry.id.clone(), row.entry.game_domain.clone()))
                }
            };
            if let Some((download_id, game_domain)) = no_nexus_ids {
                let fallback_domain = self
                    .selected_game()
                    .and_then(game::nexus_domain)
                    .unwrap_or("skyrimspecialedition")
                    .to_string();
                let domain = game_domain
                    .filter(|d| !d.is_empty())
                    .unwrap_or(fallback_domain);

                let text_entry = gtk::Entry::builder()
                    .placeholder_text("Nexus mod URL or ID  (e.g. 101)")
                    .hexpand(true)
                    .activates_default(true)
                    .margin_top(8)
                    .margin_bottom(8)
                    .margin_start(8)
                    .margin_end(8)
                    .build();

                let dialog = adw::AlertDialog::builder()
                    .heading("Enter Nexus Mod ID")
                    .body("Paste a Nexus mod URL or type the numeric mod ID.")
                    .build();
                dialog.set_extra_child(Some(&text_entry));
                dialog.add_response("cancel", "Cancel");
                dialog.add_response("fetch", "Fetch");
                dialog.set_default_response(Some("fetch"));
                dialog.set_close_response("cancel");
                dialog.set_response_appearance("fetch", adw::ResponseAppearance::Suggested);

                let input_sender = sender.input_sender().clone();
                dialog.connect_response(None, move |_, response| {
                    if response != "fetch" {
                        return;
                    }
                    let raw = text_entry.text().to_string();
                    let Some(mod_id) = crate::app::free_fns::parse_nexus_mod_id_from_input(&raw)
                    else {
                        return;
                    };
                    let _ = input_sender.send(AppMsg::ConfirmNexusIdEntry(
                        download_id.clone(),
                        mod_id,
                        domain.clone(),
                    ));
                });
                dialog.present(Some(root));
                return;
            }
        }

        let download_id = {
            let guard = self.downloads.guard();
            let Some(row) = guard.get(idx) else { return };
            row.entry.id.clone()
        };
        self.start_nexus_metadata_fetch(download_id, sender);
    }

    /// Called after the user confirms a Nexus mod ID in the "Enter Nexus Mod ID" dialog.
    ///
    /// Updates `nexus_ids` on the entry, persists it, then runs the metadata fetch.
    pub(crate) fn handle_confirm_nexus_id_entry(
        &mut self,
        download_id: String,
        mod_id: i64,
        domain: String,
        sender: &ComponentSender<Self>,
    ) {
        let new_nexus_ids = Some(NexusIds {
            mod_id,
            file_id: 0,
            domain,
        });

        // Update backing store
        if let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == download_id) {
            entry.nexus_ids = new_nexus_ids.clone();
        }

        // Update factory
        {
            let mut guard = self.downloads.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i)
                    && row.entry.id == download_id
                {
                    row.entry.nexus_ids = new_nexus_ids;
                    break;
                }
            }
        }

        // Persist the updated entry
        if let (Some(tracker), Some(entry)) = (
            self.tracker.clone(),
            self.all_downloads
                .iter()
                .find(|e| e.id == download_id)
                .cloned(),
        ) {
            sender.oneshot_command(async move {
                let _ = tracker.save_download_entry(&entry).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }

        self.start_nexus_metadata_fetch(download_id, sender);
    }

    /// Perform the async Nexus metadata fetch for a download entry identified by ID.
    ///
    /// Looks up the entry in `self.all_downloads` to collect the required fields,
    /// then dispatches the oneshot command that calls the API.
    pub(crate) fn start_nexus_metadata_fetch(
        &mut self,
        download_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let (
            nexus_mod_id,
            nexus_file_id,
            stored_domain,
            archive_filename,
            archive_hash,
            archive_md5,
            archive_path,
        ) = {
            let Some(entry) = self.all_downloads.iter().find(|e| e.id == download_id) else {
                return;
            };
            let Some(NexusIds {
                mod_id: nexus_mod_id,
                file_id: nexus_file_id,
                ref domain,
            }) = entry.nexus_ids
            else {
                return;
            };
            let archive_filename = entry
                .archive_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            let archive_hash = entry.archive_hash.clone();
            let archive_md5 = entry.archive_md5.clone();
            let archive_path = entry.archive_path.clone();
            (
                nexus_mod_id,
                nexus_file_id,
                domain.clone(),
                archive_filename,
                archive_hash,
                archive_md5,
                archive_path,
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
                        && if nexus_file_id != 0 {
                            // Known file ID: require exact match so different versions
                            // of the same mod each map to their own entry.
                            e.nexus_file_id == Some(nexus_file_id)
                        } else if let (Some(dl_hash), Some(mod_hash)) =
                            (&archive_hash, &e.archive_hash)
                        {
                            // Disk-scanned (file_id == 0): use archive hash to
                            // distinguish multiple versions of the same mod.
                            dl_hash == mod_hash
                        } else {
                            // No hash available: fall back to first match.
                            true
                        }
                })
                .map(|e| e.id.clone())
        };

        let input_sender = sender.input_sender().clone();
        self.begin_download_metadata_fetch(&download_id);
        self.show_toast("Fetching metadata...");
        sender.oneshot_command(async move {
            let timing_start = std::time::Instant::now();
            let result: Result<(String, String, String), String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .map_err(|e| e.to_string())?
                    .filter(|k| !k.is_empty())
                    .ok_or("No API key configured. Set it in Settings.")?;
                let client = crate::core::nexus_api::NexusClient::new(api_key);

                // ── MD5 path: single API call resolves mod + file ─────────────────
                // Lazily compute MD5 when not yet cached (archive_md5 is None).
                // The result is persisted via ArchiveMd5Computed so subsequent
                // fetches skip the file read.
                let effective_md5: Option<String> = if archive_md5.is_some() {
                    archive_md5
                } else if let Some(ref path) = archive_path {
                    let p = path.clone();
                    let md5 = tokio::task::spawn_blocking(move || {
                        crate::utils::archive::compute_md5(&p).ok()
                    })
                    .await
                    .unwrap_or(None);
                    if let Some(ref m) = md5 {
                        let _ = input_sender
                            .send(AppMsg::ArchiveMd5Computed(download_id.clone(), m.clone()));
                    }
                    md5
                } else {
                    None
                };

                if let Some(ref md5) = effective_md5 {
                    match client.md5_search(&domain, md5).await {
                        Ok((results, rl)) => {
                            if let Some(rl) = rl {
                                let _ = input_sender.send(AppMsg::RateLimitUpdated(rl));
                            }
                            if let Some(hit) = results.into_iter().next() {
                                let file_entry = hit.file_details;
                                let mod_info = hit.r#mod;
                                let file_version = file_entry.version.clone();
                                let mod_version = mod_info.version.clone();
                                let mod_author = mod_info.author.clone();
                                let resolved_version =
                                    file_version.or_else(|| Some(mod_version.clone()));
                                let correct_domain = mod_info.domain_name.clone();
                                let _ = input_sender.send(AppMsg::DownloadNameResolved(
                                    download_id.clone(),
                                    mod_info.name.clone(),
                                    Some(correct_domain),
                                    Some(file_entry.name.clone()),
                                    file_entry.is_primary,
                                    Some(file_entry.file_id),
                                    resolved_version.clone(),
                                    Some(mod_author.clone()),
                                ));
                                let _ = input_sender.send(AppMsg::ShowToast(format!(
                                    "{} v{mod_version} by {mod_author}",
                                    mod_info.name
                                )));
                                if let Some(ref mod_id) = installed_mod_id {
                                    tracker
                                        .update_mod_nexus_metadata(
                                            mod_id,
                                            &mod_version,
                                            &mod_author,
                                            mod_info.summary.as_deref().unwrap_or(""),
                                        )
                                        .await
                                        .map_err(|e| e.to_string())?;
                                }
                                return Ok((mod_info.name, mod_version, mod_author));
                            }
                        }
                        Err(e) => {
                            eprintln!("deployd: md5_search failed (non-fatal, falling back): {e}");
                        }
                    }
                }
                // ─────────────────────────────────────────────────────────────────

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
                                // Disk-scan path: match by normalized archive filename (strips
                                // extension and 10-digit CDN timestamp).  When multiple Nexus
                                // files share the same base name, prefer the one whose
                                // uploaded_timestamp matches the timestamp in the local filename.
                                let raw = archive_filename.as_deref().unwrap_or("");
                                let fname_norm =
                                    crate::app::free_fns::normalize_nexus_filename(raw);
                                let local_ts = crate::app::free_fns::extract_nexus_timestamp(raw);
                                let candidates: Vec<_> = files
                                    .files
                                    .into_iter()
                                    .filter(|f| {
                                        crate::app::free_fns::normalize_nexus_filename(&f.file_name)
                                            == fname_norm
                                    })
                                    .collect();
                                local_ts
                                    .and_then(|ts| {
                                        candidates
                                            .iter()
                                            .find(|f| f.uploaded_timestamp == Some(ts))
                                            .cloned()
                                    })
                                    .or_else(|| candidates.into_iter().next())
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
                let resolved_file_id = file_info.as_ref().map(|f| f.file_id);
                let nexus_is_primary = file_info.as_ref().map(|f| f.is_primary).unwrap_or(false);
                // When we tried to match by filename but got nothing, ask the user for the file ID.
                let unresolved =
                    nexus_file_id == 0 && archive_filename.is_some() && file_info.is_none();
                // Capture before DownloadNameResolved moves info.name
                let mod_version = info.version.clone();
                let mod_author = info.author.clone();
                let resolved_version = file_version.or(Some(mod_version.clone()));
                let _ = input_sender.send(AppMsg::DownloadNameResolved(
                    download_id.clone(),
                    info.name.clone(),
                    Some(domain.clone()),
                    nexus_file_name,
                    nexus_is_primary,
                    resolved_file_id,
                    resolved_version.clone(),
                    Some(mod_author.clone()),
                ));
                if unresolved {
                    let _ = input_sender.send(AppMsg::ShowFileIdDialog {
                        download_id: download_id.clone(),
                        mod_id: nexus_mod_id,
                        domain: domain.clone(),
                        partial_name: Some(info.name.clone()),
                    });
                } else {
                    // Toast for user-triggered fetches; install-path results are silent since
                    // the install completion toast already ran.
                    let _ = input_sender.send(AppMsg::ShowToast(format!(
                        "{} v{mod_version} by {mod_author}",
                        info.name
                    )));
                }
                // Mirror NXM auto-path: write mod-page metadata (latest_version/author/summary)
                // back to the installed mod row. The per-file installed version is written by
                // handle_download_name_resolved via update_mod_version_by_nexus_ids, which is
                // keyed on (game_id, nexus_mod_id, nexus_file_id) so an older-version fetch
                // does not overwrite the currently installed version.
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
                }
                Ok((info.name, mod_version, mod_author))
            }
            .await;
            match result {
                Ok((name, version, author)) => {
                    crate::app::timing::log_phase("metadata.fetch", &domain, timing_start, Some(1));
                    AppCmdMsg::NexusMetadataFetched(
                        Some(download_id),
                        Ok((String::new(), version, author, name, None)),
                    )
                }
                Err(e) => AppCmdMsg::NexusMetadataFetched(Some(download_id), Err(e)),
            }
        });
    }

    pub(crate) fn handle_pause_download(&mut self, index: DynamicIndex) {
        let idx = index.current_index();
        let download_id = {
            let guard = self.downloads.guard();
            guard.get(idx).map(|r| r.entry.id.clone())
        };
        let Some(download_id) = download_id else {
            return;
        };
        self.update_download_status(&download_id, DownloadStatus::Paused, "Paused");
    }

    pub(crate) fn handle_resume_download(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (download_id, nexus_ids) = {
            let guard = self.downloads.guard();
            let Some(row) = guard.get(idx) else { return };
            (row.entry.id.clone(), row.entry.nexus_ids.clone())
        };
        // Only resume if actually paused
        let is_paused = self
            .all_downloads
            .iter()
            .any(|e| e.id == download_id && e.status == DownloadStatus::Paused);
        if !is_paused {
            return;
        }
        // Re-request the NXM download URL from Nexus and re-enqueue
        if let Some(NexusIds {
            mod_id,
            file_id,
            ref domain,
        }) = nexus_ids
        {
            let uri = format!("nxm://{domain}/mods/{mod_id}/files/{file_id}");
            sender.input(AppMsg::NxmLinkReceived(uri));
        }
    }

    pub(crate) fn handle_delete_download(
        &mut self,
        index: DynamicIndex,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let idx = index.current_index();
        let (download_id, mod_name, archive_path) = {
            let guard = self.downloads.guard();
            let Some(row) = guard.get(idx) else { return };
            (
                row.entry.id.clone(),
                row.entry.mod_name.clone(),
                row.entry.archive_path.clone(),
            )
        };

        let dialog = adw::AlertDialog::builder()
            .heading("Delete Download")
            .body(format!(
                "Delete \"{}\" and its archive file from disk?",
                mod_name
            ))
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("delete", "Delete");
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");

        let input_sender = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "delete" {
                if let Some(ref path) = archive_path {
                    let _ = std::fs::remove_file(path);
                }
                input_sender
                    .send(AppMsg::ConfirmDeleteDownload(download_id.clone()))
                    .ok();
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_confirm_delete_download(
        &mut self,
        download_id: String,
        sender: &ComponentSender<Self>,
    ) {
        self.all_downloads.retain(|e| e.id != download_id);
        {
            let mut guard = self.downloads.guard();
            for i in (0..guard.len()).rev() {
                if guard.get(i).is_some_and(|r| r.entry.id == download_id) {
                    guard.remove(i);
                    break;
                }
            }
        }
        if let Some(tracker) = self.tracker.clone() {
            let id = download_id.clone();
            sender.oneshot_command(async move {
                let _ = tracker.delete_download_entries(&[id]).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
        self.refresh_download_counts();
    }

    pub(crate) fn handle_hide_download(
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

        if let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == download_id) {
            entry.hidden = !entry.hidden;
        }
        if let Some(tracker) = self.tracker.clone()
            && let Some(entry) = self.all_downloads.iter().find(|e| e.id == download_id)
        {
            let entry = entry.clone();
            sender.oneshot_command(async move {
                let _ = tracker.save_download_entry(&entry).await;
                AppCmdMsg::PrioritySaved(Ok(()))
            });
        }
        self.rebuild_downloads_view();
    }

    pub(crate) fn handle_set_show_hidden_downloads(&mut self, show: bool) {
        self.show_hidden_downloads = show;
        self.rebuild_downloads_view();
    }
}
