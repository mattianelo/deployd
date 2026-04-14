use gtk::gio;
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
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        if self.global_active_downloads > 0 {
            let dialog = gtk::AlertDialog::builder()
                .message("Downloads in Progress")
                .detail(format!(
                    "{} download(s) are still in progress. Close anyway?",
                    self.global_active_downloads
                ))
                .buttons(["Cancel", "Close"])
                .cancel_button(0)
                .default_button(1)
                .modal(true)
                .build();

            let sender = sender.input_sender().clone();
            dialog.choose(Some(root), None::<&gio::Cancellable>, move |result| {
                if result == Ok(1) {
                    sender.send(AppMsg::ConfirmClose).unwrap();
                }
            });
        } else {
            root.destroy();
        }
    }

    pub(crate) fn handle_confirm_close(&mut self, root: &adw::Window) {
        root.destroy();
    }

    pub(crate) fn handle_search_toggled(&mut self, active: bool) {
        self.search_active = active;
        if !active {
            self.search_text.clear();
            self.apply_search_filter();
        }
    }

    pub(crate) fn handle_search_changed(&mut self, text: String) {
        self.search_text = text;
        self.apply_search_filter();
    }

    pub(crate) fn handle_search_scope_changed(&mut self, idx: u32) {
        self.search_scope = match idx {
            1 => super::super::types::SearchScope::ModOrder,
            2 => super::super::types::SearchScope::PluginOrder,
            3 => super::super::types::SearchScope::Downloads,
            _ => super::super::types::SearchScope::All,
        };
        self.apply_search_filter();
    }

    pub(crate) fn handle_show_toast(&mut self, msg: String) {
        self.toaster.toast(&msg);
    }
}
