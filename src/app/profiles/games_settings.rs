use std::path::PathBuf;

use adw::prelude::*;
use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::app::types::PendingMigrationImport;
use crate::core::game;
use crate::core::migration_export::{ExportGameRequest, export_game_bundle};
use crate::core::migration_import::{
    ImportBundleRequest, PreviewConflict, PreviewImportRequest, PreviewImportResult,
    ValidationItem, import_bundle, preview_import_bundle,
};
use crate::models::game::GameEngine;
use crate::ui::game_setup_dialog::{GameSetupDialog, GameSetupOutput};
use crate::ui::settings_dialog::{SettingsDialog, SettingsDialogOutput};
use crate::ui::welcome_wizard::{WelcomeWizard, WelcomeWizardOutput};
use crate::utils;
use crate::utils::snap::{self, SelectedFolderKind};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::types::WorkKind;

impl App {
    pub(crate) fn handle_welcome_wizard_skipped(&mut self) {
        self.ui.welcome_wizard = None;
    }

    pub(crate) fn handle_remove_current_game(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(game) = self.session.games.get(self.session.selected_game_idx) {
            self.confirm_remove_game(game.id.clone(), root, sender);
        }
    }

    pub(crate) fn handle_cmd_games_persisted(&mut self, sender: &ComponentSender<Self>) {
        // Force the reload even when the newly persisted game is already index zero.
        self.session.selected_game_idx = usize::MAX;
        sender.input(AppMsg::Games(crate::app::messages::GamesMsg::GameSelected(
            0,
        )));
    }

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
                let dir = tracker
                    .get_setting("downloads_dir")
                    .await
                    .ok()
                    .flatten()
                    .map(PathBuf::from);
                AppCmdMsg::Downloads(crate::app::messages::DownloadsCmdMsg::DownloadsDirUpdated(
                    dir,
                ))
            });
        }
    }

    pub(crate) fn handle_nexus_api_key_updated(&mut self, sender: &ComponentSender<Self>) {
        self.show_toast("Nexus Mods key updated.");
        // Re-validate to refresh username and avatar displayed in the headerbar.
        if let Some(tracker) = self.session.tracker.clone() {
            sender.oneshot_command(async move {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .ok()
                    .flatten()
                    .filter(|k| !k.is_empty());
                match api_key {
                    Some(key) => {
                        let client = crate::core::nexus_api::NexusClient::new(key);
                        match client.validate_key().await {
                            Ok((user, _)) => {
                                let _ = tracker.save_nexus_user(&user).await;
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
            let _ = tracker.clear_nexus_user().await;
            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::NexusUserRefreshed(
                None, None, false,
            ))
        });
        self.show_toast("Logged out of Nexus Mods");
    }

    pub(crate) fn handle_manage_games_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let detected: Vec<crate::models::game::Game> = self.session.games.clone();
        let cache_dirs = self.session.game_cache_dirs.clone();
        let can_export_for_snap = self.shell.running_as_appimage
            && std::env::var_os("SNAP").is_none()
            && utils::experimental_enabled();

        self.ui.game_setup_dialog = Some(
            GameSetupDialog::builder()
                .transient_for(root)
                .launch((detected, vec![], cache_dirs, can_export_for_snap))
                .forward(sender.input_sender(), |output| match output {
                    GameSetupOutput::Confirmed {
                        enabled,
                        hidden_ids,
                    } => AppMsg::Games(crate::app::messages::GamesMsg::GamesConfigured(
                        enabled, hidden_ids,
                    )),
                    GameSetupOutput::Closed => {
                        AppMsg::Games(crate::app::messages::GamesMsg::ManageGamesClosed)
                    }
                    GameSetupOutput::CacheDirChangeRequested { game_id, new_dir } => {
                        AppMsg::Games(crate::app::messages::GamesMsg::CacheDirChangeRequested {
                            game_id,
                            new_dir,
                        })
                    }
                    GameSetupOutput::CacheDirResetRequested { game_id } => {
                        AppMsg::Games(crate::app::messages::GamesMsg::CacheDirResetRequested {
                            game_id,
                        })
                    }
                    GameSetupOutput::ExportForSnapRequested { game_id } => AppMsg::Migration(
                        crate::app::messages::MigrationMsg::ExportGameForSnap(game_id),
                    ),
                }),
        );
    }

    pub(crate) fn handle_export_game_for_snap(
        &mut self,
        game_id: String,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if !self.shell.running_as_appimage || std::env::var_os("SNAP").is_some() {
            self.show_toast("Snap migration export is only available from the AppImage.");
            return;
        }
        if !utils::experimental_enabled() {
            self.show_toast("Snap migration export is experimental and disabled.");
            return;
        }
        let Some(game) = self.session.games.iter().find(|g| g.id == game_id) else {
            self.show_toast("Game is no longer managed.");
            return;
        };

        let dialog = gtk::FileDialog::builder()
            .title(format!("Export {} for Snap", game.title))
            .modal(true)
            .initial_name(format!(
                "{}.deployd-export.zip",
                export_file_stem(&game.title)
            ))
            .build();

        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.deployd-export.zip");
        filter.add_pattern("*.zip");
        filter.set_name(Some("Deployd Export Bundle"));
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));

        let input_sender = sender.input_sender().clone();
        dialog.save(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let output_path = normalize_export_path(path);
                input_sender
                    .send(AppMsg::Migration(
                        crate::app::messages::MigrationMsg::ExportGameForSnapChosen {
                            game_id: game_id.clone(),
                            output_path,
                        },
                    ))
                    .ok();
            }
        });
    }

    pub(crate) fn handle_export_game_for_snap_chosen(
        &mut self,
        game_id: String,
        output_path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        if !self.shell.running_as_appimage || std::env::var_os("SNAP").is_some() {
            self.show_toast("Snap migration export is only available from the AppImage.");
            return;
        }
        if !utils::experimental_enabled() {
            self.show_toast("Snap migration export is experimental and disabled.");
            return;
        }
        let Some(game) = self.session.games.iter().find(|g| g.id == game_id).cloned() else {
            self.show_toast("Game is no longer managed.");
            return;
        };
        let Ok(cache_root) = self.cache_root_for(&game.id) else {
            self.push_notification("Cannot resolve this game's cache folder.");
            return;
        };

        self.begin_work(
            WorkKind::ExportingMigration,
            format!("Exporting {} for Snap...", game.title),
        );

        let request = ExportGameRequest {
            game,
            cache_root,
            downloads_dir: self.download.directory.clone(),
            output_path,
        };
        sender.oneshot_command(async move {
            AppCmdMsg::Migration(crate::app::messages::MigrationCmdMsg::GameExportedForSnap(
                export_game_bundle(request).await.map_err(|e| e.to_string()),
            ))
        });
    }

    pub(crate) fn handle_cmd_game_exported_for_snap(
        &mut self,
        result: Result<crate::core::migration_export::ExportGameResult, String>,
    ) {
        self.finish_work(WorkKind::ExportingMigration);
        match result {
            Ok(result) => {
                let mut message =
                    format!("Export bundle saved to {}", result.output_path.display());
                if !result.warnings.is_empty() {
                    message.push_str(&format!(" ({} warning(s))", result.warnings.len()));
                    for warning in result.warnings.iter().take(3) {
                        self.push_notification(&format!("Export warning: {warning}"));
                    }
                }
                self.show_toast(&message);
            }
            Err(e) => {
                self.push_notification(&format!("Export failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_preview_appimage_export(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if !game::is_snap() {
            self.show_toast("AppImage export preview is only available from the Snap.");
            return;
        }
        if !utils::experimental_enabled() {
            self.show_toast("AppImage export import is experimental and disabled.");
            return;
        }
        let dialog = gtk::FileDialog::builder()
            .title("Preview AppImage Export")
            .modal(true)
            .build();

        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.deployd-export.zip");
        filter.add_pattern("*.zip");
        filter.set_name(Some("Deployd Export Bundle"));
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));

        let input_sender = sender.input_sender().clone();
        dialog.open(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                input_sender
                    .send(AppMsg::Migration(
                        crate::app::messages::MigrationMsg::PreviewAppImageExportChosen(path),
                    ))
                    .ok();
            }
        });
    }

    pub(crate) fn handle_preview_appimage_export_chosen(
        &mut self,
        bundle_path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        if !game::is_snap() {
            self.show_toast("AppImage export preview is only available from the Snap.");
            return;
        }
        if !utils::experimental_enabled() {
            self.show_toast("AppImage export import is experimental and disabled.");
            return;
        }
        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Database not ready yet.");
            return;
        };

        self.begin_work(
            WorkKind::PreviewingMigration,
            "Previewing AppImage export...",
        );
        sender.oneshot_command(async move {
            AppCmdMsg::Migration(
                crate::app::messages::MigrationCmdMsg::AppImageExportPreviewed(
                    preview_import_bundle(&tracker, PreviewImportRequest { bundle_path })
                        .await
                        .map_err(|e| e.to_string()),
                ),
            )
        });
    }

    pub(crate) fn handle_cmd_appimage_export_previewed(
        &mut self,
        result: Result<PreviewImportResult, String>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.finish_work(WorkKind::PreviewingMigration);
        match result {
            Ok(result) => show_migration_preview_dialog(root, &result, sender),
            Err(e) => show_invalid_migration_preview_dialog(root, &e),
        }
    }

    pub(crate) fn handle_import_appimage_export(
        &mut self,
        bundle_path: PathBuf,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if !game::is_snap() {
            self.show_toast("AppImage export import is only available from the Snap.");
            return;
        }
        if !utils::experimental_enabled() {
            self.show_toast("AppImage export import is experimental and disabled.");
            return;
        }
        self.ui.pending_migration_import = Some(PendingMigrationImport {
            bundle_path,
            confirmed_game_path: None,
            confirmed_wine_prefix: None,
        });
        self.select_import_game_folder(root, sender);
    }

    pub(crate) fn handle_import_game_folder_chosen(
        &mut self,
        path: PathBuf,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if let Err(message) = snap::validate_selected_folder(&path, SelectedFolderKind::GameFolder)
        {
            self.push_notification(&message);
            return;
        }
        let Some(pending) = self.ui.pending_migration_import.as_mut() else {
            return;
        };
        pending.confirmed_game_path = Some(path);
        self.select_import_wine_prefix(root, sender);
    }

    pub(crate) fn handle_import_wine_prefix_chosen(
        &mut self,
        path: PathBuf,
        sender: &ComponentSender<Self>,
    ) {
        if let Err(message) = snap::validate_selected_folder(&path, SelectedFolderKind::WinePrefix)
        {
            self.push_notification(&message);
            return;
        }
        let Some(pending) = self.ui.pending_migration_import.as_mut() else {
            return;
        };
        pending.confirmed_wine_prefix = Some(path);
        self.start_appimage_export_import(sender);
    }

    fn start_appimage_export_import(&mut self, sender: &ComponentSender<Self>) {
        if !game::is_snap() {
            self.show_toast("AppImage export import is only available from the Snap.");
            return;
        }
        if !utils::experimental_enabled() {
            self.show_toast("AppImage export import is experimental and disabled.");
            return;
        }
        let Some(pending) = self.ui.pending_migration_import.take() else {
            return;
        };
        let Some(confirmed_game_path) = pending.confirmed_game_path else {
            self.show_toast("Import cancelled: game folder was not confirmed.");
            return;
        };
        let Some(confirmed_wine_prefix) = pending.confirmed_wine_prefix else {
            self.show_toast("Import cancelled: Wine prefix was not confirmed.");
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            self.push_notification("Database not ready yet.");
            return;
        };

        self.begin_work(WorkKind::ImportingMigration, "Importing AppImage export...");
        let request = ImportBundleRequest {
            bundle_path: pending.bundle_path,
            confirmed_game_path,
            confirmed_wine_prefix,
        };
        sender.oneshot_command(async move {
            AppCmdMsg::Migration(
                crate::app::messages::MigrationCmdMsg::AppImageExportImported(
                    import_bundle(&tracker, request)
                        .await
                        .map_err(|e| e.to_string()),
                ),
            )
        });
    }

    pub(crate) fn handle_cmd_appimage_export_imported(
        &mut self,
        result: Result<crate::core::migration_import::ImportBundleResult, String>,
        sender: &ComponentSender<Self>,
    ) {
        self.finish_work(WorkKind::ImportingMigration);
        match result {
            Ok(result) => {
                let imported_id = result.game.id.clone();
                self.download.all = result.download_entries;
                if self.session.games.iter().all(|game| game.id != imported_id) {
                    self.ui.game_model.append(&result.game.title);
                    self.session.games.push(result.game.clone());
                }
                if let Some(idx) = self
                    .session
                    .games
                    .iter()
                    .position(|game| game.id == imported_id)
                {
                    self.ui.game_dropdown.set_selected(idx as u32);
                    sender.input(AppMsg::Games(crate::app::messages::GamesMsg::GameSelected(
                        idx as u32,
                    )));
                }
                self.rebuild_downloads_view();
                if !result.warnings.is_empty() {
                    for warning in result.warnings.iter().take(3) {
                        self.push_notification(&format!("Import warning: {warning}"));
                    }
                }
                let mut message = format!(
                    "Imported {} ({} mod(s), {} profile(s))",
                    result.game.title, result.counts.mods, result.counts.profiles
                );
                if !result.warnings.is_empty() {
                    message.push_str(&format!(" ({} warning(s))", result.warnings.len()));
                }
                self.show_toast(&message);
            }
            Err(e) => {
                self.push_notification(&format!("Import failed: {e}"));
            }
        }
    }

    fn select_import_game_folder(
        &self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = gtk::FileDialog::builder()
            .title("Confirm Imported Game Folder")
            .modal(true)
            .build();
        let input_sender = sender.input_sender().clone();
        dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                input_sender
                    .send(AppMsg::Migration(
                        crate::app::messages::MigrationMsg::ImportGameFolderChosen(path),
                    ))
                    .ok();
            }
        });
    }

    fn select_import_wine_prefix(
        &self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = gtk::FileDialog::builder()
            .title("Confirm Imported Wine Prefix")
            .modal(true)
            .build();
        let input_sender = sender.input_sender().clone();
        dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                input_sender
                    .send(AppMsg::Migration(
                        crate::app::messages::MigrationMsg::ImportWinePrefixChosen(path),
                    ))
                    .ok();
            }
        });
    }

    pub(crate) fn handle_games_configured(
        &mut self,
        configs: Vec<crate::models::game::GameConfig>,
        hidden_ids: Vec<String>,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(dialog) = self.ui.game_setup_dialog.take() {
            dialog.widget().close();
        }

        if configs.is_empty() && hidden_ids.is_empty() {
            return;
        }

        let n_existing = self.ui.game_model.n_items();
        for _ in 0..n_existing {
            self.ui.game_model.remove(0);
        }
        self.session.games.clear();

        for cfg in &configs {
            self.ui.game_model.append(&cfg.game.title);
            self.session.games.push(cfg.game.clone());
        }

        if let Some(tracker) = self.session.tracker.clone() {
            let configs_for_db = configs.clone();
            let hidden_for_db = hidden_ids;
            let first_game_id = self.session.games.first().map(|g| g.id.clone());
            sender.oneshot_command(async move {
                for cfg in &configs_for_db {
                    let engine_str = match cfg.game.engine {
                        GameEngine::REDEngine => "redengine",
                        GameEngine::Eclipse => "eclipse",
                        GameEngine::Aurora => "aurora",
                        GameEngine::Bethesda => "bethesda",
                    };
                    let _ = tracker
                        .upsert_game(
                            &cfg.game.id,
                            &cfg.game.title,
                            &cfg.game.path,
                            &cfg.game.data_subdir,
                            engine_str,
                            cfg.game.wine_prefix.as_deref(),
                            cfg.custom,
                        )
                        .await;
                }
                for id in &hidden_for_db {
                    let _ = tracker.hide_game(id).await;
                }
                if let Some(id) = first_game_id {
                    let _ = tracker.set_setting("last_game_id", &id).await;
                }
                AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::GamesPersisted)
            });
        }

        self.session.pending_new_game_ids.clear();
        self.session.selected_game_idx = 0;
        self.ui.game_dropdown.set_selected(0);
    }

    pub(crate) fn handle_manage_games_closed(&mut self, sender: &ComponentSender<Self>) {
        let ids = std::mem::take(&mut self.session.pending_new_game_ids);
        if ids.is_empty() {
            return;
        }
        // Remove the new (unconfirmed) games from the in-memory list and the dropdown.
        for id in &ids {
            if let Some(idx) = self.session.games.iter().position(|g| &g.id == id) {
                self.session.games.remove(idx);
                self.ui.game_model.remove(idx as u32);
            }
        }
        if let Some(tracker) = self.session.tracker.clone() {
            sender.oneshot_command(async move {
                for id in &ids {
                    let _ = tracker.hide_game(id).await;
                }
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(Ok(())))
            });
        }
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
        // Persist the wizard-shown marker so we don't show it again.
        if let Some(ref tracker) = self.session.tracker {
            let t = tracker.clone();
            relm4::spawn(async move {
                let _ = t.set_setting("welcome_wizard_shown", "1").await;
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

    pub(crate) fn confirm_remove_game(
        &mut self,
        game_id: String,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading("Stop managing this game?")
            .body("Installed mods stay in the cache and can be kept or permanently deleted.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("remove", "Remove only");
        dialog.add_response("remove-delete", "Remove & delete mods");
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("remove-delete", adw::ResponseAppearance::Destructive);
        let s = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| match response {
            "remove" => {
                let _ = s.send(AppMsg::Games(
                    crate::app::messages::GamesMsg::RemoveGameConfirmed {
                        game_id: game_id.clone(),
                        delete_mods: false,
                    },
                ));
            }
            "remove-delete" => {
                let _ = s.send(AppMsg::Games(
                    crate::app::messages::GamesMsg::RemoveGameConfirmed {
                        game_id: game_id.clone(),
                        delete_mods: true,
                    },
                ));
            }
            _ => {}
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_remove_game(
        &mut self,
        game_id: String,
        delete_mods: bool,
        sender: &ComponentSender<Self>,
    ) {
        let Some(idx) = self.session.games.iter().position(|g| g.id == game_id) else {
            return;
        };

        self.session.games.remove(idx);
        self.ui.game_model.remove(idx as u32);

        if let Some(tracker) = self.session.tracker.clone() {
            let cache_root = self
                .cache_root_for(&game_id)
                .unwrap_or_else(|_| crate::utils::paths::cache_root().unwrap_or_default());
            sender.oneshot_command(async move {
                let _ = tracker.hide_game(&game_id).await;
                if delete_mods && let Ok(mods) = tracker.list_mods(&game_id).await {
                    for m in mods {
                        let _ = tracker.delete_plugins_for_mod(&m.id).await;
                        let _ = tracker.delete_mod_files(&m.id).await;
                        let _ = tracker.delete_mod(&m.id).await;
                        let cache = crate::utils::paths::mod_cache_dir_in(&cache_root, &m.id);
                        if cache.exists() {
                            let _ = std::fs::remove_dir_all(&cache);
                        }
                    }
                }
                AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(Ok(())))
            });
        }

        if self.session.games.is_empty() {
            return;
        }

        let new_idx = idx.min(self.session.games.len() - 1);
        self.session.selected_game_idx = new_idx;
        self.ui.game_dropdown.set_selected(new_idx as u32);
        sender.input(AppMsg::Games(crate::app::messages::GamesMsg::GameSelected(
            new_idx as u32,
        )));
    }
}

fn show_migration_preview_dialog(
    root: &adw::ApplicationWindow,
    result: &PreviewImportResult,
    sender: &ComponentSender<App>,
) {
    let status = match result.conflict {
        PreviewConflict::NewGame => "Ready to preview",
        PreviewConflict::ExistingGame => "Already managed in Snap",
    };
    let counts = &result.counts;
    let mut body = format!(
        "{}\n\nGame ID: {}\nSource Deployd: {}\nStatus: {}\n\nContents:\n- Mods: {}\n- Plugins: {}\n- Profiles: {}\n- Tools: {}\n- Downloads: {}\n- Cache files: {}\n- Vanilla backups: {}\n- Save snapshots: {}\n\nNeeds confirmation later:\n{}",
        result.manifest.game_title,
        result.manifest.game_id,
        result.manifest.deployd_version,
        status,
        counts.mods,
        counts.plugins,
        counts.profiles,
        counts.tools,
        counts.downloads,
        counts.cache_files,
        counts.vanilla_backups,
        counts.save_snapshots,
        result
            .validation_items
            .iter()
            .map(validation_item_label)
            .collect::<Vec<_>>()
            .join("\n")
    );

    if result.conflict == PreviewConflict::ExistingGame {
        body.push_str(
            "\n\nThis game already exists in the Snap. A later import phase will skip it by default unless an explicit merge or replace choice is implemented.",
        );
    }

    if !result.warnings.is_empty() {
        body.push_str("\n\nWarnings:");
        for warning in result.warnings.iter().take(5) {
            body.push_str("\n- ");
            body.push_str(warning);
        }
        if result.warnings.len() > 5 {
            body.push_str(&format!(
                "\n- {} more warning(s)",
                result.warnings.len() - 5
            ));
        }
    }

    let dialog = adw::AlertDialog::builder()
        .heading("AppImage Export Preview")
        .body(body)
        .build();
    dialog.add_response("close", "Close");
    if result.conflict == PreviewConflict::NewGame {
        dialog.add_response("import", "Import");
        dialog.set_response_appearance("import", adw::ResponseAppearance::Suggested);
    }
    dialog.set_close_response("close");
    let bundle_path = result.bundle_path.clone();
    let input_sender = sender.input_sender().clone();
    dialog.connect_response(None, move |_, response| {
        if response == "import" {
            input_sender
                .send(AppMsg::Migration(
                    crate::app::messages::MigrationMsg::ImportAppImageExport(bundle_path.clone()),
                ))
                .ok();
        }
    });
    dialog.present(Some(root));
}

fn show_invalid_migration_preview_dialog(root: &adw::ApplicationWindow, error: &str) {
    let dialog = adw::AlertDialog::builder()
        .heading("Invalid Export Bundle")
        .body(format!(
            "Deployd could not preview this export bundle.\n\n{error}"
        ))
        .build();
    dialog.add_response("close", "Close");
    dialog.set_close_response("close");
    dialog.present(Some(root));
}

fn validation_item_label(item: &ValidationItem) -> &'static str {
    match item {
        ValidationItem::NeedsGameFolderConfirmation => "- Game folder",
        ValidationItem::NeedsWinePrefixConfirmation => "- Wine prefix",
        ValidationItem::ToolsNeedSnapRuntimeRebind => "- External tools through Snap Wine runtime",
    }
}

fn export_file_stem(title: &str) -> String {
    let stem: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let collapsed = stem
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if collapsed.is_empty() {
        "deployd-game".to_string()
    } else {
        collapsed
    }
}

fn normalize_export_path(mut path: PathBuf) -> PathBuf {
    if path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".deployd-export.zip"))
    {
        return path;
    }
    path.set_extension("deployd-export.zip");
    path
}
