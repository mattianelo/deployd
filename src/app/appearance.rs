use relm4::adw;
use relm4::prelude::*;

use super::App;
use super::messages::{AppCmdMsg, ShellCmdMsg};

impl App {
    pub(crate) fn apply_color_scheme(&mut self, index: u32) {
        self.shell.color_scheme_idx = index;
        let scheme = match index {
            1 => adw::ColorScheme::ForceLight,
            2 => adw::ColorScheme::ForceDark,
            _ => adw::ColorScheme::Default,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
    }

    pub(crate) fn handle_set_color_scheme(&mut self, index: u32, sender: &ComponentSender<Self>) {
        self.apply_color_scheme(index);
        if let Some(tracker) = self.session.tracker.clone() {
            let value = index.to_string();
            sender.oneshot_command(async move {
                AppCmdMsg::Shell(ShellCmdMsg::PrioritySaved(
                    tracker
                        .set_setting("color_scheme", &value)
                        .await
                        .map_err(|error| error.to_string()),
                ))
            });
        }
    }
}
