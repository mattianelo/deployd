use std::collections::{HashMap, HashSet};

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use super::App;
use crate::app::messages::{AppCmdMsg, AppMsg, PrepareResultMsg};
use crate::app::session::load_game_data;
use crate::app::state::{InstallIdentity, InstallStage};
use crate::app::types::PendingInstall;
use crate::core::installer::AddResult;

impl App {
    pub(crate) fn handle_cmd_mod_prepared(
        &mut self,
        identity: &InstallIdentity,
        result: Result<PrepareResultMsg, String>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if !self.install.accepts(identity) {
            return;
        }
        match result {
            Ok(PrepareResultMsg::Normal {
                file_list,
                stripped_wrapper,
                tmp_dir,
                mod_name,
                archive_hash,
                archive_path,
            }) => {
                let Some(game) = self.selected_game().cloned() else {
                    self.install.set_stage(InstallStage::Failed);
                    self.finish_current_work();
                    self.install.reinstalling = false;
                    self.install.nexus_ids = None;
                    self.push_notification("Add failed: no game is selected");
                    return;
                };
                self.install.pending = Some(PendingInstall {
                    tmp_dir,
                    mod_name: mod_name.clone(),
                    game,
                    file_list: Some(file_list),
                    stripped_wrapper,
                    fomod_config_path: None,
                    fomod_config: None,
                    nexus_ids: self.install.nexus_ids.take(),
                    archive_hash,
                    archive_path,
                    file_targets: HashMap::new(),
                    excluded_files: HashSet::new(),
                });
                let hash_existing = self
                    .install
                    .pending
                    .as_ref()
                    .and_then(|p| p.archive_hash.as_deref())
                    .and_then(|h| self.find_mod_by_archive_hash(h));
                let nexus_ids = self
                    .install
                    .pending
                    .as_ref()
                    .and_then(|p| p.nexus_ids.as_ref())
                    .map(|n| (n.mod_id, n.file_id));
                let existing = hash_existing.or_else(|| {
                    nexus_ids.and_then(|(mid, fid)| self.find_installed_mod_by_nexus_id(mid, fid))
                });
                if let Some((old_mod_id, old_mod_name, old_priority)) = existing {
                    if self.install.reinstalling {
                        self.install.reinstalling = false;
                        self.handle_open_pre_install_dialog_replacing(
                            old_mod_id,
                            old_priority,
                            root,
                            sender,
                        );
                    } else {
                        let body = format!(
                            "\"{old_mod_name}\" is already installed. Replace it or install alongside?"
                        );
                        let dialog = adw::AlertDialog::builder()
                            .heading("Mod Already Installed")
                            .body(&body)
                            .build();
                        dialog.add_response("cancel", "Cancel");
                        dialog.add_response("alongside", "Install Alongside");
                        dialog.add_response("replace", "Replace");
                        dialog.set_default_response(Some("alongside"));
                        dialog.set_close_response("cancel");
                        dialog.set_response_appearance(
                            "replace",
                            adw::ResponseAppearance::Destructive,
                        );
                        let input_sender = sender.input_sender().clone();
                        dialog.connect_response(None, move |_, response| {
                            let _ = match response {
                                "alongside" => input_sender.send(AppMsg::Install(
                                    crate::app::messages::InstallMsg::OpenPreInstallDialog,
                                )),
                                "replace" => input_sender.send(AppMsg::Install(
                                    crate::app::messages::InstallMsg::OpenPreInstallDialogReplacing(
                                        old_mod_id.clone(),
                                        old_priority,
                                    ),
                                )),
                                _ => input_sender.send(AppMsg::Install(
                                    crate::app::messages::InstallMsg::PreInstallCancelled,
                                )),
                            };
                        });
                        self.install.set_stage(InstallStage::AwaitingPreInstall);
                        self.finish_current_work();
                        dialog.present(Some(root));
                    }
                } else {
                    self.install.reinstalling = false;
                    self.open_pre_install_dialog(root, sender);
                }
            }
            Ok(PrepareResultMsg::Fomod {
                config,
                config_path,
                tmp_dir,
                mod_name,
                archive_hash,
                archive_path,
            }) => {
                let Some(game) = self.selected_game().cloned() else {
                    self.install.set_stage(InstallStage::Failed);
                    self.finish_current_work();
                    self.install.reinstalling = false;
                    self.install.nexus_ids = None;
                    self.push_notification("Add failed: no game is selected");
                    return;
                };
                self.install.pending = Some(PendingInstall {
                    tmp_dir,
                    mod_name: mod_name.clone(),
                    game,
                    file_list: None,
                    stripped_wrapper: None,
                    fomod_config_path: Some(config_path),
                    fomod_config: Some(config),
                    nexus_ids: self.install.nexus_ids.take(),
                    archive_hash,
                    archive_path,
                    file_targets: HashMap::new(),
                    excluded_files: HashSet::new(),
                });
                let hash_existing = self
                    .install
                    .pending
                    .as_ref()
                    .and_then(|p| p.archive_hash.as_deref())
                    .and_then(|h| self.find_mod_by_archive_hash(h));
                let nexus_ids = self
                    .install
                    .pending
                    .as_ref()
                    .and_then(|p| p.nexus_ids.as_ref())
                    .map(|n| (n.mod_id, n.file_id));
                let existing = hash_existing.or_else(|| {
                    nexus_ids.and_then(|(mid, fid)| self.find_installed_mod_by_nexus_id(mid, fid))
                });
                if let Some((old_mod_id, old_mod_name, old_priority)) = existing {
                    if self.install.reinstalling {
                        self.install.reinstalling = false;
                        // Set up replace context now; open dialog after fetching previous selections.
                        let old_name = self.mod_name_for_id(&old_mod_id);
                        if let Some(pending) = &mut self.install.pending {
                            pending.mod_name = old_name;
                        }
                        self.install.replacement = Some((old_mod_id.clone(), old_priority));
                        self.install.fetched_name = None;
                        self.install.file_id_needed = None;
                        let tracker = self.session.tracker.clone();
                        sender.oneshot_command(async move {
                            let selections = if let Some(t) = tracker {
                                t.get_fomod_selections(&old_mod_id)
                                    .await
                                    .ok()
                                    .flatten()
                                    .and_then(|json| {
                                        let raw: Option<Vec<Vec<Vec<usize>>>> =
                                            serde_json::from_str(&json).ok();
                                        raw.map(|steps| {
                                            steps
                                                .into_iter()
                                                .map(|step| {
                                                    step.into_iter()
                                                        .map(|g| {
                                                            g.into_iter()
                                                                .collect::<std::collections::HashSet<usize>>()
                                                        })
                                                        .collect::<Vec<_>>()
                                                })
                                                .collect::<Vec<_>>()
                                        })
                                    })
                            } else {
                                None
                            };
                            AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::FomodSelectionsLoaded(selections))
                        });
                    } else {
                        let body = format!(
                            "\"{old_mod_name}\" is already installed. Replace it or install alongside?"
                        );
                        let dialog = adw::AlertDialog::builder()
                            .heading("Mod Already Installed")
                            .body(&body)
                            .build();
                        dialog.add_response("cancel", "Cancel");
                        dialog.add_response("alongside", "Install Alongside");
                        dialog.add_response("replace", "Replace");
                        dialog.set_default_response(Some("alongside"));
                        dialog.set_close_response("cancel");
                        dialog.set_response_appearance(
                            "replace",
                            adw::ResponseAppearance::Destructive,
                        );
                        let input_sender = sender.input_sender().clone();
                        dialog.connect_response(None, move |_, response| {
                            let _ = match response {
                                "alongside" => input_sender.send(AppMsg::Install(
                                    crate::app::messages::InstallMsg::OpenPreInstallDialog,
                                )),
                                "replace" => input_sender.send(AppMsg::Install(
                                    crate::app::messages::InstallMsg::OpenPreInstallDialogReplacing(
                                        old_mod_id.clone(),
                                        old_priority,
                                    ),
                                )),
                                _ => input_sender.send(AppMsg::Install(
                                    crate::app::messages::InstallMsg::PreInstallCancelled,
                                )),
                            };
                        });
                        self.install.set_stage(InstallStage::AwaitingPreInstall);
                        self.finish_current_work();
                        dialog.present(Some(root));
                    }
                } else {
                    self.install.reinstalling = false;
                    self.open_pre_install_dialog(root, sender);
                }
            }
            Err(e) => {
                self.install.set_stage(InstallStage::Failed);
                self.finish_current_work();
                self.install.reinstalling = false;
                self.install.fomod_selections = None;
                if let Some(dl_id) = self.install.active_download_id.take() {
                    self.update_download_status(
                        &dl_id,
                        crate::models::download::DownloadStatus::Failed,
                        &format!("Extraction failed: {e}"),
                    );
                    if let Some(tracker) = self.session.tracker.clone()
                        && let Some(entry) = self.download.all.iter().find(|e| e.id == dl_id)
                    {
                        let entry = entry.clone();
                        sender.oneshot_command(async move {
                            let result = tracker
                                .save_download_entry(&entry)
                                .await
                                .map_err(|error| error.to_string());
                            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(
                                result,
                            ))
                        });
                    }
                }
                self.push_notification(&format!("Add failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_mod_added(
        &mut self,
        identity: &InstallIdentity,
        result: Result<AddResult, String>,
        was_replace: bool,
        sender: &ComponentSender<Self>,
    ) {
        if !self.install.accepts(identity) {
            return;
        }
        self.install.set_stage(if result.is_ok() {
            InstallStage::Succeeded
        } else {
            InstallStage::Failed
        });
        self.finish_current_work();

        let maybe_archive_hash = match &result {
            Ok(add_result) => add_result.mod_entry.archive_hash.clone(),
            Err(_) => None,
        };
        let metadata_dl_id = self.install.active_download_id.clone();

        if let Some(dl_id) = self.install.active_download_id.take() {
            let (status, msg) = if result.is_ok() {
                (
                    crate::models::download::DownloadStatus::Installed,
                    "Installed",
                )
            } else {
                (
                    crate::models::download::DownloadStatus::Failed,
                    "Install failed",
                )
            };
            self.update_download_status(&dl_id, status, msg);
            if let (Some(hash), Some(entry)) = (
                &maybe_archive_hash,
                self.download.all.iter_mut().find(|e| e.id == dl_id),
            ) {
                entry.archive_hash = Some(hash.clone());
            }
            if let Some(tracker) = self.session.tracker.clone()
                && let Some(entry) = self.download.all.iter().find(|e| e.id == dl_id)
            {
                let entry = entry.clone();
                sender.oneshot_command(async move {
                    let result = tracker
                        .save_download_entry(&entry)
                        .await
                        .map_err(|error| error.to_string());
                    AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
                });
            }
        }

        match result {
            Ok(add_result) => {
                let count = add_result.files_cached;
                let plugins = add_result.plugins_found.len();
                for warning in &add_result.warnings {
                    self.push_notification(warning);
                }
                self.shell.needs_deploy = true;
                self.auto_save_profile(sender);

                // If the download entry already has resolved metadata, write the version to
                // the mods table and reload in a single chained command so the read cannot
                // race ahead of the write. Otherwise a plain reload is sufficient — the
                // metadata-fetch path will update version/author and trigger its own reload.
                let (version_from_dl, author_from_dl): (Option<String>, Option<String>) =
                    metadata_dl_id
                        .as_ref()
                        .and_then(|id| self.download.all.iter().find(|e| &e.id == id))
                        .filter(|e| e.metadata_fetched)
                        .map(|e| (e.version.clone(), e.author.clone()))
                        .unwrap_or((None, None));

                let already_fetched = version_from_dl.is_some()
                    || metadata_dl_id
                        .as_ref()
                        .and_then(|id| self.download.all.iter().find(|e| &e.id == id))
                        .is_some_and(|e| e.metadata_fetched);

                if let (Some(tracker), Some(game)) =
                    (self.session.tracker.clone(), self.selected_game().cloned())
                {
                    let mod_id = add_result.mod_entry.id.clone();
                    sender.oneshot_command(async move {
                        let result = async {
                            if let Some(ref version) = version_from_dl {
                                tracker
                                    .set_mod_installed_version(&mod_id, version)
                                    .await
                                    .map_err(|error| error.to_string())?;
                            }
                            if let Some(ref author) = author_from_dl {
                                tracker
                                    .set_mod_author(&mod_id, author)
                                    .await
                                    .map_err(|error| error.to_string())?;
                            }
                            load_game_data(
                                &tracker,
                                &game,
                                crate::app::session::GameLoadMode::Refresh,
                            )
                            .await
                        }
                        .await;
                        AppCmdMsg::Games(crate::app::messages::GamesCmdMsg::ModsLoaded(
                            result, true,
                        ))
                    });
                } else {
                    self.reload_mods(sender);
                }

                let msg = if was_replace {
                    "Mod replaced — deploy to update game files".to_string()
                } else {
                    let mut m = format!("Added {count} files");
                    if plugins > 0 {
                        m.push_str(&format!(", {plugins} plugin(s)"));
                    }
                    m
                };
                self.show_toast(&msg);
                if let Some(dialog) = self.ui.absorb_dialog.take() {
                    dialog.widget().destroy();
                }

                if !already_fetched
                    && let (Some(nexus_mod_id), Some(nexus_domain)) = (
                        add_result.mod_entry.nexus_mod_id,
                        add_result.mod_entry.nexus_domain.as_deref(),
                    )
                {
                    let Some(tracker) = self.session.tracker.clone() else {
                        self.push_notification(
                            "Nexus metadata was not refreshed because the database is unavailable",
                        );
                        return;
                    };
                    let mod_id = add_result.mod_entry.id.clone();
                    let domain = nexus_domain.to_string();
                    let nexus_file_id = add_result.mod_entry.nexus_file_id;
                    let dl_id_for_metadata = metadata_dl_id;
                    sender.oneshot_command(async move {
                        let result: Result<
                            (String, String, String, String, Option<String>),
                            String,
                        > = async {
                            let api_key = tracker
                                .get_setting("nexus_api_key")
                                .await
                                .map_err(|e| e.to_string())?
                                .ok_or("No API key")?;
                            let client = crate::core::nexus_api::NexusClient::new(api_key);
                            let (info, _rate_limits) = client
                                .get_mod_info(&domain, nexus_mod_id)
                                .await
                                .map_err(|e| e.to_string())?;
                            tracker
                                .update_mod_nexus_metadata(
                                    &mod_id,
                                    &info.version,
                                    &info.author,
                                    info.summary.as_deref().unwrap_or(""),
                                )
                                .await
                                .map_err(|e| e.to_string())?;
                            let (installed_version, nexus_file_name) =
                                if let Some(fid) = nexus_file_id.filter(|&f| f != 0) {
                                    let (files_resp, _) = client
                                        .get_mod_files(&domain, nexus_mod_id)
                                        .await
                                        .map_err(|e| e.to_string())?;
                                    let matched =
                                        files_resp.files.into_iter().find(|f| f.file_id == fid);
                                    let version = matched
                                        .as_ref()
                                        .and_then(|f| f.version.clone())
                                        .unwrap_or_else(|| info.version.clone());
                                    let file_name = matched.map(|f| f.name);
                                    (version, file_name)
                                } else {
                                    (info.version.clone(), None)
                                };
                            tracker
                                .set_mod_installed_version(&mod_id, &installed_version)
                                .await
                                .map_err(|e| e.to_string())?;
                            Ok((
                                mod_id,
                                installed_version,
                                info.author,
                                info.name,
                                nexus_file_name,
                            ))
                        }
                        .await;
                        AppCmdMsg::Downloads(
                            crate::app::messages::DownloadsCmdMsg::NexusMetadataFetched(
                                dl_id_for_metadata,
                                result,
                            ),
                        )
                    });
                }
            }
            Err(e) => {
                self.push_notification(&format!("Add failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_mod_merged(
        &mut self,
        identity: &InstallIdentity,
        result: Result<(String, usize), String>,
        sender: &ComponentSender<Self>,
    ) {
        if !self.install.accepts(identity) {
            return;
        }
        self.install.set_stage(if result.is_ok() {
            InstallStage::Succeeded
        } else {
            InstallStage::Failed
        });
        self.finish_current_work();

        if let Some(dl_id) = self.install.active_download_id.take() {
            let (status, msg) = if result.is_ok() {
                (
                    crate::models::download::DownloadStatus::Installed,
                    "Installed",
                )
            } else {
                (
                    crate::models::download::DownloadStatus::Failed,
                    "Merge failed",
                )
            };
            self.update_download_status(&dl_id, status, msg);
            if let Some(tracker) = self.session.tracker.clone()
                && let Some(entry) = self.download.all.iter().find(|e| e.id == dl_id)
            {
                let entry = entry.clone();
                sender.oneshot_command(async move {
                    let result = tracker
                        .save_download_entry(&entry)
                        .await
                        .map_err(|error| error.to_string());
                    AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::PrioritySaved(result))
                });
            }
        }

        match result {
            Ok((mod_name, count)) => {
                self.shell.needs_deploy = true;
                self.auto_save_profile(sender);
                self.reload_mods(sender);
                self.show_toast(&format!(
                    "Merged {count} file(s) into '{mod_name}' — deploy to update game files"
                ));
                if let Some(dialog) = self.ui.absorb_dialog.take() {
                    dialog.widget().destroy();
                }
            }
            Err(e) => {
                self.push_notification(&format!("Merge failed: {e}"));
            }
        }
    }
}
