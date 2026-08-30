use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use adw::prelude::*;
use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::installer::{self, AddResult, PrepareResult};
use crate::dlog;
use crate::models::download::DownloadEntry;
use crate::ui::fomod_dialog::{
    FomodDialog, FomodDialogInit, FomodDialogOutput, default_fomod_selections,
};
use crate::utils::{fomod_resolver, paths};

use super::App;
use super::messages::{AppCmdMsg, AppMsg, PrepareResultMsg};
use super::progress::throttled_install_progress;
use super::state::InstallStage;
use super::types::{FileIdNeeded, WorkKind};

mod dialogs;
mod results;

fn next_unresolved_sibling_id(
    downloads: &[DownloadEntry],
    resolved_download_id: &str,
) -> Option<String> {
    let resolved_mod_id = downloads
        .iter()
        .find(|entry| entry.id == resolved_download_id)
        .and_then(|entry| entry.nexus_ids.as_ref())
        .map(|ids| ids.mod_id)?;

    downloads
        .iter()
        .find(|entry| {
            entry.id != resolved_download_id
                && entry.nexus_ids.as_ref().map(|ids| ids.mod_id) == Some(resolved_mod_id)
                && entry.nexus_ids.as_ref().is_some_and(|ids| ids.file_id == 0)
                && !entry.metadata_fetched
        })
        .map(|entry| entry.id.clone())
}

fn parse_fomod_selections(json: &str) -> Option<Vec<Vec<HashSet<usize>>>> {
    let raw: Vec<Vec<Vec<usize>>> = serde_json::from_str(json).ok()?;
    Some(
        raw.into_iter()
            .map(|step| {
                step.into_iter()
                    .map(|group| group.into_iter().collect())
                    .collect()
            })
            .collect(),
    )
}

impl App {
    pub(crate) fn handle_open_pre_install_dialog_replacing_request(
        &mut self,
        id: String,
        priority: i32,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if self
            .install
            .pending
            .as_ref()
            .is_none_or(|pending| pending.fomod_config.is_none())
        {
            self.handle_open_pre_install_dialog_replacing(id, priority, root, sender);
            return;
        }

        let old_name = self.mod_name_for_id(&id);
        if let Some(pending) = &mut self.install.pending {
            pending.mod_name = old_name;
        }
        self.install.replacement = Some((id.clone(), priority));
        self.install.fetched_name = None;
        self.install.file_id_needed = None;
        let tracker = self.session.tracker.clone();
        sender.oneshot_command(async move {
            let selections = if let Some(tracker) = tracker {
                tracker
                    .get_fomod_selections(&id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|json| parse_fomod_selections(&json))
            } else {
                None
            };
            AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::FomodSelectionsLoaded(
                selections,
            ))
        });
    }

    pub(crate) fn handle_cmd_fomod_selections_loaded(
        &mut self,
        selections: Option<Vec<Vec<HashSet<usize>>>>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.install.fomod_selections = selections;
        self.open_pre_install_dialog(root, sender);
    }

    pub(crate) fn handle_show_file_id_dialog(
        &mut self,
        download_id: String,
        mod_id: i64,
        domain: String,
        partial_name: Option<String>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(name) = partial_name {
            self.install.fetched_name = Some(name);
        }
        self.install.file_id_needed = Some(FileIdNeeded {
            download_id,
            mod_id,
            domain,
        });
        self.show_file_id_dialog(root, sender);
    }

    pub(crate) fn handle_cmd_pending_metadata_fetched(
        &mut self,
        identity: &super::state::InstallIdentity,
        name: String,
    ) {
        if !self.install.accepts(identity) {
            return;
        }
        self.install.fetched_name = Some(name);
    }

    pub(crate) fn handle_cmd_pending_file_name_unresolved(
        &mut self,
        identity: &super::state::InstallIdentity,
        partial_name: String,
        download_id: String,
        mod_id: i64,
        domain: String,
    ) {
        if !self.install.accepts(identity) {
            return;
        }
        self.install.fetched_name = Some(partial_name);
        self.install.file_id_needed = Some(FileIdNeeded {
            download_id,
            mod_id,
            domain,
        });
    }

    pub(crate) fn handle_cmd_file_id_fetched(
        &mut self,
        combined_name: Option<String>,
        download_id: Option<String>,
        version: Option<String>,
        file_id: Option<i64>,
        sender: &ComponentSender<Self>,
    ) {
        self.finish_work(WorkKind::FetchingMetadata);
        self.install.file_id_needed = None;
        if let Some(download_id) = download_id {
            self.finish_download_metadata_fetch(&download_id);
            if let Some(name) = combined_name {
                self.handle_download_name_resolved(
                    download_id.clone(),
                    name,
                    None,
                    None,
                    false,
                    file_id,
                    version,
                    None,
                    sender,
                );
            }
            self.show_toast("Metadata updated");
            if let Some(next_id) = next_unresolved_sibling_id(&self.download.all, &download_id) {
                self.start_nexus_metadata_fetch(next_id, sender);
            }
        } else {
            if let Some(name) = combined_name {
                self.install.fetched_name = Some(name);
            }
            let _ = sender.input_sender().send(AppMsg::Install(
                crate::app::messages::InstallMsg::OpenPreInstallDialog,
            ));
        }
    }

    pub(crate) fn handle_install_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = gtk::FileDialog::builder()
            .title("Select Mod Archive")
            .modal(true)
            .build();

        let filter = gtk::FileFilter::new();
        filter.add_pattern("*.zip");
        filter.add_pattern("*.7z");
        filter.add_pattern("*.rar");
        filter.add_pattern("*.dazip");
        filter.set_name(Some("Mod Archives (zip, 7z, rar, dazip)"));
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        dialog.set_filters(Some(&filters));

        let input_sender = sender.input_sender().clone();
        dialog.open(Some(root), None::<&gio::Cancellable>, move |result| {
            if let Ok(file) = result
                && let Some(path) = file.path()
            {
                let _ = input_sender.send(AppMsg::Install(
                    crate::app::messages::InstallMsg::FileChosen(path),
                ));
            }
        });
    }

    pub(crate) fn handle_file_chosen(&mut self, path: PathBuf, sender: &ComponentSender<Self>) {
        let mod_name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        let game_id = self
            .selected_game()
            .map(|game| game.id.clone())
            .unwrap_or_default();
        let identity = self.install.begin(game_id, None);
        self.install.set_stage(InstallStage::PreparingArchive);
        self.begin_work(WorkKind::PreparingArchive, format!("Hashing {mod_name}..."));

        let extract_sender = sender.input_sender().clone();
        let on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>> = Some(
            throttled_install_progress(extract_sender, identity.clone(), "Extracting"),
        );
        let processing_sender = sender.input_sender().clone();
        let processing_identity = identity.clone();
        let on_processing: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
            let _ = processing_sender.send(AppMsg::Install(
                crate::app::messages::InstallMsg::InstallProgress(
                    processing_identity,
                    1.0,
                    "Processing mod structure...".to_string(),
                ),
            ));
        }));

        sender.oneshot_command(async move {
            let result: Result<PrepareResultMsg, String> = async {
                let timing_start = std::time::Instant::now();
                let hash_path = path.clone();
                let archive_path = Some(path.to_string_lossy().to_string());
                let archive_hash = tokio::task::spawn_blocking(move || {
                    crate::core::archive::hash_archive_file(&hash_path).ok()
                })
                .await
                .unwrap_or(None);
                crate::app::timing::log_phase(
                    "install.hash_archive",
                    "manual",
                    timing_start,
                    Some(1),
                );

                let timing_start = std::time::Instant::now();
                let archive_label = path.display().to_string();
                let prepare = installer::prepare_mod(&path, on_extract_progress, on_processing)
                    .await
                    .map_err(|e| format!("{e:#}\nArchive: {archive_label}"))?;
                crate::app::timing::log_phase(
                    "install.prepare_archive",
                    "manual",
                    timing_start,
                    None,
                );
                match prepare {
                    PrepareResult::Normal {
                        file_list,
                        stripped_wrapper,
                        tmp_dir,
                    } => Ok(PrepareResultMsg::Normal {
                        file_list,
                        stripped_wrapper,
                        tmp_dir,
                        mod_name,
                        archive_hash,
                        archive_path,
                    }),
                    PrepareResult::Fomod {
                        config,
                        config_path,
                        tmp_dir,
                    } => Ok(PrepareResultMsg::Fomod {
                        config,
                        config_path,
                        tmp_dir,
                        mod_name,
                        archive_hash,
                        archive_path,
                    }),
                }
            }
            .await;
            AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::ModPrepared(
                identity,
                Box::new(result),
            ))
        });
    }

    pub(crate) fn handle_pre_install_confirmed(
        &mut self,
        edited_name: String,
        file_targets: HashMap<String, crate::models::mod_entry::InstallTarget>,
        excluded_files: HashSet<String>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(dialog) = self.ui.pre_install_dialog.take() {
            dialog.widget().destroy();
        }
        let Some(pending) = self.install.pending.as_mut() else {
            return;
        };
        pending.mod_name = edited_name;
        pending.file_targets = file_targets;
        pending.excluded_files = excluded_files;

        if let Some(config) = pending.fomod_config.take() {
            let active_plugin_files: HashSet<String> = {
                let guard = self.plugins.rows.guard();
                (0..guard.len())
                    .filter_map(|i| guard.get(i))
                    .map(|row| row.plugin.filename.to_lowercase())
                    .collect()
            };

            if fomod_resolver::needs_user_input(&config) {
                let extracted_root = pending
                    .fomod_config_path
                    .as_deref()
                    .and_then(|p| p.parent())
                    .and_then(|p| p.parent())
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| pending.tmp_dir.path().to_path_buf());
                self.install.set_stage(InstallStage::AwaitingFomod);
                self.ui.fomod_dialog = Some(
                    FomodDialog::builder()
                        .transient_for(root)
                        .launch(FomodDialogInit {
                            config,
                            extracted_root,
                            active_plugin_files,
                            previous_selections: self.install.fomod_selections.take(),
                        })
                        .forward(sender.input_sender(), |output| match output {
                            FomodDialogOutput::Confirmed(sel) => AppMsg::Install(
                                crate::app::messages::InstallMsg::FomodConfirmed(sel),
                            ),
                            FomodDialogOutput::Cancelled => {
                                AppMsg::Install(crate::app::messages::InstallMsg::FomodCancelled)
                            }
                        }),
                );
                return;
            }
            // No user choices needed — auto-install with defaults
            sender.input_sender().emit(AppMsg::Install(
                crate::app::messages::InstallMsg::FomodConfirmed(default_fomod_selections(
                    &config,
                    &active_plugin_files,
                )),
            ));
            return;
        }

        let conflict = {
            let mod_name = self
                .install
                .pending
                .as_ref()
                .map(|p| p.mod_name.clone())
                .unwrap_or_default();
            if self.install.replacement.is_none() {
                self.find_mod_id_and_priority_by_name(&mod_name)
            } else {
                None
            }
        };

        if let Some((existing_id, existing_priority)) = conflict {
            let mod_name = self
                .install
                .pending
                .as_ref()
                .map(|p| p.mod_name.clone())
                .unwrap_or_default();
            let body = format!(
                "'{}' already exists. Merge files into it, replace it, or create a separate mod?",
                mod_name
            );
            let dialog = adw::AlertDialog::builder()
                .heading("Mod name already exists")
                .body(&body)
                .build();
            dialog.add_response("create", "Create New");
            dialog.add_response("merge", "Merge");
            dialog.add_response("replace", "Replace");
            dialog.set_default_response(Some("merge"));
            dialog.set_close_response("create");
            dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
            let input_sender = sender.input_sender().clone();
            dialog.connect_response(None, move |_, response| {
                let msg = match response {
                    "merge" => AppMsg::Install(crate::app::messages::InstallMsg::PreInstallMerge(
                        existing_id.clone(),
                    )),
                    "replace" => {
                        AppMsg::Install(crate::app::messages::InstallMsg::PreInstallReplace(
                            existing_id.clone(),
                            existing_priority,
                        ))
                    }
                    _ => AppMsg::Install(crate::app::messages::InstallMsg::PreInstallCreateNew),
                };
                let _ = input_sender.send(msg);
            });
            dialog.present(Some(root));
            return;
        }

        self.proceed_with_install(sender);
    }

    /// Shared install path: takes `pending_install` and runs the actual add-mod async task,
    /// honoring `pending_replace_mod_id` if set.
    fn proceed_with_install(&mut self, sender: &ComponentSender<Self>) {
        let pending = match self.install.pending.take() {
            Some(p) => p,
            None => return,
        };
        let Some(original_file_list) = pending.file_list else {
            return;
        };
        // Re-scan the staging dir to pick up any modifications the user made
        // after initial extraction (e.g. moving files into a system/ subfolder).
        let file_list = {
            let rescanned = installer::rescan_staged_files(
                pending.tmp_dir.path(),
                pending.stripped_wrapper.as_deref(),
            );
            if rescanned.is_empty() {
                original_file_list
            } else {
                rescanned
            }
        };
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let replace_info = self.install.replacement.take();
        let was_replace = replace_info.is_some();
        let cache_root = match self.cache_root_for(&pending.game.id) {
            Ok(r) => r,
            Err(e) => {
                self.push_notification(&format!("Cannot resolve cache dir: {e}"));
                return;
            }
        };

        self.install.set_stage(InstallStage::Committing);
        self.begin_work(
            WorkKind::Installing,
            format!("Installing {}...", pending.mod_name),
        );
        if let Some(dl_id) = self.install.active_download_id.clone() {
            self.update_download_status(
                &dl_id,
                crate::models::download::DownloadStatus::Extracting,
                "Caching files...",
            );
        }

        let input_sender = sender.input_sender().clone();
        let Some(identity) = self.install.identity() else {
            self.install.set_stage(InstallStage::Failed);
            self.finish_current_work();
            return;
        };
        sender.oneshot_command(async move {
            let result: Result<AddResult, String> = async {
                let archive_label = pending
                    .archive_path
                    .clone()
                    .unwrap_or_else(|| pending.mod_name.clone());
                let progress_sender = input_sender.clone();
                let on_progress: Option<Box<dyn Fn(usize, usize) + Send>> = Some(
                    throttled_install_progress(progress_sender, identity.clone(), "Caching"),
                );
                let timing_start = std::time::Instant::now();
                let result = installer::add_mod_with_file_list(installer::AddModRequest {
                    file_list,
                    game: &pending.game,
                    mod_name: &pending.mod_name,
                    tracker: &tracker,
                    cache_root: &cache_root,
                    nexus_ids: pending.nexus_ids,
                    archive_hash: pending.archive_hash,
                    archive_path: pending.archive_path,
                    file_targets: pending.file_targets,
                    stripped_wrapper: pending.stripped_wrapper,
                    excluded_files: &pending.excluded_files,
                    on_progress,
                })
                .await
                .map_err(|e| format!("{e:#}\nArchive: {archive_label}"))?;
                crate::app::timing::log_phase(
                    "install.cache_files",
                    &pending.game.id,
                    timing_start,
                    Some(result.files_cached),
                );
                drop(pending.tmp_dir);
                if let Some((old_id, old_priority)) = replace_info {
                    let _ = tracker
                        .update_priorities(&[(result.mod_entry.id.clone(), old_priority)])
                        .await;
                    let old_plugins = tracker
                        .get_plugins_for_mod(&old_id)
                        .await
                        .unwrap_or_default();
                    let old_state: std::collections::HashMap<String, (i32, bool)> = old_plugins
                        .into_iter()
                        .map(|(_, filename, lo, en)| (filename.to_lowercase(), (lo, en)))
                        .collect();
                    let _ = tracker.delete_plugins_for_mod(&old_id).await;
                    let _ = tracker.delete_mod_files(&old_id).await;
                    let _ = tracker.delete_mod(&old_id).await;
                    let old_cache = paths::mod_cache_dir_in(&cache_root, &old_id);
                    if old_cache.exists() {
                        let _ = std::fs::remove_dir_all(&old_cache);
                    }
                    if !old_state.is_empty() {
                        let new_plugins = tracker
                            .get_plugins_for_mod(&result.mod_entry.id)
                            .await
                            .unwrap_or_default();
                        let updates: Vec<(String, i32, bool)> = new_plugins
                            .into_iter()
                            .filter_map(|(pid, filename, _, _)| {
                                old_state
                                    .get(&filename.to_lowercase())
                                    .map(|&(lo, en)| (pid, lo, en))
                            })
                            .collect();
                        if !updates.is_empty() {
                            let _ = tracker.update_plugin_states(&updates).await;
                        }
                    }
                }
                Ok(result)
            }
            .await;
            AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::ModAdded(
                identity,
                Box::new(result),
                was_replace,
            ))
        });
    }

    pub(crate) fn handle_pre_install_replace(
        &mut self,
        existing_id: String,
        existing_priority: i32,
        sender: &ComponentSender<Self>,
    ) {
        self.install.replacement = Some((existing_id, existing_priority));
        self.proceed_with_install(sender);
    }

    pub(crate) fn handle_open_pre_install_dialog(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.open_pre_install_dialog(root, sender);
    }

    pub(crate) fn handle_open_pre_install_dialog_replacing(
        &mut self,
        old_mod_id: String,
        old_priority: i32,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let old_name = self.mod_name_for_id(&old_mod_id);
        if let Some(pending) = &mut self.install.pending {
            pending.mod_name = old_name;
        }
        self.install.replacement = Some((old_mod_id, old_priority));
        // Drop any fetched name and file-ID context; replacements keep the existing mod's name.
        self.install.fetched_name = None;
        self.install.file_id_needed = None;
        self.open_pre_install_dialog(root, sender);
    }

    pub(crate) fn handle_pre_install_cancelled(&mut self) {
        if let Some(dialog) = self.ui.pre_install_dialog.take() {
            dialog.widget().destroy();
        }
        if let Some(dialog) = self.ui.absorb_dialog.take() {
            dialog.widget().destroy();
        }
        self.install.pending = None;
        self.install.nexus_ids = None;
        self.install.replacement = None;
        self.install.fetched_name = None;
        self.install.file_id_needed = None;
        let was_reinstall = self.install.reinstalling;
        self.install.reinstalling = false;
        self.install.set_stage(InstallStage::Cancelled);
        self.install.invalidate();
        self.finish_current_work();

        if let Some(dl_id) = self.install.active_download_id.take() {
            let status = crate::models::download::DownloadStatus::restored_after_cancelled_install(
                was_reinstall,
            );
            let msg = status.default_status_msg().to_string();
            self.update_download_status(&dl_id, status, &msg);
        }
    }

    pub(crate) fn handle_fomod_confirmed(
        &mut self,
        selections: crate::utils::fomod_resolver::FomodSelections,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(dialog) = self.ui.fomod_dialog.take() {
            dialog.widget().destroy();
        }
        let Some(pending) = self.install.pending.take() else {
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let Some(config_path) = pending.fomod_config_path else {
            return;
        };
        let replace_info = self.install.replacement.take();
        let was_replace = replace_info.is_some();
        let cache_root = match self.cache_root_for(&pending.game.id) {
            Ok(r) => r,
            Err(e) => {
                self.push_notification(&format!("Cannot resolve cache dir: {e}"));
                return;
            }
        };

        self.install.set_stage(InstallStage::Committing);
        self.begin_work(
            WorkKind::Installing,
            format!("Installing {}...", pending.mod_name),
        );
        if let Some(dl_id) = self.install.active_download_id.clone() {
            self.update_download_status(
                &dl_id,
                crate::models::download::DownloadStatus::Extracting,
                "Caching files...",
            );
        }

        // Serialize selections before moving into the async block.
        let serialized_selections: Vec<Vec<Vec<usize>>> = selections
            .selections
            .iter()
            .map(|step| {
                step.iter()
                    .map(|g| {
                        let mut v: Vec<usize> = g.iter().copied().collect();
                        v.sort_unstable();
                        v
                    })
                    .collect()
            })
            .collect();

        let input_sender = sender.input_sender().clone();
        let Some(identity) = self.install.identity() else {
            self.install.set_stage(InstallStage::Failed);
            self.finish_current_work();
            return;
        };
        sender.oneshot_command(async move {
            let result: Result<AddResult, String> = async {
                let archive_label = pending
                    .archive_path
                    .clone()
                    .unwrap_or_else(|| pending.mod_name.clone());
                let file_list = fomod_resolver::resolve_fomod_with_selections(
                    &config_path,
                    pending.tmp_dir.path(),
                    &selections,
                )
                .map_err(|e| {
                    let msg = format!("{e:#}\nArchive: {archive_label}");
                    eprintln!("[deployd] FOMOD resolve error: {msg}");
                    msg
                })?;
                dlog!(
                    "[deployd] FOMOD resolved: {} file(s) to install",
                    file_list.len()
                );
                let file_list: Vec<_> = file_list
                    .into_iter()
                    .map(|m| {
                        (
                            pending.tmp_dir.path().join(&m.source_relative),
                            m.dest_relative,
                        )
                    })
                    .collect();
                let progress_sender = input_sender.clone();
                let on_progress: Option<Box<dyn Fn(usize, usize) + Send>> = Some(
                    throttled_install_progress(progress_sender, identity.clone(), "Caching"),
                );
                let timing_start = std::time::Instant::now();
                let result = installer::add_mod_with_file_list(installer::AddModRequest {
                    file_list,
                    game: &pending.game,
                    mod_name: &pending.mod_name,
                    tracker: &tracker,
                    cache_root: &cache_root,
                    nexus_ids: pending.nexus_ids,
                    archive_hash: pending.archive_hash,
                    archive_path: pending.archive_path,
                    file_targets: pending.file_targets,
                    stripped_wrapper: pending.stripped_wrapper,
                    excluded_files: &pending.excluded_files,
                    on_progress,
                })
                .await
                .map_err(|e| {
                    let msg = format!("{e:#}\nArchive: {archive_label}");
                    eprintln!("[deployd] FOMOD install error: {msg}");
                    msg
                })?;
                crate::app::timing::log_phase(
                    "install.cache_files",
                    &pending.game.id,
                    timing_start,
                    Some(result.files_cached),
                );
                if let Ok(json) = serde_json::to_string(&serialized_selections) {
                    let _ = tracker
                        .save_fomod_selections(&result.mod_entry.id, &json)
                        .await;
                }
                drop(pending.tmp_dir);
                if let Some((old_id, old_priority)) = replace_info {
                    let _ = tracker
                        .update_priorities(&[(result.mod_entry.id.clone(), old_priority)])
                        .await;
                    let old_plugins = tracker
                        .get_plugins_for_mod(&old_id)
                        .await
                        .unwrap_or_default();
                    let old_state: std::collections::HashMap<String, (i32, bool)> = old_plugins
                        .into_iter()
                        .map(|(_, filename, lo, en)| (filename.to_lowercase(), (lo, en)))
                        .collect();
                    let _ = tracker.delete_plugins_for_mod(&old_id).await;
                    let _ = tracker.delete_mod_files(&old_id).await;
                    let _ = tracker.delete_mod(&old_id).await;
                    let old_cache = paths::mod_cache_dir_in(&cache_root, &old_id);
                    if old_cache.exists() {
                        let _ = std::fs::remove_dir_all(&old_cache);
                    }
                    if !old_state.is_empty() {
                        let new_plugins = tracker
                            .get_plugins_for_mod(&result.mod_entry.id)
                            .await
                            .unwrap_or_default();
                        let updates: Vec<(String, i32, bool)> = new_plugins
                            .into_iter()
                            .filter_map(|(pid, filename, _, _)| {
                                old_state
                                    .get(&filename.to_lowercase())
                                    .map(|&(lo, en)| (pid, lo, en))
                            })
                            .collect();
                        if !updates.is_empty() {
                            let _ = tracker.update_plugin_states(&updates).await;
                        }
                    }
                }
                Ok(result)
            }
            .await;
            AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::ModAdded(
                identity,
                Box::new(result),
                was_replace,
            ))
        });
    }

    pub(crate) fn handle_fomod_cancelled(&mut self) {
        if let Some(dialog) = self.ui.fomod_dialog.take() {
            dialog.widget().destroy();
        }
        self.install.pending = None;
        self.install.nexus_ids = None;
        self.install.replacement = None;
        self.install.fetched_name = None;
        self.install.file_id_needed = None;
        let was_reinstall = self.install.reinstalling;
        self.install.reinstalling = false;
        self.install.set_stage(InstallStage::Cancelled);
        self.install.invalidate();
        self.finish_current_work();

        if let Some(dl_id) = self.install.active_download_id.take() {
            let status = crate::models::download::DownloadStatus::restored_after_cancelled_install(
                was_reinstall,
            );
            let msg = status.default_status_msg().to_string();
            self.update_download_status(&dl_id, status, &msg);
        }
    }

    pub(crate) fn handle_install_progress(
        &mut self,
        identity: &super::state::InstallIdentity,
        fraction: f64,
        msg: String,
    ) {
        if !self.install.accepts(identity) || !self.install.is_busy() {
            return;
        }
        let kind = if msg.starts_with("Extracting") {
            WorkKind::ExtractingArchive
        } else if msg.starts_with("Processing") {
            WorkKind::ProcessingArchive
        } else {
            WorkKind::Installing
        };
        self.update_work(kind, msg.clone(), Some(fraction));
        if let Some(ref dl_id) = self.install.active_download_id.clone() {
            if let Some(entry) = self.download.all.iter_mut().find(|e| e.id == *dl_id) {
                entry.progress = fraction;
                entry.status_msg = msg.clone();
            }
            let mut guard = self.download.rows.guard();
            for i in 0..guard.len() {
                if let Some(row) = guard.get_mut(i)
                    && row.entry.id == *dl_id
                {
                    row.entry.progress = fraction;
                    row.entry.status_msg = msg;
                    break;
                }
            }
        }
    }

    pub(crate) fn handle_pre_install_merge(
        &mut self,
        existing_mod_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(pending) = self.install.pending.take() else {
            return;
        };
        let Some(file_list) = pending.file_list else {
            return;
        };
        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };
        let mod_name = pending.mod_name.clone();
        let cache_root = match self.cache_root_for(&pending.game.id) {
            Ok(r) => r,
            Err(e) => {
                self.push_notification(&format!("Cannot resolve cache dir: {e}"));
                return;
            }
        };

        self.install.set_stage(InstallStage::Committing);
        self.begin_work(
            WorkKind::Installing,
            format!("Merging into {}...", mod_name),
        );
        if let Some(dl_id) = self.install.active_download_id.clone() {
            self.update_download_status(
                &dl_id,
                crate::models::download::DownloadStatus::Extracting,
                "Caching files...",
            );
        }

        let input_sender = sender.input_sender().clone();
        let Some(identity) = self.install.identity() else {
            self.install.set_stage(InstallStage::Failed);
            self.finish_current_work();
            return;
        };
        sender.oneshot_command(async move {
            let result: Result<(String, usize), String> = async {
                let archive_label = pending
                    .archive_path
                    .clone()
                    .unwrap_or_else(|| pending.mod_name.clone());
                let progress_sender = input_sender.clone();
                let on_progress: Option<Box<dyn Fn(usize, usize) + Send>> = Some(
                    throttled_install_progress(progress_sender, identity.clone(), "Caching"),
                );
                let count = installer::merge_files_into_mod(installer::MergeModRequest {
                    file_list,
                    game: &pending.game,
                    mod_name: &mod_name,
                    existing_mod_id: &existing_mod_id,
                    tracker: &tracker,
                    cache_root: &cache_root,
                    file_targets: pending.file_targets,
                    stripped_wrapper: pending.stripped_wrapper,
                    excluded_files: &pending.excluded_files,
                    on_progress,
                })
                .await
                .map_err(|e| format!("{e:#}\nArchive: {archive_label}"))?;
                drop(pending.tmp_dir);
                Ok((mod_name, count))
            }
            .await;
            AppCmdMsg::Install(crate::app::messages::InstallCmdMsg::ModMerged(
                identity, result,
            ))
        });
    }

    pub(crate) fn handle_pre_install_create_new(&mut self, sender: &ComponentSender<Self>) {
        self.proceed_with_install(sender);
    }
}

#[cfg(test)]
mod tests {
    use crate::models::download::{DownloadEntry, NexusIds};

    use super::{next_unresolved_sibling_id, parse_fomod_selections};

    fn download(id: &str, mod_id: i64, file_id: i64, metadata_fetched: bool) -> DownloadEntry {
        let mut entry = DownloadEntry::new(
            id.to_string(),
            id.to_string(),
            Some(NexusIds {
                mod_id,
                file_id,
                domain: "skyrim".to_string(),
            }),
        );
        entry.metadata_fetched = metadata_fetched;
        entry
    }

    #[test]
    fn selects_unresolved_sibling_for_same_mod() {
        let downloads = vec![
            download("resolved", 10, 123, true),
            download("other-mod", 20, 0, false),
            download("sibling", 10, 0, false),
        ];

        assert_eq!(
            next_unresolved_sibling_id(&downloads, "resolved"),
            Some("sibling".to_string())
        );
    }

    #[test]
    fn skips_siblings_with_resolved_metadata() {
        let downloads = vec![
            download("resolved", 10, 123, true),
            download("known-file", 10, 456, false),
            download("metadata-fetched", 10, 0, true),
        ];

        assert_eq!(next_unresolved_sibling_id(&downloads, "resolved"), None);
    }

    #[test]
    fn returns_none_when_resolved_download_is_missing() {
        let downloads = vec![download("sibling", 10, 0, false)];

        assert_eq!(next_unresolved_sibling_id(&downloads, "missing"), None);
    }

    #[test]
    fn parses_saved_fomod_selections_into_sets() {
        let selections = parse_fomod_selections("[[[2,1,2],[4]]]").expect("valid selections");

        assert_eq!(selections.len(), 1);
        assert_eq!(selections[0].len(), 2);
        assert_eq!(selections[0][0], [1, 2].into_iter().collect());
        assert_eq!(selections[0][1], [4].into_iter().collect());
    }

    #[test]
    fn rejects_invalid_saved_fomod_selections() {
        assert_eq!(parse_fomod_selections("not json"), None);
    }
}
