use adw::prelude::*;
use relm4::prelude::*;

use crate::core::generations::{api, content::Control};

use super::session::{GameLoadMode, load_game_data};
use super::types::{LoadedData, WorkKind};
use super::{App, AppCmdMsg, AppMsg};

#[derive(Debug)]
pub(crate) enum Msg {
    Open,
    Verify {
        game: String,
    },
    Restore {
        game: String,
        generation: String,
        name: String,
    },
    Delete {
        game: String,
        generation: String,
    },
}

#[derive(Debug)]
pub(crate) enum Cmd {
    Listed(String, Result<api::Overview, String>),
    Restored(Box<Result<LoadedData, String>>),
    Deleted(Result<(), String>),
    Verified(Result<api::IntegrityReport, String>),
}

impl App {
    pub(crate) fn handle_generation(&mut self, message: Msg, sender: &ComponentSender<Self>) {
        if self.is_busy() {
            self.show_toast("Wait for the current operation to finish");
            return;
        }
        self.ui.deploy_options_btn.popdown();
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let current_profile = self
            .session
            .profiles
            .get(self.session.active_profile_idx)
            .map(|profile| profile.id.clone());
        let cache = match self.cache_root_for(&game.id) {
            Ok(cache) => cache,
            Err(error) => {
                self.push_notification(&format!("Cannot access deployment history: {error}"));
                return;
            }
        };
        let requested_game = match &message {
            Msg::Open => &game.id,
            Msg::Restore { game, .. } | Msg::Delete { game, .. } | Msg::Verify { game } => game,
        };
        if requested_game != &game.id {
            self.show_toast("Reopen deployment history for the selected game");
            return;
        }
        self.begin_work(WorkKind::Deploying, "Working with deployment history…");
        let control = if matches!(&message, Msg::Verify { .. }) {
            self.begin_integrity_check();
            self.deployment_control()
        } else {
            Control::default()
        };
        self.location_command(sender, async move {
            let result = match message {
                Msg::Verify { .. } => Cmd::Verified(
                    api::verify_integrity(&tracker, &game, &cache, control)
                        .await
                        .map_err(|error| format!("{error:#}")),
                ),
                Msg::Open => Cmd::Listed(
                    game.id.clone(),
                    api::list(&tracker, &game.id)
                        .await
                        .map_err(|error| error.to_string()),
                ),
                Msg::Restore {
                    generation, name, ..
                } => {
                    let result = async {
                        if let Some(profile) = current_profile {
                            tracker
                                .save_to_profile(&profile, &game.id)
                                .await
                                .map_err(|error| error.to_string())?;
                        }
                        let profile = api::restore(
                            &tracker,
                            &game,
                            &cache,
                            &generation,
                            &name,
                            Control::default(),
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                        tracker
                            .switch_profile(&game.id, &profile)
                            .await
                            .map_err(|error| error.to_string())?;
                        load_game_data(&tracker, &game, GameLoadMode::Refresh).await
                    }
                    .await;
                    Cmd::Restored(Box::new(result))
                }
                Msg::Delete { generation, .. } => Cmd::Deleted(
                    api::delete(&tracker, &game, &cache, &generation)
                        .await
                        .map_err(|error| error.to_string()),
                ),
            };
            AppCmdMsg::Generations(result)
        });
    }

    pub(crate) fn handle_generation_command(
        &mut self,
        message: Cmd,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        self.finish_work(WorkKind::Deploying);
        match message {
            Cmd::Verified(result) => match result {
                Ok(report) => self.deployment_result("Integrity check complete", &format!("Checked {} generations and the deployed files, reading {} bytes. {} deployed file(s) differ from their recorded state. No game files or saves were changed.", report.generations, report.hashed_bytes, report.differing_files)),
                Err(error) => self.deployment_result("Integrity check stopped", &error),
            },
            Cmd::Listed(game, Ok(overview)) => {
                if self
                    .selected_game()
                    .is_some_and(|selected| selected.id == game)
                {
                    show_history(&game, overview, sender, root);
                }
            }
            Cmd::Restored(result) => match *result {
                Ok(data) => {
                    self.apply_loaded_data(data, sender);
                    self.show_toast("History restored as a new profile — Deploy to apply");
                }
                Err(error) => {
                    self.push_notification(&format!("Could not restore history: {error}"))
                }
            },
            Cmd::Deleted(Ok(())) => {
                self.show_toast("Deployment history deleted");
                sender.input(AppMsg::Generations(Msg::Open));
            }
            Cmd::Listed(_, Err(error)) | Cmd::Deleted(Err(error)) => {
                self.push_notification(&format!("Deployment history: {error}"))
            }
        }
    }
}

fn show_history(
    game: &str,
    overview: api::Overview,
    sender: &ComponentSender<App>,
    root: &adw::ApplicationWindow,
) {
    let dialog = adw::AlertDialog::builder().heading("Deployment history")
        .body(format!("Retained content: {} bytes. Restore a generation as a new editable profile, then Deploy it when ready. Saves are not included.", overview.bytes)).build();
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");
    dialog.add_response("verify", "Full integrity check");
    dialog.set_response_enabled("verify", !overview.entries.is_empty());
    let input = sender.input_sender().clone();
    let selected_game = game.to_owned();
    dialog.connect_response(Some("verify"), move |_, _| {
        let _ = input.send(AppMsg::Generations(Msg::Verify {
            game: selected_game.clone(),
        }));
    });
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    if overview.entries.is_empty() {
        let empty = adw::ActionRow::builder()
            .title("No deployment history yet")
            .subtitle("History begins with the next successful explicit Deploy.")
            .build();
        list.append(&empty);
    }
    for entry in overview.entries {
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        let title = gtk::Label::builder()
            .label(&entry.name)
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .build();
        title.add_css_class("heading");
        content.append(&title);
        let details = gtk::Label::builder()
            .label(format!(
                "{}{}{}{}",
                entry.timestamp,
                if entry.deployed { " · Deployed" } else { "" },
                if entry.modified { " · Modified" } else { "" },
                if entry.recovery_pending {
                    " · Recovery required"
                } else {
                    ""
                }
            ))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .build();
        details.add_css_class("dim-label");
        content.append(&details);
        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .margin_top(4)
            .build();
        let restore = gtk::Button::with_label("Restore as new profile");
        restore.set_valign(gtk::Align::Center);
        restore.set_sensitive(!entry.recovery_pending);
        let input = sender.input_sender().clone();
        let restore_game = game.to_owned();
        let generation = entry.id.clone();
        let name = entry.name.clone();
        let parent = root.clone();
        let history_dialog = dialog.clone();
        restore.connect_clicked(move |_| {
            let prompt = adw::AlertDialog::builder().heading("Restore as new profile").body("The new profile remains editable. Game files and live saves change only when you Deploy it.").build();
            let name = adw::EntryRow::builder().title("Profile name").text(format!("Restored {name}")).build();
            prompt.set_extra_child(Some(&name));
            prompt.add_response("cancel", "Cancel");
            prompt.add_response("restore", "Restore");
            prompt.set_close_response("cancel");
            prompt.set_response_appearance("restore", adw::ResponseAppearance::Suggested);
            let input = input.clone(); let game = restore_game.clone(); let generation = generation.clone(); let history = history_dialog.clone();
            prompt.connect_response(None, move |_, response| {
                if response == "restore" {
                    history.close();
                    let _ = input.send(AppMsg::Generations(Msg::Restore { game: game.clone(), generation: generation.clone(), name: name.text().to_string() }));
                }
            });
            prompt.present(Some(&parent));
        });
        actions.append(&restore);
        let delete = gtk::Button::with_label("Delete");
        delete.set_valign(gtk::Align::Center);
        delete.set_sensitive(entry.deletion.is_ok());
        if let Err(reason) = &entry.deletion {
            delete.set_tooltip_text(Some(reason));
        }
        if let Ok(bytes) = entry.deletion {
            let input = sender.input_sender().clone();
            let game = game.to_owned();
            let generation = entry.id;
            let parent = root.clone();
            let history = dialog.clone();
            delete.connect_clicked(move |_| {
                let prompt = adw::AlertDialog::builder().heading("Delete this generation?").body(format!("This permanently deletes the history entry and reclaims {bytes} bytes. Shared content used by other history entries is retained.")).build();
                prompt.add_response("cancel", "Cancel"); prompt.add_response("delete", "Delete"); prompt.set_close_response("cancel");
                prompt.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                let input = input.clone(); let game = game.clone(); let generation = generation.clone(); let history = history.clone();
                prompt.connect_response(None, move |_, response| {
                    if response == "delete" { history.close(); let _ = input.send(AppMsg::Generations(Msg::Delete { game: game.clone(), generation: generation.clone() })); }
                });
                prompt.present(Some(&parent));
            });
        }
        actions.append(&delete);
        content.append(&actions);
        let row = gtk::ListBoxRow::builder()
            .child(&content)
            .activatable(false)
            .selectable(false)
            .build();
        list.append(&row);
    }
    let scroll = gtk::ScrolledWindow::builder()
        .child(&list)
        .min_content_height(100)
        .max_content_height(440)
        .propagate_natural_height(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    dialog.set_extra_child(Some(&scroll));
    dialog.present(Some(root));
}
