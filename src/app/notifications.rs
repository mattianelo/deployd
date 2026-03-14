use super::App;

impl App {
    pub(crate) fn handle_toggle_notifications(&mut self) {
        self.notifications_visible = !self.notifications_visible;
    }

    pub(crate) fn handle_set_notifications_visible(&mut self, visible: bool) {
        self.notifications_visible = visible;
    }
}
