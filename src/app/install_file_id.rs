use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::app::App;
use crate::app::downloads::nexus_download_metadata;
use crate::app::messages::{AppCmdMsg, AppMsg};
use crate::app::types::ManualMetadataResult;

impl App {
    pub(crate) fn show_file_id_dialog(
        &mut self,
        download_id: String,
        mod_id: i64,
        domain: String,
        partial_name: Option<String>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let text_entry = gtk::Entry::builder()
            .placeholder_text("File ID (e.g. 123456)")
            .hexpand(true)
            .activates_default(true)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        let dialog = adw::AlertDialog::builder()
            .heading("File Not Found on Nexus")
            .body(
                "The archive filename did not match any file on this mod page. \
                 Enter the Nexus file ID to complete the metadata, or skip.",
            )
            .build();
        dialog.set_extra_child(Some(&text_entry));
        dialog.add_response("skip", "Skip");
        dialog.add_response("fetch", "Fetch");
        dialog.set_default_response(Some("fetch"));
        dialog.set_close_response("skip");
        dialog.set_response_appearance("fetch", adw::ResponseAppearance::Suggested);

        let input_sender = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response != "fetch" {
                return;
            }
            let Ok(file_id) = text_entry.text().trim().parse::<i64>() else {
                return;
            };
            if file_id <= 0 {
                return;
            }
            let _ = input_sender.send(AppMsg::Install(
                crate::app::messages::InstallMsg::FileIdDialogConfirmed {
                    download_id: download_id.clone(),
                    file_id,
                    mod_id,
                    domain: domain.clone(),
                    partial_name: partial_name.clone(),
                },
            ));
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_file_id_dialog_confirmed(
        &mut self,
        download_id: String,
        file_id: i64,
        mod_id: i64,
        domain: String,
        partial_name: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        self.begin_download_metadata_fetch(&download_id);
        let input_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result: Result<ManualMetadataResult, String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .map_err(|error| error.to_string())?
                    .filter(|key| !key.is_empty())
                    .ok_or("No API key configured. Set it in Settings.")?;
                let client = crate::core::nexus_api::NexusClient::new(api_key);
                let mod_info = match client.get_mod_info(&domain, mod_id).await {
                    Ok((info, rate_limits)) => {
                        if let Some(rate_limits) = rate_limits {
                            let _ = input_sender.send(AppMsg::Downloads(
                                crate::app::messages::DownloadsMsg::RateLimitUpdated(rate_limits),
                            ));
                        }
                        Some(info)
                    }
                    Err(error) => {
                        eprintln!(
                            "deployd: Nexus mod page metadata unavailable; using exact file metadata: {error:#}"
                        );
                        None
                    }
                };
                let (files, rate_limits) = client
                    .get_mod_files(&domain, mod_id)
                    .await
                    .map_err(|error| format!("failed to fetch Nexus file metadata: {error:#}"))?;
                if let Some(rate_limits) = rate_limits {
                    let _ = input_sender.send(AppMsg::Downloads(
                        crate::app::messages::DownloadsMsg::RateLimitUpdated(rate_limits),
                    ));
                }
                let file = files
                    .files
                    .iter()
                    .find(|file| file.file_id == file_id)
                    .ok_or_else(|| {
                        format!("Nexus file ID {file_id} was not found on this mod page")
                    })?;
                let fallback_name = partial_name.as_deref().unwrap_or(file.display_name());
                let latest_version = crate::app::downloads::latest_file_version(
                    &files.files,
                    &files.file_updates,
                    file_id,
                );
                Ok(ManualMetadataResult::Resolved(nexus_download_metadata(
                    &domain,
                    fallback_name,
                    mod_info.as_ref(),
                    Some(file),
                    Some(file_id),
                    latest_version,
                )))
            }
            .await;
            AppCmdMsg::Downloads(
                crate::app::messages::DownloadsCmdMsg::NexusMetadataFetched(download_id, result),
            )
        });
    }
}
