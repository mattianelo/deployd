use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use super::App;
use super::messages::AppMsg;

impl App {
    /// Add a notification message to the notification panel.
    ///
    /// Each item stays until the user explicitly dismisses it, giving a
    /// persistent history that the floating toast overlay could not provide.
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
