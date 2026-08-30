use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::core::game;
use crate::models::download::NexusIds;
use crate::ui::mod_list::ModListItemKind;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

impl App {
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
                let guard = self.download.rows.guard();
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
                    let Some(mod_id) =
                        crate::core::nexus_identity::parse_nexus_mod_id_from_input(&raw)
                    else {
                        return;
                    };
                    let _ = input_sender.send(AppMsg::Downloads(
                        crate::app::messages::DownloadsMsg::ConfirmNexusIdEntry(
                            download_id.clone(),
                            mod_id,
                            domain.clone(),
                        ),
                    ));
                });
                dialog.present(Some(root));
                return;
            }
        }

        let download_id = {
            let guard = self.download.rows.guard();
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
        if let Some(entry) = self.download.all.iter_mut().find(|e| e.id == download_id) {
            entry.nexus_ids = new_nexus_ids.clone();
        }

        // Update factory
        {
            let mut guard = self.download.rows.guard();
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
            self.session.tracker.clone(),
            self.download
                .all
                .iter()
                .find(|e| e.id == download_id)
                .cloned(),
        ) {
            sender.oneshot_command(async move {
                let result = tracker
                    .save_download_entry(&entry)
                    .await
                    .map_err(|error| error.to_string());
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
            });
        }

        self.start_nexus_metadata_fetch(download_id, sender);
    }

    /// Perform the async Nexus metadata fetch for a download entry identified by ID.
    ///
    /// Looks up the entry in `self.download.all` to collect the required fields,
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
            let Some(entry) = self.download.all.iter().find(|e| e.id == download_id) else {
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

        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };

        // Find the installed mod that corresponds to this download entry (if any),
        // so the manual fetch can mirror the NXM auto-path and write metadata to
        // the mods table too (fixes disparity between manual vs. NXM metadata fetch).
        let installed_mod_id: Option<String> = {
            let guard = self.mods.rows.guard();
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
                        crate::core::archive::compute_md5(&p).ok()
                    })
                    .await
                    .unwrap_or(None);
                    if let Some(ref m) = md5 {
                        let _ = input_sender.send(AppMsg::Downloads(
                            crate::app::messages::DownloadsMsg::ArchiveMd5Computed(
                                download_id.clone(),
                                m.clone(),
                            ),
                        ));
                    }
                    md5
                } else {
                    None
                };

                if let Some(ref md5) = effective_md5 {
                    match client.md5_search(&domain, md5).await {
                        Ok((results, rl)) => {
                            if let Some(rl) = rl {
                                let _ = input_sender.send(AppMsg::Downloads(
                                    crate::app::messages::DownloadsMsg::RateLimitUpdated(rl),
                                ));
                            }
                            let matching_hit = results.into_iter().find(|hit| {
                                hit.r#mod.mod_id == nexus_mod_id
                                    && hit.r#mod.domain_name.eq_ignore_ascii_case(&domain)
                            });
                            if let Some(hit) = matching_hit {
                                let file_entry = hit.file_details;
                                let mod_info = hit.r#mod;
                                let file_version = file_entry.version.clone();
                                let mod_version = mod_info.version.clone();
                                let mod_author = mod_info.author.clone();
                                let resolved_version =
                                    file_version.or_else(|| Some(mod_version.clone()));
                                let correct_domain = mod_info.domain_name.clone();
                                let _ = input_sender.send(AppMsg::Downloads(
                                    crate::app::messages::DownloadsMsg::DownloadNameResolved(
                                        download_id.clone(),
                                        mod_info.name.clone(),
                                        Some(correct_domain),
                                        Some(file_entry.name.clone()),
                                        file_entry.is_primary,
                                        Some(file_entry.file_id),
                                        resolved_version.clone(),
                                        Some(mod_author.clone()),
                                    ),
                                ));
                                let _ = input_sender.send(AppMsg::Shell(
                                    crate::app::messages::ShellMsg::ShowToast(format!(
                                        "{} v{mod_version} by {mod_author}",
                                        mod_info.name
                                    )),
                                ));
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
                    let _ = input_sender.send(AppMsg::Downloads(
                        crate::app::messages::DownloadsMsg::RateLimitUpdated(rl),
                    ));
                }
                // Fetch file list to resolve the per-file display name.
                // • file_id != 0 (NXM download): match by exact file_id.
                // • file_id == 0 (disk-scanned): match by archive filename so
                //   manually downloaded files also get their proper Nexus name.
                let file_info = if nexus_file_id != 0 || archive_filename.is_some() {
                    match client.get_mod_files(&domain, nexus_mod_id).await {
                        Ok((files, rate_limits)) => {
                            if let Some(rl) = rate_limits {
                                let _ = input_sender.send(AppMsg::Downloads(
                                    crate::app::messages::DownloadsMsg::RateLimitUpdated(rl),
                                ));
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
                                    crate::core::nexus_identity::normalize_nexus_filename(raw);
                                let local_ts =
                                    crate::core::nexus_identity::extract_nexus_timestamp(raw);
                                let candidates: Vec<_> = files
                                    .files
                                    .into_iter()
                                    .filter(|f| {
                                        crate::core::nexus_identity::normalize_nexus_filename(
                                            &f.file_name,
                                        ) == fname_norm
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
                            let _ = input_sender.send(AppMsg::Shell(
                                crate::app::messages::ShellMsg::ShowToast(format!(
                                    "Mod name fetched, but file list unavailable: {e}"
                                )),
                            ));
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
                let _ = input_sender.send(AppMsg::Downloads(
                    crate::app::messages::DownloadsMsg::DownloadNameResolved(
                        download_id.clone(),
                        info.name.clone(),
                        Some(domain.clone()),
                        nexus_file_name,
                        nexus_is_primary,
                        resolved_file_id,
                        resolved_version.clone(),
                        Some(mod_author.clone()),
                    ),
                ));
                if unresolved {
                    let _ = input_sender.send(AppMsg::Downloads(
                        crate::app::messages::DownloadsMsg::ShowFileIdDialog {
                            download_id: download_id.clone(),
                            mod_id: nexus_mod_id,
                            domain: domain.clone(),
                            partial_name: Some(info.name.clone()),
                        },
                    ));
                } else {
                    // Toast for user-triggered fetches; install-path results are silent since
                    // the install completion toast already ran.
                    let _ = input_sender.send(AppMsg::Shell(
                        crate::app::messages::ShellMsg::ShowToast(format!(
                            "{} v{mod_version} by {mod_author}",
                            info.name
                        )),
                    ));
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
                    AppCmdMsg::Downloads(
                        crate::app::messages::DownloadsCmdMsg::NexusMetadataFetched(
                            Some(download_id),
                            Ok((String::new(), version, author, name, None)),
                        ),
                    )
                }
                Err(e) => AppCmdMsg::Downloads(
                    crate::app::messages::DownloadsCmdMsg::NexusMetadataFetched(
                        Some(download_id),
                        Err(e),
                    ),
                ),
            }
        });
    }
}
