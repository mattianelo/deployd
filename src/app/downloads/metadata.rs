use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::app::types::{ManualMetadataResult, NexusDownloadMetadata};
use crate::core::game;
use crate::models::download::NexusIds;
use crate::models::nexus::{NexusFileEntry, NexusFileUpdate, NexusModInfo};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

pub(crate) fn nexus_download_metadata(
    domain: &str,
    fallback_name: &str,
    mod_info: Option<&NexusModInfo>,
    file: Option<&NexusFileEntry>,
    known_file_id: Option<i64>,
    latest_version: Option<String>,
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
        .filter(|version| !version.is_empty());
    let version = file
        .and_then(|entry| entry.version.as_deref())
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_string)
        .or_else(|| page_version.map(str::to_string));
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
        latest_version,
        author,
        summary: mod_info.and_then(|info| info.summary.clone()),
    }
}

pub(crate) fn latest_file_version(
    files: &[NexusFileEntry],
    updates: &[NexusFileUpdate],
    installed_file_id: i64,
) -> Option<String> {
    let mut current = installed_file_id;
    let mut visited = std::collections::HashSet::new();
    while visited.insert(current) {
        let Some(update) = updates.iter().find(|update| update.old_file_id == current) else {
            break;
        };
        current = update.new_file_id;
    }
    if current == installed_file_id {
        return None;
    }
    let installed_version = files
        .iter()
        .find(|file| file.file_id == installed_file_id)
        .and_then(|file| file.version.as_deref())
        .map(str::trim)
        .filter(|version| !version.is_empty());
    let candidate = files
        .iter()
        .find(|file| file.file_id == current)
        .and_then(|file| file.version.as_deref())
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .map(str::to_string)?;
    if let Some(installed) = installed_version
        && !version_is_strictly_newer(&candidate, installed)
    {
        return None;
    }
    Some(candidate)
}

fn version_is_strictly_newer(candidate: &str, installed: &str) -> bool {
    match (
        numeric_version_components(candidate),
        numeric_version_components(installed),
    ) {
        (Some(candidate), Some(installed)) => candidate > installed,
        _ => true,
    }
}

fn numeric_version_components(version: &str) -> Option<Vec<u64>> {
    let mut components: Vec<u64> = version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    while components.last() == Some(&0) {
        components.pop();
    }
    (!components.is_empty()).then_some(components)
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
    if let Some(identity) = crate::core::nexus_identity::current_nexus_file_identity(raw) {
        return files.into_iter().find(|file| {
            file.display_name()
                .trim()
                .eq_ignore_ascii_case(&identity.label)
                && file
                    .version
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|version| version.eq_ignore_ascii_case(&identity.version))
        });
    }
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
        let nexus_ids = NexusIds {
            mod_id,
            file_id: 0,
            domain,
        };

        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Nexus identity could not be saved: database unavailable");
            return;
        };
        let persisted_download_id = download_id.clone();
        sender.oneshot_command(async move {
            let result = tracker
                .update_download_nexus_identity(&persisted_download_id, &nexus_ids)
                .await
                .map_err(|error| error.to_string());
            AppCmdMsg::Downloads(
                crate::app::messages::DownloadsCmdMsg::NexusIdentityPersisted {
                    download_id,
                    nexus_ids,
                    result,
                },
            )
        });
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
                                        None,
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
                    files.files.clone(),
                    nexus_file_id,
                    archive_filename.as_deref(),
                );
                let latest_version = file.as_ref().and_then(|file| {
                    latest_file_version(&files.files, &files.file_updates, file.file_id)
                });
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
                    latest_version,
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
    use super::{
        latest_file_version, match_nexus_file, nexus_download_metadata, version_is_strictly_newer,
    };
    use crate::models::nexus::{NexusFileEntry, NexusFileUpdate, NexusModInfo};

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
    fn resolves_current_nexus_filename_by_file_display_name() {
        let old_file: NexusFileEntry = serde_json::from_value(serde_json::json!({
            "file_id": 409998,
            "name": "Dynamic Grass",
            "version": "1.1.0",
            "file_name": "Dynamic-Grass-1.1.0.zip",
            "category_name": "MAIN",
            "is_primary": true,
            "uploaded_timestamp": 1788000000
        }))
        .unwrap();
        let current_file: NexusFileEntry = serde_json::from_value(serde_json::json!({
            "file_id": 409999,
            "name": "Dynamic Grass",
            "version": "1.3.0",
            "file_name": "Dynamic-Grass-1.3.0.zip",
            "category_name": "MAIN",
            "is_primary": true,
            "uploaded_timestamp": 1788177600
        }))
        .unwrap();
        let matched = match_nexus_file(
            vec![old_file, current_file],
            0,
            Some("Dynamic Grass 108480 1.3.0 2026-08-31T12-00Z Gpr9A6gVu.zip"),
        )
        .expect("the current Nexus filename should match its display name");

        assert_eq!(matched.file_id, 409999);
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
            None,
        );

        assert_eq!(metadata.mod_name, "Texture Collection");
        assert_eq!(metadata.nexus_file_name.as_deref(), Some("Legacy textures"));
        assert_eq!(metadata.file_id, Some(77));
        assert_eq!(metadata.version.as_deref(), Some("1.2"));
        assert_eq!(metadata.author.as_deref(), Some("Mod Author"));
    }

    #[test]
    fn follows_nexus_file_update_chain() {
        let mut files = vec![archived_file()];
        let mut second = files[0].clone();
        second.file_id = 78;
        second.version = Some("1.4".to_string());
        let mut latest = second.clone();
        latest.file_id = 79;
        latest.version = Some("1.4.1".to_string());
        files.extend([second, latest]);
        let updates = vec![
            NexusFileUpdate {
                old_file_id: 77,
                new_file_id: 78,
            },
            NexusFileUpdate {
                old_file_id: 78,
                new_file_id: 79,
            },
        ];

        assert_eq!(
            latest_file_version(&files, &updates, 77).as_deref(),
            Some("1.4.1")
        );
        assert_eq!(latest_file_version(&files, &updates, 79), None);
    }

    #[test]
    fn ignores_unrelated_newer_files() {
        let mut unrelated = archived_file();
        unrelated.file_id = 99;
        unrelated.version = Some("V1".to_string());

        assert_eq!(
            latest_file_version(&[archived_file(), unrelated], &[], 77),
            None
        );
    }

    #[test]
    fn rejects_archived_downgrade_in_update_chain() {
        let mut installed = archived_file();
        installed.file_id = 100;
        installed.version = Some("1.3.0".to_string());
        let mut archived = installed.clone();
        archived.file_id = 101;
        archived.version = Some("1.2.0".to_string());
        let updates = [NexusFileUpdate {
            old_file_id: 100,
            new_file_id: 101,
        }];

        assert_eq!(
            latest_file_version(&[installed, archived], &updates, 100),
            None
        );
    }

    #[test]
    fn compares_multi_digit_mod_versions_numerically() {
        assert!(version_is_strictly_newer("v1.10.0", "1.9"));
        assert!(!version_is_strictly_newer("1.2.0", "1.3.0"));
        assert!(!version_is_strictly_newer("1.3", "1.3.0"));
    }
}
