use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use adw::prelude::*;
use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::installer::{self, AddResult, PrepareResult};
use crate::dlog;
use crate::ui::fomod_dialog::{
    FomodDialog, FomodDialogInit, FomodDialogOutput, default_fomod_selections,
};
use crate::utils::{fomod_resolver, paths};

use super::App;
use super::free_fns::load_game_data;
use super::messages::{AppCmdMsg, AppMsg, PrepareResultMsg};
use super::progress::throttled_install_progress;
use super::types::{PendingInstall, WorkKind};

impl App {
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
                input_sender.send(AppMsg::FileChosen(path)).unwrap();
            }
        });
    }

    pub(crate) fn handle_file_chosen(&mut self, path: PathBuf, sender: &ComponentSender<Self>) {
        let mod_name = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        self.installing = true;
        self.begin_work(WorkKind::PreparingArchive, format!("Hashing {mod_name}..."));

        let extract_sender = sender.input_sender().clone();
        let on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
            Some(throttled_install_progress(extract_sender, "Extracting"));
        let processing_sender = sender.input_sender().clone();
        let on_processing: Option<Box<dyn FnOnce() + Send>> = Some(Box::new(move || {
            let _ = processing_sender.send(AppMsg::InstallProgress(
                1.0,
                "Processing mod structure...".to_string(),
            ));
        }));

        sender.oneshot_command(async move {
            let result: Result<PrepareResultMsg, String> = async {
                let timing_start = std::time::Instant::now();
                let hash_path = path.clone();
                let archive_path = Some(path.to_string_lossy().to_string());
                let archive_hash = tokio::task::spawn_blocking(move || {
                    crate::utils::archive::hash_archive_file(&hash_path).ok()
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
            AppCmdMsg::ModPrepared(result)
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
        if let Some(dialog) = self.pre_install_dialog.take() {
            dialog.widget().destroy();
        }
        let Some(pending) = self.pending_install.as_mut() else {
            return;
        };
        pending.mod_name = edited_name;
        pending.file_targets = file_targets;
        pending.excluded_files = excluded_files;

        if let Some(config) = pending.fomod_config.take() {
            let active_plugin_files: HashSet<String> = {
                let guard = self.plugins.guard();
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
                self.fomod_dialog = Some(
                    FomodDialog::builder()
                        .transient_for(root)
                        .launch(FomodDialogInit {
                            config,
                            extracted_root,
                            active_plugin_files,
                            previous_selections: self.pending_fomod_selections.take(),
                        })
                        .forward(sender.input_sender(), |output| match output {
                            FomodDialogOutput::Confirmed(sel) => AppMsg::FomodConfirmed(sel),
                            FomodDialogOutput::Cancelled => AppMsg::FomodCancelled,
                        }),
                );
                return;
            }
            // No user choices needed — auto-install with defaults
            sender
                .input_sender()
                .emit(AppMsg::FomodConfirmed(default_fomod_selections(
                    &config,
                    &active_plugin_files,
                )));
            return;
        }

        let conflict = {
            let mod_name = self
                .pending_install
                .as_ref()
                .map(|p| p.mod_name.clone())
                .unwrap_or_default();
            if self.pending_replace_mod_id.is_none() {
                self.find_mod_id_and_priority_by_name(&mod_name)
            } else {
                None
            }
        };

        if let Some((existing_id, existing_priority)) = conflict {
            let mod_name = self
                .pending_install
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
                    "merge" => AppMsg::PreInstallMerge(existing_id.clone()),
                    "replace" => AppMsg::PreInstallReplace(existing_id.clone(), existing_priority),
                    _ => AppMsg::PreInstallCreateNew,
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
        let pending = match self.pending_install.take() {
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
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let replace_info = self.pending_replace_mod_id.take();
        let was_replace = replace_info.is_some();
        let cache_root = match self.cache_root_for(&pending.game.id) {
            Ok(r) => r,
            Err(e) => {
                self.push_notification(&format!("Cannot resolve cache dir: {e}"));
                return;
            }
        };

        self.installing = true;
        self.begin_work(
            WorkKind::Installing,
            format!("Installing {}...", pending.mod_name),
        );
        if let Some(dl_id) = self.active_install_download_id.clone() {
            self.update_download_status(
                &dl_id,
                crate::models::download::DownloadStatus::Extracting,
                "Caching files...",
            );
        }

        let input_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result: Result<AddResult, String> = async {
                let archive_label = pending
                    .archive_path
                    .clone()
                    .unwrap_or_else(|| pending.mod_name.clone());
                let progress_sender = input_sender.clone();
                let on_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
                    Some(throttled_install_progress(progress_sender, "Caching"));
                let timing_start = std::time::Instant::now();
                let result = installer::add_mod_with_file_list(
                    file_list,
                    &pending.game,
                    &pending.mod_name,
                    &tracker,
                    &cache_root,
                    pending.nexus_ids,
                    pending.archive_hash,
                    pending.archive_path,
                    pending.file_targets,
                    pending.stripped_wrapper,
                    &pending.excluded_files,
                    on_progress,
                )
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
            AppCmdMsg::ModAdded(result, was_replace)
        });
    }

    pub(crate) fn handle_pre_install_replace(
        &mut self,
        existing_id: String,
        existing_priority: i32,
        sender: &ComponentSender<Self>,
    ) {
        self.pending_replace_mod_id = Some((existing_id, existing_priority));
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
        if let Some(pending) = &mut self.pending_install {
            pending.mod_name = old_name;
        }
        self.pending_replace_mod_id = Some((old_mod_id, old_priority));
        // Drop any fetched name and file-ID context; replacements keep the existing mod's name.
        self.pending_fetched_name = None;
        self.pending_file_id_needed = None;
        self.open_pre_install_dialog(root, sender);
    }

    pub(crate) fn handle_pre_install_cancelled(&mut self) {
        if let Some(dialog) = self.pre_install_dialog.take() {
            dialog.widget().destroy();
        }
        if let Some(dialog) = self.absorb_dialog.take() {
            dialog.widget().destroy();
        }
        self.pending_install = None;
        self.pending_nexus_ids = None;
        self.pending_replace_mod_id = None;
        self.pending_fetched_name = None;
        self.pending_file_id_needed = None;
        let was_reinstall = self.reinstall_mode;
        self.reinstall_mode = false;
        self.installing = false;
        self.finish_current_work();

        if let Some(dl_id) = self.active_install_download_id.take() {
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
        if let Some(dialog) = self.fomod_dialog.take() {
            dialog.widget().destroy();
        }
        let Some(pending) = self.pending_install.take() else {
            return;
        };
        let Some(tracker) = self.tracker.clone() else {
            return;
        };
        let Some(config_path) = pending.fomod_config_path else {
            return;
        };
        let replace_info = self.pending_replace_mod_id.take();
        let was_replace = replace_info.is_some();
        let cache_root = match self.cache_root_for(&pending.game.id) {
            Ok(r) => r,
            Err(e) => {
                self.push_notification(&format!("Cannot resolve cache dir: {e}"));
                return;
            }
        };

        self.installing = true;
        self.begin_work(
            WorkKind::Installing,
            format!("Installing {}...", pending.mod_name),
        );
        if let Some(dl_id) = self.active_install_download_id.clone() {
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
                let on_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
                    Some(throttled_install_progress(progress_sender, "Caching"));
                let timing_start = std::time::Instant::now();
                let result = installer::add_mod_with_file_list(
                    file_list,
                    &pending.game,
                    &pending.mod_name,
                    &tracker,
                    &cache_root,
                    pending.nexus_ids,
                    pending.archive_hash,
                    pending.archive_path,
                    pending.file_targets,
                    pending.stripped_wrapper,
                    &pending.excluded_files,
                    on_progress,
                )
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
            AppCmdMsg::ModAdded(result, was_replace)
        });
    }

    pub(crate) fn handle_fomod_cancelled(&mut self) {
        if let Some(dialog) = self.fomod_dialog.take() {
            dialog.widget().destroy();
        }
        self.pending_install = None;
        self.pending_nexus_ids = None;
        self.pending_replace_mod_id = None;
        self.pending_fetched_name = None;
        self.pending_file_id_needed = None;
        let was_reinstall = self.reinstall_mode;
        self.reinstall_mode = false;
        self.installing = false;
        self.finish_current_work();

        if let Some(dl_id) = self.active_install_download_id.take() {
            let status = crate::models::download::DownloadStatus::restored_after_cancelled_install(
                was_reinstall,
            );
            let msg = status.default_status_msg().to_string();
            self.update_download_status(&dl_id, status, &msg);
        }
    }

    pub(crate) fn handle_install_progress(&mut self, fraction: f64, msg: String) {
        if !self.installing {
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
        if let Some(ref dl_id) = self.active_install_download_id.clone() {
            if let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == *dl_id) {
                entry.progress = fraction;
                entry.status_msg = msg.clone();
            }
            let mut guard = self.downloads.guard();
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
        let Some(pending) = self.pending_install.take() else {
            return;
        };
        let Some(file_list) = pending.file_list else {
            return;
        };
        let Some(tracker) = self.tracker.clone() else {
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

        self.installing = true;
        self.begin_work(
            WorkKind::Installing,
            format!("Merging into {}...", mod_name),
        );
        if let Some(dl_id) = self.active_install_download_id.clone() {
            self.update_download_status(
                &dl_id,
                crate::models::download::DownloadStatus::Extracting,
                "Caching files...",
            );
        }

        let input_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result: Result<(String, usize), String> = async {
                let archive_label = pending
                    .archive_path
                    .clone()
                    .unwrap_or_else(|| pending.mod_name.clone());
                let progress_sender = input_sender.clone();
                let on_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
                    Some(throttled_install_progress(progress_sender, "Caching"));
                let count = installer::merge_files_into_mod(
                    file_list,
                    &pending.game,
                    &mod_name,
                    &existing_mod_id,
                    &tracker,
                    &cache_root,
                    pending.file_targets,
                    pending.stripped_wrapper,
                    &pending.excluded_files,
                    on_progress,
                )
                .await
                .map_err(|e| format!("{e:#}\nArchive: {archive_label}"))?;
                drop(pending.tmp_dir);
                Ok((mod_name, count))
            }
            .await;
            AppCmdMsg::ModMerged(result)
        });
    }

    pub(crate) fn handle_pre_install_create_new(&mut self, sender: &ComponentSender<Self>) {
        self.proceed_with_install(sender);
    }
}

// ─── AppCmdMsg handlers ──────────────────────────────────────────────────────

impl App {
    pub(crate) fn handle_cmd_mod_prepared(
        &mut self,
        result: Result<PrepareResultMsg, String>,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        match result {
            Ok(PrepareResultMsg::Normal {
                file_list,
                stripped_wrapper,
                tmp_dir,
                mod_name,
                archive_hash,
                archive_path,
            }) => {
                let game = self.selected_game().cloned().expect(
                    "PrepareResult received without a selected game — UI invariant violated",
                );
                self.pending_install = Some(PendingInstall {
                    tmp_dir,
                    mod_name: mod_name.clone(),
                    game,
                    file_list: Some(file_list),
                    stripped_wrapper,
                    fomod_config_path: None,
                    fomod_config: None,
                    nexus_ids: self.pending_nexus_ids.take(),
                    archive_hash,
                    archive_path,
                    file_targets: HashMap::new(),
                    excluded_files: HashSet::new(),
                });
                let hash_existing = self
                    .pending_install
                    .as_ref()
                    .and_then(|p| p.archive_hash.as_deref())
                    .and_then(|h| self.find_mod_by_archive_hash(h));
                let nexus_ids = self
                    .pending_install
                    .as_ref()
                    .and_then(|p| p.nexus_ids.as_ref())
                    .map(|n| (n.mod_id, n.file_id));
                let existing = hash_existing.or_else(|| {
                    nexus_ids.and_then(|(mid, fid)| self.find_installed_mod_by_nexus_id(mid, fid))
                });
                if let Some((old_mod_id, old_mod_name, old_priority)) = existing {
                    if self.reinstall_mode {
                        self.reinstall_mode = false;
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
                                "alongside" => input_sender.send(AppMsg::OpenPreInstallDialog),
                                "replace" => {
                                    input_sender.send(AppMsg::OpenPreInstallDialogReplacing(
                                        old_mod_id.clone(),
                                        old_priority,
                                    ))
                                }
                                _ => input_sender.send(AppMsg::PreInstallCancelled),
                            };
                        });
                        self.installing = false;
                        self.finish_current_work();
                        dialog.present(Some(root));
                    }
                } else {
                    self.reinstall_mode = false;
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
                let game = self.selected_game().cloned().expect(
                    "PrepareResult received without a selected game — UI invariant violated",
                );
                self.pending_install = Some(PendingInstall {
                    tmp_dir,
                    mod_name: mod_name.clone(),
                    game,
                    file_list: None,
                    stripped_wrapper: None,
                    fomod_config_path: Some(config_path),
                    fomod_config: Some(config),
                    nexus_ids: self.pending_nexus_ids.take(),
                    archive_hash,
                    archive_path,
                    file_targets: HashMap::new(),
                    excluded_files: HashSet::new(),
                });
                let hash_existing = self
                    .pending_install
                    .as_ref()
                    .and_then(|p| p.archive_hash.as_deref())
                    .and_then(|h| self.find_mod_by_archive_hash(h));
                let nexus_ids = self
                    .pending_install
                    .as_ref()
                    .and_then(|p| p.nexus_ids.as_ref())
                    .map(|n| (n.mod_id, n.file_id));
                let existing = hash_existing.or_else(|| {
                    nexus_ids.and_then(|(mid, fid)| self.find_installed_mod_by_nexus_id(mid, fid))
                });
                if let Some((old_mod_id, old_mod_name, old_priority)) = existing {
                    if self.reinstall_mode {
                        self.reinstall_mode = false;
                        // Set up replace context now; open dialog after fetching previous selections.
                        let old_name = self.mod_name_for_id(&old_mod_id);
                        if let Some(pending) = &mut self.pending_install {
                            pending.mod_name = old_name;
                        }
                        self.pending_replace_mod_id = Some((old_mod_id.clone(), old_priority));
                        self.pending_fetched_name = None;
                        self.pending_file_id_needed = None;
                        let tracker = self.tracker.clone();
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
                            AppCmdMsg::FomodSelectionsLoaded(selections)
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
                                "alongside" => input_sender.send(AppMsg::OpenPreInstallDialog),
                                "replace" => {
                                    input_sender.send(AppMsg::OpenPreInstallDialogReplacing(
                                        old_mod_id.clone(),
                                        old_priority,
                                    ))
                                }
                                _ => input_sender.send(AppMsg::PreInstallCancelled),
                            };
                        });
                        self.installing = false;
                        self.finish_current_work();
                        dialog.present(Some(root));
                    }
                } else {
                    self.reinstall_mode = false;
                    self.open_pre_install_dialog(root, sender);
                }
            }
            Err(e) => {
                self.installing = false;
                self.finish_current_work();
                self.reinstall_mode = false;
                self.pending_fomod_selections = None;
                if let Some(dl_id) = self.active_install_download_id.take() {
                    self.update_download_status(
                        &dl_id,
                        crate::models::download::DownloadStatus::Failed,
                        &format!("Extraction failed: {e}"),
                    );
                    if let Some(tracker) = self.tracker.clone()
                        && let Some(entry) = self.all_downloads.iter().find(|e| e.id == dl_id)
                    {
                        let entry = entry.clone();
                        sender.oneshot_command(async move {
                            let _ = tracker.save_download_entry(&entry).await;
                            AppCmdMsg::PrioritySaved(Ok(()))
                        });
                    }
                }
                self.push_notification(&format!("Add failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_mod_added(
        &mut self,
        result: Result<AddResult, String>,
        was_replace: bool,
        sender: &ComponentSender<Self>,
    ) {
        self.installing = false;
        self.finish_current_work();

        let maybe_archive_hash = match &result {
            Ok(add_result) => add_result.mod_entry.archive_hash.clone(),
            Err(_) => None,
        };
        let metadata_dl_id = self.active_install_download_id.clone();

        if let Some(dl_id) = self.active_install_download_id.take() {
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
                self.all_downloads.iter_mut().find(|e| e.id == dl_id),
            ) {
                entry.archive_hash = Some(hash.clone());
            }
            if let Some(tracker) = self.tracker.clone()
                && let Some(entry) = self.all_downloads.iter().find(|e| e.id == dl_id)
            {
                let entry = entry.clone();
                sender.oneshot_command(async move {
                    let _ = tracker.save_download_entry(&entry).await;
                    AppCmdMsg::PrioritySaved(Ok(()))
                });
            }
        }

        match result {
            Ok(add_result) => {
                let count = add_result.files_cached;
                let plugins = add_result.plugins_found.len();
                self.needs_deploy = true;
                self.auto_save_profile(sender);

                // If the download entry already has resolved metadata, write the version to
                // the mods table and reload in a single chained command so the read cannot
                // race ahead of the write. Otherwise a plain reload is sufficient — the
                // metadata-fetch path will update version/author and trigger its own reload.
                let (version_from_dl, author_from_dl): (Option<String>, Option<String>) =
                    metadata_dl_id
                        .as_ref()
                        .and_then(|id| self.all_downloads.iter().find(|e| &e.id == id))
                        .filter(|e| e.metadata_fetched)
                        .map(|e| (e.version.clone(), e.author.clone()))
                        .unwrap_or((None, None));

                let already_fetched = version_from_dl.is_some()
                    || metadata_dl_id
                        .as_ref()
                        .and_then(|id| self.all_downloads.iter().find(|e| &e.id == id))
                        .is_some_and(|e| e.metadata_fetched);

                if let (Some(tracker), Some(game)) =
                    (self.tracker.clone(), self.selected_game().cloned())
                {
                    let mod_id = add_result.mod_entry.id.clone();
                    sender.oneshot_command(async move {
                        if let Some(ref version) = version_from_dl {
                            let _ = tracker.set_mod_installed_version(&mod_id, version).await;
                        }
                        if let Some(ref author) = author_from_dl {
                            let _ = tracker.set_mod_author(&mod_id, author).await;
                        }
                        AppCmdMsg::ModsLoaded(load_game_data(&tracker, &game, false).await, true)
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
                if let Some(dialog) = self.absorb_dialog.take() {
                    dialog.widget().destroy();
                }

                if !already_fetched
                    && let (Some(nexus_mod_id), Some(nexus_domain)) = (
                        add_result.mod_entry.nexus_mod_id,
                        add_result.mod_entry.nexus_domain.as_deref(),
                    )
                {
                    let tracker = self
                        .tracker
                        .clone()
                        .expect("tracker not initialized at absorb result handling");
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
                        AppCmdMsg::NexusMetadataFetched(dl_id_for_metadata, result)
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
        result: Result<(String, usize), String>,
        sender: &ComponentSender<Self>,
    ) {
        self.installing = false;
        self.finish_current_work();

        if let Some(dl_id) = self.active_install_download_id.take() {
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
            if let Some(tracker) = self.tracker.clone()
                && let Some(entry) = self.all_downloads.iter().find(|e| e.id == dl_id)
            {
                let entry = entry.clone();
                sender.oneshot_command(async move {
                    let _ = tracker.save_download_entry(&entry).await;
                    AppCmdMsg::PrioritySaved(Ok(()))
                });
            }
        }

        match result {
            Ok((mod_name, count)) => {
                self.needs_deploy = true;
                self.auto_save_profile(sender);
                self.reload_mods(sender);
                self.show_toast(&format!(
                    "Merged {count} file(s) into '{mod_name}' — deploy to update game files"
                ));
                if let Some(dialog) = self.absorb_dialog.take() {
                    dialog.widget().destroy();
                }
            }
            Err(e) => {
                self.push_notification(&format!("Merge failed: {e}"));
            }
        }
    }

    /// Show a dialog asking the user to enter the Nexus file ID when the install-time
    /// metadata fetch could not match the archive filename to any file on the mod page.
    ///
    /// On confirm: sends `AppMsg::FileIdDialogConfirmed` → async fetch → `AppCmdMsg::FileIdFetched`
    ///             → applies combined name → opens pre-install dialog.
    /// On skip: clears `pending_file_id_needed` and opens the pre-install dialog with the
    ///          partial mod name already stored in `pending_fetched_name`.
    pub(crate) fn show_file_id_dialog(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let Some(ctx) = self.pending_file_id_needed.take() else {
            return;
        };

        let text_entry = gtk::Entry::builder()
            .placeholder_text("File ID (e.g. 123456)")
            .hexpand(true)
            .activates_default(true)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(8)
            .margin_end(8)
            .build();

        let dialog = adw::AlertDialog::builder()
            .heading("File Not Found on Nexus")
            .body(
                "The archive filename did not match any file on this mod page. \
                 Enter the Nexus file ID to complete the metadata, or skip to proceed.",
            )
            .build();
        dialog.set_extra_child(Some(&text_entry));
        dialog.add_response("skip", "Skip");
        dialog.add_response("fetch", "Fetch");
        dialog.set_default_response(Some("fetch"));
        dialog.set_close_response("skip");
        dialog.set_response_appearance("fetch", adw::ResponseAppearance::Suggested);

        let input_sender = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "fetch" {
                let raw = text_entry.text().to_string();
                if let Ok(file_id) = raw.trim().parse::<i64>()
                    && file_id > 0
                {
                    let _ = input_sender.send(AppMsg::FileIdDialogConfirmed {
                        download_id: ctx.download_id.clone(),
                        file_id,
                        mod_id: ctx.mod_id,
                        domain: ctx.domain.clone(),
                    });
                    return;
                }
            }
            // Skip or invalid input — open pre-install dialog with partial name.
            let _ = input_sender.send(AppMsg::OpenPreInstallDialog);
        });
        dialog.present(Some(root));
    }

    /// Fetch the Nexus file entry for the user-supplied `file_id` and resolve the combined
    /// mod+file name. Fires `AppCmdMsg::FileIdFetched` when done.
    pub(crate) fn handle_file_id_dialog_confirmed(
        &mut self,
        download_id: String,
        file_id: i64,
        mod_id: i64,
        domain: String,
        sender: &ComponentSender<Self>,
    ) {
        let is_install = self.pending_install.is_some();
        let Some(tracker) = self.tracker.clone() else {
            if is_install {
                let _ = sender.input_sender().send(AppMsg::OpenPreInstallDialog);
            }
            return;
        };
        // Persist the resolved file_id on the download entry so future installs skip this dialog.
        if let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == download_id)
            && let Some(ref mut ids) = entry.nexus_ids
        {
            ids.file_id = file_id;
        }
        self.begin_work(WorkKind::FetchingMetadata, "Fetching Nexus metadata...");
        self.begin_download_metadata_fetch(&download_id);
        let partial_name = self.pending_fetched_name.clone();
        let download_id_for_result = (!is_install).then(|| download_id.clone());
        sender.oneshot_command(async move {
            let Some(api_key) = tracker
                .get_setting("nexus_api_key")
                .await
                .ok()
                .flatten()
                .filter(|k| !k.is_empty())
            else {
                return AppCmdMsg::FileIdFetched {
                    combined_name: None,
                    download_id: download_id_for_result,
                    version: None,
                    file_id: None,
                };
            };
            let client = crate::core::nexus_api::NexusClient::new(api_key);
            let file_entry = client
                .get_mod_files(&domain, mod_id)
                .await
                .ok()
                .and_then(|(resp, _)| resp.files.into_iter().find(|f| f.file_id == file_id));
            if let Some(ref entry) = file_entry {
                let fname = &entry.name;
                let mod_name = partial_name.as_deref().unwrap_or(fname.as_str());
                let combined = if fname.to_lowercase().contains(&mod_name.to_lowercase()) {
                    fname.clone()
                } else {
                    format!("{mod_name} - {fname}")
                };
                AppCmdMsg::FileIdFetched {
                    combined_name: Some(combined),
                    download_id: download_id_for_result,
                    version: entry.version.clone(),
                    file_id: Some(entry.file_id),
                }
            } else {
                AppCmdMsg::FileIdFetched {
                    combined_name: None,
                    download_id: download_id_for_result,
                    version: None,
                    file_id: None,
                }
            }
        });
    }
}
