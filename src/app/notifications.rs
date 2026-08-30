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
        self.ui.toast_overlay.add_toast(toast);
    }

    pub(crate) fn handle_show_toast(&mut self, message: String) {
        self.show_toast(&message);
    }

    pub(crate) fn handle_notification_dismissed(&mut self) {
        self.ui.notification_count = self.ui.notification_count.saturating_sub(1);
    }

    pub(crate) fn handle_clear_notifications(&mut self) {
        while let Some(child) = self.ui.notification_list.first_child() {
            self.ui.notification_list.remove(&child);
        }
        self.ui.notification_count = 0;
    }

    /// Add an actionable notification to the persistent notification panel.
    ///
    /// Each item stays until the user explicitly dismisses it. Reserve this
    /// for things that require the user's attention (errors, failures).
    pub(crate) fn push_notification(&mut self, message: &str) {
        let row = adw::ExpanderRow::new();
        row.set_title(&gtk::glib::markup_escape_text(message));
        row.set_title_lines(2);

        let dismiss_btn = gtk::Button::new();
        dismiss_btn.set_icon_name("window-close-symbolic");
        dismiss_btn.add_css_class("flat");
        dismiss_btn.set_valign(gtk::Align::Center);
        dismiss_btn.set_tooltip_text(Some("Dismiss"));

        let row_ref = row.clone();
        let s = self.ui.notification_sender.clone();
        dismiss_btn.connect_clicked(move |_| {
            if let Some(lb) = row_ref.parent().and_downcast::<gtk::ListBox>() {
                lb.remove(&row_ref);
            }
            let _ = s.send(AppMsg::Shell(
                crate::app::messages::ShellMsg::NotificationDismissed,
            ));
        });

        row.add_suffix(&dismiss_btn);

        // Full message revealed on expand, selectable for copy-paste
        let label = gtk::Label::new(Some(message));
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_xalign(0.0);
        label.set_selectable(true);
        label.set_margin_top(8);
        label.set_margin_bottom(8);
        label.set_margin_start(16);
        label.set_margin_end(16);
        let detail_row = gtk::ListBoxRow::new();
        detail_row.set_child(Some(&label));
        detail_row.set_activatable(false);
        detail_row.set_selectable(false);
        row.add_row(&detail_row);

        self.ui.notification_list.prepend(&row);
        self.ui.notification_count += 1;
    }
}
