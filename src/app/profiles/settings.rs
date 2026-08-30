use std::path::PathBuf;

use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game;
use crate::ui::settings_dialog::{SettingsDialog, SettingsDialogOutput};
use crate::utils;

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

impl App {
    pub(crate) fn handle_settings_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.ui.overflow_menu_btn.popdown();
        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        self.ui.settings_dialog = Some(
            SettingsDialog::builder()
                .transient_for(root)
                .launch((
                    tracker,
                    self.shell.nexus_username.is_some(),
                    self.shell.color_scheme_idx,
                    game::is_snap() && utils::experimental_enabled(),
                ))
                .forward(sender.input_sender(), |output| match output {
                    SettingsDialogOutput::Closed => {
                        AppMsg::Games(crate::app::messages::GamesMsg::SettingsClosed)
                    }
                    SettingsDialogOutput::ApiKeyChanged => {
                        AppMsg::Games(crate::app::messages::GamesMsg::NexusApiKeyUpdated)
                    }
                    SettingsDialogOutput::ManageGames => {
                        AppMsg::Games(crate::app::messages::GamesMsg::ManageGamesClicked)
                    }
                    SettingsDialogOutput::PreviewAppImageExport => {
                        AppMsg::Migration(crate::app::messages::MigrationMsg::PreviewAppImageExport)
                    }
                    SettingsDialogOutput::ColorSchemeChanged(idx) => {
                        AppMsg::Shell(crate::app::messages::ShellMsg::SetColorScheme(idx))
                    }
                }),
        );
    }

    pub(crate) fn handle_settings_closed(&mut self, sender: &ComponentSender<Self>) {
        if let Some(dialog) = self.ui.settings_dialog.take() {
            dialog.widget().destroy();
        }
        if let Some(tracker) = self.session.tracker.clone() {
            sender.oneshot_command(async move {
                let result = tracker
                    .get_setting("downloads_dir")
                    .await
                    .map(|dir| dir.map(PathBuf::from))
                    .map_err(|error| error.to_string());
                AppCmdMsg::Downloads(crate::app::messages::DownloadsCmdMsg::DownloadsDirUpdated(
                    result,
                ))
            });
        }
    }

    pub(crate) fn handle_nexus_api_key_updated(&mut self, sender: &ComponentSender<Self>) {
        self.show_toast("Nexus Mods key updated.");
        // Re-validate to refresh username and avatar displayed in the headerbar.
        if let Some(tracker) = self.session.tracker.clone() {
            sender.oneshot_command(async move {
                let api_key = match tracker.get_setting("nexus_api_key").await {
                    Ok(key) => key.filter(|key| !key.is_empty()),
                    Err(error) => {
                        return AppCmdMsg::Shell(
                            crate::app::messages::ShellCmdMsg::NexusUserRefreshFailed(
                                error.to_string(),
                            ),
                        );
                    }
                };
                match api_key {
                    Some(key) => {
                        let client = crate::core::nexus_api::NexusClient::new(key);
                        match client.validate_key().await {
                            Ok((user, _)) => {
                                if let Err(error) = tracker.save_nexus_user(&user).await {
                                    return AppCmdMsg::Shell(
                                        crate::app::messages::ShellCmdMsg::NexusUserRefreshFailed(
                                            error.to_string(),
                                        ),
                                    );
                                }
                                AppCmdMsg::Shell(
                                    crate::app::messages::ShellCmdMsg::NexusUserRefreshed(
                                        Some(user.name),
                                        user.profile_url,
                                        user.is_premium,
                                    ),
                                )
                            }
                            Err(_) => AppCmdMsg::Shell(
                                crate::app::messages::ShellCmdMsg::NexusUserRefreshed(
                                    None, None, false,
                                ),
                            ),
                        }
                    }
                    None => AppCmdMsg::Shell(
                        crate::app::messages::ShellCmdMsg::NexusUserRefreshed(None, None, false),
                    ),
                }
            });
        }
    }

    pub(crate) fn handle_nexus_login_clicked(&mut self, sender: &ComponentSender<Self>) {
        self.ui.nexus_user_btn.popdown();
        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Database not ready yet");
            return;
        };
        let input = sender.input_sender().clone();
        relm4::spawn(async move {
            match crate::core::nexus_api::sso_login().await {
                Ok(api_key) => {
                    if let Err(e) = tracker.set_setting("nexus_api_key", &api_key).await {
                        let _ = input.send(AppMsg::Shell(
                            crate::app::messages::ShellMsg::ShowToast(format!("Login error: {e}")),
                        ));
                        return;
                    }
                    if let Err(e) = tracker.set_setting("nexus_login_source", "sso").await {
                        let _ = input.send(AppMsg::Shell(
                            crate::app::messages::ShellMsg::ShowToast(format!("Login error: {e}")),
                        ));
                        return;
                    }
                    let _ = input.send(AppMsg::Games(
                        crate::app::messages::GamesMsg::NexusApiKeyUpdated,
                    ));
                }
                Err(e) => {
                    let _ = input.send(AppMsg::Shell(crate::app::messages::ShellMsg::ShowToast(
                        format!("Nexus login failed: {e}"),
                    )));
                }
            }
        });
    }

    pub(crate) fn handle_nexus_logout_clicked(&mut self, sender: &ComponentSender<Self>) {
        self.ui.nexus_user_btn.popdown();
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        sender.oneshot_command(async move {
            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::NexusLogoutDone(
                tracker
                    .clear_nexus_user()
                    .await
                    .map_err(|error| error.to_string()),
            ))
        });
    }
}
