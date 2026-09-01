use relm4::prelude::*;

use super::App;
use super::messages::{AppCmdMsg, AppMsg};
use super::order_snapshots::SnapshotAction;

impl App {
    pub(crate) fn dispatch_input(
        &mut self,
        msg: AppMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        match msg {
            AppMsg::Shell(msg) => self.dispatch_shell_input(msg, sender, root),
            AppMsg::Games(msg) => self.dispatch_games_input(msg, sender, root),
            AppMsg::Mods(msg) => self.dispatch_mods_input(msg, sender, root),
            AppMsg::Plugins(msg) => self.dispatch_plugins_input(msg, sender, root),
            AppMsg::Downloads(msg) => self.dispatch_downloads_input(msg, sender, root),
            AppMsg::Install(msg) => self.dispatch_install_input(msg, sender, root),
            AppMsg::Tools(msg) => self.dispatch_tools_input(msg, sender, root),
            AppMsg::Migration(msg) => self.dispatch_migration_input(msg, sender, root),
        }
    }

    fn dispatch_shell_input(
        &mut self,
        msg: crate::app::messages::ShellMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::ShellMsg;

        match msg {
            ShellMsg::DeployClicked => self.handle_deploy_clicked(root, &sender),
            ShellMsg::DeployConfirmed => self.execute_deploy(&sender),
            ShellMsg::PurgeClicked => self.handle_purge_clicked(root, &sender),
            ShellMsg::PurgeConfirmed => self.handle_purge_confirmed(&sender),
            ShellMsg::GrantGameFolderAccess => self.handle_grant_game_folder_access(root, &sender),
            ShellMsg::GameFolderGranted(path) => self.handle_game_folder_granted(path, &sender),
            ShellMsg::SearchToggled(active) => self.handle_search_toggled(active),
            ShellMsg::SearchChanged(text) => self.handle_search_changed(text),
            ShellMsg::ApplySearch => self.handle_apply_search(),
            ShellMsg::SearchScopeChanged(idx) => self.handle_search_scope_changed(idx),
            ShellMsg::CloseRequested => self.handle_close_requested(root, &sender),
            ShellMsg::ConfirmClose => self.handle_confirm_close(root),
            ShellMsg::ShowToast(message) => self.handle_show_toast(message),
            ShellMsg::NotificationDismissed => self.handle_notification_dismissed(),
            ShellMsg::ClearNotifications => self.handle_clear_notifications(),
            ShellMsg::AppUpdateAvailable(version, url) => {
                self.handle_app_update_available(version, url)
            }
            ShellMsg::SelfUpdateDownload => self.handle_self_update_clicked(&sender),
            ShellMsg::OpenDeploymentFolder => self.handle_open_deployment_folder(),
            ShellMsg::SetColorScheme(idx) => self.handle_set_color_scheme(idx, &sender),
            ShellMsg::NexusLoginClicked => self.handle_nexus_login_clicked(&sender),
            ShellMsg::NexusLogoutClicked => self.handle_nexus_logout_clicked(&sender),
        }
    }

    fn dispatch_games_input(
        &mut self,
        msg: crate::app::messages::GamesMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::GamesMsg;

        match msg {
            GamesMsg::GameSelected(idx) => self.handle_game_selected(idx, &sender),
            GamesMsg::ProfileSelected(idx) => self.handle_profile_selected(idx, &sender),
            GamesMsg::InitializePendingSaveSet => self.handle_initialize_pending_save_set(&sender),
            GamesMsg::UseGlobalForPendingProfile => {
                self.handle_use_global_for_pending_profile(&sender)
            }
            GamesMsg::NewProfileClicked => self.handle_new_profile_requested(&sender),
            GamesMsg::CloneProfileClicked => self.handle_clone_profile_requested(&sender),
            GamesMsg::RenameProfile(name) => self.handle_rename_profile(name, &sender),
            GamesMsg::DeleteProfileClicked => self.handle_delete_profile_requested(root, &sender),
            GamesMsg::DeleteProfileConfirmed => self.handle_delete_profile_clicked(&sender),
            GamesMsg::SettingsClicked => self.handle_settings_clicked(root, &sender),
            GamesMsg::SettingsClosed => self.handle_settings_closed(&sender),
            GamesMsg::ManageGamesClicked => self.handle_manage_games_clicked(root, &sender),
            GamesMsg::ManageGamesClosed => self.handle_manage_games_closed(&sender),
            GamesMsg::GamesConfigured(configs, hidden_ids) => {
                self.handle_games_configured(configs, hidden_ids, &sender)
            }
            GamesMsg::ShowWelcomeWizard => self.handle_show_welcome_wizard(root, &sender),
            GamesMsg::WelcomeWizardConfirmed(configs, hidden_ids) => {
                self.handle_welcome_wizard_confirmed(configs, hidden_ids, &sender)
            }
            GamesMsg::WelcomeWizardSkipped => self.handle_welcome_wizard_skipped(),
            GamesMsg::RemoveCurrentGame => self.handle_remove_current_game(root, &sender),
            GamesMsg::RemoveGameConfirmed {
                game_id,
                delete_mods,
            } => self.handle_remove_game(game_id, delete_mods, &sender),
            GamesMsg::CacheDirChangeRequested { game_id, new_dir } => {
                self.handle_cache_dir_change_requested(game_id, new_dir, &sender)
            }
            GamesMsg::CacheDirResetRequested { game_id } => {
                self.handle_cache_dir_reset_requested(game_id, &sender)
            }
            GamesMsg::NexusApiKeyUpdated => self.handle_nexus_api_key_updated(&sender),
            GamesMsg::ToggleProfileSaveMode => {
                self.handle_toggle_profile_save_mode_requested(root, &sender)
            }
            GamesMsg::ToggleProfileSaveModeConfirmed => {
                self.handle_toggle_profile_save_mode(&sender)
            }
            GamesMsg::InitializeGlobalAndDisableIsolation => {
                self.handle_initialize_global_and_disable_isolation(&sender)
            }
            GamesMsg::SyncSaves => self.handle_sync_saves_requested(root, &sender),
            GamesMsg::SyncSavesConfirmed => self.handle_sync_saves(&sender),
            GamesMsg::ManageSaveBackups => self.handle_manage_save_backups(&sender),
            GamesMsg::CreateSaveBackup(label) => self.handle_create_save_backup(label, &sender),
            GamesMsg::RestoreSaveBackupRequested(id) => {
                self.handle_restore_save_backup_requested(id, root, &sender)
            }
            GamesMsg::RestoreSaveBackupConfirmed(id) => {
                self.handle_restore_save_backup(id, &sender)
            }
            GamesMsg::DeleteSaveBackupRequested(id) => {
                self.handle_delete_save_backup_requested(id, root, &sender)
            }
            GamesMsg::DeleteSaveBackupConfirmed(id) => self.handle_delete_save_backup(id, &sender),
        }
    }

    fn dispatch_mods_input(
        &mut self,
        msg: crate::app::messages::ModsMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::ModsMsg;

        match msg {
            ModsMsg::ReinstallMod(idx) => self.handle_reinstall_mod(idx, &sender),
            ModsMsg::MoveModTo(from, to) => self.handle_move_mod_to(from, to, &sender),
            ModsMsg::MoveGroupTo(from, to) => self.handle_move_group_to(from, to, &sender),
            ModsMsg::MoveSelectedModsTo { selected, from, to } => {
                self.handle_move_selected_mods_to(selected, from, to, &sender)
            }
            ModsMsg::ToggleGroupCollapse(idx) => self.handle_toggle_group_collapse(idx, &sender),
            ModsMsg::DeleteGroup(idx) => self.handle_delete_group(idx, &sender),
            ModsMsg::CreateGroup(name) => self.handle_create_group(name, &sender),
            ModsMsg::RenameGroup(idx, name) => self.handle_rename_group(idx, name, &sender),
            ModsMsg::SetGroupColor(idx, color) => self.handle_set_group_color(idx, color, &sender),
            ModsMsg::OpenModProperties(idx) => self.handle_open_mod_properties(idx, root, &sender),
            ModsMsg::ModPropertiesApplied {
                mod_id,
                mod_idx,
                name,
                notes,
                nexus_mod_id,
                nexus_id_changed,
                install_target,
                file_targets,
            } => self.handle_mod_properties_applied(
                super::mods::properties::AppliedModProperties {
                    mod_id,
                    mod_idx,
                    name,
                    notes,
                    nexus_mod_id,
                    nexus_id_changed,
                    install_target,
                    file_targets,
                },
                &sender,
            ),
            ModsMsg::ModPropertiesCancelled => self.handle_mod_properties_cancelled(),
            ModsMsg::ScanExternalFiles => self.handle_scan_external_files(&sender),
            ModsMsg::AbsorbExternalFiles => {
                self.handle_absorb_external_files_requested(root, &sender)
            }
            ModsMsg::AbsorbFilesSelected(pairs) => {
                self.handle_absorb_files_selected(pairs, root, &sender)
            }
            ModsMsg::DiscardExternalFiles(paths) => {
                self.handle_discard_external_files(paths, &sender)
            }
            ModsMsg::CreateModFromExternalCancelled => {
                self.handle_create_mod_from_external_cancelled()
            }
            ModsMsg::CreateEmptyMod => self.handle_create_empty_mod(&sender),
            ModsMsg::ScanModFromCache(mod_id) => self.handle_scan_mod_from_cache(mod_id, &sender),
            ModsMsg::EnableAllMods => self.handle_enable_all_mods(&sender),
            ModsMsg::DisableAllMods => self.handle_disable_all_mods(&sender),
            ModsMsg::SaveModOrderSnapshot(name) => {
                self.handle_snapshot_action(SnapshotAction::SaveMod(name), &sender)
            }
            ModsMsg::LoadModOrderSnapshot(id) => {
                self.handle_snapshot_action(SnapshotAction::LoadMod(id), &sender)
            }
            ModsMsg::DeleteModOrderSnapshot(id) => {
                self.handle_snapshot_action(SnapshotAction::Delete(id), &sender)
            }
            ModsMsg::SetModFilter(filter) => self.handle_set_mod_filter(filter),
            ModsMsg::EnterModSelectionMode => self.handle_enter_mod_selection_mode(),
            ModsMsg::ExitModSelectionMode => self.handle_exit_mod_selection_mode(),
            ModsMsg::ToggleModRowSelected(idx) => self.handle_toggle_mod_row_selected(idx),
            ModsMsg::SetModRowSelected(idx, selected) => {
                self.handle_set_mod_row_selected(idx.current_index(), selected)
            }
            ModsMsg::EnableSelectedMods => self.handle_enable_selected_mods(&sender),
            ModsMsg::DisableSelectedMods => self.handle_disable_selected_mods(&sender),
            ModsMsg::RemoveSelectedMods => self.handle_remove_selected_mods(root, &sender),
            ModsMsg::ConfirmRemoveSelectedMods => self.handle_confirm_remove_selected_mods(&sender),
        }
    }

    fn dispatch_plugins_input(
        &mut self,
        msg: crate::app::messages::PluginsMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::PluginsMsg;

        match msg {
            PluginsMsg::MovePluginTo(from, to) => self.handle_move_plugin_to(from, to, &sender),
            PluginsMsg::AdoptManagedPluginChanges(files) => {
                self.handle_adopt_managed_plugin_changes(files, &sender)
            }
            PluginsMsg::RestoreFromXEditBackup(files) => {
                self.handle_restore_from_xedit_backup(files, &sender)
            }
            PluginsMsg::ResetVanillaBaseline => self.handle_reset_vanilla_baseline(root, &sender),
            PluginsMsg::ResetVanillaBaselineConfirmed => {
                self.handle_reset_vanilla_baseline_confirmed(&sender)
            }
            PluginsMsg::MarkExternalFilesAsVanilla(files) => {
                self.handle_mark_external_files_as_vanilla(files, &sender)
            }
            PluginsMsg::SortWithLoot => self.handle_sort_with_loot(&sender),
            PluginsMsg::EnableAllPlugins => self.handle_enable_all_plugins(&sender),
            PluginsMsg::DisableAllPlugins => self.handle_disable_all_plugins(&sender),
            PluginsMsg::ToggleShowVanillaPlugins => self.handle_toggle_show_vanilla_plugins(),
            PluginsMsg::SavePluginOrderSnapshot(name) => {
                self.handle_snapshot_action(SnapshotAction::SavePlugin(name), &sender)
            }
            PluginsMsg::LoadPluginOrderSnapshot(id) => {
                self.handle_snapshot_action(SnapshotAction::LoadPlugin(id), &sender)
            }
            PluginsMsg::DeletePluginOrderSnapshot(id) => {
                self.handle_snapshot_action(SnapshotAction::Delete(id), &sender)
            }
            PluginsMsg::EnterPluginSelectionMode => self.handle_enter_plugin_selection_mode(),
            PluginsMsg::ExitPluginSelectionMode => self.handle_exit_plugin_selection_mode(),
            PluginsMsg::TogglePluginRowSelected(idx) => self.handle_toggle_plugin_row_selected(idx),
            PluginsMsg::SetPluginRowSelected(idx, selected) => {
                self.handle_set_plugin_row_selected(idx.current_index(), selected)
            }
            PluginsMsg::EnableSelectedPlugins => self.handle_enable_selected_plugins(&sender),
            PluginsMsg::DisableSelectedPlugins => self.handle_disable_selected_plugins(&sender),
        }
    }

    fn dispatch_downloads_input(
        &mut self,
        msg: crate::app::messages::DownloadsMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::DownloadsMsg;

        match msg {
            DownloadsMsg::NxmLinkReceived(link) => self.handle_nxm_link_received(link, &sender),
            DownloadsMsg::SetDownloadsVisible(visible) => {
                self.handle_set_downloads_visible(visible)
            }
            DownloadsMsg::InstallDownload(idx) => self.handle_install_download(idx, &sender),
            DownloadsMsg::ReinstallDownload(idx) => self.handle_reinstall_download(idx, &sender),
            DownloadsMsg::ClearDownloadMetadata(idx) => {
                self.handle_clear_download_metadata(idx, &sender)
            }
            DownloadsMsg::RenameDownload(idx) => self.handle_rename_download(idx, root, &sender),
            DownloadsMsg::DeleteDownload(idx) => self.handle_delete_download(idx, root, &sender),
            DownloadsMsg::ConfirmDeleteDownload(id) => {
                self.handle_confirm_delete_download(id, &sender)
            }
            DownloadsMsg::HideDownload(idx) => self.handle_hide_download(idx, &sender),
            DownloadsMsg::SetShowHiddenDownloads(show) => {
                self.handle_set_show_hidden_downloads(show)
            }
            DownloadsMsg::ConfirmDownloadRename(id, name) => {
                self.handle_confirm_download_rename(id, name, &sender)
            }
            DownloadsMsg::ConfirmNexusIdEntry(download_id, mod_id, domain) => {
                self.handle_confirm_nexus_id_entry(download_id, mod_id, domain, &sender)
            }
            DownloadsMsg::ShowFileIdDialog {
                download_id,
                mod_id,
                domain,
                partial_name,
            } => self.handle_show_file_id_dialog(
                download_id,
                mod_id,
                domain,
                partial_name,
                root,
                &sender,
            ),
            DownloadsMsg::DownloadProgress(id, fraction, message) => {
                self.handle_download_progress(id, fraction, message)
            }
            DownloadsMsg::ArchiveMd5Computed(download_id, md5) => {
                self.handle_archive_md5_computed(download_id, md5, &sender)
            }
            DownloadsMsg::FetchDownloadMetadata(idx) => {
                self.handle_fetch_download_metadata(idx, root, &sender)
            }
            DownloadsMsg::ScanDownloadsFolder => self.handle_scan_downloads_folder(&sender),
            DownloadsMsg::DownloadSortChanged(idx) => self.handle_download_sort_changed(idx),
            DownloadsMsg::RateLimitUpdated(info) => self.handle_rate_limit_updated(info, &sender),
            DownloadsMsg::SetDownloadFilter(filter) => self.handle_set_download_filter(filter),
            DownloadsMsg::PauseDownload(idx) => self.handle_pause_download(idx),
            DownloadsMsg::ResumeDownload(idx) => self.handle_resume_download(idx, &sender),
        }
    }

    fn dispatch_install_input(
        &mut self,
        msg: crate::app::messages::InstallMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::InstallMsg;

        match msg {
            InstallMsg::InstallClicked => self.handle_install_clicked(root, &sender),
            InstallMsg::FileChosen(path) => self.handle_file_chosen(path, &sender),
            InstallMsg::PreInstallConfirmed(name, targets, excluded) => {
                self.handle_pre_install_confirmed(name, targets, excluded, root, &sender)
            }
            InstallMsg::PreInstallCancelled => self.handle_pre_install_cancelled(),
            InstallMsg::FomodConfirmed(selections) => {
                self.handle_fomod_confirmed(selections, &sender)
            }
            InstallMsg::FomodCancelled => self.handle_fomod_cancelled(),
            InstallMsg::PreInstallMerge(mod_id) => self.handle_pre_install_merge(mod_id, &sender),
            InstallMsg::PreInstallReplace(id, priority) => {
                self.handle_pre_install_replace(id, priority, &sender)
            }
            InstallMsg::PreInstallCreateNew => self.handle_pre_install_create_new(&sender),
            InstallMsg::InstallProgress(identity, frac, msg) => {
                self.handle_install_progress(&identity, frac, msg)
            }
            InstallMsg::FileIdDialogConfirmed {
                download_id,
                file_id,
                mod_id,
                domain,
                partial_name,
            } => self.handle_file_id_dialog_confirmed(
                download_id,
                file_id,
                mod_id,
                domain,
                partial_name,
                &sender,
            ),
            InstallMsg::OpenPreInstallDialog => self.handle_open_pre_install_dialog(root, &sender),
            InstallMsg::OpenPreInstallDialogReplacing(id, priority) => {
                self.handle_open_pre_install_dialog_replacing_request(id, priority, root, &sender)
            }
        }
    }

    fn dispatch_tools_input(
        &mut self,
        msg: crate::app::messages::ToolsMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::ToolsMsg;

        match msg {
            ToolsMsg::LaunchTool(name) => self.handle_launch_tool(name, root, &sender),
            ToolsMsg::CancelToolLaunch => self.handle_cancel_tool_launch(),
            ToolsMsg::ToolSessionStarted(handle) => self.handle_tool_session_started(handle),
            ToolsMsg::ToolExited(name, error) => self.handle_tool_exited(name, error, &sender),
            ToolsMsg::ConfirmProtonSetup(tool_id) => {
                self.handle_confirm_proton_setup(tool_id, root, &sender)
            }
            ToolsMsg::ProtonSetupConfirmed(tool_id) => {
                self.handle_proton_setup_confirmed(tool_id, root, &sender)
            }
            ToolsMsg::ConfirmSnapWineSetup(tool_id, missing) => {
                self.handle_confirm_snap_wine_setup(tool_id, missing, root, &sender)
            }
            ToolsMsg::ProtonSetupReady => self.handle_proton_setup_ready(),
            ToolsMsg::ToolSetupProgress(stage) => self.handle_tool_setup_progress(stage),
            ToolsMsg::RetryMonoSetup => self.handle_retry_mono_setup(root, &sender),
            ToolsMsg::LaunchWithoutMono => self.handle_launch_without_mono(root, &sender),
            ToolsMsg::ManageToolsClicked => self.handle_manage_tools_clicked(root, &sender),
            ToolsMsg::ToolAdded(tool) => self.handle_tool_added(tool, &sender),
            ToolsMsg::ToolRemoved(name) => self.handle_tool_removed(name, &sender),
            ToolsMsg::ToolWorkingDirChanged(name, dir) => {
                self.handle_tool_working_dir_changed(name, dir, &sender)
            }
            ToolsMsg::ToolManagerClosed => self.handle_tool_manager_closed(),
        }
    }

    fn dispatch_migration_input(
        &mut self,
        msg: crate::app::messages::MigrationMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::MigrationMsg;

        match msg {
            MigrationMsg::ExportGameForSnap(game_id) => {
                self.handle_export_game_for_snap(game_id, root, &sender)
            }
            MigrationMsg::ExportGameForSnapChosen {
                game_id,
                output_path,
            } => self.handle_export_game_for_snap_chosen(game_id, output_path, &sender),
            MigrationMsg::PreviewAppImageExport => {
                self.handle_preview_appimage_export(root, &sender)
            }
            MigrationMsg::PreviewAppImageExportChosen(bundle_path) => {
                self.handle_preview_appimage_export_chosen(bundle_path, &sender)
            }
            MigrationMsg::ImportAppImageExport(bundle_path) => {
                self.handle_import_appimage_export(bundle_path, root, &sender)
            }
            MigrationMsg::ImportGameFolderChosen(path) => {
                self.handle_import_game_folder_chosen(path, root, &sender)
            }
            MigrationMsg::ImportWinePrefixChosen(path) => {
                self.handle_import_wine_prefix_chosen(path, &sender)
            }
        }
    }

    pub(crate) fn dispatch_command(
        &mut self,
        msg: AppCmdMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        match msg {
            AppCmdMsg::Shell(msg) => self.dispatch_shell_command(msg, sender, root),
            AppCmdMsg::Games(msg) => self.dispatch_games_command(msg, sender, root),
            AppCmdMsg::Mods(msg) => self.dispatch_mods_command(msg, sender, root),
            AppCmdMsg::Plugins(msg) => self.dispatch_plugins_command(msg, sender, root),
            AppCmdMsg::Downloads(msg) => self.dispatch_downloads_command(msg, sender, root),
            AppCmdMsg::Install(msg) => self.dispatch_install_command(msg, sender, root),
            AppCmdMsg::Tools(msg) => self.dispatch_tools_command(msg, sender, root),
            AppCmdMsg::Migration(msg) => self.dispatch_migration_command(msg, sender, root),
        }
    }

    fn dispatch_shell_command(
        &mut self,
        msg: crate::app::messages::ShellCmdMsg,
        sender: ComponentSender<Self>,
        _root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::ShellCmdMsg;

        match msg {
            ShellCmdMsg::Initialized(result) => self.handle_cmd_initialized(*result, &sender),
            ShellCmdMsg::DeployDone(result) => self.handle_cmd_deploy_done(result, &sender),
            ShellCmdMsg::PurgeDone(result) => self.handle_cmd_purge_done(result),
            ShellCmdMsg::GamePathSaved {
                game_id,
                path,
                result,
            } => self.handle_cmd_game_path_saved(game_id, path, result),
            ShellCmdMsg::PrioritySaved(result) => self.handle_cmd_priority_saved(result, &sender),
            ShellCmdMsg::AppUpdateResult(result) => self.handle_cmd_app_update_result(result),
            ShellCmdMsg::NexusAvatarLoaded(bytes) => self.handle_cmd_nexus_avatar_loaded(bytes),
            ShellCmdMsg::NexusUserRefreshed(username, avatar_url, is_premium) => {
                self.handle_cmd_nexus_user_refreshed(username, avatar_url, is_premium, &sender)
            }
            ShellCmdMsg::NexusUserRefreshFailed(error) => {
                self.push_notification(&format!("Failed to update Nexus account: {error}"));
            }
            ShellCmdMsg::NexusLogoutDone(result) => match result {
                Ok(()) => {
                    self.handle_cmd_nexus_user_refreshed(None, None, false, &sender);
                    self.show_toast("Logged out of Nexus Mods");
                }
                Err(error) => {
                    self.push_notification(&format!("Failed to log out of Nexus Mods: {error}"));
                }
            },
        }
    }

    fn dispatch_games_command(
        &mut self,
        msg: crate::app::messages::GamesCmdMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::GamesCmdMsg;

        match msg {
            GamesCmdMsg::ModsLoaded(result, preserve) => {
                self.handle_cmd_mods_loaded(result, preserve, &sender)
            }
            GamesCmdMsg::CacheDirMoved {
                game_id,
                new_dir,
                result,
            } => self.handle_cmd_cache_dir_moved(game_id, new_dir, result),
            GamesCmdMsg::CacheDirReset { game_id, result } => {
                self.handle_cmd_cache_dir_reset(game_id, result)
            }
            GamesCmdMsg::ProfileSwitched(result) => {
                self.handle_cmd_profile_switched(result, root, &sender)
            }
            GamesCmdMsg::PendingSaveSetPrepared(result) => {
                self.handle_cmd_pending_save_set_prepared(result)
            }
            GamesCmdMsg::ProfileCreated(result) => self.handle_cmd_profile_created(result, &sender),
            GamesCmdMsg::ProfileCloned(result) => self.handle_cmd_profile_cloned(result, &sender),
            GamesCmdMsg::ProfileRenamed(result) => self.handle_cmd_profile_renamed(result),
            GamesCmdMsg::ProfileDeleted(result) => self.handle_cmd_profile_deleted(result, &sender),
            GamesCmdMsg::SaveModeToggled(result) => {
                self.handle_cmd_save_mode_toggled(result, root, &sender)
            }
            GamesCmdMsg::SavesSynced(result) => self.handle_cmd_saves_synced(result),
            GamesCmdMsg::SaveBackupsLoaded(result) => {
                self.handle_cmd_save_backups_loaded(result, root, &sender)
            }
            GamesCmdMsg::SaveBackupMutation(result) => {
                self.handle_cmd_save_backup_mutation(result, &sender)
            }
            GamesCmdMsg::LastDeployedProfileLoaded(id) => {
                self.handle_cmd_last_deployed_profile_loaded(id)
            }
            GamesCmdMsg::GamesPersisted(result) => self.handle_cmd_games_persisted(result, &sender),
            GamesCmdMsg::GameRemoved { game_id, result } => {
                self.handle_cmd_game_removed(game_id, result, &sender)
            }
            GamesCmdMsg::OrderSnapshotsLoaded(mod_snapshots, plugin_snapshots) => {
                self.handle_cmd_order_snapshots_loaded(mod_snapshots, plugin_snapshots, &sender)
            }
            GamesCmdMsg::OrderSnapshotDeleted(result) => {
                self.handle_cmd_order_snapshot_deleted(result, &sender)
            }
        }
    }

    fn dispatch_mods_command(
        &mut self,
        msg: crate::app::messages::ModsCmdMsg,
        sender: ComponentSender<Self>,
        _root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::ModsCmdMsg;

        match msg {
            ModsCmdMsg::ModRemoved(result, nexus_ids, mod_name, archive_hash) => {
                self.handle_cmd_mod_removed(result, nexus_ids, mod_name, archive_hash, &sender)
            }
            ModsCmdMsg::OverridesRefreshed(result) => {
                self.handle_cmd_overrides_refreshed(result, &sender)
            }
            ModsCmdMsg::ModNexusMetadataRefreshed { mod_id, result } => {
                self.handle_cmd_mod_nexus_metadata_refreshed(mod_id, result)
            }
            ModsCmdMsg::ExternalScanDone(result) => self.handle_cmd_external_scan_done(result),
            ModsCmdMsg::EmptyModCreated(result) => {
                self.handle_cmd_empty_mod_created(result, &sender)
            }
            ModsCmdMsg::ModFilesRescanned(result) => {
                self.handle_cmd_mod_files_rescanned(result, &sender)
            }
            #[cfg(feature = "loot")]
            ModsCmdMsg::ModFilesLoaded(files) => self.handle_cmd_mod_files_loaded(files),
            ModsCmdMsg::ModOrderSnapshotSaved(result) => {
                self.handle_cmd_mod_order_snapshot_saved(result, &sender)
            }
            ModsCmdMsg::ModOrderSnapshotRestored(result) => {
                self.handle_cmd_mod_order_snapshot_restored(*result, &sender)
            }
        }
    }

    fn dispatch_plugins_command(
        &mut self,
        msg: crate::app::messages::PluginsCmdMsg,
        sender: ComponentSender<Self>,
        _root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::PluginsCmdMsg;

        match msg {
            PluginsCmdMsg::PluginOrderSaved(result) => self.handle_cmd_plugin_order_saved(result),
            PluginsCmdMsg::ManagedPluginsAdopted(result) => {
                self.handle_cmd_managed_plugins_adopted(result, &sender)
            }
            PluginsCmdMsg::BackupRestored(result) => {
                self.handle_cmd_backup_restored(result, &sender)
            }
            PluginsCmdMsg::VanillaBaselineReset(result) => {
                self.handle_cmd_vanilla_baseline_reset(result, &sender)
            }
            PluginsCmdMsg::VanillaEntriesUpdated(result) => {
                self.handle_cmd_vanilla_entries_updated(result, &sender)
            }
            PluginsCmdMsg::LootSortDone(game_id, result) => {
                self.handle_cmd_loot_sort_done(game_id, result, &sender)
            }
            #[cfg(feature = "loot")]
            PluginsCmdMsg::LootOrderApplied(result, post_action) => {
                self.handle_cmd_loot_order_applied(*result, post_action, &sender)
            }
            PluginsCmdMsg::PluginOrderSnapshotSaved(result) => {
                self.handle_cmd_plugin_order_snapshot_saved(result, &sender)
            }
            PluginsCmdMsg::PluginOrderSnapshotRestored(result) => {
                self.handle_cmd_plugin_order_snapshot_restored(*result, &sender)
            }
        }
    }

    fn dispatch_downloads_command(
        &mut self,
        msg: crate::app::messages::DownloadsCmdMsg,
        sender: ComponentSender<Self>,
        _root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::DownloadsCmdMsg;

        match msg {
            DownloadsCmdMsg::DownloadArchiveTrashed {
                download_id,
                result,
            } => self.handle_download_archive_trashed(download_id, result, &sender),
            DownloadsCmdMsg::NxmDownloadComplete(id, result) => {
                self.handle_cmd_nxm_download_complete(id, result, &sender)
            }
            DownloadsCmdMsg::NexusMetadataFetched(download_id, result) => {
                self.handle_cmd_nexus_metadata_fetched(download_id, result, &sender)
            }
            DownloadsCmdMsg::NexusIdentityPersisted {
                download_id,
                nexus_ids,
                result,
            } => self.handle_cmd_nexus_identity_persisted(download_id, nexus_ids, result, &sender),
            DownloadsCmdMsg::NexusMetadataPersisted { toast, result } => {
                self.handle_cmd_nexus_metadata_persisted(toast, result)
            }
            DownloadsCmdMsg::DownloadsDirUpdated(dir) => self.handle_cmd_downloads_dir_updated(dir),
            DownloadsCmdMsg::DownloadsScanned(result) => self.handle_cmd_downloads_scanned(result),
        }
    }

    fn dispatch_install_command(
        &mut self,
        msg: crate::app::messages::InstallCmdMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::InstallCmdMsg;

        match msg {
            InstallCmdMsg::ModAdded(identity, result, replacement) => {
                self.handle_cmd_mod_added(&identity, *result, replacement, &sender)
            }
            InstallCmdMsg::ModPrepared(identity, result) => {
                self.handle_cmd_mod_prepared(&identity, *result, root, &sender)
            }
            InstallCmdMsg::ModMerged(identity, result) => {
                self.handle_cmd_mod_merged(&identity, result, &sender)
            }
            InstallCmdMsg::FomodSelectionsLoaded(selections) => {
                self.handle_cmd_fomod_selections_loaded(selections, root, &sender)
            }
        }
    }

    fn dispatch_tools_command(
        &mut self,
        msg: crate::app::messages::ToolsCmdMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::ToolsCmdMsg;

        match msg {
            ToolsCmdMsg::Saved(result) => self.handle_cmd_tool_saved(result),
            ToolsCmdMsg::Deleted(result) => self.handle_cmd_tool_deleted(result),
            ToolsCmdMsg::WorkingDirSaved(result) => self.handle_cmd_tool_working_dir_saved(result),
            ToolsCmdMsg::Launched(result) => self.handle_cmd_tool_launched(result, root, &sender),
            ToolsCmdMsg::LaunchCancelled(name) => self.handle_cmd_tool_launch_cancelled(name),
        }
    }

    fn dispatch_migration_command(
        &mut self,
        msg: crate::app::messages::MigrationCmdMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        use crate::app::messages::MigrationCmdMsg;

        match msg {
            MigrationCmdMsg::GameExportedForSnap(result) => {
                self.handle_cmd_game_exported_for_snap(result)
            }
            MigrationCmdMsg::AppImageExportPreviewed(result) => {
                self.handle_cmd_appimage_export_previewed(result, root, &sender)
            }
            MigrationCmdMsg::AppImageExportImported(result) => {
                self.handle_cmd_appimage_export_imported(result, &sender)
            }
        }
    }
}
