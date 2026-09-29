use std::future::Future;

use adw::prelude::*;
use relm4::prelude::*;

use crate::core::location_recovery::{self as recovery, activity_lock};
use crate::core::tracker::locations::LocationRecord;
use crate::core::tracker::{PersistedGame, Tracker};
use crate::utils::location::{FolderRole, SelectedLocation};

use super::App;
use super::messages::{AppCmdMsg, AppMsg};
use super::types::WorkKind;

#[derive(Debug)]
pub(crate) enum RecoveryMsg {
    Start(String, FolderRole),
    Pick(LocationRecord, FolderRole),
    Confirm(LocationRecord, SelectedLocation),
    Cancel,
}

#[derive(Debug)]
pub(crate) enum RecoveryCmd {
    Prepared {
        role: FolderRole,
        record: Result<LocationRecord, String>,
    },
    Selected(LocationRecord, Result<Option<SelectedLocation>, String>),
    Finished {
        games: Vec<PersistedGame>,
        blocked: Vec<String>,
        error: Option<String>,
    },
}

async fn finished(tracker: &Tracker, result: anyhow::Result<()>) -> AppCmdMsg {
    let mut error = result.err().map(|error| format!("{error:#}"));
    let games = match tracker.load_persisted_games().await {
        Ok(games) => games,
        Err(failure) => {
            error = Some(format!("Could not reload folder settings: {failure}"));
            Vec::new()
        }
    };
    let blocked = match recovery::blocked_games(tracker).await {
        Ok(blocked) => blocked,
        Err(failure) => {
            error = Some(format!("Could not inspect folder access: {failure}"));
            games.iter().map(|game| game.id.clone()).collect()
        }
    };
    AppCmdMsg::Recovery(RecoveryCmd::Finished {
        games,
        blocked,
        error,
    })
}

impl App {
    pub(crate) fn location_command(
        &self,
        sender: &ComponentSender<Self>,
        future: impl Future<Output = AppCmdMsg> + Send + 'static,
    ) {
        let Ok(lease) = activity_lock().try_read_owned() else {
            sender.input(AppMsg::Shell(super::messages::ShellMsg::ShowToast(
                "Wait for folder access recovery to finish".to_string(),
            )));
            return;
        };
        sender.oneshot_command(async move {
            let result = future.await;
            AppCmdMsg::LocationActivityCompleted(lease, Box::new(result))
        });
    }

    pub(crate) fn handle_recovery(
        &mut self,
        msg: RecoveryMsg,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        match msg {
            RecoveryMsg::Start(game_id, role) => {
                if !crate::utils::snap::is_snap() {
                    return;
                }
                if self.is_busy()
                    || self.session.initializing
                    || self.tools.launch_session.is_some()
                {
                    self.push_notification("Finish the current operation and close running tools before restoring folder access");
                    return;
                }
                let Some(tracker) = self.session.tracker.clone() else {
                    return;
                };
                let Ok(lease) = activity_lock().try_write_owned() else {
                    self.push_notification(
                        "Wait for game and save operations to finish, then restore folder access",
                    );
                    return;
                };
                self.shell.location_recovery = Some(lease);
                self.begin_work(WorkKind::RecoveringLocation, "Restoring folder access...");
                if let Some(dialog) = self.ui.game_setup_dialog.take() {
                    dialog.widget().close();
                }
                sender.oneshot_command(async move {
                    let record = tracker.folder_location(&game_id, role).await;
                    if let Ok(record) = &record {
                        match tracker.pending_location_repairs().await {
                            Ok(pending) => {
                                if let Some(pending) = pending
                                    .iter()
                                    .find(|pending| pending.location_id == record.id)
                                {
                                    let result = recovery::repair(&tracker, pending).await;
                                    if result.is_ok() {
                                        return finished(&tracker, result).await;
                                    }
                                    return AppCmdMsg::Recovery(RecoveryCmd::Prepared {
                                        role,
                                        record: Ok(record.clone()),
                                    });
                                }
                            }
                            Err(error) => return finished(&tracker, Err(error)).await,
                        }
                    }
                    AppCmdMsg::Recovery(RecoveryCmd::Prepared {
                        role,
                        record: record.map_err(|error| error.to_string()),
                    })
                });
            }
            RecoveryMsg::Pick(record, role) => {
                if self.shell.location_recovery.is_none() {
                    return;
                }
                sender.oneshot_command(async move {
                    let result = match role {
                        FolderRole::Game => {
                            crate::utils::portal::select_location(
                                "Restore game access — select the original folder",
                                record.selection.host_hint.as_deref(),
                                crate::utils::snap::SelectedFolderKind::GameFolder,
                            )
                            .await
                        }
                        FolderRole::Prefix => {
                            crate::utils::portal::select_prefix_recovery_location(&record.selection)
                                .await
                        }
                    };
                    AppCmdMsg::Recovery(RecoveryCmd::Selected(
                        record,
                        result.map_err(|error| error.to_string()),
                    ))
                });
            }
            RecoveryMsg::Confirm(record, selected) => {
                if self.shell.location_recovery.is_none() {
                    return;
                }
                let Some(tracker) = self.session.tracker.clone() else {
                    return;
                };
                sender.oneshot_command(async move {
                    let result = async {
                        recovery::validate_selection(record.clone(), selected.clone()).await?;
                        let pending = tracker
                            .commit_location_recovery(&record, &selected, true)
                            .await?;
                        recovery::repair(&tracker, &pending).await
                    }
                    .await;
                    finished(&tracker, result).await
                });
            }
            RecoveryMsg::Cancel => {
                self.shell.location_recovery = None;
                self.finish_work(WorkKind::RecoveringLocation);
                self.handle_manage_games_clicked(root, sender);
            }
        }
    }

    pub(crate) fn handle_recovery_command(
        &mut self,
        msg: RecoveryCmd,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        match msg {
            RecoveryCmd::Prepared {
                role,
                record: Ok(record),
            } => {
                let titles = record
                    .bindings
                    .iter()
                    .map(|binding| format!("{} ({})", binding.title, binding.role.label()))
                    .collect::<Vec<_>>()
                    .join("\n");
                let hint = record
                    .selection
                    .host_hint
                    .as_deref()
                    .unwrap_or(&record.selection.root);
                let instruction = match role {
                    FolderRole::Game => "Select the original game folder",
                    FolderRole::Prefix => {
                        "Select the original Wine prefix or its containing folder (for Steam, pfx or its numbered parent). Deployd will retain access through the containing folder so replacing the prefix does not invalidate access again"
                    }
                };
                let body = format!(
                    "{instruction} to restore access for:\n\n{titles}\n\nPrevious location: {}\n\nClose the game before continuing. This reconnects existing files; it does not move an installation.",
                    hint.display()
                );
                let dialog = adw::AlertDialog::builder()
                    .heading("Restore folder access")
                    .body(&body)
                    .build();
                dialog.add_response("cancel", "Cancel");
                let select_label = match role {
                    FolderRole::Game => "Select original folder",
                    FolderRole::Prefix => "Select prefix or containing folder",
                };
                dialog.add_response("select", select_label);
                dialog.set_close_response("cancel");
                dialog.set_response_appearance("select", adw::ResponseAppearance::Suggested);
                let input = sender.input_sender().clone();
                dialog.connect_response(None, move |_, response| {
                    let message = if response == "select" {
                        RecoveryMsg::Pick(record.clone(), role)
                    } else {
                        RecoveryMsg::Cancel
                    };
                    let _ = input.send(AppMsg::Recovery(message));
                });
                dialog.present(Some(root));
            }
            RecoveryCmd::Prepared {
                record: Err(error), ..
            }
            | RecoveryCmd::Selected(_, Err(error)) => {
                self.push_notification(&format!("Could not restore folder access: {error}"));
                self.handle_recovery(RecoveryMsg::Cancel, sender, root);
            }
            RecoveryCmd::Selected(_, Ok(None)) => {
                self.handle_recovery(RecoveryMsg::Cancel, sender, root)
            }
            RecoveryCmd::Selected(record, Ok(Some(selected))) => {
                let known = match selected.validate_identity(&record.selection) {
                    Ok(known) => known,
                    Err(error) => {
                        self.push_notification(&error.to_string());
                        self.handle_recovery(RecoveryMsg::Cancel, sender, root);
                        return;
                    }
                };
                if known {
                    self.handle_recovery(RecoveryMsg::Confirm(record, selected), sender, root);
                } else {
                    let dialog = adw::AlertDialog::builder().heading("Is this the original folder?")
                        .body("The desktop portal could not verify the original location. Continue only if you selected the same installation or Wine prefix. Existing game state and save references will be reconnected to this folder.").build();
                    dialog.add_response("cancel", "Cancel");
                    dialog.add_response("confirm", "This is the original folder");
                    dialog.set_close_response("cancel");
                    let input = sender.input_sender().clone();
                    dialog.connect_response(None, move |_, response| {
                        let message = if response == "confirm" {
                            RecoveryMsg::Confirm(record.clone(), selected.clone())
                        } else {
                            RecoveryMsg::Cancel
                        };
                        let _ = input.send(AppMsg::Recovery(message));
                    });
                    dialog.present(Some(root));
                }
            }
            RecoveryCmd::Finished {
                games,
                blocked,
                error,
            } => {
                self.session.location_blocked = if error.is_some() && games.is_empty() {
                    self.session
                        .games
                        .iter()
                        .map(|game| game.id.clone())
                        .collect()
                } else {
                    blocked.into_iter().collect()
                };
                self.shell.location_recovery = None;
                self.finish_work(WorkKind::RecoveringLocation);
                if !games.is_empty() {
                    let configs = games
                        .into_iter()
                        .map(|record| crate::models::game::GameConfig {
                            custom: record.custom,
                            game: recovery::persisted_game(record),
                            locations: Vec::new(),
                        })
                        .collect();
                    self.handle_cmd_games_persisted(Ok(configs), sender);
                }
                if let Some(error) = error {
                    self.push_notification(&format!("Folder recovery needs attention: {error}. Use Restore folder access to retry; affected operations remain blocked."));
                } else {
                    self.push_notification("Folder access restored and dependent paths repaired");
                }
            }
        }
    }
}
