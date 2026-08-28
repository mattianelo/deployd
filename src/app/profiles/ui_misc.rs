use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use super::super::App;
use super::super::messages::AppMsg;

impl App {
    pub(crate) fn handle_rate_limit_updated(
        &mut self,
        info: crate::core::nexus_api::RateLimitInfo,
    ) {
        self.rate_limit_info = Some(info.clone());
        if let Some(tracker) = self.tracker.clone() {
            tokio::spawn(async move {
                let _ = tracker.save_rate_limits(&info).await;
            });
        }
    }

    pub(crate) fn handle_close_requested(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if self.global_active_downloads > 0 {
            let body = format!(
                "{} download(s) are still in progress. Close anyway?",
                self.global_active_downloads
            );
            let dialog = adw::AlertDialog::builder()
                .heading("Downloads in Progress")
                .body(&body)
                .build();
            dialog.add_response("cancel", "Cancel");
            dialog.add_response("close", "Close");
            dialog.set_default_response(Some("close"));
            dialog.set_close_response("cancel");

            let sender = sender.input_sender().clone();
            dialog.connect_response(None, move |_, response| {
                if response == "close" {
                    sender.send(AppMsg::ConfirmClose).unwrap();
                }
            });
            dialog.present(Some(root));
        } else {
            root.destroy();
        }
    }

    pub(crate) fn handle_confirm_close(&mut self, root: &adw::ApplicationWindow) {
        root.destroy();
    }

    pub(crate) fn handle_search_toggled(&mut self, active: bool) {
        self.search_active = active;
        if !active {
            if let Some(source) = self.search_debounce.take() {
                source.remove();
            }
            self.pending_search_text = None;
            self.search_text.clear();
            self.apply_search_filter();
        }
    }

    pub(crate) fn handle_search_changed(&mut self, text: String) {
        if self.search_text == text && self.pending_search_text.is_none() {
            return;
        }
        if let Some(source) = self.search_debounce.take() {
            source.remove();
        }
        self.pending_search_text = Some(text);
        let sender = self.notification_sender.clone();
        let source =
            glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
                let _ = sender.send(AppMsg::ApplySearch);
            });
        self.search_debounce = Some(source);
    }

    pub(crate) fn handle_apply_search(&mut self) {
        self.search_debounce = None;
        let Some(text) = self.pending_search_text.take() else {
            return;
        };
        if self.search_text == text {
            return;
        }
        self.search_text = text;
        self.apply_search_filter();
    }

    pub(crate) fn handle_search_scope_changed(&mut self, idx: u32) {
        let next_scope = match idx {
            1 => super::super::types::SearchScope::ModOrder,
            2 => super::super::types::SearchScope::PluginOrder,
            3 => super::super::types::SearchScope::Downloads,
            _ => super::super::types::SearchScope::All,
        };
        if self.search_scope == next_scope {
            return;
        }
        self.search_scope = next_scope;
        self.apply_search_filter();
    }
}
