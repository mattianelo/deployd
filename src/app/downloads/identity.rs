use adw::prelude::*;
use relm4::factory::DynamicIndex;
use relm4::prelude::*;

use crate::app::messages::{AppCmdMsg, AppMsg, DownloadsCmdMsg, DownloadsMsg};
use crate::app::types::NexusIdentityCandidate;
use crate::core::game;
use crate::models::download::NexusIds;

use super::super::App;

impl App {
    pub(crate) fn handle_fetch_download_metadata(
        &mut self,
        index: DynamicIndex,
        sender: &ComponentSender<Self>,
    ) {
        let id = self
            .download
            .rows
            .get(index.current_index())
            .map(|row| row.entry.id.clone());
        if let Some(id) = id {
            self.start_nexus_metadata_fetch(id, sender, false);
        }
    }

    pub(crate) fn handle_edit_download_identity(
        &mut self,
        index: DynamicIndex,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let id = self
            .download
            .rows
            .get(index.current_index())
            .map(|row| row.entry.id.clone());
        if let Some(id) = id {
            self.show_download_identity_dialog(id, Vec::new(), root, sender);
        }
    }

    pub(crate) fn show_download_identity_dialog(
        &mut self,
        download_id: String,
        candidates: Vec<NexusIdentityCandidate>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let Some(entry) = self
            .download
            .all
            .iter()
            .find(|entry| entry.id == download_id)
        else {
            return;
        };
        if entry.is_active() {
            return;
        }
        let domain = entry
            .nexus_ids
            .as_ref()
            .map(|ids| ids.domain.as_str())
            .filter(|domain| !domain.is_empty())
            .or(entry.game_domain.as_deref())
            .or_else(|| self.selected_game().and_then(game::nexus_domain))
            .unwrap_or("skyrimspecialedition")
            .to_string();
        let current = entry
            .nexus_ids
            .as_ref()
            .map(|ids| format!("{}/mods/{}", domain, ids.mod_id));
        let body = if candidates.is_empty() {
            format!(
                "Confirm the Nexus page for this archive. Filename guesses are not verified. A numeric ID uses {domain}; a Nexus URL also selects its game."
            )
        } else {
            format!(
                "The archive hash matches the file(s) below. Confirm a match to replace the current identity ({}), or enter a different Nexus page. Cancel keeps the current identity.",
                current.as_deref().unwrap_or("none")
            )
        };
        let text_entry = gtk::Entry::builder()
            .placeholder_text("Nexus mod URL or ID")
            .hexpand(true)
            .activates_default(true)
            .build();
        if let Some(current) = &current {
            text_entry.set_text(&format!("https://www.nexusmods.com/{current}"));
        }
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let error_label = gtk::Label::builder().wrap(true).visible(false).build();
        error_label.add_css_class("error");
        let dialog = adw::AlertDialog::builder()
            .heading("Confirm Nexus Mod")
            .body(body)
            .build();
        for candidate in candidates {
            let button = gtk::Button::with_label(&format!(
                "{}\n{}/mods/{} · file {}",
                candidate.name, candidate.ids.domain, candidate.ids.mod_id, candidate.ids.file_id
            ));
            let input_sender = sender.input_sender().clone();
            let id = download_id.clone();
            let dialog_weak = dialog.downgrade();
            button.connect_clicked(move |_| {
                if let Some(dialog) = dialog_weak.upgrade() {
                    dialog.close();
                }
                let _ = input_sender.send(AppMsg::Downloads(DownloadsMsg::ConfirmNexusIdEntry(
                    id.clone(),
                    candidate.ids.clone(),
                )));
            });
            content.append(&button);
        }
        content.append(&text_entry);
        content.append(&error_label);
        let confirm = gtk::Button::with_label("Use this page");
        confirm.add_css_class("suggested-action");
        content.append(&confirm);
        dialog.set_extra_child(Some(&content));
        dialog.add_response("cancel", "Cancel");
        dialog.set_close_response("cancel");
        let input_sender = sender.input_sender().clone();
        let dialog_weak = dialog.downgrade();
        let submit_button = confirm.clone();
        text_entry.connect_activate(move |_| submit_button.emit_clicked());
        let submit = move || match crate::core::nexus_identity::parse_nexus_identity_input(
            &text_entry.text(),
            &domain,
        ) {
            Ok(ids) => {
                if let Some(dialog) = dialog_weak.upgrade() {
                    dialog.close();
                }
                let _ = input_sender.send(AppMsg::Downloads(DownloadsMsg::ConfirmNexusIdEntry(
                    download_id.clone(),
                    ids,
                )));
            }
            Err(error) => {
                error_label.set_text(&error);
                error_label.set_visible(true);
            }
        };
        confirm.connect_clicked(move |_| submit());
        dialog.present(Some(root));
    }

    pub(crate) fn handle_confirm_nexus_id_entry(
        &mut self,
        download_id: String,
        nexus_ids: NexusIds,
        sender: &ComponentSender<Self>,
    ) {
        if !self.download_metadata_available() {
            return;
        }
        let Some(entry) = self
            .download
            .all
            .iter()
            .find(|entry| entry.id == download_id)
        else {
            return;
        };
        if entry.is_active() {
            return;
        }
        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Nexus identity could not be saved: database unavailable");
            return;
        };
        self.begin_download_metadata_fetch(&download_id);
        sender.oneshot_command(async move {
            let result = tracker
                .update_download_nexus_identity(&download_id, &nexus_ids)
                .await
                .map_err(|error| error.to_string());
            AppCmdMsg::Downloads(DownloadsCmdMsg::NexusIdentityPersisted {
                download_id,
                nexus_ids,
                result,
            })
        });
    }
}
