use std::collections::HashMap;
use std::path::PathBuf;

use relm4::factory::DynamicIndex;
use tempfile::TempDir;

use crate::core::deployer::DeployResult;
use crate::core::detector::ExternalFile;
use crate::core::installer::AddResult;
use crate::core::save_manager;
use crate::models::game::Game;
use crate::models::manifest::ModFile;
use crate::models::mod_entry::InstallTarget;
#[cfg(feature = "loot")]
use crate::models::plugin::PluginDirtyInfo;
use crate::models::tool::Tool;
use crate::utils::fomod_resolver;

use super::types::{InitData, LoadedData, NxmDownloadResult};

/// A game entry produced by the game setup dialog or welcome wizard, carrying the user-confirmed configuration.
#[derive(Debug, Clone)]
pub struct GameConfig {
    pub game: Game,
    /// `true` if manually added by the user.
    pub custom: bool,
}

#[derive(Debug)]
pub enum AppMsg {
    Noop,
    GameSelected(u32),
    InstallClicked,
    FileChosen(PathBuf),
    PreInstallConfirmed(String, HashMap<String, InstallTarget>),
    PreInstallCancelled,
    FomodConfirmed(fomod_resolver::FomodSelections),
    FomodCancelled,
    RemoveMod(DynamicIndex),
    ToggleModEnabled(DynamicIndex, bool),
    MoveModTo(usize, usize),
    MoveGroupTo(usize, usize),
    MoveSelectedModsTo {
        selected: Vec<usize>,
        from: usize,
        to: usize,
    },
    MovePluginTo(usize, usize),
    MoveSelectedPluginsTo {
        selected: Vec<usize>,
        from: usize,
        to: usize,
    },
    TogglePluginEnabled(DynamicIndex, bool),
    RenameMod(DynamicIndex, String),
    ProfileSelected(u32),
    NewProfileClicked,
    CloneProfileClicked,
    RenameProfile(String),
    DeleteProfileClicked,
    DeployClicked,
    /// User confirmed deploy after the cross-profile mismatch warning dialog.
    DeployConfirmed,
    PurgeClicked,
    PurgeConfirmed,
    /// Open a file-chooser dialog so the user can confirm access to the current
    /// game's installation folder.
    GrantGameFolderAccess,
    /// The user confirmed a game folder path; update the in-memory path and
    /// persist it as a hint for future sessions.
    GameFolderGranted(PathBuf),
    LaunchTool(String),
    /// Fired from the background wait-thread when a launched tool's Wine process exits.
    ToolExited(String),
    ManageToolsClicked,
    ToolAdded(Tool),
    ToolRemoved(String),
    ToolWorkingDirChanged(String, String),
    /// User confirmed merging pending files into an existing mod.
    PreInstallMerge(String),
    /// User chose to replace an existing mod (name-conflict dialog).
    PreInstallReplace(String, i32),
    /// User chose to create a new mod despite the name conflict.
    PreInstallCreateNew,
    InstallProgress(f64, String),
    ToolManagerClosed,
    SettingsClicked,
    SettingsClosed,
    /// Open the "Manage Games" setup dialog.
    ManageGamesClicked,
    /// Manage Games dialog was closed without confirming. Hides any games that
    /// auto-triggered the dialog so they do not re-prompt on the next startup.
    ManageGamesClosed,
    /// Games confirmed from the setup dialog; apply and persist the configuration.
    /// Second argument is the list of game IDs the user unchecked (to be hidden).
    GamesConfigured(Vec<GameConfig>, Vec<String>),
    /// User removed a game from management (headerbar "×" button or Manage Games dialog).
    RemoveGame(String),
    /// Remove the currently selected game (fired from the headerbar "×" button).
    RemoveCurrentGame,
    /// Emitted by SettingsDialog whenever the Nexus API key is set or cleared.
    NexusApiKeyUpdated,
    NxmLinkReceived(String),
    CheckUpdatesClicked,
    ToggleDownloads,
    SetDownloadsVisible(bool),
    InstallDownload(DynamicIndex),
    /// Reinstall an already-installed download, replacing the existing mod.
    ReinstallDownload(DynamicIndex),
    ClearDownloadMetadata(DynamicIndex),
    RenameDownload(DynamicIndex),
    /// (download_id, new_name) — confirmed from the rename dialog
    ConfirmDownloadRename(String, String),
    DownloadProgress(String, f64, String),
    /// (download_id, mod_name, game_domain, nexus_file_name, nexus_is_primary)
    DownloadNameResolved(String, String, Option<String>, Option<String>, bool),
    FetchDownloadMetadata(DynamicIndex),
    ScanDownloadsFolder,
    DownloadSortChanged(u32),
    SearchToggled(bool),
    SearchChanged(String),
    SearchScopeChanged(u32),
    RateLimitUpdated(crate::core::nexus_api::RateLimitInfo),
    CloseRequested,
    ConfirmClose,
    /// Toggle collapse state of a group separator (identified by factory index).
    ToggleGroupCollapse(DynamicIndex),
    /// Delete a group separator (identified by factory index).
    DeleteGroup(DynamicIndex),
    /// Create a new group at the end of the mod list with the given name.
    CreateGroup(String),
    /// Rename an existing group separator (identified by factory index).
    RenameGroup(DynamicIndex, String),
    /// Open the Properties dialog for a mod row (right-click).
    OpenModProperties(DynamicIndex),
    /// Apply changes from the mod Properties dialog.
    ModPropertiesApplied {
        mod_id: String,
        mod_idx: usize,
        name: String,
        notes: String,
        install_target: InstallTarget,
        /// Per-file targets: current game_rel_lowercase → desired InstallTarget.
        file_targets: HashMap<String, InstallTarget>,
    },
    /// User cancelled the mod Properties dialog.
    ModPropertiesCancelled,
    /// Trigger a scan of the game folder for files not tracked by any mod.
    ScanExternalFiles,
    /// User clicked the external-changes badge — open the file-selection dialog.
    AbsorbExternalFiles,
    /// Files selected in the "Create Mod from External Files" dialog; open PreInstallDialog.
    AbsorbFilesSelected(Vec<(PathBuf, PathBuf)>),
    /// User chose to discard (delete) selected external files from the game folder.
    DiscardExternalFiles(Vec<PathBuf>),
    /// User cancelled (or closed) the "Create Mod from External Files" dialog.
    CreateModFromExternalCancelled,
    /// User chose to adopt externally-cleaned managed plugins: copy cleaned content into the
    /// deployd cache and re-hardlink so the mod stays managed with the cleaned plugin version.
    AdoptManagedPluginChanges(Vec<ExternalFile>),
    /// User chose to restore managed plugins from their xEdit backup (undo the external clean).
    RestoreFromXEditBackup(Vec<ExternalFile>),
    /// Show confirmation dialog before resetting the vanilla baseline.
    ResetVanillaBaseline,
    /// User confirmed the reset — delete and re-take the vanilla snapshot.
    ResetVanillaBaselineConfirmed,
    /// Mark selected external files as vanilla (update their baseline entry in DB).
    MarkExternalFilesAsVanilla(Vec<ExternalFile>),
    /// Create an empty mod with a cache folder, then open the file manager there.
    CreateEmptyMod,
    /// Re-register all files in a mod's cache folder as its mod_files records.
    ScanModFromCache(String),
    /// Export the active profile to a JSON file.
    ExportProfileClicked,
    /// Import a profile from a JSON file (open file chooser).
    ImportProfileClicked,
    /// File chosen for profile import.
    ImportProfileFileChosen(PathBuf),
    /// Result of an async profile export operation.
    ProfileExported(Result<(), String>),
    /// Open the pre-install dialog without replacing any existing mod.
    OpenPreInstallDialog,
    /// Open the pre-install dialog and replace the given mod (id, old_priority) after successful install.
    OpenPreInstallDialogReplacing(String, i32),
    /// Show the first-launch welcome wizard.
    ShowWelcomeWizard,
    /// The welcome wizard was confirmed — apply game configuration.
    WelcomeWizardConfirmed(Vec<GameConfig>, Vec<String>),
    /// The welcome wizard was closed without confirming.
    WelcomeWizardSkipped,
    /// Sort the Plugin Order panel using LOOT's masterlist algorithm.
    SortWithLoot,
    /// Enable all mods for the current game.
    EnableAllMods,
    /// Disable all mods for the current game.
    DisableAllMods,
    /// Enable all plugins for the current game.
    EnableAllPlugins,
    /// Disable all plugins for the current game.
    DisableAllPlugins,
    /// Toggle visibility of vanilla/DLC plugins in the plugin panel.
    ToggleShowVanillaPlugins,
    /// Show a toast notification with the given message.
    /// Used by async tasks that cannot call self.toaster directly.
    ShowToast(String),
    /// Toggle save management mode for the active profile.
    ToggleProfileSaveMode,
    /// Manually sync the active profile's saves from the game save directory.
    SyncSaves,
    /// Save the current mod order as a named snapshot.
    SaveModOrderSnapshot(String),
    /// Save the current plugin order as a named snapshot.
    SavePluginOrderSnapshot(String),
    /// Restore mod order from a saved snapshot (snapshot_id).
    LoadModOrderSnapshot(String),
    /// Restore plugin order from a saved snapshot (snapshot_id).
    LoadPluginOrderSnapshot(String),
    /// Delete a saved mod order snapshot (snapshot_id).
    DeleteModOrderSnapshot(String),
    /// Delete a saved plugin order snapshot (snapshot_id).
    DeletePluginOrderSnapshot(String),
    /// A newer app version is available; reveal the update banner.
    AppUpdateAvailable(String, String),
    /// User clicked the update banner button — open the update page.
    OpenUpdatePage,
    /// User clicked "Download Update" — download and replace the current AppImage.
    SelfUpdateDownload,
}

pub(crate) enum PrepareResultMsg {
    Normal {
        file_list: Vec<(PathBuf, PathBuf)>,
        stripped_wrapper: Option<String>,
        tmp_dir: TempDir,
        mod_name: String,
        archive_hash: Option<String>,
    },
    Fomod {
        config: fomod_resolver::FomodUiConfig,
        config_path: PathBuf,
        tmp_dir: TempDir,
        mod_name: String,
        archive_hash: Option<String>,
    },
}

// PrepareResultMsg contains TempDir which is not Debug
impl std::fmt::Debug for PrepareResultMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrepareResultMsg::Normal { mod_name, .. } => f
                .debug_struct("Normal")
                .field("mod_name", mod_name)
                .finish(),
            PrepareResultMsg::Fomod { mod_name, .. } => {
                f.debug_struct("Fomod").field("mod_name", mod_name).finish()
            }
        }
    }
}

#[derive(Debug)]
pub enum AppCmdMsg {
    Initialized(Result<InitData, String>),
    ModsLoaded(Result<LoadedData, String>),
    ModAdded(Result<AddResult, String>, bool),
    ModPrepared(Result<PrepareResultMsg, String>),
    ModRemoved(
        Result<String, String>,
        Option<(i64, i64)>,
        String,
        Option<String>,
    ),
    DeployDone(Result<DeployResult, String>),
    PurgeDone(Result<usize, String>),
    PrioritySaved(Result<(), String>),
    PluginOrderSaved(Result<(), String>),
    ProfileSwitched(Result<(LoadedData, Option<save_manager::SaveSyncResult>), String>),
    ProfileCreated(Result<LoadedData, String>),
    ProfileCloned(Result<LoadedData, String>),
    ProfileRenamed(Result<(), String>),
    ProfileDeleted(Result<LoadedData, String>),
    ToolSaved(Result<(), String>),
    ToolDeleted(Result<String, String>),
    ToolWorkingDirSaved(Result<(), String>),
    ToolLaunched(Result<String, String>),
    /// Files were merged into an existing mod. Carries `(mod_name, files_merged)`.
    ModMerged(Result<(String, usize), String>),
    NxmDownloadComplete(String, Result<NxmDownloadResult, String>),
    NexusMetadataFetched(Result<(String, String, String, String), String>),
    UpdatesChecked(Result<Vec<(String, String, String)>, String>),
    DownloadsDirUpdated(Option<PathBuf>),
    ExternalScanDone(Result<Vec<ExternalFile>, String>),
    /// Result of adopting externally-cleaned managed plugins into the deployd cache.
    ManagedPluginsAdopted(Result<usize, String>),
    /// Result of restoring managed plugins from their xEdit backup.
    BackupRestored(Result<usize, String>),
    /// Result of resetting the vanilla snapshot for the selected game.
    VanillaBaselineReset(Result<(), String>),
    /// Result of upserting vanilla entries for individually marked files.
    VanillaEntriesUpdated(Result<usize, String>),
    ProfileImported(Result<LoadedData, String>),
    /// Last-deployed profile ID loaded from DB settings after a game switch.
    LastDeployedProfileLoaded(Option<String>),
    /// Empty mod created (mod_id, cache_dir_path).
    EmptyModCreated(Result<(String, std::path::PathBuf), String>),
    /// Mod cache rescanned — payload is a user-readable summary or error.
    ModFilesRescanned(Result<String, String>),
    /// Result of the async LOOT sort; payload is (sorted filenames, dirty-info map) on success.
    #[cfg(feature = "loot")]
    LootSortDone(Result<(Vec<String>, HashMap<String, PluginDirtyInfo>), String>),
    /// Per-file list loaded for the open mod properties dialog.
    ModFilesLoaded(Vec<ModFile>),
    /// Result of toggling profile save mode (+ optional save backup/restore op).
    SaveModeToggled(Result<(), String>),
    /// Result of a manual save sync triggered by the user.
    SavesSynced(Result<save_manager::SaveSyncResult, String>),
    /// Snapshot lists loaded for the current game.
    OrderSnapshotsLoaded(
        Vec<crate::models::order_snapshot::OrderSnapshot>,
        Vec<crate::models::order_snapshot::OrderSnapshot>,
    ),
    /// Mod order snapshot saved.
    ModOrderSnapshotSaved(Result<(), String>),
    /// Plugin order snapshot saved.
    PluginOrderSnapshotSaved(Result<(), String>),
    /// Mod order snapshot restored.
    ModOrderSnapshotRestored(Result<crate::app::types::LoadedData, String>),
    /// Plugin order snapshot restored.
    PluginOrderSnapshotRestored(Result<crate::app::types::LoadedData, String>),
    /// Mod or plugin order snapshot deleted; carries updated snapshot list (game_id, kind).
    OrderSnapshotDeleted(Result<(), String>),
    /// Result of the self-update AppImage download + replace.
    AppUpdateResult(Result<(), String>),
    /// All games have been persisted to DB after Manage Games; safe to select the first game now.
    GamesPersisted,
}
