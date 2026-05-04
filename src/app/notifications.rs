use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use super::App;
use super::messages::AppMsg;

impl App {
    /// Show a transient floating toast that auto-dismisses after ~4 s.
    /// Use for non-actionable confirmations (success, info). Errors that
    /// require user attention belong in push_notification instead.
    pub(crate) fn show_toast(&mut self, message: &str) {
        let toast = adw::Toast::new(message);
        toast.set_timeout(4);
        self.toast_overlay.add_toast(toast);
    }

    /// Add an actionable notification to the persistent notification panel.
    ///
    /// Each item stays until the user explicitly dismisses it. Reserve this
    /// for things that require the user's attention (errors, failures).
    pub(crate) fn push_notification(&mut self, message: &str) {
        let row = adw::ActionRow::new();
        row.set_title(message);
        row.set_title_lines(2);

        let dismiss_btn = gtk::Button::new();
        dismiss_btn.set_icon_name("window-close-symbolic");
        dismiss_btn.add_css_class("flat");
        dismiss_btn.set_valign(gtk::Align::Center);
        dismiss_btn.set_tooltip_text(Some("Dismiss"));

        let row_ref = row.clone();
        let s = self.notification_sender.clone();
        dismiss_btn.connect_clicked(move |_| {
            if let Some(lb) = row_ref.parent().and_downcast::<gtk::ListBox>() {
                lb.remove(&row_ref);
            }
            let _ = s.send(AppMsg::NotificationDismissed);
        });

        row.add_suffix(&dismiss_btn);
        self.notification_list.prepend(&row);
        self.notification_count += 1;
    }
}
