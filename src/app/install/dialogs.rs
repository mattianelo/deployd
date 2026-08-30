use relm4::prelude::*;

use crate::ui::mod_list::ModListItemKind;
use crate::ui::pre_install_dialog::{
    PreInstallDialog, PreInstallDialogInit, PreInstallDialogOutput,
};

use super::super::App;
use super::super::messages::AppMsg;
use super::super::state::InstallStage;
use super::super::types::WorkKind;

impl App {
    /// Find an installed mod that has the given Nexus mod ID.
    /// Returns `(mod_id, mod_name, priority)` if found.
    pub(crate) fn find_installed_mod_by_nexus_id(
        &mut self,
        nexus_mod_id: i64,
        nexus_file_id: i64,
    ) -> Option<(String, String, i32)> {
        // file_id == 0 means the archive was scanned from disk without a known Nexus file ID
        // (the filename only encodes the mod ID). We can't distinguish between different files
        // from the same mod page in that case, so skip the duplicate check to avoid false positives.
        if nexus_file_id == 0 {
            return None;
        }
        let guard = self.mods.rows.guard();
        for item in guard.iter() {
            if let ModListItemKind::Mod(ref init) = item.kind
                && init.mod_entry.nexus_mod_id == Some(nexus_mod_id)
                && init.mod_entry.nexus_file_id == Some(nexus_file_id)
            {
                return Some((
                    init.mod_entry.id.clone(),
                    init.mod_entry.name.clone(),
                    init.mod_entry.priority,
                ));
            }
        }
        None
    }

    /// Find an installed mod whose `archive_hash` matches the given SHA-256 hex string.
    /// Returns `(mod_id, mod_name, priority)` if found.
    pub(crate) fn find_mod_by_archive_hash(&self, hash: &str) -> Option<(String, String, i32)> {
        self.mods.rows.iter().find_map(|item| {
            if let ModListItemKind::Mod(ref init) = item.kind
                && init
                    .mod_entry
                    .archive_hash
                    .as_deref()
                    .is_some_and(|h| h == hash)
            {
                Some((
                    init.mod_entry.id.clone(),
                    init.mod_entry.name.clone(),
                    init.mod_entry.priority,
                ))
            } else {
                None
            }
        })
    }

    /// Open the pre-install name/target dialog for the current `pending_install`.
    pub(crate) fn open_pre_install_dialog(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        // If the install-time fetch found the mod name but could not match a Nexus file
        // entry, ask the user to supply a file ID before opening the pre-install dialog.
        if self.install.file_id_needed.is_some() {
            self.install.set_stage(InstallStage::AwaitingFileId);
            self.finish_current_work();
            self.show_file_id_dialog(root, sender);
            return;
        }
        // If a background Nexus fetch completed during extraction, apply it before
        // reading pending.mod_name so the dialog proposes the real mod name.
        if let Some(fetched) = self.install.fetched_name.take()
            && let Some(pending) = &mut self.install.pending
        {
            pending.mod_name = fetched;
        }
        let Some(pending) = &self.install.pending else {
            self.install.set_stage(InstallStage::Failed);
            self.finish_current_work();
            return;
        };
        let mod_name = pending.mod_name.clone();
        let is_fomod = pending.fomod_config.is_some();
        let is_bethesda = pending.game.engine == crate::models::game::GameEngine::Bethesda;
        let is_aurora = pending.game.engine == crate::models::game::GameEngine::Aurora;
        let game_id = pending.game.id.clone();
        let engine = pending.game.engine.clone();
        let data_subdir = pending.game.data_subdir.clone();
        let file_list = pending.file_list.clone();
        self.update_work(WorkKind::PreparingSetup, "Preparing setup screen...", None);
        let file_preview = if let Some(ref fl) = file_list {
            let rules = crate::core::rules::rules_for_game(&game_id);
            crate::ui::pre_install_dialog::file_preview_from_list(fl, &rules, engine, &data_subdir)
        } else {
            vec![]
        };
        let mod_names: Vec<String> = self
            .mods
            .rows
            .iter()
            .filter_map(|item| {
                if item.is_separator() {
                    None
                } else {
                    Some(item.mod_name().to_owned())
                }
            })
            .collect();
        self.ui.pre_install_dialog = Some(
            PreInstallDialog::builder()
                .transient_for(root)
                .launch(PreInstallDialogInit {
                    mod_name,
                    file_preview,
                    is_fomod,
                    is_bethesda,
                    is_aurora,
                    mod_names,
                })
                .forward(sender.input_sender(), |output| match output {
                    PreInstallDialogOutput::Confirmed(name, targets, excluded) => {
                        AppMsg::Install(crate::app::messages::InstallMsg::PreInstallConfirmed(
                            name, targets, excluded,
                        ))
                    }
                    PreInstallDialogOutput::Cancelled => {
                        AppMsg::Install(crate::app::messages::InstallMsg::PreInstallCancelled)
                    }
                }),
        );
        self.install.set_stage(InstallStage::AwaitingPreInstall);
        if let Some(dl_id) = self.install.active_download_id.clone() {
            self.update_download_status(
                &dl_id,
                crate::models::download::DownloadStatus::Extracting,
                "Preparing setup screen...",
            );
        }
        self.finish_work(WorkKind::PreparingSetup);
    }
}
