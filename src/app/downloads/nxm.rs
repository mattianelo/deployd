use std::path::PathBuf;

use relm4::prelude::*;

use crate::core::game;
use crate::models::download::{DownloadEntry, NexusIds};
use crate::utils::paths;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::types::NxmDownloadResult;

impl App {
    pub(crate) fn handle_nxm_link_received(&mut self, uri: String, sender: &ComponentSender<Self>) {
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
                self.push_notification(&format!("Invalid NXM link: {e}"));
                return;
            }
        };

        // Check game domain is supported
        if game::game_id_for_nexus_domain(&link.domain).is_none() {
            self.push_notification(&format!("Unsupported game: {}", link.domain));
            return;
        }

        // Create download entry and add to sidebar
        let download_id = uuid::Uuid::new_v4().to_string();
        let mod_name = format!("Mod {} (file {})", link.mod_id, link.file_id);
        let nexus_ids = Some(NexusIds { mod_id: link.mod_id, file_id: link.file_id, domain: link.domain.clone() });
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
                    let base = match tracker.get_setting("downloads_dir").await.ok().flatten() {
                        Some(dir) => PathBuf::from(dir),
                        None => paths::default_downloads_dir(),
                    };
                    base.join(&link.domain)
                };

                let client = NexusClient::new(api_key);

                // Fetch mod info to get the real name
                let mut mod_info_version: Option<String> = None;
                if let Ok((mod_info, rate_limits)) =
                    client.get_mod_info(&link.domain, link.mod_id).await
                {
                    if let Some(rl) = rate_limits {
                        let _ = input_sender.send(AppMsg::RateLimitUpdated(rl));
                    }
                    mod_info_version = Some(mod_info.version.clone());
                    let _ = input_sender.send(AppMsg::DownloadNameResolved(
                        download_id.clone(),
                        mod_info.name,
                        Some(link.domain.clone()),
                        None,
                        false,
                        None,
                        None,
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
                    .unwrap_or_else(|| format!("nexus_{}_{}.zip", link.mod_id, link.file_id));
                let nexus_file_name = nexus_file.map(|f| f.name.clone());
                let nexus_is_primary = nexus_file.map(|f| f.is_primary).unwrap_or(false);
                let nexus_file_version = nexus_file.and_then(|f| f.version.clone());

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
                    version: nexus_file_version.or(mod_info_version),
                })
            }
            .await;
            AppCmdMsg::NxmDownloadComplete(download_id, result)
        });
    }
}
