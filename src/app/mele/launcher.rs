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
}

#[derive(Debug)]
pub(crate) enum Command {
    Loaded(Result<Box<Snapshot>, String>),
    Applied(Result<(), String>),
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
                    command(Command::Applied(
                        launcher::apply(tracker, *snapshot, action, cancelled)
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
        }
    }
}

fn show(root: &adw::ApplicationWindow, input: relm4::Sender<AppMsg>, snapshot: Snapshot) {
    let dialog = adw::AlertDialog::builder().heading("Shared Launcher Mods").body("These mods affect LE1, LE2 and LE3. They stay installed when you switch game profiles or purge a game. Later mods take priority.").build();
    dialog.add_response("close", "Close");
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
    dialog.present(Some(root));
}
