use gtk::prelude::*;
use relm4::prelude::*;

use crate::models::order_snapshot::{OrderSnapshot, SnapshotKind};
use crate::ui::mod_list::ModListItemKind;

use super::App;
use super::free_fns::load_game_data;
use super::messages::{AppCmdMsg, AppMsg};
use super::types::LoadedData;

pub(super) enum SnapshotAction {
    SaveMod(String),
    SavePlugin(String),
    LoadMod(String),
    LoadPlugin(String),
    Delete(String),
}

impl App {
    pub(super) fn handle_snapshot_action(
        &mut self,
        action: SnapshotAction,
        sender: &ComponentSender<Self>,
    ) {
        match action {
            SnapshotAction::SaveMod(name) => self.handle_save_mod_order_snapshot(name, sender),
            SnapshotAction::SavePlugin(name) => {
                self.handle_save_plugin_order_snapshot(name, sender)
            }
            SnapshotAction::LoadMod(id) => self.handle_load_mod_order_snapshot(id, sender),
            SnapshotAction::LoadPlugin(id) => self.handle_load_plugin_order_snapshot(id, sender),
            SnapshotAction::Delete(id) => self.handle_delete_order_snapshot(id, sender),
        }
    }

    pub(crate) fn reload_order_snapshots(&self, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        sender.oneshot_command(async move {
            let mod_snaps = tracker
                .list_order_snapshots(&game.id, SnapshotKind::Mod)
                .await
                .unwrap_or_default();
            let plugin_snaps = tracker
                .list_order_snapshots(&game.id, SnapshotKind::Plugin)
                .await
                .unwrap_or_default();
            AppCmdMsg::OrderSnapshotsLoaded(mod_snaps, plugin_snaps)
        });
    }

    /// Rebuild the Load popover listboxes from the current snapshot lists.
    pub(crate) fn rebuild_snapshot_lists(&self, sender: &ComponentSender<Self>) {
        rebuild_list(
            &self.mod_snapshots_list,
            &self.mod_order_snapshots,
            true,
            sender,
        );
        rebuild_list(
            &self.plugin_snapshots_list,
            &self.plugin_order_snapshots,
            false,
            sender,
        );
    }

    fn handle_save_mod_order_snapshot(&mut self, name: String, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        let entries: Vec<(String, i32)> = {
            let guard = self.mods.guard();
            let mut priority = 0i32;
            (0..guard.len())
                .filter_map(|i| {
                    guard.get(i).and_then(|item| {
                        if let ModListItemKind::Mod(ref r) = item.kind {
                            let entry = (r.mod_entry.id.clone(), priority);
                            priority += 1;
                            Some(entry)
                        } else {
                            None
                        }
                    })
                })
                .collect()
        };

        sender.oneshot_command(async move {
            AppCmdMsg::ModOrderSnapshotSaved(
                tracker
                    .save_order_snapshot(&game.id, &name, SnapshotKind::Mod, &entries)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }

    fn handle_save_plugin_order_snapshot(&mut self, name: String, sender: &ComponentSender<Self>) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        let entries: Vec<(String, i32)> = {
            let guard = self.plugins.guard();
            (0..guard.len())
                .filter_map(|i| {
                    guard.get(i).and_then(|row| {
                        if row.is_vanilla {
                            None
                        } else {
                            Some((row.plugin.id.clone(), i as i32))
                        }
                    })
                })
                .collect()
        };

        sender.oneshot_command(async move {
            AppCmdMsg::PluginOrderSnapshotSaved(
                tracker
                    .save_order_snapshot(&game.id, &name, SnapshotKind::Plugin, &entries)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }

    fn handle_load_mod_order_snapshot(
        &mut self,
        snapshot_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        sender.oneshot_command(async move {
            let result = async {
                tracker
                    .restore_mod_order_snapshot(&snapshot_id, &game.id)
                    .await
                    .map_err(|e| e.to_string())?;
                load_game_data(&tracker, &game, crate::app::free_fns::GameLoadMode::Refresh)
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            AppCmdMsg::ModOrderSnapshotRestored(result)
        });
    }

    fn handle_load_plugin_order_snapshot(
        &mut self,
        snapshot_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        sender.oneshot_command(async move {
            let result = async {
                tracker
                    .restore_plugin_order_snapshot(&snapshot_id, &game.id)
                    .await
                    .map_err(|e| e.to_string())?;
                load_game_data(&tracker, &game, crate::app::free_fns::GameLoadMode::Refresh)
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            AppCmdMsg::PluginOrderSnapshotRestored(result)
        });
    }

    fn handle_delete_order_snapshot(
        &mut self,
        snapshot_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };

        sender.oneshot_command(async move {
            AppCmdMsg::OrderSnapshotDeleted(
                tracker
                    .delete_order_snapshot(&snapshot_id)
                    .await
                    .map_err(|e| e.to_string()),
            )
        });
    }

    pub(crate) fn handle_cmd_order_snapshots_loaded(
        &mut self,
        mod_snapshots: Vec<OrderSnapshot>,
        plugin_snapshots: Vec<OrderSnapshot>,
        sender: &ComponentSender<Self>,
    ) {
        self.mod_order_snapshots = mod_snapshots;
        self.plugin_order_snapshots = plugin_snapshots;
        self.rebuild_snapshot_lists(sender);
    }

    pub(crate) fn handle_cmd_mod_order_snapshot_saved(
        &mut self,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        self.handle_cmd_order_snapshot_saved(result, "Mod order snapshot saved", sender);
    }

    pub(crate) fn handle_cmd_plugin_order_snapshot_saved(
        &mut self,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        self.handle_cmd_order_snapshot_saved(result, "Plugin order snapshot saved", sender);
    }

    fn handle_cmd_order_snapshot_saved(
        &mut self,
        result: Result<(), String>,
        success_message: &str,
        sender: &ComponentSender<Self>,
    ) {
        if let Err(error) = result {
            self.push_notification(&format!("Failed to save snapshot: {error}"));
        } else {
            self.show_toast(success_message);
            self.reload_order_snapshots(sender);
        }
    }

    pub(crate) fn handle_cmd_mod_order_snapshot_restored(
        &mut self,
        result: Result<LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.handle_cmd_order_snapshot_restored(result, "Mod order restored", sender);
    }

    pub(crate) fn handle_cmd_plugin_order_snapshot_restored(
        &mut self,
        result: Result<LoadedData, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.handle_cmd_order_snapshot_restored(result, "Plugin order restored", sender);
    }

    fn handle_cmd_order_snapshot_restored(
        &mut self,
        result: Result<LoadedData, String>,
        success_message: &str,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(data) => {
                self.apply_loaded_data(data, sender);
                self.show_toast(success_message);
            }
            Err(error) => {
                self.push_notification(&format!("Failed to restore snapshot: {error}"));
            }
        }
    }

    pub(crate) fn handle_cmd_order_snapshot_deleted(
        &mut self,
        result: Result<(), String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Err(error) = result {
            self.push_notification(&format!("Failed to delete snapshot: {error}"));
        }
        self.reload_order_snapshots(sender);
    }
}

fn rebuild_list(
    list: &gtk::ListBox,
    snapshots: &[OrderSnapshot],
    is_mod: bool,
    sender: &ComponentSender<App>,
) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    if snapshots.is_empty() {
        let placeholder = gtk::Label::builder()
            .label("No saved snapshots")
            .css_classes(["dim-label"])
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();
        list.append(&placeholder);
        return;
    }

    for snap in snapshots {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(8)
            .margin_end(8)
            .build();

        let created = snap.created_at.get(..10).unwrap_or(&snap.created_at);
        let label = gtk::Label::builder()
            .label(format!("{} ({})", snap.name, created))
            .hexpand(true)
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .width_chars(22)
            .build();

        let restore_btn = gtk::Button::builder()
            .icon_name("edit-redo-symbolic")
            .tooltip_text("Restore this order")
            .css_classes(["flat", "circular"])
            .valign(gtk::Align::Center)
            .build();

        let delete_btn = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .tooltip_text("Delete snapshot")
            .css_classes(["flat", "circular"])
            .valign(gtk::Align::Center)
            .build();

        let snap_id = snap.id.clone();
        let s = sender.input_sender().clone();
        restore_btn.connect_clicked(move |btn| {
            if is_mod {
                let _ = s.send(AppMsg::LoadModOrderSnapshot(snap_id.clone()));
            } else {
                let _ = s.send(AppMsg::LoadPluginOrderSnapshot(snap_id.clone()));
            }
            if let Some(popover) = btn
                .ancestor(gtk::Popover::static_type())
                .and_downcast::<gtk::Popover>()
            {
                popover.popdown();
            }
        });

        let snap_id = snap.id.clone();
        let s = sender.input_sender().clone();
        delete_btn.connect_clicked(move |btn| {
            if is_mod {
                let _ = s.send(AppMsg::DeleteModOrderSnapshot(snap_id.clone()));
            } else {
                let _ = s.send(AppMsg::DeletePluginOrderSnapshot(snap_id.clone()));
            }
            if let Some(popover) = btn
                .ancestor(gtk::Popover::static_type())
                .and_downcast::<gtk::Popover>()
            {
                popover.popdown();
            }
        });

        row.append(&label);
        row.append(&restore_btn);
        row.append(&delete_btn);

        let list_row = gtk::ListBoxRow::new();
        list_row.set_child(Some(&row));
        list.append(&list_row);
    }
}
