use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::app::App;
use crate::app::messages::{AppCmdMsg, AppMsg};
use crate::app::types::WorkKind;

impl App {
    /// Show a dialog asking the user to enter the Nexus file ID when the install-time
    /// metadata fetch could not match the archive filename to any file on the mod page.
    ///
    /// On confirm: sends `AppMsg::Install(crate::app::messages::InstallMsg::FileIdDialogConfirmed)` → async fetch → `AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::FileIdFetched)`
    ///             → applies combined name → opens pre-install dialog.
    /// On skip: clears `pending_file_id_needed` and opens the pre-install dialog with the
    ///          partial mod name already stored in `pending_fetched_name`.
    pub(crate) fn show_file_id_dialog(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let Some(ctx) = self.install.file_id_needed.take() else {
            return;
        };

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
                 Enter the Nexus file ID to complete the metadata, or skip to proceed.",
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
            if response == "fetch" {
                let raw = text_entry.text().to_string();
                if let Ok(file_id) = raw.trim().parse::<i64>()
                    && file_id > 0
                {
                    let _ = input_sender.send(AppMsg::Install(
                        crate::app::messages::InstallMsg::FileIdDialogConfirmed {
                            download_id: ctx.download_id.clone(),
                            file_id,
                            mod_id: ctx.mod_id,
                            domain: ctx.domain.clone(),
                        },
                    ));
                    return;
                }
            }
            // Skip or invalid input — open pre-install dialog with partial name.
            let _ = input_sender.send(AppMsg::Install(
                crate::app::messages::InstallMsg::OpenPreInstallDialog,
            ));
        });
        dialog.present(Some(root));
    }

    /// Fetch the Nexus file entry for the user-supplied `file_id` and resolve the combined
    /// mod+file name. Fires `AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::FileIdFetched)` when done.
    pub(crate) fn handle_file_id_dialog_confirmed(
        &mut self,
        download_id: String,
        file_id: i64,
        mod_id: i64,
        domain: String,
        sender: &ComponentSender<Self>,
    ) {
        let is_install = self.install.pending.is_some();
        let Some(tracker) = self.session.tracker.clone() else {
            if is_install {
                let _ = sender.input_sender().send(AppMsg::Install(
                    crate::app::messages::InstallMsg::OpenPreInstallDialog,
                ));
            }
            return;
        };
        // Persist the resolved file_id on the download entry so future installs skip this dialog.
        if let Some(entry) = self.download.all.iter_mut().find(|e| e.id == download_id)
            && let Some(ref mut ids) = entry.nexus_ids
        {
            ids.file_id = file_id;
        }
        self.begin_work(WorkKind::FetchingMetadata, "Fetching Nexus metadata...");
        self.begin_download_metadata_fetch(&download_id);
        let partial_name = self.install.fetched_name.clone();
        let download_id_for_result = (!is_install).then(|| download_id.clone());
        sender.oneshot_command(async move {
            let Some(api_key) = tracker
                .get_setting("nexus_api_key")
                .await
                .ok()
                .flatten()
                .filter(|k| !k.is_empty())
            else {
                return AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::FileIdFetched {
                    combined_name: None,
                    download_id: download_id_for_result,
                    version: None,
                    file_id: None,
                });
            };
            let client = crate::core::nexus_api::NexusClient::new(api_key);
            let file_entry = client
                .get_mod_files(&domain, mod_id)
                .await
                .ok()
                .and_then(|(resp, _)| resp.files.into_iter().find(|f| f.file_id == file_id));
            if let Some(ref entry) = file_entry {
                let fname = &entry.name;
                let mod_name = partial_name.as_deref().unwrap_or(fname.as_str());
                let combined = if fname.to_lowercase().contains(&mod_name.to_lowercase()) {
                    fname.clone()
                } else {
                    format!("{mod_name} - {fname}")
                };
                AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::FileIdFetched {
                    combined_name: Some(combined),
                    download_id: download_id_for_result,
                    version: entry.version.clone(),
                    file_id: Some(entry.file_id),
                })
            } else {
                AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::FileIdFetched {
                    combined_name: None,
                    download_id: download_id_for_result,
                    version: None,
                    file_id: None,
                })
            }
        });
    }
}
