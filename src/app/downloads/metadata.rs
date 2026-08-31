use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::app::types::{ManualMetadataResult, NexusDownloadMetadata};
use crate::core::game;
use crate::models::download::NexusIds;
use crate::models::nexus::{NexusFileEntry, NexusModInfo};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

pub(crate) fn nexus_download_metadata(
    domain: &str,
    fallback_name: &str,
    mod_info: Option<&NexusModInfo>,
    file: Option<&NexusFileEntry>,
    known_file_id: Option<i64>,
) -> NexusDownloadMetadata {
    let nexus_file_name = file
        .map(NexusFileEntry::display_name)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string);
    let mod_name = mod_info
        .map(|info| info.name.trim())
        .filter(|name| !name.is_empty())
        .or(nexus_file_name.as_deref())
        .unwrap_or(fallback_name)
        .to_string();
    let page_version = mod_info
        .map(|info| info.version.trim())
        .filter(|version| !version.is_empty())
        .map(str::to_string);
    let version = file
        .and_then(|entry| entry.version.as_deref())
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .or_else(|| page_version.clone());
    let author = mod_info
        .map(|info| info.author.trim())
        .filter(|author| !author.is_empty())
        .map(str::to_string);
    let resolved_domain = mod_info
        .map(|info| info.domain_name.trim())
        .filter(|resolved| !resolved.is_empty())
        .unwrap_or(domain)
        .to_string();

    NexusDownloadMetadata {
        mod_name,
        domain: resolved_domain,
        nexus_file_name,
        nexus_is_primary: file.is_some_and(|entry| entry.is_primary),
        file_id: file.map(|entry| entry.file_id).or(known_file_id),
        version,
        author,
        page_version,
        summary: mod_info.and_then(|info| info.summary.clone()),
    }
}

fn match_nexus_file(
    files: Vec<NexusFileEntry>,
    file_id: i64,
    archive_filename: Option<&str>,
) -> Option<NexusFileEntry> {
    if file_id > 0 {
        return files.into_iter().find(|file| file.file_id == file_id);
    }

    let raw = archive_filename?;
    let normalized = crate::core::nexus_identity::normalize_nexus_filename(raw);
    let timestamp = crate::core::nexus_identity::extract_nexus_timestamp(raw);
    let candidates: Vec<_> = files
        .into_iter()
        .filter(|file| {
            crate::core::nexus_identity::normalize_nexus_filename(&file.file_name) == normalized
        })
        .collect();
    timestamp
        .and_then(|timestamp| {
            candidates
                .iter()
                .find(|file| file.uploaded_timestamp == Some(timestamp))
                .cloned()
        })
        .or_else(|| candidates.into_iter().next())
}

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
            archive_md5,
            archive_path,
        ) = {
            let Some(entry) = self.download.all.iter().find(|e| e.id == download_id) else {
                return;
            };
            if entry.is_active() {
                return;
            }
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
            (
                nexus_mod_id,
                nexus_file_id,
                domain.clone(),
                archive_filename,
                entry.archive_md5.clone(),
                entry.archive_path.clone(),
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

        let input_sender = sender.input_sender().clone();
        self.begin_download_metadata_fetch(&download_id);
        self.show_toast("Fetching metadata...");
        sender.oneshot_command(async move {
            let timing_start = std::time::Instant::now();
            let result: Result<ManualMetadataResult, String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .map_err(|error| error.to_string())?
                    .filter(|k| !k.is_empty())
                    .ok_or("No API key configured. Set it in Settings.")?;
                let client = crate::core::nexus_api::NexusClient::new(api_key);

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
                                    && (hit.r#mod.domain_name.is_empty()
                                        || hit.r#mod.domain_name.eq_ignore_ascii_case(&domain))
                            });
                            if let Some(hit) = matching_hit {
                                return Ok(ManualMetadataResult::Resolved(
                                    nexus_download_metadata(
                                        &domain,
                                        archive_filename.as_deref().unwrap_or("Unknown mod"),
                                        Some(&hit.r#mod),
                                        Some(&hit.file_details),
                                        Some(hit.file_details.file_id),
                                    ),
                                ));
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "deployd: MD5 metadata lookup failed; trying mod/file lookup: {e:#}"
                            );
                        }
                    }
                }

                let mod_info_result = client.get_mod_info(&domain, nexus_mod_id).await;
                if let Ok((_, Some(rate_limits))) = &mod_info_result {
                    let _ = input_sender.send(AppMsg::Downloads(
                        crate::app::messages::DownloadsMsg::RateLimitUpdated(rate_limits.clone()),
                    ));
                }
                let files_result = client.get_mod_files(&domain, nexus_mod_id).await;
                let (files, file_rate_limits) = files_result.map_err(|error| {
                    format!("failed to fetch Nexus file metadata: {error:#}")
                })?;
                if let Some(rate_limits) = file_rate_limits {
                    let _ = input_sender.send(AppMsg::Downloads(
                        crate::app::messages::DownloadsMsg::RateLimitUpdated(rate_limits),
                    ));
                }
                let file = match_nexus_file(
                    files.files,
                    nexus_file_id,
                    archive_filename.as_deref(),
                );
                let mod_info = match mod_info_result {
                    Ok((info, _)) => Some(info),
                    Err(error) if file.is_some() => {
                        eprintln!(
                            "deployd: Nexus mod page metadata unavailable; using exact file metadata: {error:#}"
                        );
                        None
                    }
                    Err(error) => {
                        return Err(format!("failed to fetch Nexus mod metadata: {error:#}"));
                    }
                };
                let fallback_name = archive_filename.as_deref().unwrap_or("Unknown mod");
                let metadata = nexus_download_metadata(
                    &domain,
                    fallback_name,
                    mod_info.as_ref(),
                    file.as_ref(),
                    (nexus_file_id > 0).then_some(nexus_file_id),
                );
                if file.is_some() {
                    Ok(ManualMetadataResult::Resolved(metadata))
                } else if nexus_file_id > 0 {
                    Err(format!(
                        "Nexus file ID {nexus_file_id} was not found on this mod page"
                    ))
                } else {
                    Ok(ManualMetadataResult::NeedsFileId(metadata))
                }
            }
            .await;
            crate::app::timing::log_phase("metadata.fetch", &domain, timing_start, Some(1));
            AppCmdMsg::Downloads(
                crate::app::messages::DownloadsCmdMsg::NexusMetadataFetched(download_id, result),
            )
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{match_nexus_file, nexus_download_metadata};
    use crate::models::nexus::{NexusFileEntry, NexusModInfo};

    fn archived_file() -> NexusFileEntry {
        serde_json::from_value(serde_json::json!({
            "file_id": 77,
            "name": "Legacy textures",
            "version": "1.2",
            "file_name": "Legacy-Textures-77-1700000000.7z",
            "category_name": "OLD_VERSION",
            "is_primary": false,
            "uploaded_timestamp": 1700000000
        }))
        .unwrap()
    }

    fn mod_info() -> NexusModInfo {
        serde_json::from_value(serde_json::json!({
            "mod_id": 12,
            "name": "Texture Collection",
            "author": "Mod Author",
            "version": "2.0",
            "summary": "Summary",
            "domain_name": "skyrimspecialedition",
            "status": "published"
        }))
        .unwrap()
    }

    #[test]
    fn resolves_archived_file_by_normalized_archive_name_and_timestamp() {
        let file = archived_file();
        let matched = match_nexus_file(vec![file], 0, Some("Legacy-Textures-77-1700000000.7z"))
            .expect("the archived file should be matched");

        assert_eq!(matched.file_id, 77);
    }

    #[test]
    fn manual_and_nxm_metadata_preserve_the_exact_file_label() {
        let file = archived_file();
        let info = mod_info();
        let metadata = nexus_download_metadata(
            "skyrimspecialedition",
            "downloaded-archive",
            Some(&info),
            Some(&file),
            Some(77),
        );

        assert_eq!(metadata.mod_name, "Texture Collection");
        assert_eq!(metadata.nexus_file_name.as_deref(), Some("Legacy textures"));
        assert_eq!(metadata.file_id, Some(77));
        assert_eq!(metadata.version.as_deref(), Some("1.2"));
        assert_eq!(metadata.author.as_deref(), Some("Mod Author"));
    }
}
