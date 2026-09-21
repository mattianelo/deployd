use relm4::prelude::*;

use crate::app::types::{ManualMetadataResult, NexusDownloadMetadata};
use crate::core::game;
use crate::models::download::{NexusIdentitySource, NexusIds};
use crate::models::nexus::{NexusFileEntry, NexusFileUpdate, NexusModInfo};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

pub(super) async fn persist_manual_metadata(
    tracker: &crate::core::tracker::Tracker,
    mut entry: crate::models::download::DownloadEntry,
    metadata: &NexusDownloadMetadata,
    directory: std::path::PathBuf,
) -> Result<crate::models::download::DownloadEntry, String> {
    let ids = entry
        .nexus_ids
        .as_mut()
        .ok_or("Nexus identity is missing")?;
    ids.domain = metadata.domain.clone();
    if let Some(file_id) = metadata.file_id {
        ids.file_id = file_id;
    }
    entry.nexus_identity_source = NexusIdentitySource::Confirmed;
    entry.mod_name = metadata.mod_name.clone();
    entry.game_domain = Some(metadata.domain.clone());
    entry.metadata_fetched = metadata.file_id.is_some_and(|id| id > 0)
        || metadata
            .nexus_file_name
            .as_ref()
            .is_some_and(|name| !name.trim().is_empty());
    entry.nexus_file_name = metadata.nexus_file_name.clone();
    entry.nexus_is_primary = metadata.nexus_is_primary;
    entry.version = metadata.version.clone().or(entry.version);
    entry.author = metadata.author.clone().or(entry.author);
    let old_path = entry.archive_path.clone();
    let entry = tokio::task::spawn_blocking(move || stage_metadata_archive(entry, &directory))
        .await
        .map_err(|error| format!("Archive relocation task failed: {error}"))?;
    if let Err(error) = tracker
        .persist_fetched_download_metadata(
            &entry,
            metadata.latest_version.as_deref(),
            metadata.summary.as_deref(),
        )
        .await
    {
        if old_path != entry.archive_path {
            let new_path = entry.archive_path.clone();
            let rollback = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
                if let Some(new) = new_path {
                    std::fs::remove_file(new)?;
                }
                Ok(())
            })
            .await;
            if !matches!(rollback, Ok(Ok(()))) {
                return Err(format!(
                    "{error}; archive relocation could not be undone. Rescan Downloads to locate the archive."
                ));
            }
        }
        return Err(error.to_string());
    }
    if old_path != entry.archive_path {
        let cleanup = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(old) = old_path {
                std::fs::remove_file(old)?;
            }
            Ok(())
        })
        .await;
        if !matches!(cleanup, Ok(Ok(()))) {
            eprintln!("deployd: metadata saved; original archive copy remains in Downloads");
        }
    }
    Ok(entry)
}

fn stage_metadata_archive(
    mut entry: crate::models::download::DownloadEntry,
    directory: &std::path::Path,
) -> crate::models::download::DownloadEntry {
    if let Some(path) = &entry.archive_path
        && path.parent() == Some(directory)
        && let Some(name) = path.file_name()
        && let Some(domain) = entry.game_domain.as_deref()
        && crate::core::game::all_nexus_domains().contains(&domain)
    {
        let target_dir = directory.join(domain);
        let target = target_dir.join(name);
        let relocated = (|| -> std::io::Result<()> {
            std::fs::create_dir_all(target_dir)?;
            // Keep the original until the database commits, and refuse target collisions.
            std::fs::hard_link(path, &target)
        })();
        match relocated {
            Ok(()) => entry.archive_path = Some(target),
            Err(error) => eprintln!("deployd: keeping archive in its original folder: {error}"),
        }
    }
    entry
}

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

#[cfg(test)]
fn mod_identity(mod_id: i64, domain: String) -> NexusIds {
    NexusIds {
        mod_id,
        file_id: 0,
        domain,
    }
}

fn resolve_md5_results(
    results: Vec<crate::models::nexus::Md5SearchResult>,
    identity: Option<&NexusIds>,
    source: NexusIdentitySource,
    domain: &str,
) -> (
    Option<crate::models::nexus::Md5SearchResult>,
    Vec<crate::app::types::NexusIdentityCandidate>,
) {
    let mut results: Vec<_> = results
        .into_iter()
        .filter(|hit| {
            hit.r#mod.mod_id > 0
                && hit.file_details.file_id > 0
                && (hit.r#mod.domain_name.is_empty()
                    || hit.r#mod.domain_name.eq_ignore_ascii_case(domain))
        })
        .collect();
    results.sort_by_key(|hit| (hit.r#mod.mod_id, hit.file_details.file_id));
    results.dedup_by_key(|hit| (hit.r#mod.mod_id, hit.file_details.file_id));
    let matches: Vec<_> = results
        .iter()
        .enumerate()
        .filter(|(_, hit)| {
            identity.is_some_and(|ids| {
                ids.mod_id == hit.r#mod.mod_id
                    && (ids.domain.is_empty() || ids.domain.eq_ignore_ascii_case(domain))
                    && (ids.file_id == 0 || ids.file_id == hit.file_details.file_id)
            })
        })
        .map(|(index, _)| index)
        .collect();
    if let [index] = matches.as_slice()
        && (source == NexusIdentitySource::Confirmed || results.len() == 1)
    {
        return (Some(results.remove(*index)), Vec::new());
    }
    let candidates = results
        .into_iter()
        .map(|hit| crate::app::types::NexusIdentityCandidate {
            ids: NexusIds {
                mod_id: hit.r#mod.mod_id,
                file_id: hit.file_details.file_id,
                domain: domain.to_string(),
            },
            name: format!("{} — {}", hit.r#mod.name, hit.file_details.display_name()),
        })
        .collect();
    (None, candidates)
}

impl App {
    /// Perform the async Nexus metadata fetch for a download entry identified by ID.
    ///
    /// Looks up the entry in `self.download.all` to collect the required fields,
    /// then dispatches the oneshot command that calls the API.
    pub(crate) fn start_nexus_metadata_fetch(
        &mut self,
        download_id: String,
        sender: &ComponentSender<Self>,
        use_selected_page: bool,
    ) {
        if !self.download_metadata_available() {
            return;
        }
        let (identity, identity_source, stored_domain, archive_filename, archive_md5, archive_path) = {
            let Some(entry) = self.download.all.iter().find(|e| e.id == download_id) else {
                return;
            };
            if entry.is_active() {
                return;
            }
            let archive_filename = entry
                .archive_path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            (
                entry.nexus_ids.clone(),
                entry.nexus_identity_source,
                entry
                    .nexus_ids
                    .as_ref()
                    .map(|ids| ids.domain.clone())
                    .filter(|domain| !domain.is_empty())
                    .or_else(|| entry.game_domain.clone())
                    .unwrap_or_default(),
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
                            let (matched, candidates) = resolve_md5_results(results, identity.as_ref(), identity_source, &domain);
                            if let Some(hit) = matched {
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
                            if !candidates.is_empty() && !use_selected_page {
                                return Ok(ManualMetadataResult::NeedsIdentity(candidates));
                            }
                        }
                        Err(e) => {
                            eprintln!(
                                "deployd: MD5 metadata lookup failed: {e:#}"
                            );
                        }
                    }
                }

                let Some(identity) = identity.filter(|_| identity_source == NexusIdentitySource::Confirmed) else {
                    return Ok(ManualMetadataResult::NeedsIdentity(Vec::new()));
                };
                let nexus_mod_id = identity.mod_id;
                let nexus_file_id = identity.file_id;

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
                        "Nexus file ID {nexus_file_id} was not found on {domain}/mods/{nexus_mod_id}. Check the file ID on that page’s Files tab"
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

    // @variants: both
    #[test]
    fn entering_mele_mod_id_keeps_file_identity_unknown_until_matched() {
        let id = crate::core::nexus_identity::parse_nexus_mod_id_from_input("8").unwrap();
        let identity = super::mod_identity(id, "masseffectlegendaryedition".into());
        assert_eq!(identity.mod_id, 8);
        assert_eq!(identity.file_id, 0);
        assert_eq!(identity.domain, "masseffectlegendaryedition");
        let filename = "Unofficial Mass Effect 2 Legendary Edition Patch-8-0-9-6-1762432362.7z";
        let file: NexusFileEntry = serde_json::from_value(serde_json::json!({
            "file_id": 12199, "file_name": filename, "name": "Unofficial LE2 Patch", "version": "0.9.6"
        })).unwrap();
        let matched =
            match_nexus_file(vec![file.clone()], identity.file_id, Some(filename)).unwrap();
        assert_eq!(matched.file_id, 12199);
        assert!(
            match_nexus_file(
                vec![file.clone()],
                identity.file_id,
                Some("LE1 Community Patch-23-2-0-1762480826.7z")
            )
            .is_none()
        );
        assert!(match_nexus_file(vec![file], 8, Some(filename)).is_none());
        let partial = nexus_download_metadata(&identity.domain, "Patch", None, None, None, None);
        assert_eq!(partial.file_id, None);
        assert_eq!(partial.domain, "masseffectlegendaryedition");
    }

    fn hash_hit(mod_id: i64, file_id: i64, domain: &str) -> crate::models::nexus::Md5SearchResult {
        let mut info = mod_info();
        info.mod_id = mod_id;
        info.domain_name = domain.into();
        let mut file = archived_file();
        file.file_id = file_id;
        crate::models::nexus::Md5SearchResult {
            r#mod: info,
            file_details: file,
        }
    }

    // @variants: both
    #[test]
    fn looksmenu_hash_conflict_requires_confirmation_of_the_correct_page() {
        use crate::models::download::NexusIdentitySource;
        let name = "LooksMenu v1-6-20-12631-1-6-20-1604483725.7z";
        let guessed = crate::core::nexus_identity::parse_nexus_mod_id(name).unwrap();
        assert_eq!(guessed, 6);
        let identity = super::mod_identity(guessed, "fallout4".into());
        for source in [
            NexusIdentitySource::Filename,
            NexusIdentitySource::Legacy,
            NexusIdentitySource::Confirmed,
        ] {
            let (matched, candidates) = super::resolve_md5_results(
                vec![hash_hit(12631, 100, "fallout4")],
                Some(&identity),
                source,
                "fallout4",
            );
            assert!(matched.is_none());
            assert_eq!(candidates.len(), 1);
            assert_eq!(candidates[0].ids.mod_id, 12631);
            assert_eq!(identity.mod_id, 6);
        }
    }

    // @variants: both
    #[test]
    fn hash_resolution_keeps_mele_mods_separate() {
        use crate::models::download::NexusIdentitySource;
        let domain = "masseffectlegendaryedition";
        for id in [8, 23] {
            let identity = super::mod_identity(id, domain.into());
            let (matched, candidates) = super::resolve_md5_results(
                vec![hash_hit(id, id * 100, domain)],
                Some(&identity),
                NexusIdentitySource::Filename,
                domain,
            );
            assert_eq!(matched.unwrap().r#mod.mod_id, id);
            assert!(candidates.is_empty());
        }
        let identity = super::mod_identity(23, domain.into());
        let (matched, candidates) = super::resolve_md5_results(
            vec![hash_hit(8, 800, domain)],
            Some(&identity),
            NexusIdentitySource::Confirmed,
            domain,
        );
        assert!(matched.is_none());
        assert_eq!(candidates[0].ids.mod_id, 8);
        assert_eq!(identity.mod_id, 23);
    }

    #[test]
    fn ambiguous_hashes_only_prefer_a_confirmed_identity() {
        use crate::models::download::NexusIdentitySource;
        let identity = super::mod_identity(23, "masseffectlegendaryedition".into());
        let hits = vec![
            hash_hit(8, 800, &identity.domain),
            hash_hit(23, 2300, &identity.domain),
        ];
        let (matched, candidates) = super::resolve_md5_results(
            hits.clone(),
            Some(&identity),
            NexusIdentitySource::Filename,
            &identity.domain,
        );
        assert!(matched.is_none());
        assert_eq!(candidates.len(), 2);
        let (matched, candidates) = super::resolve_md5_results(
            hits,
            Some(&identity),
            NexusIdentitySource::Confirmed,
            &identity.domain,
        );
        assert_eq!(matched.unwrap().r#mod.mod_id, 23);
        assert!(candidates.is_empty());
    }

    #[test]
    fn hash_resolution_rejects_other_games_and_does_not_replace_exact_files() {
        use crate::models::download::NexusIdentitySource;
        let mut identity = super::mod_identity(8, "masseffectlegendaryedition".into());
        identity.file_id = 800;
        let hits = vec![
            hash_hit(8, 800, "fallout4"),
            hash_hit(8, 801, &identity.domain),
        ];
        let (matched, candidates) = super::resolve_md5_results(
            hits,
            Some(&identity),
            NexusIdentitySource::Confirmed,
            &identity.domain,
        );
        assert!(matched.is_none());
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].ids.file_id, 801);
    }

    #[test]
    fn unidentified_archives_offer_hash_matches_without_adopting_them() {
        use crate::models::download::NexusIdentitySource;
        let (matched, candidates) = super::resolve_md5_results(
            vec![hash_hit(12631, 100, "fallout4")],
            None,
            NexusIdentitySource::Filename,
            "fallout4",
        );
        assert!(matched.is_none());
        assert_eq!(candidates[0].ids.mod_id, 12631);
        let (matched, candidates) =
            super::resolve_md5_results(Vec::new(), None, NexusIdentitySource::Legacy, "fallout4");
        assert!(matched.is_none());
        assert!(candidates.is_empty());
    }

    // @variants: both
    #[tokio::test]
    async fn metadata_persistence_relocates_archives_without_overwriting_files()
    -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let tracker = crate::core::tracker::Tracker::open("sqlite::memory:")
            .await?
            .tracker;
        let mut entry = crate::models::download::DownloadEntry::new(
            "archive".into(),
            "Archive".into(),
            Some(super::mod_identity(12, "fallout4".into())),
        );
        entry.status = crate::models::download::DownloadStatus::Downloaded;
        let original = directory.path().join("Archive.7z");
        std::fs::write(&original, b"archive")?;
        entry.archive_path = Some(original.clone());
        tracker.save_download_entry(&entry).await?;
        let metadata = nexus_download_metadata(
            "fallout4",
            "Archive",
            None,
            Some(&archived_file()),
            Some(77),
            None,
        );
        let saved =
            super::persist_manual_metadata(&tracker, entry, &metadata, directory.path().into())
                .await
                .map_err(anyhow::Error::msg)?;
        let relocated = directory.path().join("fallout4/Archive.7z");
        assert_eq!(saved.archive_path.as_deref(), Some(relocated.as_path()));
        assert!(!original.exists());
        let loaded = tracker.load_download_entries().await?;
        assert_eq!(loaded[0].archive_path, saved.archive_path);
        assert_eq!(
            loaded[0].nexus_identity_source,
            crate::models::download::NexusIdentitySource::Confirmed
        );
        std::fs::write(&original, b"different")?;
        let mut conflicting = saved;
        conflicting.id = "second".into();
        conflicting.archive_path = Some(original.clone());
        tracker.save_download_entry(&conflicting).await?;
        let saved = super::persist_manual_metadata(
            &tracker,
            conflicting,
            &metadata,
            directory.path().into(),
        )
        .await
        .map_err(anyhow::Error::msg)?;
        assert_eq!(saved.archive_path, Some(original));
        assert_eq!(std::fs::read(relocated)?, b"archive");
        Ok(())
    }

    #[tokio::test]
    async fn failed_metadata_commit_restores_the_original_archive() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let tracker = crate::core::tracker::Tracker::open("sqlite::memory:")
            .await?
            .tracker;
        let mut entry = crate::models::download::DownloadEntry::new(
            "missing-row".into(),
            "Archive".into(),
            Some(super::mod_identity(12, "fallout4".into())),
        );
        let original = directory.path().join("Archive.7z");
        std::fs::write(&original, b"archive")?;
        entry.archive_path = Some(original.clone());
        let metadata = nexus_download_metadata(
            "fallout4",
            "Archive",
            None,
            Some(&archived_file()),
            Some(77),
            None,
        );
        assert!(
            super::persist_manual_metadata(&tracker, entry, &metadata, directory.path().into())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(original)?, b"archive");
        assert!(!directory.path().join("fallout4/Archive.7z").exists());
        Ok(())
    }

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
