use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use relm4::abstractions::Toaster;
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;

use super::types::{DownloadFilter, DownloadSort, ModFilter, PendingInstall, SearchScope};
use crate::core::detector::ExternalFile;
use crate::core::tracker::Tracker;
use crate::models::download::{DownloadEntry, NexusIds};
use crate::models::game::Game;
use crate::models::order_snapshot::OrderSnapshot;
use crate::models::plugin::PluginDirtyInfo;
use crate::models::profile::Profile;
use crate::models::tool::Tool;
use crate::ui::absorb_dialog::AbsorbDialog;
use crate::ui::download_row::DownloadRow;
use crate::ui::fomod_dialog::FomodDialog;
use crate::ui::game_setup_dialog::GameSetupDialog;
use crate::ui::mod_list::ModListItem;
use crate::ui::mod_properties_dialog::ModPropertiesDialog;
use crate::ui::plugin_list::PluginRow;
use crate::ui::pre_install_dialog::PreInstallDialog;
use crate::ui::settings_dialog::SettingsDialog;
use crate::ui::tool_manager::ToolManager;
use crate::ui::welcome_wizard::WelcomeWizard;

pub struct App {
    pub(crate) initializing: bool,
    pub(crate) tracker: Option<Tracker>,
    pub(crate) games: Vec<Game>,
    pub(crate) selected_game_idx: usize,
    pub(crate) mods: FactoryVecDeque<ModListItem>,
    /// Group IDs that are currently collapsed (kept in sync with DB on toggle).
    pub(crate) collapsed_groups: HashSet<String>,
    pub(crate) plugins: FactoryVecDeque<PluginRow>,
    pub(crate) installing: bool,
    pub(crate) deploying: bool,
    pub(crate) needs_deploy: bool,
    /// Profile ID that was active during the last successful deploy for the current game.
    pub(crate) last_deployed_profile_id: Option<String>,
    pub(crate) status_msg: Option<String>,
    pub(crate) toaster: Toaster,
    pub(crate) profiles: Vec<Profile>,
    pub(crate) active_profile_idx: usize,
    pub(crate) profile_model: gtk::StringList,
    pub(crate) profile_dropdown: gtk::DropDown,
    /// Suppresses the profile dropdown signal while we programmatically update it.
    pub(crate) updating_profiles: bool,
    /// Backing string model for the game dropdown (allows dynamic updates).
    pub(crate) game_model: gtk::StringList,
    /// Game selection dropdown widget.
    pub(crate) game_dropdown: gtk::DropDown,
    /// Holds extracted archive data while pre-install or FOMOD dialog is open.
    pub(crate) pending_install: Option<PendingInstall>,
    /// Active pre-install rename dialog controller.
    pub(crate) pre_install_dialog: Option<Controller<PreInstallDialog>>,
    /// Active FOMOD dialog controller.
    pub(crate) fomod_dialog: Option<Controller<FomodDialog>>,
    /// External tools configured for the selected game.
    pub(crate) tools: Vec<Tool>,
    /// Container for dynamic tool buttons in the headerbar.
    pub(crate) tool_buttons_box: gtk::Box,
    /// Active tool manager dialog controller.
    pub(crate) tool_manager_dialog: Option<Controller<ToolManager>>,
    /// Active game setup dialog controller.
    pub(crate) game_setup_dialog: Option<Controller<GameSetupDialog>>,
    /// Active first-launch welcome wizard controller.
    pub(crate) welcome_wizard: Option<Controller<WelcomeWizard>>,
    /// Active settings dialog controller.
    pub(crate) settings_dialog: Option<Controller<SettingsDialog>>,
    /// Active mod properties dialog controller.
    pub(crate) mod_properties_dialog: Option<Controller<ModPropertiesDialog>>,
    /// Active external-file absorption dialog controller.
    pub(crate) absorb_dialog: Option<Controller<AbsorbDialog>>,
    /// Entry widget for profile rename popover.
    pub(crate) profile_rename_entry: gtk::Entry,
    /// Pending NXM link received at startup (processed after initialization).
    pub(crate) pending_nxm: Option<String>,
    /// Nexus IDs to attach to the next PendingInstall (handoff from NXM download).
    pub(crate) pending_nexus_ids: Option<NexusIds>,
    /// Downloads sidebar entries (game-filtered view).
    pub(crate) downloads: FactoryVecDeque<DownloadRow>,
    /// All downloads across all games (backing store).
    pub(crate) all_downloads: Vec<DownloadEntry>,
    /// Whether the downloads sidebar is visible.
    pub(crate) downloads_visible: bool,
    /// ID of the currently active download (for status updates).
    pub(crate) active_download_id: Option<String>,
    /// Cached count of active downloads for current game (for sidebar view).
    pub(crate) active_download_count: usize,
    /// Count of active downloads across ALL games (for headerbar indicator).
    pub(crate) global_active_downloads: usize,
    /// Cached downloads directory path (avoids async DB reads during scan).
    pub(crate) downloads_dir: PathBuf,
    /// Custom cache roots per game_id. Absent = use global cache_root().
    pub(crate) game_cache_dirs: HashMap<String, PathBuf>,
    /// Whether the initial auto-scan has been performed (suppresses toast).
    pub(crate) initial_scan_done: bool,
    /// Whether the search bar is shown.
    pub(crate) search_active: bool,
    /// Current search query text.
    pub(crate) search_text: String,
    /// Which panel(s) the search applies to.
    pub(crate) search_scope: SearchScope,
    /// Latest Nexus API rate limit info.
    pub(crate) rate_limit_info: Option<crate::core::nexus_api::RateLimitInfo>,
    /// Current sort order for the downloads sidebar.
    pub(crate) download_sort: DownloadSort,
    /// Active filter chip selection for the mod order pane.
    pub(crate) mod_filter: ModFilter,
    /// Active filter chip selection for the downloads sidebar.
    pub(crate) download_filter: DownloadFilter,
    /// Files found in the game folder that are not tracked by any mod.
    pub(crate) pending_external_files: Vec<ExternalFile>,
    /// Number of external files detected (drives the badge button visibility).
    pub(crate) external_changes_count: usize,
    /// When set, (mod_id, old_priority) to remove after the pending install completes (replace flow).
    pub(crate) pending_replace_mod_id: Option<(String, i32)>,
    /// When true, the next ModPrepared result should skip the "Already Installed" dialog and go
    /// straight into the replace flow (set by the Downloads panel "Reinstall" button).
    pub(crate) reinstall_mode: bool,
    /// Scroll position to restore after the next mod list rebuild (set before a factory remove).
    pub(crate) pending_scroll_restore: Option<f64>,
    /// ScrolledWindow wrapping the mod list — held so we can restore scroll position.
    pub(crate) mod_scroll: gtk::ScrolledWindow,
    /// ScrolledWindow wrapping the downloads list — held so we can restore scroll position.
    pub(crate) downloads_scroll: gtk::ScrolledWindow,
    /// Dirty-edit info keyed by lowercase plugin filename, from the LOOT masterlist.
    /// Populated after each LOOT sort; cleared on game switch. CRC-based so entries
    /// drop automatically once the user cleans a plugin and re-sorts.
    #[cfg(feature = "loot")]
    pub(crate) dirty_plugins: HashMap<String, PluginDirtyInfo>,
    /// MenuButton that opens the notifications popover.
    pub(crate) notifications_menu_btn: gtk::MenuButton,
    /// MenuButton that opens the deploy options popover (Purge, Open folder).
    pub(crate) deploy_options_btn: gtk::MenuButton,
    /// MenuButton that opens the overflow actions popover (Settings, Purge, etc.).
    pub(crate) overflow_menu_btn: gtk::MenuButton,
    /// MenuButton that opens the profile management popover.
    pub(crate) profile_menu_btn: gtk::MenuButton,
    /// Button that shows/switches the active profile's save mode.
    pub(crate) save_mode_btn: gtk::Button,
    /// Button to manually sync saves from the game directory to the active profile snapshot.
    pub(crate) sync_saves_btn: gtk::Button,
    /// Whether vanilla/DLC plugins are shown in the plugin panel.
    pub(crate) show_vanilla_plugins: bool,
    /// Number of Deployd-managed plugin rows (vanilla rows come after this index).
    pub(crate) managed_plugins_count: usize,
    /// Sorted vanilla/DLC plugin filenames for the current game (used when toggling visibility).
    /// Also includes vanilla-derived filenames (managed mods that replaced a vanilla file).
    pub(crate) vanilla_plugin_names: Vec<String>,
    /// Lowercase filenames of managed plugins that originally were vanilla game files
    /// (e.g. a user-cleaned Fallout4.esm installed as a mod). Used to render them in the
    /// vanilla section with a "Vanilla / Modified" label instead of as a movable managed plugin.
    pub(crate) vanilla_derived_plugins: HashSet<String>,
    /// Master dependency map: plugin_id → list of master filenames (from TES4 header).
    /// Used to block drag-and-drop moves that would place a plugin before its masters.
    pub(crate) plugin_masters: HashMap<String, Vec<String>>,
    /// Nexus username of the logged-in user (cached from last successful validate_key).
    pub(crate) nexus_username: Option<String>,
    /// Avatar image URL for the logged-in Nexus user.
    pub(crate) nexus_avatar_url: Option<String>,
    /// Whether the logged-in Nexus user is a premium member.
    pub(crate) nexus_is_premium: bool,
    /// MenuButton holding the Nexus user avatar in the headerbar.
    pub(crate) nexus_user_btn: gtk::MenuButton,
    /// Avatar widget inside the nexus_user_btn child box.
    pub(crate) nexus_avatar_widget: adw::Avatar,
    /// Version string of available app update, if any.
    pub(crate) app_update_version: Option<String>,
    /// URL for the available app update (Nexus page or download link).
    pub(crate) app_update_url: Option<String>,
    /// True when the app is running as an AppImage (APPIMAGE env var is set).
    pub(crate) running_as_appimage: bool,
    /// IDs of newly-detected games that auto-triggered the Manage Games dialog.
    /// If the dialog is dismissed without confirming, these are hidden so they
    /// do not keep re-triggering the prompt on every startup.
    pub(crate) pending_new_game_ids: Vec<String>,
    /// Saved mod order snapshots for the current game.
    pub(crate) mod_order_snapshots: Vec<OrderSnapshot>,
    /// Saved plugin order snapshots for the current game.
    pub(crate) plugin_order_snapshots: Vec<OrderSnapshot>,
    /// Entry widget for the mod order snapshot name (in save popover).
    pub(crate) mod_snapshot_save_entry: gtk::Entry,
    /// Entry widget for the plugin order snapshot name (in save popover).
    pub(crate) plugin_snapshot_save_entry: gtk::Entry,
    /// ListBox holding the mod order snapshot list (in load popover).
    pub(crate) mod_snapshots_list: gtk::ListBox,
    /// ListBox holding the plugin order snapshot list (in load popover).
    pub(crate) plugin_snapshots_list: gtk::ListBox,
    /// True while UMU is downloading Proton GE for first-time tool use.
    /// Drives the headerbar spinner + status label and disables the "Add Mod"
    /// button until the download completes.
    pub(crate) proton_setup: bool,
    /// Whether plugin list rows should use compact (reduced height) display.
    pub(crate) compact_plugin_rows: bool,
    /// Persisted color scheme index: 0=System, 1=Light, 2=Dark.
    pub(crate) color_scheme_idx: u32,
    /// Mod name fetched from Nexus in parallel with archive extraction. Used to
    /// override the archive-stem name in the pre-install dialog when available.
    pub(crate) pending_fetched_name: Option<String>,
    /// Set when the install-time fetch resolved a mod name but could not match any
    /// Nexus file entry. The pre-install dialog is deferred until the user either
    /// provides a file ID or skips.
    pub(crate) pending_file_id_needed: Option<crate::app::types::FileIdNeeded>,
    /// FOMOD selections from the previous install of this mod, loaded for the
    /// reinstall/replace flow so they can be pre-populated in the dialog.
    pub(crate) pending_fomod_selections: Option<Vec<Vec<std::collections::HashSet<usize>>>>,
}
