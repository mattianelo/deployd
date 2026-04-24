use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

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
use super::messages::{AppCmdMsg, AppMsg, PrepareResultMsg};
use super::types::PendingInstall;

impl App {
    pub(crate) fn handle_install_clicked(
        &mut self,
        root: &adw::Window,
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
        self.status_msg = Some(format!("Extracting {mod_name}..."));

        let extract_sender = sender.input_sender().clone();
        let on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
            Some(Box::new(move |done, total| {
                let frac = done as f64 / total as f64;
                let _ = extract_sender.send(AppMsg::InstallProgress(
                    frac,
                    format!("Extracting file {done}/{total}"),
                ));
            }));

        sender.oneshot_command(async move {
            let result: Result<PrepareResultMsg, String> = async {
                let hash_path = path.clone();
                let archive_hash = tokio::task::spawn_blocking(move || {
                    crate::utils::archive::hash_archive_file(&hash_path).ok()
                })
                .await
                .unwrap_or(None);

                let prepare = installer::prepare_mod(&path, on_extract_progress)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
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
        root: &adw::Window,
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

        if let Some(config) = pending.fomod_config.take() {
            let active_plugin_files: HashSet<String> = {
                let guard = self.plugins.guard();
                (0..guard.len())
                    .filter_map(|i| guard.get(i))
                    .map(|row| row.plugin.filename.to_lowercase())
                    .collect()
            };

            if fomod_resolver::needs_user_input(&config) {
                let extracted_root = pending.tmp_dir.path().to_path_buf();
                self.fomod_dialog = Some(
                    FomodDialog::builder()
                        .transient_for(root)
                        .launch(FomodDialogInit {
                            config,
                            extracted_root,
                            active_plugin_files,
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
            let dialog = gtk::AlertDialog::builder()
                .message("Mod name already exists")
                .detail(format!(
                    "'{}' already exists. Merge files into it, replace it, or create a separate mod?",
                    mod_name
                ))
                .buttons(["Create New", "Merge", "Replace"])
                .cancel_button(0)
                .default_button(1)
                .modal(true)
                .build();
            let input_sender = sender.input_sender().clone();
            dialog.choose(Some(root), None::<&gio::Cancellable>, move |result| {
                let msg = match result {
                    Ok(1) => AppMsg::PreInstallMerge(existing_id),
                    Ok(2) => AppMsg::PreInstallReplace(existing_id, existing_priority),
                    _ => AppMsg::PreInstallCreateNew,
                };
                let _ = input_sender.send(msg);
            });
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
                self.toaster.toast(&format!("Cannot resolve cache dir: {e}"));
                return;
            }
        };

        self.installing = true;
        self.status_msg = Some(format!("Installing {}...", pending.mod_name));

        let input_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result: Result<AddResult, String> = async {
                let progress_sender = input_sender.clone();
                let on_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
                    Some(Box::new(move |done, total| {
                        let frac = done as f64 / total as f64;
                        let _ = progress_sender.send(AppMsg::InstallProgress(
                            frac,
                            format!("Caching file {done}/{total}"),
                        ));
                    }));
                let result = installer::add_mod_with_file_list(
                    file_list,
                    &pending.game,
                    &pending.mod_name,
                    &tracker,
                    &cache_root,
                    pending.nexus_ids,
                    pending.archive_hash,
                    pending.file_targets,
                    pending.stripped_wrapper,
                    on_progress,
                )
                .await
                .map_err(|e| e.to_string())?;
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
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        self.open_pre_install_dialog(root, sender);
    }

    pub(crate) fn handle_open_pre_install_dialog_replacing(
        &mut self,
        old_mod_id: String,
        old_priority: i32,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let old_name = self.mod_name_for_id(&old_mod_id);
        if let Some(pending) = &mut self.pending_install {
            pending.mod_name = old_name;
        }
        self.pending_replace_mod_id = Some((old_mod_id, old_priority));
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
        self.reinstall_mode = false;
        self.installing = false;
        self.status_msg = None;

        if let Some(dl_id) = self.active_download_id.take() {
            self.update_download_status(
                &dl_id,
                crate::models::download::DownloadStatus::Downloaded,
                "Ready to install",
            );
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
                self.toaster.toast(&format!("Cannot resolve cache dir: {e}"));
                return;
            }
        };

        self.installing = true;
        self.status_msg = Some(format!("Installing {}...", pending.mod_name));

        let input_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result: Result<AddResult, String> = async {
                let file_list = fomod_resolver::resolve_fomod_with_selections(
                    &config_path,
                    pending.tmp_dir.path(),
                    &selections,
                )
                .map_err(|e| {
                    let msg = format!("{e:#}");
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
                    Some(Box::new(move |done, total| {
                        let frac = done as f64 / total as f64;
                        let _ = progress_sender.send(AppMsg::InstallProgress(
                            frac,
                            format!("Caching file {done}/{total}"),
                        ));
                    }));
                let result = installer::add_mod_with_file_list(
                    file_list,
                    &pending.game,
                    &pending.mod_name,
                    &tracker,
                    &cache_root,
                    pending.nexus_ids,
                    pending.archive_hash,
                    pending.file_targets,
                    pending.stripped_wrapper,
                    on_progress,
                )
                .await
                .map_err(|e| {
                    let msg = format!("{e:#}");
                    eprintln!("[deployd] FOMOD install error: {msg}");
                    msg
                })?;
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
        self.reinstall_mode = false;
        self.installing = false;
        self.status_msg = None;

        if let Some(dl_id) = self.active_download_id.take() {
            self.update_download_status(
                &dl_id,
                crate::models::download::DownloadStatus::Downloaded,
                "Ready to install",
            );
        }
    }

    pub(crate) fn handle_install_progress(&mut self, fraction: f64, msg: String) {
        if !self.installing {
            return;
        }
        self.status_msg = Some(msg.clone());
        if let Some(ref dl_id) = self.active_download_id.clone() {
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
                self.toaster.toast(&format!("Cannot resolve cache dir: {e}"));
                return;
            }
        };

        self.installing = true;
        self.status_msg = Some(format!("Merging into {}…", mod_name));

        let input_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result: Result<(String, usize), String> = async {
                let progress_sender = input_sender.clone();
                let on_progress: Option<Box<dyn Fn(usize, usize) + Send>> =
                    Some(Box::new(move |done, total| {
                        let frac = done as f64 / total as f64;
                        let _ = progress_sender.send(AppMsg::InstallProgress(
                            frac,
                            format!("Caching file {done}/{total}"),
                        ));
                    }));
                let count = installer::merge_files_into_mod(
                    file_list,
                    &pending.game,
                    &mod_name,
                    &existing_mod_id,
                    &tracker,
                    &cache_root,
                    pending.file_targets,
                    pending.stripped_wrapper,
                    on_progress,
                )
                .await
                .map_err(|e| e.to_string())?;
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
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        self.installing = false;
        self.status_msg = None;

        match result {
            Ok(PrepareResultMsg::Normal {
                file_list,
                stripped_wrapper,
                tmp_dir,
                mod_name,
                archive_hash,
            }) => {
                let game = self.selected_game().cloned().unwrap();
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
                    file_targets: HashMap::new(),
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
                    .map(|(mid, fid, _)| (*mid, *fid));
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
                        let dialog = gtk::AlertDialog::builder()
                            .message("Mod Already Installed")
                            .detail(format!(
                                "\"{old_mod_name}\" is already installed. Replace it or install alongside?"
                            ))
                            .buttons(["Cancel", "Install Alongside", "Replace"])
                            .cancel_button(0)
                            .default_button(1)
                            .modal(true)
                            .build();
                        let input_sender = sender.input_sender().clone();
                        dialog.choose(Some(root), None::<&gio::Cancellable>, move |result| {
                            let _ = match result {
                                Ok(1) => input_sender.send(AppMsg::OpenPreInstallDialog),
                                Ok(2) => input_sender.send(AppMsg::OpenPreInstallDialogReplacing(
                                    old_mod_id,
                                    old_priority,
                                )),
                                _ => input_sender.send(AppMsg::PreInstallCancelled),
                            };
                        });
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
            }) => {
                let game = self.selected_game().cloned().unwrap();
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
                    file_targets: HashMap::new(),
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
                    .map(|(mid, fid, _)| (*mid, *fid));
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
                        let dialog = gtk::AlertDialog::builder()
                            .message("Mod Already Installed")
                            .detail(format!(
                                "\"{old_mod_name}\" is already installed. Replace it or install alongside?"
                            ))
                            .buttons(["Cancel", "Install Alongside", "Replace"])
                            .cancel_button(0)
                            .default_button(1)
                            .modal(true)
                            .build();
                        let input_sender = sender.input_sender().clone();
                        dialog.choose(Some(root), None::<&gio::Cancellable>, move |result| {
                            let _ = match result {
                                Ok(1) => input_sender.send(AppMsg::OpenPreInstallDialog),
                                Ok(2) => input_sender.send(AppMsg::OpenPreInstallDialogReplacing(
                                    old_mod_id,
                                    old_priority,
                                )),
                                _ => input_sender.send(AppMsg::PreInstallCancelled),
                            };
                        });
                    }
                } else {
                    self.reinstall_mode = false;
                    self.open_pre_install_dialog(root, sender);
                }
            }
            Err(e) => {
                self.reinstall_mode = false;
                if let Some(dl_id) = self.active_download_id.take() {
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
                self.toaster.toast(&format!("Add failed: {e}"));
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
        self.status_msg = None;

        let maybe_archive_hash = match &result {
            Ok(add_result) => add_result.mod_entry.archive_hash.clone(),
            Err(_) => None,
        };
        let metadata_dl_id = self.active_download_id.clone();

        if let Some(dl_id) = self.active_download_id.take() {
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
                self.reload_mods(sender);
                let msg = if was_replace {
                    "Mod replaced — deploy to update game files".to_string()
                } else {
                    let mut m = format!("Added {count} files");
                    if plugins > 0 {
                        m.push_str(&format!(", {plugins} plugin(s)"));
                    }
                    m
                };
                self.toaster.toast(&msg);
                if let Some(dialog) = self.absorb_dialog.take() {
                    dialog.widget().destroy();
                }

                if let (Some(nexus_mod_id), Some(nexus_domain)) = (
                    add_result.mod_entry.nexus_mod_id,
                    add_result.mod_entry.nexus_domain.as_deref(),
                ) {
                    let tracker = self.tracker.clone().unwrap();
                    let mod_id = add_result.mod_entry.id.clone();
                    let domain = nexus_domain.to_string();
                    let nexus_file_id = add_result.mod_entry.nexus_file_id;
                    let dl_id_for_metadata = metadata_dl_id;
                    sender.oneshot_command(async move {
                        let result: Result<(String, String, String, String), String> = async {
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
                            let installed_version =
                                if let Some(fid) = nexus_file_id.filter(|&f| f != 0) {
                                    let (files_resp, _) = client
                                        .get_mod_files(&domain, nexus_mod_id)
                                        .await
                                        .map_err(|e| e.to_string())?;
                                    files_resp
                                        .files
                                        .into_iter()
                                        .find(|f| f.file_id == fid)
                                        .and_then(|f| f.version)
                                        .unwrap_or_else(|| info.version.clone())
                                } else {
                                    info.version.clone()
                                };
                            tracker
                                .set_mod_installed_version(&mod_id, &installed_version)
                                .await
                                .map_err(|e| e.to_string())?;
                            Ok((mod_id, installed_version, info.author, info.name))
                        }
                        .await;
                        AppCmdMsg::NexusMetadataFetched(dl_id_for_metadata, result)
                    });
                }
            }
            Err(e) => {
                self.toaster.toast(&format!("Add failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_cmd_mod_merged(
        &mut self,
        result: Result<(String, usize), String>,
        sender: &ComponentSender<Self>,
    ) {
        self.installing = false;
        self.status_msg = None;

        if let Some(dl_id) = self.active_download_id.take() {
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
                self.toaster.toast(&format!(
                    "Merged {count} file(s) into '{mod_name}' — deploy to update game files"
                ));
                if let Some(dialog) = self.absorb_dialog.take() {
                    dialog.widget().destroy();
                }
            }
            Err(e) => {
                self.toaster.toast(&format!("Merge failed: {e}"));
            }
        }
    }
}
