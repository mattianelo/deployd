use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, atomic::AtomicBool};

use gtk::glib;
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;

use super::types::{
    ModFilter, PendingInstall, PendingMigrationImport, SearchScope, ToolLaunchSession, WorkStatus,
};
use crate::core::detector::ExternalFile;
use crate::core::tracker::Tracker;
use crate::models::download::{DownloadEntry, DownloadFilter, DownloadSort};
use crate::models::game::Game;
use crate::models::order_snapshot::OrderSnapshot;
use crate::models::plugin::PluginDirtyInfo;
use crate::models::profile::Profile;
use crate::models::tool::Tool;
use crate::ui::absorb_dialog::AbsorbDialog;
use crate::ui::bottom_status::BottomStatus;
use crate::ui::download_row::DownloadRow;
use crate::ui::downloads_pane::DownloadsPane;
use crate::ui::fomod_dialog::FomodDialog;
use crate::ui::game_setup_dialog::GameSetupDialog;
use crate::ui::header::Header;
use crate::ui::mod_list::ModListItem;
use crate::ui::mod_properties_dialog::ModPropertiesDialog;
use crate::ui::plugin_list::PluginRow;
use crate::ui::pre_install_dialog::PreInstallDialog;
use crate::ui::settings_dialog::SettingsDialog;
use crate::ui::tool_manager::ToolManager;
use crate::ui::welcome_wizard::WelcomeWizard;

pub(crate) struct App {
    pub(crate) shell: ShellState,
    pub(crate) session: SessionState,
    pub(crate) mods: ModState,
    pub(crate) plugins: PluginState,
    pub(crate) install: InstallSession,
    pub(crate) tools: ToolState,
    pub(crate) ui: UiState,
    pub(crate) download: DownloadState,
}

pub(crate) struct ShellState {
    pub(crate) deploying: bool,
    pub(crate) needs_deploy: bool,
    pub(crate) status_msg: Option<String>,
    pub(crate) work_status: Option<WorkStatus>,
    pub(crate) search_active: bool,
    pub(crate) search_text: String,
    pub(crate) pending_search_text: Option<String>,
    pub(crate) search_debounce: Option<glib::SourceId>,
    pub(crate) search_scope: SearchScope,
    pub(crate) nexus_username: Option<String>,
    pub(crate) nexus_avatar_url: Option<String>,
    pub(crate) nexus_is_premium: bool,
    pub(crate) app_update_version: Option<String>,
    pub(crate) app_update_url: Option<String>,
    pub(crate) running_as_appimage: bool,
    pub(crate) color_scheme_idx: u32,
}

pub(crate) struct ToolState {
    pub(crate) entries: Vec<Tool>,
    pub(crate) launch_cancel: Option<Arc<AtomicBool>>,
    pub(crate) launch_session: Option<ToolLaunchSession>,
    pub(crate) proton_setup: bool,
}

pub(crate) struct UiState {
    pub(crate) header: Controller<Header>,
    pub(crate) bottom_status: Controller<BottomStatus>,
    pub(crate) toast_overlay: adw::ToastOverlay,
    pub(crate) notification_sender: relm4::Sender<super::messages::AppMsg>,
    pub(crate) notification_list: gtk::ListBox,
    pub(crate) notification_count: usize,
    pub(crate) profile_model: gtk::StringList,
    pub(crate) profile_dropdown: gtk::DropDown,
    pub(crate) game_model: gtk::StringList,
    pub(crate) game_dropdown: gtk::DropDown,
    pub(crate) pre_install_dialog: Option<Controller<PreInstallDialog>>,
    pub(crate) fomod_dialog: Option<Controller<FomodDialog>>,
    pub(crate) downloads_pane: Controller<DownloadsPane>,
    pub(crate) tool_buttons_box: gtk::Box,
    pub(crate) tool_manager_dialog: Option<Controller<ToolManager>>,
    pub(crate) tool_launch_dialog: Option<adw::AlertDialog>,
    pub(crate) game_setup_dialog: Option<Controller<GameSetupDialog>>,
    pub(crate) welcome_wizard: Option<Controller<WelcomeWizard>>,
    pub(crate) settings_dialog: Option<Controller<SettingsDialog>>,
    pub(crate) pending_migration_import: Option<PendingMigrationImport>,
    pub(crate) mod_properties_dialog: Option<Controller<ModPropertiesDialog>>,
    pub(crate) absorb_dialog: Option<Controller<AbsorbDialog>>,
    pub(crate) profile_rename_entry: gtk::Entry,
    pub(crate) notifications_menu_btn: gtk::MenuButton,
    pub(crate) deploy_options_btn: gtk::MenuButton,
    pub(crate) overflow_menu_btn: gtk::MenuButton,
    pub(crate) profile_menu_btn: gtk::MenuButton,
    pub(crate) nexus_user_btn: gtk::MenuButton,
    pub(crate) nexus_avatar_widget: adw::Avatar,
}

pub(crate) struct SessionState {
    pub(crate) initializing: bool,
    pub(crate) tracker: Option<Tracker>,
    pub(crate) games: Vec<Game>,
    pub(crate) selected_game_idx: usize,
    pub(crate) profiles: Vec<Profile>,
    pub(crate) active_profile_idx: usize,
    pub(crate) updating_profiles: bool,
    pub(crate) pending_save_profile_idx: Option<usize>,
    pub(crate) game_cache_dirs: HashMap<String, PathBuf>,
    pub(crate) pending_new_game_ids: Vec<String>,
    pub(crate) last_deployed_profile_id: Option<String>,
}

pub(crate) struct ModState {
    pub(crate) rows: FactoryVecDeque<ModListItem>,
    pub(crate) collapsed_groups: HashSet<String>,
    pub(crate) selection_active: bool,
    pub(crate) selection_dirty: bool,
    pub(crate) selected: HashSet<usize>,
    pub(crate) filter: ModFilter,
    pub(crate) pending_external_files: Vec<ExternalFile>,
    pub(crate) external_changes_count: usize,
    pub(crate) pending_scroll_restore: Option<f64>,
    pub(crate) scroll: gtk::ScrolledWindow,
    pub(crate) snapshots: Vec<OrderSnapshot>,
    pub(crate) snapshot_save_entry: gtk::Entry,
    pub(crate) snapshots_list: gtk::ListBox,
}

pub(crate) struct PluginState {
    pub(crate) rows: FactoryVecDeque<PluginRow>,
    pub(crate) selection_active: bool,
    pub(crate) selection_dirty: bool,
    pub(crate) selected: HashSet<usize>,
    pub(crate) scroll: gtk::ScrolledWindow,
    #[cfg(feature = "loot")]
    pub(crate) dirty: HashMap<String, PluginDirtyInfo>,
    pub(crate) pending_post_loot_action: super::types::PostLootAction,
    pub(crate) show_vanilla: bool,
    pub(crate) managed_count: usize,
    pub(crate) vanilla_names: Vec<String>,
    pub(crate) vanilla_derived: HashSet<String>,
    pub(crate) masters: HashMap<String, Vec<String>>,
    pub(crate) snapshots: Vec<OrderSnapshot>,
    pub(crate) snapshot_save_entry: gtk::Entry,
    pub(crate) snapshots_list: gtk::ListBox,
}

pub(crate) struct DownloadState {
    pub(crate) rows: FactoryVecDeque<DownloadRow>,
    pub(crate) all: Vec<DownloadEntry>,
    pub(crate) visible: bool,
    pub(crate) metadata_previous_status: HashMap<String, crate::models::download::DownloadStatus>,
    pub(crate) active_count: usize,
    pub(crate) global_active_count: usize,
    pub(crate) directory: PathBuf,
    pub(crate) initial_scan_done: bool,
    pub(crate) sort: DownloadSort,
    pub(crate) filter: DownloadFilter,
    pub(crate) show_hidden: bool,
    pub(crate) scroll: gtk::ScrolledWindow,
    pub(crate) rate_limit: Option<crate::core::nexus_api::RateLimitInfo>,
    pub(crate) pending_nxm: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum InstallStage {
    #[default]
    Idle,
    PreparingArchive,
    AwaitingFileId,
    AwaitingPreInstall,
    AwaitingFomod,
    Committing,
    Cancelled,
    Succeeded,
    Failed,
}

#[derive(Default)]
pub(crate) struct InstallSession {
    pub(crate) stage: InstallStage,
    generation: u64,
    identity: Option<InstallIdentity>,
    pub(crate) pending: Option<PendingInstall>,
    pub(crate) nexus_ids: Option<crate::models::download::NexusIds>,
    pub(crate) active_download_id: Option<String>,
    pub(crate) replacement: Option<(String, i32)>,
    pub(crate) reinstalling: bool,
    pub(crate) fetched_name: Option<String>,
    pub(crate) file_id_needed: Option<crate::app::types::FileIdNeeded>,
    pub(crate) fomod_selections: Option<Vec<Vec<std::collections::HashSet<usize>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallIdentity {
    generation: u64,
    pub(crate) game_id: String,
    pub(crate) download_id: Option<String>,
}

impl InstallSession {
    pub(crate) fn begin(
        &mut self,
        game_id: String,
        download_id: Option<String>,
    ) -> InstallIdentity {
        self.generation = self.generation.wrapping_add(1);
        let identity = InstallIdentity {
            generation: self.generation,
            game_id,
            download_id,
        };
        self.identity = Some(identity.clone());
        identity
    }

    pub(crate) fn identity(&self) -> Option<InstallIdentity> {
        self.identity.clone()
    }

    pub(crate) fn accepts(&self, identity: &InstallIdentity) -> bool {
        self.identity.as_ref() == Some(identity)
    }

    pub(crate) fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.identity = None;
    }

    pub(crate) fn set_stage(&mut self, stage: InstallStage) {
        self.stage = stage;
    }

    pub(crate) fn is_busy(&self) -> bool {
        matches!(
            self.stage,
            InstallStage::PreparingArchive | InstallStage::Committing
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{InstallSession, InstallStage};

    #[test]
    fn reports_only_active_install_work_as_busy() {
        let mut state = InstallSession::default();
        assert!(!state.is_busy());

        for stage in [InstallStage::PreparingArchive, InstallStage::Committing] {
            state.set_stage(stage);
            assert!(state.is_busy());
        }

        for stage in [
            InstallStage::AwaitingFileId,
            InstallStage::AwaitingPreInstall,
            InstallStage::AwaitingFomod,
            InstallStage::Cancelled,
            InstallStage::Succeeded,
            InstallStage::Failed,
        ] {
            state.set_stage(stage);
            assert!(!state.is_busy());
        }
    }

    #[test]
    fn rejects_results_from_an_older_install_session() {
        let mut state = InstallSession::default();
        let stale = state.begin("game-a".to_string(), Some("download-a".to_string()));
        let current = state.begin("game-b".to_string(), Some("download-b".to_string()));

        assert!(!state.accepts(&stale));
        assert!(state.accepts(&current));
    }

    #[test]
    fn cancellation_invalidates_pending_results() {
        let mut state = InstallSession::default();
        let identity = state.begin("game".to_string(), None);

        state.invalidate();

        assert!(!state.accepts(&identity));
    }
}
