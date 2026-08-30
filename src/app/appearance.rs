use relm4::adw;

use super::App;

impl App {
    pub(crate) fn handle_set_color_scheme(&mut self, index: u32) {
        self.shell.color_scheme_idx = index;
        let scheme = match index {
            1 => adw::ColorScheme::ForceLight,
            2 => adw::ColorScheme::ForceDark,
            _ => adw::ColorScheme::Default,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
        if let Some(tracker) = self.session.tracker.clone() {
            let value = index.to_string();
            tokio::spawn(async move {
                let _ = tracker.set_setting("color_scheme", &value).await;
            });
        }
    }
}
