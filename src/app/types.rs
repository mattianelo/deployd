use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tempfile::TempDir;

use crate::core::tracker::{OverrideInfo, PersistedGame, Tracker};
use crate::models::download::{DownloadEntry, NexusIds};
use crate::models::game::Game;
use crate::models::group::ModGroup;
use crate::models::mod_entry::{InstallTarget, ModEntry};
use crate::models::plugin::Plugin;
use crate::models::profile::Profile;
use crate::models::tool::Tool;
use crate::utils::fomod_resolver;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    All,
    ModOrder,
    PluginOrder,
    Downloads,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum DownloadSort {
    #[default]
    Default,
    Name,
    Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModFilter {
    #[default]
    All,
    Enabled,
    Issues,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DownloadFilter {
    #[default]
    All,
    Active,
    Completed,
}

pub(crate) struct PendingInstall {
    pub(crate) tmp_dir: TempDir,
    pub(crate) mod_name: String,
    pub(crate) game: Game,
    /// For normal mods: pre-resolved file list ready to install.
    pub(crate) file_list: Option<Vec<(PathBuf, PathBuf)>>,
    /// Wrapper directory name stripped by detect_wrapper (e.g. "modSkipMovies").
    /// Passed to the installer so W3 mods keep their original Mods/ folder name.
    pub(crate) stripped_wrapper: Option<String>,
    /// For FOMOD mods: path to the ModuleConfig.xml.
    pub(crate) fomod_config_path: Option<PathBuf>,
    /// For FOMOD mods: parsed UI config for the wizard.
    pub(crate) fomod_config: Option<fomod_resolver::FomodUiConfig>,
    /// Nexus IDs from NXM download (mod_id, file_id, domain).
    pub(crate) nexus_ids: Option<NexusIds>,
    /// SHA-256 hex digest of the source archive, used for duplicate detection.
    pub(crate) archive_hash: Option<String>,
    /// Per-file install targets keyed by dest_rel path string.
    /// Empty map means auto-detect all (used for FOMOD installs).
    pub(crate) file_targets: HashMap<String, InstallTarget>,
    /// Dest_rel keys for files the user chose to skip. Empty for FOMOD installs.
    pub(crate) excluded_files: HashSet<String>,
}

/// Carried when the install-time Nexus fetch resolved the mod name but
/// could not match any file entry (neither by file_id nor by archive filename).
#[derive(Debug)]
pub(crate) struct FileIdNeeded {
    pub(crate) download_id: String,
    pub(crate) mod_id: i64,
    pub(crate) domain: String,
}

#[derive(Debug)]
pub struct NxmDownloadResult {
    pub download_id: String,
    pub archive_path: PathBuf,
    pub mod_id: i64,
    pub file_id: i64,
    pub domain: String,
    pub file_name: String,
    pub nexus_file_name: Option<String>,
    pub nexus_is_primary: bool,
    pub version: Option<String>,
}

#[derive(Debug)]
pub struct InitData {
    pub tracker: Tracker,
    pub mods: Vec<ModEntry>,
    pub plugins: Vec<Plugin>,
    pub plugin_masters: HashMap<String, Vec<String>>,
    pub overrides: HashMap<String, OverrideInfo>,
    pub profiles: Vec<Profile>,
    pub active_profile_idx: usize,
    pub tools: Vec<Tool>,
    /// ID of the game that should be selected on startup. Resolved to a list index
    /// in handle_cmd_initialized after self.games is fully merged and pruned.
    pub init_game_id: Option<String>,
    pub downloads_dir: Option<PathBuf>,
    pub game_cache_dirs: HashMap<String, PathBuf>,
    pub download_entries: Vec<DownloadEntry>,
    pub rate_limit_info: Option<crate::core::nexus_api::RateLimitInfo>,
    pub vanilla_plugins: HashSet<String>,
    pub groups: Vec<ModGroup>,
    pub vanilla_plugin_master_counts: HashMap<String, usize>,
    pub vanilla_derived_plugins: HashSet<String>,
    /// Game records previously persisted to the DB (custom games + overrides).
    pub persisted_games: Vec<PersistedGame>,
    /// IDs of games the user has explicitly hidden.
    pub hidden_game_ids: Vec<String>,
    /// Profile ID that was active during the last successful deploy for the selected game.
    pub last_deployed_profile_id: Option<String>,
    /// True when this is the very first launch (no games persisted, wizard not yet shown).
    pub first_launch: bool,
    /// Nexus username from the last successful API key validation.
    pub nexus_username: Option<String>,
    /// Nexus avatar image URL from the last successful API key validation.
    pub nexus_avatar_url: Option<String>,
    /// Whether the logged-in Nexus user is premium.
    pub nexus_is_premium: bool,
    pub compact_plugin_rows: bool,
    pub color_scheme_idx: u32,
    /// True when this launch follows a full-backup DB restore (marker file was present).
    pub restored_from_backup: bool,
}

#[derive(Debug)]
pub struct LoadedData {
    pub mods: Vec<ModEntry>,
    pub plugins: Vec<Plugin>,
    pub plugin_masters: HashMap<String, Vec<String>>,
    pub overrides: HashMap<String, OverrideInfo>,
    pub profiles: Vec<Profile>,
    pub active_profile_idx: usize,
    pub tools: Vec<Tool>,
    pub vanilla_plugins: HashSet<String>,
    pub groups: Vec<ModGroup>,
    /// Lowercase filename → number of masters declared in TES4 header.
    /// Used to sort vanilla/DLC plugins by dependency depth (root masters first).
    pub vanilla_plugin_master_counts: HashMap<String, usize>,
    /// Lowercase filenames of managed plugins that were originally vanilla game files
    /// (deployd backed them up before a mod overwrote them, e.g. a cleaned Fallout4.esm).
    pub vanilla_derived_plugins: HashSet<String>,
}

pub(crate) fn download_status_sort_key(status: &crate::models::download::DownloadStatus) -> u8 {
    use crate::models::download::DownloadStatus;
    match status {
        DownloadStatus::Downloading => 0,
        DownloadStatus::Paused => 1,
        DownloadStatus::Extracting => 2,
        DownloadStatus::Downloaded => 3,
        DownloadStatus::Installed => 4,
        DownloadStatus::Failed => 5,
    }
}
