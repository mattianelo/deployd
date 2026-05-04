use std::path::PathBuf;

use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use super::super::App;
use super::super::free_fns::load_game_data;
use super::super::messages::{AppCmdMsg, AppMsg};

impl App {
    pub(crate) fn handle_export_profile_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(profile) = self.profiles.get(self.active_profile_idx).cloned() else {
            return;
        };
        let initial_name = format!("{}.deployd-profile.json", profile.name.replace(' ', "_"));
        let dialog = gtk::FileDialog::builder()
            .title("Export Profile")
            .initial_name(&initial_name)
            .build();
        let input_sender = sender.input_sender().clone();
        dialog.save(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let tracker = tracker.clone();
                let profile_id = profile.id.clone();
                gtk::glib::spawn_future_local(async move {
                    let result = async {
                        let export = tracker
                            .export_profile(&profile_id)
                            .await
                            .map_err(|e| e.to_string())?;
                        let json =
                            serde_json::to_string_pretty(&export).map_err(|e| e.to_string())?;
                        std::fs::write(&path, json).map_err(|e| e.to_string())?;
                        Ok(())
                    }
                    .await;
                    let _ = input_sender.send(AppMsg::ProfileExported(result));
                });
            }
        });
    }

    pub(crate) fn handle_profile_exported(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => self.show_toast("Profile exported"),
            Err(e) => self.push_notification(&format!("Export failed: {e}")),
        }
    }

    pub(crate) fn handle_import_profile_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Deployd Profile (*.json)"));
        filter.add_pattern("*.json");
        let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let dialog = gtk::FileDialog::builder()
            .title("Import Profile")
            .filters(&filters)
            .build();
        let input_sender = sender.input_sender().clone();
        dialog.open(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let _ = input_sender.send(AppMsg::ImportProfileFileChosen(path));
            }
        });
    }

    pub(crate) fn handle_import_profile_file_chosen(
        &mut self,
        path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        sender.oneshot_command(async move {
            AppCmdMsg::ProfileImported(
                async {
                    let json = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
                    let export: crate::models::profile_export::ProfileExport =
                        serde_json::from_str(&json).map_err(|e| e.to_string())?;
                    let (new_profile_id, _) = tracker
                        .import_profile(&game.id, &export)
                        .await
                        .map_err(|e| e.to_string())?;
                    tracker
                        .switch_profile(&game.id, &new_profile_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    load_game_data(&tracker, &game, false).await
                }
                .await,
            )
        });
    }
}
