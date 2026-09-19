use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, atomic::AtomicBool};

use adw::prelude::*;
use relm4::prelude::*;

use crate::core::game::mass_effect::launcher::{self, Action, Snapshot};
use crate::ui::mele_dialog;

use super::super::{
    App,
    messages::{AppCmdMsg, AppMsg},
    types::WorkKind,
};

#[derive(Debug)]
pub(crate) enum Msg {
    Open,
    Apply(Box<Snapshot>, Action),
    ApplyPrepared(Box<crate::core::generations::activation::PreparedShared>),
    History,
    RestoreRevision(String),
    DeleteRevision(String),
}

#[derive(Debug)]
pub(crate) enum Command {
    Loaded(Result<Box<Snapshot>, String>),
    Prepared(Result<Box<crate::core::generations::activation::PreparedShared>, String>),
    Applied(Result<(), String>),
    HistoryLoaded(Result<Vec<crate::core::generations::api::SharedEntry>, String>),
    RevisionDeleted(Result<(), String>),
}

fn message(message: Msg) -> AppMsg {
    AppMsg::Mele(super::Msg::Launcher(Box::new(message)))
}
fn command(command: Command) -> AppCmdMsg {
    AppCmdMsg::Mele(super::Command::Launcher(Box::new(command)))
}

impl App {
    pub(super) fn handle_launcher(
        &mut self,
        action: Msg,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        if self.is_busy() {
            return;
        }
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        match action {
            Msg::Open => {
                let Some(game) = self.selected_game().cloned() else {
                    return;
                };
                self.begin_work(WorkKind::Deploying, "Loading shared launcher mods...");
                self.location_command(sender, async move {
                    command(Command::Loaded(
                        launcher::load(&tracker, &game)
                            .await
                            .map(Box::new)
                            .map_err(|error| format!("{error:#}")),
                    ))
                });
            }
            Msg::Apply(snapshot, action) => {
                self.begin_work(WorkKind::Deploying, "Updating shared launcher mods...");
                let cancelled = Arc::new(AtomicBool::new(false));
                self.ui.mele_operation = Some(mele_dialog::progress(
                    root,
                    "Updating Shared Launcher Mods",
                    cancelled.clone(),
                ));
                self.location_command(sender, async move {
                    let result = async {
                        let (game, entries) =
                            launcher::generation_entries(&tracker, *snapshot, action, cancelled)
                                .await?;
                        let cache = tracker
                            .get_setting(&format!("cache_dir_{}", game.id))
                            .await?
                            .map(PathBuf::from)
                            .map_or_else(crate::utils::paths::cache_root, Ok)?;
                        crate::core::generations::activation::prepare_shared(
                            &tracker,
                            &game,
                            &cache,
                            entries,
                            crate::core::generations::content::Control::default(),
                        )
                        .await
                    }
                    .await
                    .map(Box::new)
                    .map_err(|error| format!("{error:#}"));
                    command(Command::Prepared(result))
                });
            }
            Msg::ApplyPrepared(prepared) => {
                self.begin_work(WorkKind::Deploying, "Applying shared launcher revision...");
                self.location_command(sender, async move {
                    command(Command::Applied(
                        prepared
                            .apply(crate::core::generations::content::Control::default())
                            .await
                            .map_err(|error| format!("{error:#}")),
                    ))
                });
            }
            Msg::History => {
                let Some(game) = self.selected_game().cloned() else {
                    return;
                };
                let cache = match self.cache_root_for(&game.id) {
                    Ok(cache) => cache,
                    Err(error) => {
                        self.push_notification(&format!("Cannot access launcher history: {error}"));
                        return;
                    }
                };
                self.begin_work(WorkKind::Deploying, "Loading shared launcher history...");
                self.location_command(sender, async move {
                    command(Command::HistoryLoaded(
                        crate::core::generations::api::list_shared(&tracker, &game, &cache)
                            .await
                            .map_err(|error| format!("{error:#}")),
                    ))
                });
            }
            Msg::RestoreRevision(revision) => {
                let Some(game) = self.selected_game().cloned() else {
                    return;
                };
                let cache = match self.cache_root_for(&game.id) {
                    Ok(cache) => cache,
                    Err(error) => {
                        self.push_notification(&format!("Cannot access launcher history: {error}"));
                        return;
                    }
                };
                self.begin_work(
                    WorkKind::Deploying,
                    "Preparing shared launcher restoration...",
                );
                self.location_command(sender, async move {
                    command(Command::Prepared(
                        crate::core::generations::activation::prepare_shared_restore(
                            &tracker,
                            &game,
                            &cache,
                            &revision,
                            crate::core::generations::content::Control::default(),
                        )
                        .await
                        .map(Box::new)
                        .map_err(|error| format!("{error:#}")),
                    ))
                });
            }
            Msg::DeleteRevision(revision) => {
                let Some(game) = self.selected_game().cloned() else {
                    return;
                };
                let cache = match self.cache_root_for(&game.id) {
                    Ok(cache) => cache,
                    Err(error) => {
                        self.push_notification(&format!("Cannot access launcher history: {error}"));
                        return;
                    }
                };
                self.begin_work(WorkKind::Deploying, "Deleting shared launcher history...");
                self.location_command(sender, async move {
                    command(Command::RevisionDeleted(
                        crate::core::generations::api::delete_shared(
                            &tracker, &game, &cache, &revision,
                        )
                        .await
                        .map_err(|error| format!("{error:#}")),
                    ))
                });
            }
        }
    }

    pub(super) fn handle_launcher_command(
        &mut self,
        result: Command,
        sender: &ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        match result {
            Command::Loaded(result) => {
                self.finish_work(WorkKind::Deploying);
                match result {
                    Ok(snapshot) => show(root, sender.input_sender().clone(), *snapshot),
                    Err(error) => self.push_notification(&error),
                }
            }
            Command::Applied(result) => {
                self.close_mele_operation();
                self.finish_work(WorkKind::Deploying);
                match result {
                    Ok(()) => {
                        self.push_notification("Shared launcher mods updated for all three games.");
                        sender.input(message(Msg::Open));
                    }
                    Err(error) => self.push_notification(&error),
                }
            }
            Command::Prepared(result) => {
                self.close_mele_operation();
                self.finish_work(WorkKind::Deploying);
                match result {
                    Ok(prepared) => {
                        let body = if prepared.differences.is_empty() {
                            "No launcher files differ. Applying still records the shared revision."
                                .to_string()
                        } else {
                            format!(
                                "This affects the shared launcher used by LE1, LE2 and LE3.\n\n{}",
                                prepared
                                    .differences
                                    .iter()
                                    .take(50)
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            )
                        };
                        let dialog = adw::AlertDialog::builder()
                            .heading("Apply shared launcher revision?")
                            .body(body)
                            .build();
                        dialog.add_response("cancel", "Cancel");
                        dialog.add_response("apply", "Apply");
                        dialog.set_close_response("cancel");
                        dialog.set_response_appearance("apply", adw::ResponseAppearance::Suggested);
                        let pending = std::cell::RefCell::new(Some(prepared));
                        let input = sender.input_sender().clone();
                        dialog.connect_response(None, move |_, response| {
                            if response == "apply"
                                && let Some(prepared) = pending.borrow_mut().take()
                            {
                                let _ = input.send(message(Msg::ApplyPrepared(prepared)));
                            }
                        });
                        dialog.present(Some(root));
                    }
                    Err(error) => self.push_notification(&error),
                }
            }
            Command::HistoryLoaded(result) => {
                self.finish_work(WorkKind::Deploying);
                match result {
                    Ok(entries) => show_history(root, sender.input_sender().clone(), entries),
                    Err(error) => self.push_notification(&error),
                }
            }
            Command::RevisionDeleted(result) => {
                self.finish_work(WorkKind::Deploying);
                match result {
                    Ok(()) => {
                        self.show_toast("Shared launcher revision deleted");
                        sender.input(message(Msg::History));
                    }
                    Err(error) => self.push_notification(&error),
                }
            }
        }
    }
}

fn show(root: &adw::ApplicationWindow, input: relm4::Sender<AppMsg>, snapshot: Snapshot) {
    let dialog = adw::AlertDialog::builder().heading("Shared Launcher Mods").body("These mods affect LE1, LE2 and LE3. They stay installed when you switch game profiles or purge a game. Later mods take priority.").build();
    dialog.add_responses(&[("close", "Close"), ("history", "History")]);
    dialog.set_close_response("close");
    let group = adw::PreferencesGroup::new();
    let snapshot = Rc::new(snapshot);
    for (index, entry) in snapshot.entries.iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(&entry.name)
            .subtitle(if entry.enabled {
                "Enabled for all three games"
            } else {
                "Disabled"
            })
            .build();
        for (label, tooltip, action, sensitive) in [
            (
                if entry.enabled {
                    "media-playback-pause-symbolic"
                } else {
                    "media-playback-start-symbolic"
                },
                "Enable or disable",
                Action::Enable(entry.id.clone(), !entry.enabled),
                true,
            ),
            (
                "go-up-symbolic",
                "Move earlier",
                Action::Move(entry.id.clone(), -1),
                index > 0,
            ),
            (
                "go-down-symbolic",
                "Move later",
                Action::Move(entry.id.clone(), 1),
                index + 1 < snapshot.entries.len(),
            ),
            (
                "user-trash-symbolic",
                "Remove from all three games",
                Action::Remove(entry.id.clone()),
                true,
            ),
        ] {
            let button = gtk::Button::builder()
                .icon_name(label)
                .tooltip_text(tooltip)
                .valign(gtk::Align::Center)
                .sensitive(sensitive)
                .build();
            let pending = std::cell::RefCell::new(Some(action));
            let dialog = dialog.downgrade();
            let snapshot = snapshot.clone();
            let input = input.clone();
            button.connect_clicked(move |_| {
                if let Some(action) = pending.borrow_mut().take() {
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.close();
                    }
                    let _ = input.send(message(Msg::Apply(Box::new((*snapshot).clone()), action)));
                }
            });
            row.add_suffix(&button);
        }
        group.add(&row);
    }
    for (label, action) in [
        ("Restore launcher mod files for all games", Action::Restore),
        ("Repair missing managed launcher files", Action::Repair),
    ] {
        let button = gtk::Button::with_label(label);
        button.set_sensitive(!snapshot.entries.is_empty());
        let pending = std::cell::RefCell::new(Some(action));
        let dialog = dialog.downgrade();
        let snapshot = snapshot.clone();
        let input = input.clone();
        button.connect_clicked(move |_| {
            if let Some(action) = pending.borrow_mut().take() {
                if let Some(dialog) = dialog.upgrade() {
                    dialog.close();
                }
                let _ = input.send(message(Msg::Apply(Box::new((*snapshot).clone()), action)));
            }
        });
        group.add(&button);
    }
    let scroll = gtk::ScrolledWindow::builder()
        .max_content_height(420)
        .propagate_natural_height(true)
        .child(&group)
        .build();
    dialog.set_extra_child(Some(&scroll));
    dialog.connect_response(None, move |_, response| {
        if response == "history" {
            let _ = input.send(message(Msg::History));
        }
    });
    dialog.present(Some(root));
}

fn show_history(
    root: &adw::ApplicationWindow,
    input: relm4::Sender<AppMsg>,
    entries: Vec<crate::core::generations::api::SharedEntry>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading("Shared launcher history")
        .body("Restore is a separate shared action affecting LE1, LE2 and LE3. Revisions used by game history or the live launcher cannot be deleted.")
        .build();
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    if entries.is_empty() {
        list.append(
            &adw::ActionRow::builder()
                .title("No shared launcher history yet")
                .subtitle("History begins after the next shared Apply.")
                .build(),
        );
    }
    for entry in entries {
        let row = adw::ActionRow::builder()
            .title(&entry.timestamp)
            .subtitle(format!(
                "{}{}",
                if entry.live {
                    "Live revision"
                } else {
                    "Historical revision"
                },
                if entry.references > 0 {
                    format!(" · referenced by {} game generation(s)", entry.references)
                } else {
                    String::new()
                }
            ))
            .build();
        let restore = gtk::Button::with_label("Restore");
        restore.set_valign(gtk::Align::Center);
        let revision = entry.id.clone();
        let restore_input = input.clone();
        let history = dialog.clone();
        restore.connect_clicked(move |_| {
            history.close();
            let _ = restore_input.send(message(Msg::RestoreRevision(revision.clone())));
        });
        row.add_suffix(&restore);

        let delete = gtk::Button::with_label("Delete");
        delete.set_valign(gtk::Align::Center);
        delete.set_sensitive(entry.deletion.is_ok());
        if let Err(reason) = &entry.deletion {
            delete.set_tooltip_text(Some(reason));
        }
        if let Ok(bytes) = entry.deletion {
            let revision = entry.id;
            let delete_input = input.clone();
            let history = dialog.clone();
            let parent = root.clone();
            delete.connect_clicked(move |_| {
                let prompt = adw::AlertDialog::builder()
                    .heading("Delete this shared revision?")
                    .body(format!(
                        "This permanently removes the revision and reclaims {bytes} bytes."
                    ))
                    .build();
                prompt.add_response("cancel", "Cancel");
                prompt.add_response("delete", "Delete");
                prompt.set_close_response("cancel");
                prompt.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
                let revision = revision.clone();
                let delete_input = delete_input.clone();
                let history = history.clone();
                prompt.connect_response(None, move |_, response| {
                    if response == "delete" {
                        history.close();
                        let _ = delete_input.send(message(Msg::DeleteRevision(revision.clone())));
                    }
                });
                prompt.present(Some(&parent));
            });
        }
        row.add_suffix(&delete);
        list.append(&row);
    }
    dialog.set_extra_child(Some(
        &gtk::ScrolledWindow::builder()
            .min_content_height(100)
            .max_content_height(440)
            .propagate_natural_height(true)
            .child(&list)
            .build(),
    ));
    dialog.present(Some(root));
}
