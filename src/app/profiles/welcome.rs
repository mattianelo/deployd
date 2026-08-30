use gtk::prelude::*;
use relm4::prelude::*;

use crate::ui::welcome_wizard::{WelcomeWizard, WelcomeWizardOutput};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg, ShellCmdMsg};

impl App {
    pub(crate) fn handle_welcome_wizard_skipped(&mut self) {
        self.ui.welcome_wizard = None;
    }

    pub(crate) fn handle_show_welcome_wizard(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.ui.welcome_wizard = Some(
            WelcomeWizard::builder()
                .transient_for(root)
                .launch(())
                .forward(sender.input_sender(), |out| match out {
                    WelcomeWizardOutput::Confirmed {
                        enabled,
                        hidden_ids,
                    } => AppMsg::Games(crate::app::messages::GamesMsg::WelcomeWizardConfirmed(
                        enabled, hidden_ids,
                    )),
                    WelcomeWizardOutput::Skipped => {
                        AppMsg::Games(crate::app::messages::GamesMsg::WelcomeWizardSkipped)
                    }
                }),
        );
    }

    pub(crate) fn handle_welcome_wizard_confirmed(
        &mut self,
        configs: Vec<crate::models::game::GameConfig>,
        hidden_ids: Vec<String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(w) = self.ui.welcome_wizard.take() {
            w.widget().close();
        }
        if let Some(ref tracker) = self.session.tracker {
            let t = tracker.clone();
            sender.oneshot_command(async move {
                AppCmdMsg::Shell(ShellCmdMsg::PrioritySaved(
                    t.set_setting("welcome_wizard_shown", "1")
                        .await
                        .map_err(|error| error.to_string()),
                ))
            });
        }
        // Reuse the existing game-configure flow.
        self.handle_games_configured(configs, hidden_ids, sender);
        // Kick off the downloads scan that was deferred while waiting for the wizard.
        // External-file scan is deliberately omitted here: it must run *after* GameSelected
        // loads the game data and creates the vanilla snapshot, otherwise every game file
        // would be flagged as a new external change.
        sender.input(AppMsg::Downloads(
            crate::app::messages::DownloadsMsg::ScanDownloadsFolder,
        ));
    }
}
