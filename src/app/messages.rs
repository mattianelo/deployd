use std::collections::HashMap;
use std::path::PathBuf;

use relm4::factory::DynamicIndex;
use tempfile::TempDir;

use crate::core::detector::ExternalFile;
use crate::core::installer::AddResult;
use crate::core::save_manager;
use crate::models::game::GameConfig;
use crate::models::manifest::ModFile;
use crate::models::mod_entry::InstallTarget;
#[cfg(feature = "loot")]
use crate::models::plugin::PluginDirtyInfo;
use crate::models::tool::Tool;
use crate::utils::fomod_resolver;

use super::state::InstallIdentity;
use super::types::{
    DeployCompletion, DownloadScanResult, InitData, LoadedData, ModFilter, NxmDownloadResult,
};
use crate::models::download::DownloadFilter;

#[derive(Debug)]
pub(crate) enum AppMsg {
    EditAppearance,
    Generations(super::generations::Msg),
    Mele(super::mele::Msg),
    Recovery(super::location_recovery::RecoveryMsg),
    Shell(ShellMsg),
    Games(GamesMsg),
    Mods(ModsMsg),
    Plugins(PluginsMsg),
    Downloads(DownloadsMsg),
    Install(InstallMsg),
    Tools(ToolsMsg),
}

#[derive(Debug)]
pub(crate) enum ShellMsg {
    DeploymentPhase {
        id: u64,
        phase: &'static str,
    },
    DeployClicked,
    /// User confirmed deploy after the cross-profile mismatch warning dialog.
    DeployConfirmed,
    DeployVanillaConfirmed(bool),
    DeployPreflightCancelled,
    ApplyPreparedGeneration(Box<crate::core::generations::activation::Prepared>),
    DiscardPreparedGeneration(Box<crate::core::generations::activation::Prepared>),
    PurgeClicked,
    PurgeConfirmed,
    /// Open a file-chooser dialog so the user can confirm access to the current
    /// game's installation folder.
    GrantGameFolderAccess,
    /// The user confirmed a game folder path; update the in-memory path and
    /// persist it as a hint for future sessions.
    GameFolderGranted(PathBuf),
    SearchToggled(bool),
    SearchChanged(String),
    ApplySearch,
    SearchScopeChanged(u32),
    CloseRequested,
    ConfirmClose,
    /// Push a notification message to the notification panel.
    /// Used by async tasks that cannot call self.push_notification directly.
    ShowToast(String),
    /// Decrement the notification badge count when the user dismisses an item.
    NotificationDismissed,
    /// Remove all notification items and reset the badge count.
    ClearNotifications,
    /// A newer app version is available; reveal the update banner.
    AppUpdateAvailable(String, String),
    /// User clicked the update banner button — open the update page.
    /// User clicked "Download Update" — download and replace the current AppImage.
    SelfUpdateDownload,
    /// Open the current game's installation folder in the system file manager.
    OpenDeploymentFolder,
    /// Set and persist the color scheme (0=System, 1=Light, 2=Dark).
    SetColorScheme(u32),
    /// User clicked "Login with Nexus" in the headerbar avatar popover.
    NexusLoginClicked,
    /// User clicked "Log Out" in the headerbar avatar popover.
    NexusLogoutClicked,
}

#[derive(Debug)]
pub(crate) enum GamesMsg {
    GroupTrilogyProfiles,
    ApplyTrilogyProfiles(String, crate::core::tracker::trilogy_profiles::Mapping),
    CacheMoveProgress {
        done: usize,
        total: usize,
        phase: &'static str,
    },
    GameSelected(u32),
    ProfileSelected(u32),
    InitializePendingSaveSet,
    UseGlobalForPendingProfile,
    NewProfileClicked,
    CloneProfileClicked,
    RenameProfile(String),
    DeleteProfileClicked,
    DeleteProfileConfirmed,
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
    SetupProgress(crate::core::game::mass_effect::baseline::progress::Progress),
    /// User removed a game from management (headerbar "×" button or Manage Games dialog).
    /// Remove the currently selected game (fired from the headerbar "×" button).
    RemoveCurrentGame,
    /// Confirmed removal after the dialog; delete_mods=true also wipes the mod cache.
    RemoveGameConfirmed {
        game_id: String,
        delete_mods: bool,
    },
    /// User chose a new custom cache directory for a game (from the Manage Games dialog).
    CacheDirChangeRequested {
        game_id: String,
        new_dir: std::path::PathBuf,
    },
    /// User wants to revert a game's cache dir back to the global default.
    CacheDirResetRequested {
        game_id: String,
    },
    /// Emitted by SettingsDialog whenever the Nexus API key is set or cleared.
    NexusApiKeyUpdated,
    /// Show the first-launch welcome wizard.
    ShowWelcomeWizard,
    /// The welcome wizard was confirmed — apply game configuration.
    WelcomeWizardConfirmed(Vec<GameConfig>, Vec<String>),
    /// The welcome wizard was closed without confirming.
    WelcomeWizardSkipped,
    /// Toggle save management mode for the active profile.
    ToggleProfileSaveMode,
    ToggleProfileSaveModeConfirmed,
    InitializeGlobalAndDisableIsolation,
    /// Manually sync the active profile's saves from the game save directory.
    SyncSaves,
    SyncSavesConfirmed,
    ManageSaveBackups,
    CreateSaveBackup(String),
    RestoreSaveBackupRequested(String),
    RestoreSaveBackupConfirmed(String),
    DeleteSaveBackupRequested(String),
    DeleteSaveBackupConfirmed(String),
}

#[derive(Debug)]
pub(crate) enum ModsMsg {
    OverrideAction(String, super::override_panel::OverrideAction),
    ReinstallMod(DynamicIndex),
    MoveModTo(usize, usize),
    MoveGroupTo(usize, usize),
    MoveSelectedModsTo {
        selected: Vec<usize>,
        from: usize,
        to: usize,
    },
    /// Toggle collapse state of a group separator (identified by factory index).
    ToggleGroupCollapse(DynamicIndex),
    /// Delete a group separator (identified by factory index).
    DeleteGroup(DynamicIndex),
    /// Create a new group at the end of the mod list with the given name.
    CreateGroup(String),
    /// Rename an existing group separator (identified by factory index).
    RenameGroup(DynamicIndex, String),
    /// Set (or clear) the color label of a group.
    SetGroupColor(DynamicIndex, Option<String>),
    /// Open the Properties dialog for a mod row (right-click).
    OpenModProperties(DynamicIndex),
    /// Apply changes from the mod Properties dialog.
    ModPropertiesApplied {
        mod_id: String,
        name: String,
        notes: String,
        version: Option<String>,
        nexus_mod_id: Option<i64>,
        nexus_id_changed: bool,
        install_target: InstallTarget,
        /// Per-file targets: current game_rel_lowercase → desired InstallTarget.
        file_targets: HashMap<String, InstallTarget>,
        routing_changed: bool,
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
    /// Create an empty mod with a cache folder, then open the file manager there.
    CreateEmptyMod,
    /// Re-register all files in a mod's cache folder as its mod_files records.
    ScanModFromCache(String),
    /// Enable all mods for the current game.
    EnableAllMods,
    /// Disable all mods for the current game.
    DisableAllMods,
    /// Save the current mod order as a named snapshot.
    SaveModOrderSnapshot(String),
    /// Restore mod order from a saved snapshot (snapshot_id).
    LoadModOrderSnapshot(String),
    /// Delete a saved mod order snapshot (snapshot_id).
    DeleteModOrderSnapshot(String),
    /// Set the active filter chip for the mod order pane.
    SetModFilter(ModFilter),
    EnterModSelectionMode,
    ExitModSelectionMode,
    ToggleModRowSelected(usize),
    SetModRowSelected(DynamicIndex, bool),
    EnableSelectedMods,
    DisableSelectedMods,
    RemoveSelectedMods,
    ConfirmRemoveSelectedMods,
}

#[derive(Debug)]
pub(crate) enum PluginsMsg {
    MovePluginTo(usize, usize),
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
    /// Sort the Plugin Order panel using LOOT's masterlist algorithm.
    SortWithLoot,
    /// Enable all plugins for the current game.
    EnableAllPlugins,
    /// Disable all plugins for the current game.
    DisableAllPlugins,
    /// Toggle visibility of vanilla/DLC plugins in the plugin panel.
    ToggleShowVanillaPlugins,
    /// Save the current plugin order as a named snapshot.
    SavePluginOrderSnapshot(String),
    /// Restore plugin order from a saved snapshot (snapshot_id).
    LoadPluginOrderSnapshot(String),
    /// Delete a saved plugin order snapshot (snapshot_id).
    DeletePluginOrderSnapshot(String),
    EnterPluginSelectionMode,
    ExitPluginSelectionMode,
    TogglePluginRowSelected(usize),
    SetPluginRowSelected(DynamicIndex, bool),
    EnableSelectedPlugins,
    DisableSelectedPlugins,
}

#[derive(Debug)]
pub(crate) enum DownloadsMsg {
    NxmLinkReceived(String),
    SetDownloadsVisible(bool),
    InstallDownload(DynamicIndex),
    /// Reinstall an already-installed download, replacing the existing mod.
    ReinstallDownload(DynamicIndex),
    ClearDownloadMetadata(DynamicIndex),
    RenameDownload(DynamicIndex),
    DeleteDownload(DynamicIndex),
    /// download_id confirmed from the delete confirmation dialog
    ConfirmDeleteDownload(String),
    HideDownload(DynamicIndex),
    SetShowHiddenDownloads(bool),
    /// (download_id, new_name) — confirmed from the rename dialog
    ConfirmDownloadRename(String, String),
    /// Confirmed page or exact file chosen in the Nexus identity dialog.
    ConfirmNexusIdEntry(String, crate::models::download::NexusIds),
    EditDownloadIdentity(DynamicIndex),
    /// File entry couldn't be matched by filename during a standalone metadata fetch.
    /// Triggers the file ID entry dialog outside of the install flow.
    ShowFileIdDialog {
        download_id: String,
        mod_id: i64,
        domain: String,
        partial_name: Option<String>,
    },
    DownloadProgress(String, f64, String),
    /// Notifies that the MD5 of an archive was computed (lazily, during metadata fetch).
    /// Persisted so subsequent fetches skip recomputation.
    ArchiveMd5Computed(String, String),
    FetchDownloadMetadata(DynamicIndex),
    ScanDownloadsFolder,
    DownloadSortChanged(u32),
    RateLimitUpdated(crate::core::nexus_api::RateLimitInfo),
    /// Set the active filter chip for the downloads sidebar.
    SetDownloadFilter(DownloadFilter),
    /// Pause an in-progress download (download_id).
    PauseDownload(DynamicIndex),
    /// Resume a paused download (download_id).
    ResumeDownload(DynamicIndex),
}

#[derive(Debug)]
pub(crate) enum InstallMsg {
    InstallClicked,
    FileChosen(PathBuf),
    ArchiveDropped(PathBuf),
    PreInstallConfirmed(
        String,
        HashMap<String, InstallTarget>,
        std::collections::HashSet<String>,
    ),
    PreInstallCancelled,
    FomodConfirmed(fomod_resolver::FomodSelections),
    FomodCancelled,
    /// User confirmed merging pending files into an existing mod.
    PreInstallMerge(String),
    /// User chose to replace an existing mod (name-conflict dialog).
    PreInstallReplace(String, i32),
    /// User chose to create a new mod despite the name conflict.
    PreInstallCreateNew,
    InstallProgress(InstallIdentity, f64, String),
    /// User provided a Nexus file ID from the "file not found" dialog shown during install.
    FileIdDialogConfirmed {
        download_id: String,
        file_id: i64,
        mod_id: i64,
        domain: String,
        partial_name: Option<String>,
    },
    /// Open the pre-install dialog without replacing any existing mod.
    OpenPreInstallDialog,
    /// Open the pre-install dialog and replace the given mod (id, old_priority) after successful install.
    OpenPreInstallDialogReplacing(String, i32),
}

#[derive(Debug)]
pub(crate) enum ToolsMsg {
    LaunchTool(String),
    CancelToolLaunch,
    /// Fired from the background wait-thread when a launched tool's Wine process exits.
    /// The second field carries the stderr output if the process exited with a non-zero status.
    ToolExited(String, Option<String>),
    ToolSessionStarted(crate::core::tool_launcher::ToolProcessHandle),
    /// Show a first-run Proton GE setup confirmation dialog for `tool_id`.
    ConfirmProtonSetup(String),
    /// User confirmed the first-run Proton GE setup; launch through UMU.
    ProtonSetupConfirmed(String),
    /// Show a Snap Wine interface connection dialog.
    ConfirmSnapWineSetup(String, crate::core::game::MissingSnapWineContent),
    /// AppImage UMU has finished preparing Proton GE.
    ProtonSetupReady,
    ToolSetupProgress(crate::core::tool_launcher::ToolSetupStage),
    RetryMonoSetup,
    LaunchWithoutMono,
    ManageToolsClicked,
    ToolAdded(Tool),
    ToolRemoved(String),
    ToolWorkingDirChanged(String, String),
    ToolManagerClosed,
}

pub(crate) enum PrepareResultMsg {
    Presets(Vec<crate::core::game::mass_effect::appearance::morph::Preset>),
    Normal {
        presets: Vec<crate::core::game::mass_effect::appearance::morph::Preset>,
        dazip_sources: Vec<crate::core::installer::DazipSource>,
        mele: Option<Box<crate::core::game::mass_effect::package::PackagePlan>>,
        mele_bundled_launcher: Option<Box<crate::core::game::mass_effect::launcher::Bundled>>,
        file_list: Vec<(PathBuf, PathBuf)>,
        stripped_wrapper: Option<String>,
        tmp_dir: TempDir,
        mod_name: String,
        archive_hash: Option<String>,
        archive_path: Option<String>,
    },
    Fomod {
        dazip_sources: Vec<crate::core::installer::DazipSource>,
        config: fomod_resolver::FomodUiConfig,
        config_path: PathBuf,
        tmp_dir: TempDir,
        mod_name: String,
        archive_hash: Option<String>,
        archive_path: Option<String>,
    },
}

#[derive(Debug)]
pub(crate) struct PrepareFailure {
    pub(crate) message: String,
    pub(crate) dialog_heading: Option<&'static str>,
}

impl PrepareFailure {
    pub(crate) fn notification(message: String) -> Self {
        Self {
            message,
            dialog_heading: None,
        }
    }

    pub(crate) fn dialog(heading: &'static str, message: String) -> Self {
        Self {
            message,
            dialog_heading: Some(heading),
        }
    }
}

// PrepareResultMsg contains TempDir which is not Debug
impl std::fmt::Debug for PrepareResultMsg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PrepareResultMsg::Presets(presets) => {
                f.debug_tuple("Presets").field(&presets.len()).finish()
            }
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
pub(crate) enum AppCmdMsg {
    DeploymentStatus(super::deployment_status::Update),
    Generations(super::generations::Cmd),
    Mele(super::mele::Command),
    Recovery(super::location_recovery::RecoveryCmd),
    LocationActivityCompleted(tokio::sync::OwnedRwLockReadGuard<()>, Box<AppCmdMsg>),
    Shell(ShellCmdMsg),
    Games(GamesCmdMsg),
    Mods(ModsCmdMsg),
    Plugins(PluginsCmdMsg),
    Downloads(DownloadsCmdMsg),
    Install(InstallCmdMsg),
    Tools(ToolsCmdMsg),
}

#[derive(Debug)]
pub(crate) enum ShellCmdMsg {
    PreparedDiscarded(Result<(), String>),
    Initialized(Box<Result<InitData, String>>),
    DeployPreflightDone(Result<crate::core::deployer::DeploymentPreflight, String>),
    VanillaProtectionSaved {
        protect: bool,
        result: Result<(), String>,
    },
    GenerationPrepared(Result<Box<crate::core::generations::activation::Prepared>, String>),
    DeployDone(Result<DeployCompletion, String>),
    PurgeDone(Result<crate::core::generations::activation::PurgeReport, String>),
    GamePathSaved {
        game_id: String,
        path: PathBuf,
        result: Result<(), String>,
    },
    PrioritySaved(Result<(), String>),
    /// Result of the self-update AppImage download + replace.
    AppUpdateResult(Result<(), String>),
    /// Avatar image bytes fetched from Nexus (None = fetch failed, use initials).
    NexusAvatarLoaded(Option<Vec<u8>>),
    /// Nexus user data refreshed after key validation (username, avatar_url, is_premium).
    NexusUserRefreshed(Option<String>, Option<String>, bool),
    NexusUserRefreshFailed(String),
    NexusLogoutDone(Result<(), String>),
}

#[derive(Debug)]
pub(crate) enum GamesCmdMsg {
    TrilogyCandidates(
        String,
        Result<Vec<crate::core::tracker::trilogy_profiles::Candidate>, String>,
    ),
    TrilogyGrouped(String, Result<(), String>),
    LibraryLoaded {
        request: super::game_loading::Request,
        result: Result<super::game_loading::Library, String>,
    },
    GameOpened {
        request: super::game_loading::Request,
        result: Result<LoadedData, String>,
    },
    LocationAccessChecked(Result<Vec<String>, String>),
    ModsLoaded(Result<LoadedData, String>, bool),
    CacheDirMoved {
        game_id: String,
        new_dir: std::path::PathBuf,
        result: Result<(), String>,
    },
    CacheDirReset {
        game_id: String,
        result: Result<(), String>,
    },
    ProfileSwitched(Result<(LoadedData, Option<save_manager::SaveSyncResult>), String>),
    PendingSaveSetPrepared(Result<(usize, Option<crate::models::profile::SaveMode>), String>),
    ProfileCreated(Result<LoadedData, String>),
    ProfileCloned(Result<LoadedData, String>),
    ProfileRenamed(Result<(), String>),
    ProfileDeleted(Result<(LoadedData, Option<String>), String>),
    /// Result of toggling profile save mode (+ optional save backup/restore op).
    SaveModeToggled(Result<(), String>),
    /// Result of a manual save sync triggered by the user.
    SavesSynced(Result<save_manager::SaveSyncResult, String>),
    SaveBackupsLoaded(Result<Vec<save_manager::SaveBackupManifest>, String>),
    SaveBackupMutation(Result<String, String>),
    /// Snapshot lists loaded for the current game.
    OrderSnapshotsLoaded(
        Vec<crate::models::order_snapshot::OrderSnapshot>,
        Vec<crate::models::order_snapshot::OrderSnapshot>,
    ),
    /// Mod or plugin order snapshot deleted; carries updated snapshot list (game_id, kind).
    OrderSnapshotDeleted(Result<(), String>),
    /// Game settings have been persisted; refresh the list while retaining selection.
    GamesPersisted(Result<Vec<crate::models::game::GameConfig>, String>),
    GameRemoved {
        game_id: String,
        result: Result<Vec<String>, String>,
    },
}

#[derive(Debug)]
pub(crate) enum ModsCmdMsg {
    OverrideChanged {
        game_id: String,
        result: Box<Result<LoadedData, String>>,
    },
    ModRemoved(
        Result<(String, Vec<String>), String>,
        Option<(i64, i64)>,
        String,
        Option<String>,
    ),
    OverridesRefreshed(
        Result<std::collections::HashMap<String, crate::core::tracker::OverrideInfo>, String>,
    ),
    ModNexusMetadataRefreshed {
        mod_id: String,
        result: Result<(String, String, String), String>,
    },
    ModPropertiesSaved {
        saved: Box<crate::app::mods::properties::SavedModProperties>,
        result: Result<(), String>,
    },
    ExternalScanDone(Result<Vec<ExternalFile>, String>),
    /// Empty mod created (mod_id, cache_dir_path).
    EmptyModCreated(Result<(String, std::path::PathBuf), String>),
    /// Mod cache rescanned, including the replacement file list for the open dialog.
    ModFilesRescanned {
        mod_id: String,
        result: Result<RescannedModFiles, String>,
    },
    /// Per-file list loaded for the open mod properties dialog.
    ModFilesLoaded {
        mod_id: String,
        files: Vec<ModFile>,
    },
    /// Mod order snapshot saved.
    ModOrderSnapshotSaved(Result<(), String>),
    /// Mod order snapshot restored.
    ModOrderSnapshotRestored(Box<Result<crate::app::types::LoadedData, String>>),
}

#[derive(Debug)]
pub(crate) struct RescannedModFiles {
    pub(crate) files: Vec<ModFile>,
    pub(crate) summary: String,
}

#[derive(Debug)]
pub(crate) enum PluginsCmdMsg {
    PluginOrderSaved(Result<(), String>),
    /// Result of adopting externally-cleaned managed plugins into the deployd cache.
    ManagedPluginsAdopted(Result<usize, String>),
    /// Result of restoring managed plugins from their xEdit backup.
    BackupRestored(Result<usize, String>),
    /// Result of resetting the vanilla snapshot for the selected game.
    VanillaBaselineReset(Result<(), String>),
    /// Result of upserting vanilla entries for individually marked files.
    VanillaEntriesUpdated(Result<usize, String>),
    /// Result of the async LOOT sort; payload is (sorted filenames, dirty-info map) on success.
    #[cfg(feature = "loot")]
    LootSortDone(
        String,
        Result<(Vec<String>, HashMap<String, PluginDirtyInfo>), String>,
    ),
    #[cfg(feature = "loot")]
    LootOrderApplied(Box<Result<LoadedData, String>>),
    /// Plugin order snapshot saved.
    PluginOrderSnapshotSaved(Result<(), String>),
    /// Plugin order snapshot restored.
    PluginOrderSnapshotRestored(Box<Result<crate::app::types::LoadedData, String>>),
}

#[derive(Debug)]
pub(crate) enum DownloadsCmdMsg {
    /// The download archive was moved to Trash and the entry can be removed.
    DownloadArchiveTrashed {
        download_id: String,
        result: Result<(), String>,
    },
    DownloadArchiveDeleted {
        download_id: String,
        result: Result<(), String>,
    },
    DownloadEntryRemoved {
        download_id: String,
        success_message: String,
        result: Result<(), String>,
    },
    NxmDownloadComplete(String, Result<NxmDownloadResult, String>),
    NexusMetadataFetched(
        String,
        Result<crate::app::types::ManualMetadataResult, String>,
    ),
    NexusIdentityPersisted {
        download_id: String,
        nexus_ids: crate::models::download::NexusIds,
        result: Result<(), String>,
    },
    NexusMetadataPersisted {
        download_id: String,
        metadata: crate::app::types::NexusDownloadMetadata,
        needs_file_id: bool,
        result: Result<Box<crate::models::download::DownloadEntry>, String>,
    },
    DownloadsDirUpdated(Result<Option<PathBuf>, String>),
    DownloadsScanned(Result<DownloadScanResult, String>),
    DownloadStatusesReloaded(Result<Vec<crate::models::download::DownloadEntry>, String>),
}

#[derive(Debug)]
pub(crate) enum InstallCmdMsg {
    PresetsReady(
        InstallIdentity,
        Vec<crate::core::game::mass_effect::appearance::morph::Preset>,
        Box<Result<Option<crate::models::download::DownloadEntry>, String>>,
    ),
    ModAdded(
        InstallIdentity,
        Box<Result<AddResult, String>>,
        Option<crate::app::state::ReplacementContext>,
    ),
    ModPrepared(
        InstallIdentity,
        Box<Result<PrepareResultMsg, PrepareFailure>>,
    ),
    /// Files were merged into an existing mod. Carries `(mod_name, files_merged)`.
    ModMerged(InstallIdentity, Result<(String, usize), String>),
    /// Previous FOMOD selections loaded from DB for the reinstall/replace flow.
    /// None = no prior selections stored. Triggers opening the pre-install dialog.
    FomodSelectionsLoaded(Option<Vec<Vec<std::collections::HashSet<usize>>>>),
}

#[derive(Debug)]
pub(crate) enum ToolsCmdMsg {
    Saved(Result<(), String>),
    Deleted(Result<String, String>),
    WorkingDirSaved(Result<(), String>),
    Launched(Result<String, crate::core::tool_launcher::ToolPrepareError>),
    LaunchCancelled(String),
}
