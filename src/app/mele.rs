use std::collections::BTreeSet;
use std::sync::{Arc, atomic::AtomicBool};

use adw::prelude::*;
use relm4::prelude::*;

use crate::core::game::mass_effect::{application, library};
use crate::ui::mele_dialog;

use super::{
    App,
    messages::{AppCmdMsg, AppMsg},
    state::{InstallIdentity, InstallStage},
    types::{DeployCompletion, WorkKind},
};

pub(super) mod launcher;

#[derive(Debug)]
pub(crate) enum Msg {
    Launcher(Box<launcher::Msg>),
    Install(mele_dialog::Selection),
    Setup(bool),
    Inspect {
        language: String,
        purge: bool,
        repair: bool,
    },
    Apply(Box<application::Preview>),
    Cancel,
    Progress(f64),
}

#[derive(Debug)]
pub(crate) enum Command {
    Launcher(Box<launcher::Command>),
    Options(
        InstallIdentity,
        Result<
            (
                BTreeSet<String>,
                Option<crate::core::game::mass_effect::binary::Approval>,
            ),
            String,
        >,
    ),
    Setup {
        language: Result<String, String>,
        purge: bool,
    },
    Preview(Result<Box<application::Preview>, String>),
    Removed(Result<Vec<crate::models::mod_entry::ModEntry>, String>),
    Applied {
        result: Result<DeployCompletion, String>,
        purge: bool,
    },
}

impl App {
    pub(crate) fn remove_mele_mods(&mut self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let entries: Vec<_> = {
            let rows = self.mods.rows.guard();
            self.mods
                .selected
                .iter()
                .filter_map(|index| {
                    rows.get(*index)
                        .and_then(|row| row.mod_row())
                        .map(|row| row.mod_entry.clone())
                })
                .collect()
        };
        if entries.is_empty() {
            return;
        }
        self.shell.deploying = true;
        self.begin_work(WorkKind::Deploying, "Removing MELE library entries...");
        self.location_command(sender, async move {
            let ids = entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>();
            let result = library::remove(&tracker, &game, &ids)
                .await
                .map(|()| entries)
                .map_err(|error| format!("{error:#}"));
            AppCmdMsg::Mele(Command::Removed(result))
        });
    }

    pub(crate) fn open_mele_install_dialog(
        &mut self,
        _root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let Some(identity) = self.install.identity() else {
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let replace = self
            .install
            .replacement
            .as_ref()
            .map(|item| item.mod_id.clone());
        let defaults = self
            .install
            .pending
            .as_ref()
            .and_then(|pending| pending.mele.as_ref())
            .map(|plan| plan.default_options())
            .unwrap_or_default();
        self.install.set_stage(InstallStage::AwaitingPreInstall);
        self.finish_current_work();
        sender.oneshot_command(async move {
            let options = async {
                if let Some(id) = replace {
                    Ok(tracker
                        .mele_package(&id)
                        .await?
                        .map(|record| (record.package.options, record.package.binary_approval))
                        .unwrap_or_default())
                } else {
                    Ok((defaults, None))
                }
            }
            .await
            .map_err(|error: anyhow::Error| error.to_string());
            AppCmdMsg::Mele(Command::Options(identity, options))
        });
    }

    pub(crate) fn handle_mele(
        &mut self,
        message: Msg,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        match message {
            Msg::Launcher(message) => self.handle_launcher(*message, sender, root),
            Msg::Install(selection) => self.install_mele(selection, sender, root),
            Msg::Setup(purge) => {
                let Some(tracker) = self.session.tracker.clone() else {
                    return;
                };
                let Some(game) = self.selected_game().cloned() else {
                    return;
                };
                let Some(profile) = self
                    .session
                    .profiles
                    .get(self.session.active_profile_idx)
                    .map(|profile| profile.id.clone())
                else {
                    return;
                };
                self.shell.deploying = true;
                self.begin_work(WorkKind::Deploying, "Preparing MELE deployment...");
                sender.oneshot_command(async move {
                    let language = tracker
                        .mele_recipe(&game.id, &profile)
                        .await
                        .map(|recipe| {
                            recipe
                                .map(|recipe| recipe.language)
                                .unwrap_or_else(|| "INT".into())
                        })
                        .map_err(|error| format!("Could not load MELE profile: {error:#}"));
                    AppCmdMsg::Mele(Command::Setup { language, purge })
                });
            }
            Msg::Inspect {
                language,
                purge,
                repair,
            } => {
                let Some(tracker) = self.session.tracker.clone() else {
                    return;
                };
                let Some(game) = self.selected_game().cloned() else {
                    return;
                };
                let Some(profile) = self
                    .session
                    .profiles
                    .get(self.session.active_profile_idx)
                    .map(|profile| profile.id.clone())
                else {
                    return;
                };
                self.shell.deploying = true;
                self.begin_work(WorkKind::Deploying, "Preparing MELE deployment...");
                let cancelled = Arc::new(AtomicBool::new(false));
                self.ui.mele_operation = Some(mele_dialog::progress(
                    root,
                    "Preparing MELE Deployment",
                    cancelled.clone(),
                ));
                let progress = Arc::from(super::progress::throttled_mele_progress(
                    sender.input_sender().clone(),
                ));
                self.location_command(sender, async move {
                    AppCmdMsg::Mele(Command::Preview(
                        application::preview(
                            tracker,
                            application::Request {
                                game,
                                profile,
                                language,
                                purge,
                                repair,
                            },
                            cancelled,
                            progress,
                        )
                        .await
                        .map(Box::new)
                        .map_err(|error| format!("{error:#}")),
                    ))
                });
            }
            Msg::Apply(preview) => {
                let Some(tracker) = self.session.tracker.clone() else {
                    return;
                };
                self.begin_work(WorkKind::Deploying, "Building MELE installation...");
                let cancelled = Arc::new(AtomicBool::new(false));
                self.ui.mele_operation = Some(mele_dialog::progress(
                    root,
                    "Applying MELE Profile",
                    cancelled.clone(),
                ));
                let progress = Arc::from(super::progress::throttled_mele_progress(
                    sender.input_sender().clone(),
                ));
                self.location_command(sender, async move {
                    let purge = preview.purge;
                    let profile_id = preview.profile.clone();
                    let result = application::apply(tracker, *preview, cancelled, progress)
                        .await
                        .map(|outcome| DeployCompletion {
                            outcome,
                            profile_id,
                        })
                        .map_err(|error| format!("{error:#}"));
                    AppCmdMsg::Mele(Command::Applied { result, purge })
                });
            }
            Msg::Cancel => {
                self.shell.deploying = false;
                self.finish_work(WorkKind::Deploying);
            }
            Msg::Progress(fraction) => {
                if self.shell.deploying {
                    self.update_work(
                        WorkKind::Deploying,
                        "Building MELE installation...",
                        Some(fraction),
                    );
                    if let Some(dialog) = &self.ui.mele_operation {
                        dialog.set_body(&format!("{:.0}% — preparing, merging, and verifying files. Cancellation waits for safe cleanup.", fraction * 100.0));
                    }
                }
            }
        }
    }

    fn install_mele(
        &mut self,
        selection: mele_dialog::Selection,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(identity) = self.install.identity() else {
            return;
        };
        let Some(mut pending) = self.install.pending.take() else {
            return;
        };
        let Some(plan) = pending.mele.take() else {
            self.push_notification(
                "MELE installation plan is unavailable; inspect the archive again",
            );
            return;
        };
        let replacement = self.install.replacement.take();
        let request = library::Import {
            binary_approved: selection.binary_approved,
            source: pending.tmp_dir.path().to_path_buf(),
            plan: *plan,
            game: pending.game,
            name: selection.name,
            options: selection.options,
            nexus: pending.nexus_ids,
            archive_hash: pending.archive_hash,
            archive_path: pending.archive_path,
            replace: replacement.as_ref().map(|item| item.mod_id.clone()),
        };
        let cancelled = Arc::new(AtomicBool::new(false));
        self.ui.mele_operation = Some(mele_dialog::progress(
            root,
            "Installing MELE Mod",
            cancelled.clone(),
        ));
        self.install.set_stage(InstallStage::Committing);
        self.begin_work(WorkKind::Installing, "Retaining verified MELE sources...");
        let report = super::progress::throttled_install_progress(
            sender.input_sender().clone(),
            identity.clone(),
            "Retaining sources",
        );
        self.location_command(sender, async move {
            let result = library::import(tracker, request, cancelled, Arc::new(report))
                .await
                .map_err(|error| format!("{error:#}"));
            drop(pending.tmp_dir);
            AppCmdMsg::Install(super::messages::InstallCmdMsg::ModAdded(
                identity,
                Box::new(result),
                replacement,
            ))
        });
    }

    pub(crate) fn handle_mele_command(
        &mut self,
        command: Command,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        match command {
            Command::Launcher(command) => self.handle_launcher_command(*command, sender, root),
            Command::Removed(result) => {
                self.shell.deploying = false;
                self.finish_work(WorkKind::Deploying);
                match result {
                    Ok(entries) => {
                        self.mods.selected.clear();
                        self.mods.selection_dirty = true;
                        self.shell.needs_deploy = true;
                        for entry in entries {
                            self.handle_cmd_mod_removed(
                                Ok((entry.id, Vec::new())),
                                entry.nexus_mod_id.zip(entry.nexus_file_id),
                                entry.name,
                                entry.archive_hash,
                                sender,
                            );
                        }
                    }
                    Err(error) => self.push_notification(&format!("MELE removal failed: {error}")),
                }
            }
            Command::Options(identity, result) => {
                if !self.install.accepts(&identity) {
                    return;
                }
                match result {
                    Ok((options, approval)) => {
                        let Some(pending) = &self.install.pending else {
                            return;
                        };
                        let Some(plan) = &pending.mele else { return };
                        let input = sender.input_sender().clone();
                        mele_dialog::install(
                            root,
                            plan,
                            &pending.mod_name,
                            &options,
                            approval
                                .as_ref()
                                .is_some_and(|approval| approval.matches(plan)),
                            move |selection| {
                                let message = selection
                                    .map(|selection| AppMsg::Mele(Msg::Install(selection)))
                                    .unwrap_or(AppMsg::Install(
                                        super::messages::InstallMsg::PreInstallCancelled,
                                    ));
                                let _ = input.send(message);
                            },
                        );
                    }
                    Err(error) => {
                        self.handle_pre_install_cancelled();
                        self.push_notification(&error);
                    }
                }
            }
            Command::Setup { language, purge } => {
                self.finish_work(WorkKind::Deploying);
                let language = match language {
                    Ok(language) => language,
                    Err(error) => {
                        self.shell.deploying = false;
                        self.push_notification(&error);
                        return;
                    }
                };
                let dialog = adw::AlertDialog::builder().heading(if purge { "Restore MELE Game Files" } else { "Deploy MELE Profile" })
                    .body(if purge {
                        "Restore the game to its recorded original state. Your mod library and saves will be kept."
                    } else {
                        "Choose the game language. Deployd will prepare the selected mods and any required support files. Your saves will be kept."
                    }).build();
                dialog.add_responses(&[("cancel", "Cancel"), ("preview", "Continue")]);
                dialog.set_response_appearance("preview", adw::ResponseAppearance::Suggested);
                dialog.set_close_response("cancel");
                let group = adw::PreferencesGroup::new();
                let languages = [
                    "INT", "DE", "ES", "FE", "FR", "GE", "IE", "IT", "JA", "PL", "PLPC", "RA", "RU",
                ];
                let language_row = adw::ComboRow::builder()
                    .title("Game language")
                    .model(&gtk::StringList::new(&languages))
                    .selected(
                        languages
                            .iter()
                            .position(|value| *value == language)
                            .unwrap_or(0) as u32,
                    )
                    .build();
                group.add(&language_row);
                let repair = adw::SwitchRow::builder().title("Repair missing support files").subtitle("Replace missing files installed by Deployd. Files you changed are preserved.").build();
                group.add(&repair);
                dialog.set_extra_child(Some(&group));
                let input = sender.input_sender().clone();
                dialog.connect_response(None, move |_, response| {
                    let message = if response == "preview" {
                        Msg::Inspect {
                            language: languages
                                .get(language_row.selected() as usize)
                                .copied()
                                .unwrap_or("INT")
                                .into(),
                            purge,
                            repair: repair.is_active(),
                        }
                    } else {
                        Msg::Cancel
                    };
                    let _ = input.send(AppMsg::Mele(message));
                });
                dialog.present(Some(root));
            }
            Command::Preview(result) => {
                self.close_mele_operation();
                self.finish_work(WorkKind::Deploying);
                match result {
                    Ok(preview) => {
                        let dialog = adw::AlertDialog::builder()
                            .heading(if preview.purge {
                                "Restore game?"
                            } else {
                                "Apply mods?"
                            })
                            .body(&preview.summary)
                            .build();
                        dialog.add_responses(&[
                            ("cancel", "Cancel"),
                            ("apply", if preview.purge { "Restore" } else { "Deploy" }),
                        ]);
                        dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
                        dialog.set_close_response("cancel");
                        let plan = std::cell::RefCell::new(Some(preview));
                        let input = sender.input_sender().clone();
                        dialog.connect_response(None, move |_, response| {
                            let message = if response == "apply" {
                                plan.borrow_mut()
                                    .take()
                                    .map(Msg::Apply)
                                    .unwrap_or(Msg::Cancel)
                            } else {
                                Msg::Cancel
                            };
                            let _ = input.send(AppMsg::Mele(message));
                        });
                        dialog.present(Some(root));
                    }
                    Err(error) => {
                        self.shell.deploying = false;
                        self.push_notification(&format!("Cannot deploy MELE: {error}"));
                    }
                }
            }
            Command::Applied { result, purge } => {
                self.close_mele_operation();
                if purge {
                    self.finish_work(WorkKind::Deploying);
                    self.handle_cmd_purge_done(result.map(|done| {
                        crate::core::deployer::PurgeOutcome {
                            files_removed: done.outcome.files_removed,
                            vanilla_files_restored: done.outcome.vanilla_files_restored,
                            warnings: done.outcome.warnings,
                        }
                    }));
                } else {
                    self.handle_cmd_deploy_done(result, sender);
                }
            }
        }
    }

    pub(crate) fn close_mele_operation(&mut self) {
        if let Some(dialog) = self.ui.mele_operation.take() {
            dialog.close();
        }
    }
}
