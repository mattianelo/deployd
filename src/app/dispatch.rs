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
            AppMsg::Noop => {}
            AppMsg::GameSelected(idx) => self.handle_game_selected(idx, &sender),
            AppMsg::InstallClicked => self.handle_install_clicked(root, &sender),
            AppMsg::FileChosen(path) => self.handle_file_chosen(path, &sender),
            AppMsg::PreInstallConfirmed(name, targets, excluded) => {
                self.handle_pre_install_confirmed(name, targets, excluded, root, &sender)
            }
            AppMsg::PreInstallCancelled => self.handle_pre_install_cancelled(),
            AppMsg::FomodConfirmed(selections) => self.handle_fomod_confirmed(selections, &sender),
            AppMsg::FomodCancelled => self.handle_fomod_cancelled(),
            AppMsg::ReinstallMod(idx) => self.handle_reinstall_mod(idx, &sender),
            AppMsg::MoveModTo(from, to) => self.handle_move_mod_to(from, to, &sender),
            AppMsg::MoveGroupTo(from, to) => self.handle_move_group_to(from, to, &sender),
            AppMsg::MoveSelectedModsTo { selected, from, to } => {
                self.handle_move_selected_mods_to(selected, from, to, &sender)
            }
            AppMsg::MovePluginTo(from, to) => self.handle_move_plugin_to(from, to, &sender),
            AppMsg::MoveSelectedPluginsTo { selected, from, to } => {
                self.handle_move_selected_plugins_to(selected, from, to, &sender)
            }
            AppMsg::ProfileSelected(idx) => self.handle_profile_selected(idx, &sender),
            AppMsg::NewProfileClicked => self.handle_new_profile_requested(&sender),
            AppMsg::CloneProfileClicked => self.handle_clone_profile_requested(&sender),
            AppMsg::RenameProfile(name) => self.handle_rename_profile(name, &sender),
            AppMsg::DeleteProfileClicked => self.handle_delete_profile_requested(&sender),
            AppMsg::DeployClicked => self.handle_deploy_clicked(root, &sender),
            AppMsg::DeployConfirmed => self.execute_deploy(&sender),
            AppMsg::PurgeClicked => self.handle_purge_clicked(root, &sender),
            AppMsg::PurgeConfirmed => self.handle_purge_confirmed(&sender),
            AppMsg::GrantGameFolderAccess => self.handle_grant_game_folder_access(root, &sender),
            AppMsg::GameFolderGranted(path) => self.handle_game_folder_granted(path, &sender),
            AppMsg::LaunchTool(name) => self.handle_launch_tool(name, root, &sender),
            AppMsg::CancelToolLaunch => self.handle_cancel_tool_launch(),
            AppMsg::ToolSessionStarted(handle) => self.handle_tool_session_started(handle),
            AppMsg::ToolExited(name, error) => self.handle_tool_exited(name, error, &sender),
            AppMsg::ConfirmProtonSetup(tool_id) => {
                self.handle_confirm_proton_setup(tool_id, root, &sender)
            }
            AppMsg::ProtonSetupConfirmed(tool_id) => {
                self.handle_proton_setup_confirmed(tool_id, root, &sender)
            }
            AppMsg::ConfirmSnapWineSetup(tool_id, missing) => {
                self.handle_confirm_snap_wine_setup(tool_id, missing, root, &sender)
            }
            AppMsg::ProtonSetupReady => self.handle_proton_setup_ready(),
            AppMsg::ConfirmMonoPrompt(tool_id, prefix) => {
                self.handle_confirm_mono_prompt(tool_id, prefix, root, &sender)
            }
            AppMsg::ManageToolsClicked => self.handle_manage_tools_clicked(root, &sender),
            AppMsg::ToolAdded(tool) => self.handle_tool_added(tool, &sender),
            AppMsg::ToolRemoved(name) => self.handle_tool_removed(name, &sender),
            AppMsg::ToolWorkingDirChanged(name, dir) => {
                self.handle_tool_working_dir_changed(name, dir, &sender)
            }
            AppMsg::PreInstallMerge(mod_id) => self.handle_pre_install_merge(mod_id, &sender),
            AppMsg::PreInstallReplace(id, priority) => {
                self.handle_pre_install_replace(id, priority, &sender)
            }
            AppMsg::PreInstallCreateNew => self.handle_pre_install_create_new(&sender),
            AppMsg::InstallProgress(frac, msg) => self.handle_install_progress(frac, msg),
            AppMsg::ToolManagerClosed => self.handle_tool_manager_closed(),
            AppMsg::SettingsClicked => self.handle_settings_clicked(root, &sender),
            AppMsg::SettingsClosed => self.handle_settings_closed(&sender),
            AppMsg::ManageGamesClicked => self.handle_manage_games_clicked(root, &sender),
            AppMsg::ManageGamesClosed => self.handle_manage_games_closed(&sender),
            AppMsg::GamesConfigured(configs, hidden_ids) => {
                self.handle_games_configured(configs, hidden_ids, &sender)
            }
            AppMsg::ShowWelcomeWizard => self.handle_show_welcome_wizard(root, &sender),
            AppMsg::WelcomeWizardConfirmed(configs, hidden_ids) => {
                self.handle_welcome_wizard_confirmed(configs, hidden_ids, &sender)
            }
            AppMsg::WelcomeWizardSkipped => self.handle_welcome_wizard_skipped(),
            AppMsg::RemoveGame(id) => self.confirm_remove_game(id, root, &sender),
            AppMsg::RemoveCurrentGame => self.handle_remove_current_game(root, &sender),
            AppMsg::RemoveGameConfirmed {
                game_id,
                delete_mods,
            } => self.handle_remove_game(game_id, delete_mods, &sender),
            AppMsg::CacheDirChangeRequested { game_id, new_dir } => {
                self.handle_cache_dir_change_requested(game_id, new_dir, &sender)
            }
            AppMsg::CacheDirResetRequested { game_id } => {
                self.handle_cache_dir_reset_requested(game_id, &sender)
            }
            AppMsg::ExportGameForSnap(game_id) => {
                self.handle_export_game_for_snap(game_id, root, &sender)
            }
            AppMsg::ExportGameForSnapChosen {
                game_id,
                output_path,
            } => self.handle_export_game_for_snap_chosen(game_id, output_path, &sender),
            AppMsg::PreviewAppImageExport => self.handle_preview_appimage_export(root, &sender),
            AppMsg::PreviewAppImageExportChosen(bundle_path) => {
                self.handle_preview_appimage_export_chosen(bundle_path, &sender)
            }
            AppMsg::ImportAppImageExport(bundle_path) => {
                self.handle_import_appimage_export(bundle_path, root, &sender)
            }
            AppMsg::ImportGameFolderChosen(path) => {
                self.handle_import_game_folder_chosen(path, root, &sender)
            }
            AppMsg::ImportWinePrefixChosen(path) => {
                self.handle_import_wine_prefix_chosen(path, &sender)
            }
            AppMsg::NexusApiKeyUpdated => self.handle_nexus_api_key_updated(&sender),
            AppMsg::NxmLinkReceived(link) => self.handle_nxm_link_received(link, &sender),
            AppMsg::CheckUpdatesClicked => self.handle_check_updates(&sender),
            AppMsg::ToggleDownloads => self.handle_toggle_downloads(),
            AppMsg::SetDownloadsVisible(visible) => self.handle_set_downloads_visible(visible),
            AppMsg::InstallDownload(idx) => self.handle_install_download(idx, &sender),
            AppMsg::ReinstallDownload(idx) => self.handle_reinstall_download(idx, &sender),
            AppMsg::ClearDownloadMetadata(idx) => self.handle_clear_download_metadata(idx, &sender),
            AppMsg::RenameDownload(idx) => self.handle_rename_download(idx, root, &sender),
            AppMsg::DeleteDownload(idx) => self.handle_delete_download(idx, root, &sender),
            AppMsg::ConfirmDeleteDownload(id) => self.handle_confirm_delete_download(id, &sender),
            AppMsg::HideDownload(idx) => self.handle_hide_download(idx, &sender),
            AppMsg::SetShowHiddenDownloads(show) => self.handle_set_show_hidden_downloads(show),
            AppMsg::ConfirmDownloadRename(id, name) => {
                self.handle_confirm_download_rename(id, name, &sender)
            }
            AppMsg::ConfirmNexusIdEntry(download_id, mod_id, domain) => {
                self.handle_confirm_nexus_id_entry(download_id, mod_id, domain, &sender)
            }
            AppMsg::FileIdDialogConfirmed {
                download_id,
                file_id,
                mod_id,
                domain,
            } => {
                self.handle_file_id_dialog_confirmed(download_id, file_id, mod_id, domain, &sender)
            }
            AppMsg::ShowFileIdDialog {
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
            AppMsg::DownloadProgress(id, fraction, message) => {
                self.handle_download_progress(id, fraction, message)
            }
            AppMsg::DownloadNameResolved(
                id,
                name,
                domain,
                file_name,
                is_primary,
                file_id,
                version,
                author,
            ) => self.handle_download_name_resolved(
                id, name, domain, file_name, is_primary, file_id, version, author, &sender,
            ),
            AppMsg::ArchiveMd5Computed(download_id, md5) => {
                self.handle_archive_md5_computed(download_id, md5, &sender)
            }
            AppMsg::FetchDownloadMetadata(idx) => {
                self.handle_fetch_download_metadata(idx, root, &sender)
            }
            AppMsg::ScanDownloadsFolder => self.handle_scan_downloads_folder(&sender),
            AppMsg::DownloadSortChanged(idx) => self.handle_download_sort_changed(idx),
            AppMsg::SearchToggled(active) => self.handle_search_toggled(active),
            AppMsg::SearchChanged(text) => self.handle_search_changed(text),
            AppMsg::ApplySearch => self.handle_apply_search(),
            AppMsg::SearchScopeChanged(idx) => self.handle_search_scope_changed(idx),
            AppMsg::RateLimitUpdated(info) => self.handle_rate_limit_updated(info),
            AppMsg::CloseRequested => self.handle_close_requested(root, &sender),
            AppMsg::ConfirmClose => self.handle_confirm_close(root),
            AppMsg::ToggleGroupCollapse(idx) => self.handle_toggle_group_collapse(idx),
            AppMsg::DeleteGroup(idx) => self.handle_delete_group(idx, &sender),
            AppMsg::CreateGroup(name) => self.handle_create_group(name, &sender),
            AppMsg::RenameGroup(idx, name) => self.handle_rename_group(idx, name),
            AppMsg::SetGroupColor(idx, color) => self.handle_set_group_color(idx, color, &sender),
            AppMsg::OpenModProperties(idx) => self.handle_open_mod_properties(idx, root, &sender),
            AppMsg::ModPropertiesApplied {
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
            AppMsg::ModPropertiesCancelled => self.handle_mod_properties_cancelled(),
            AppMsg::ScanExternalFiles => self.handle_scan_external_files(&sender),
            AppMsg::AbsorbExternalFiles => {
                self.handle_absorb_external_files_requested(root, &sender)
            }
            AppMsg::AbsorbFilesSelected(pairs) => {
                self.handle_absorb_files_selected(pairs, root, &sender)
            }
            AppMsg::DiscardExternalFiles(paths) => {
                self.handle_discard_external_files(paths, &sender)
            }
            AppMsg::CreateModFromExternalCancelled => {
                self.handle_create_mod_from_external_cancelled()
            }
            AppMsg::AdoptManagedPluginChanges(files) => {
                self.handle_adopt_managed_plugin_changes(files, &sender)
            }
            AppMsg::RestoreFromXEditBackup(files) => {
                self.handle_restore_from_xedit_backup(files, &sender)
            }
            AppMsg::ResetVanillaBaseline => self.handle_reset_vanilla_baseline(root, &sender),
            AppMsg::ResetVanillaBaselineConfirmed => {
                self.handle_reset_vanilla_baseline_confirmed(&sender)
            }
            AppMsg::MarkExternalFilesAsVanilla(files) => {
                self.handle_mark_external_files_as_vanilla(files, &sender)
            }
            AppMsg::CreateEmptyMod => self.handle_create_empty_mod(&sender),
            AppMsg::ScanModFromCache(mod_id) => self.handle_scan_mod_from_cache(mod_id, &sender),
            AppMsg::OpenPreInstallDialog => self.handle_open_pre_install_dialog(root, &sender),
            AppMsg::OpenPreInstallDialogReplacing(id, priority) => {
                self.handle_open_pre_install_dialog_replacing_request(id, priority, root, &sender)
            }
            AppMsg::SortWithLoot => self.handle_sort_with_loot(&sender),
            AppMsg::EnableAllMods => self.handle_enable_all_mods(&sender),
            AppMsg::DisableAllMods => self.handle_disable_all_mods(&sender),
            AppMsg::EnableAllPlugins => self.handle_enable_all_plugins(&sender),
            AppMsg::DisableAllPlugins => self.handle_disable_all_plugins(&sender),
            AppMsg::ToggleShowVanillaPlugins => self.handle_toggle_show_vanilla_plugins(),
            AppMsg::ShowToast(message) => self.handle_show_toast(message),
            AppMsg::NotificationDismissed => self.handle_notification_dismissed(),
            AppMsg::ClearNotifications => self.handle_clear_notifications(),
            AppMsg::ToggleProfileSaveMode => {
                self.handle_toggle_profile_save_mode_requested(&sender)
            }
            AppMsg::SyncSaves => self.handle_sync_saves_requested(&sender),
            AppMsg::AppUpdateAvailable(version, url) => {
                self.handle_app_update_available(version, url)
            }
            AppMsg::OpenUpdatePage => self.handle_open_update_page(),
            AppMsg::SelfUpdateDownload => self.handle_self_update_clicked(&sender),
            AppMsg::SaveModOrderSnapshot(name) => {
                self.handle_snapshot_action(SnapshotAction::SaveMod(name), &sender)
            }
            AppMsg::SavePluginOrderSnapshot(name) => {
                self.handle_snapshot_action(SnapshotAction::SavePlugin(name), &sender)
            }
            AppMsg::LoadModOrderSnapshot(id) => {
                self.handle_snapshot_action(SnapshotAction::LoadMod(id), &sender)
            }
            AppMsg::LoadPluginOrderSnapshot(id) => {
                self.handle_snapshot_action(SnapshotAction::LoadPlugin(id), &sender)
            }
            AppMsg::DeleteModOrderSnapshot(id) | AppMsg::DeletePluginOrderSnapshot(id) => {
                self.handle_snapshot_action(SnapshotAction::Delete(id), &sender)
            }
            AppMsg::SetModFilter(filter) => self.handle_set_mod_filter(filter),
            AppMsg::SetDownloadFilter(filter) => self.handle_set_download_filter(filter),
            AppMsg::OpenDeploymentFolder => self.handle_open_deployment_folder(),
            AppMsg::PauseDownload(idx) => self.handle_pause_download(idx),
            AppMsg::ResumeDownload(idx) => self.handle_resume_download(idx, &sender),
            AppMsg::SetColorScheme(idx) => self.handle_set_color_scheme(idx),
            AppMsg::NexusLoginClicked => self.handle_nexus_login_clicked(&sender),
            AppMsg::NexusLogoutClicked => self.handle_nexus_logout_clicked(&sender),
            AppMsg::EnterModSelectionMode => self.handle_enter_mod_selection_mode(),
            AppMsg::ExitModSelectionMode => self.handle_exit_mod_selection_mode(),
            AppMsg::ToggleModRowSelected(idx) => self.handle_toggle_mod_row_selected(idx),
            AppMsg::SetModRowSelected(idx, selected) => {
                self.handle_set_mod_row_selected(idx.current_index(), selected)
            }
            AppMsg::EnableSelectedMods => self.handle_enable_selected_mods(&sender),
            AppMsg::DisableSelectedMods => self.handle_disable_selected_mods(&sender),
            AppMsg::RemoveSelectedMods => self.handle_remove_selected_mods(root, &sender),
            AppMsg::ConfirmRemoveSelectedMods => self.handle_confirm_remove_selected_mods(&sender),
            AppMsg::EnterPluginSelectionMode => self.handle_enter_plugin_selection_mode(),
            AppMsg::ExitPluginSelectionMode => self.handle_exit_plugin_selection_mode(),
            AppMsg::TogglePluginRowSelected(idx) => self.handle_toggle_plugin_row_selected(idx),
            AppMsg::SetPluginRowSelected(idx, selected) => {
                self.handle_set_plugin_row_selected(idx.current_index(), selected)
            }
            AppMsg::EnableSelectedPlugins => self.handle_enable_selected_plugins(&sender),
            AppMsg::DisableSelectedPlugins => self.handle_disable_selected_plugins(&sender),
        }
    }

    pub(crate) fn dispatch_command(
        &mut self,
        msg: AppCmdMsg,
        sender: ComponentSender<Self>,
        root: &adw::ApplicationWindow,
    ) {
        match msg {
            AppCmdMsg::Initialized(result) => self.handle_cmd_initialized(result, &sender),
            AppCmdMsg::PendingMetadataFetched(name) => {
                self.handle_cmd_pending_metadata_fetched(name)
            }
            AppCmdMsg::PendingFileNameUnresolved {
                partial_name,
                download_id,
                mod_id,
                domain,
            } => self.handle_cmd_pending_file_name_unresolved(
                partial_name,
                download_id,
                mod_id,
                domain,
            ),
            AppCmdMsg::FileIdFetched {
                combined_name,
                download_id,
                version,
                file_id,
            } => self.handle_cmd_file_id_fetched(
                combined_name,
                download_id,
                version,
                file_id,
                &sender,
            ),
            AppCmdMsg::ModsLoaded(result, preserve) => {
                self.handle_cmd_mods_loaded(result, preserve, &sender)
            }
            AppCmdMsg::ModAdded(result, was_replace) => {
                self.handle_cmd_mod_added(result, was_replace, &sender)
            }
            AppCmdMsg::ModPrepared(result) => self.handle_cmd_mod_prepared(result, root, &sender),
            AppCmdMsg::ModRemoved(result, nexus_ids, mod_name, archive_hash) => {
                self.handle_cmd_mod_removed(result, nexus_ids, mod_name, archive_hash, &sender)
            }
            AppCmdMsg::DeployDone(result) => self.handle_cmd_deploy_done(result, &sender),
            AppCmdMsg::PurgeDone(result) => self.handle_cmd_purge_done(result),
            AppCmdMsg::CacheDirMoved {
                game_id,
                new_dir,
                result,
            } => self.handle_cmd_cache_dir_moved(game_id, new_dir, result),
            AppCmdMsg::CacheDirReset { game_id, result } => {
                self.handle_cmd_cache_dir_reset(game_id, result)
            }
            AppCmdMsg::GameExportedForSnap(result) => {
                self.handle_cmd_game_exported_for_snap(result)
            }
            AppCmdMsg::AppImageExportPreviewed(result) => {
                self.handle_cmd_appimage_export_previewed(result, root, &sender)
            }
            AppCmdMsg::AppImageExportImported(result) => {
                self.handle_cmd_appimage_export_imported(result, &sender)
            }
            AppCmdMsg::DownloadArchiveTrashed {
                download_id,
                result,
            } => self.handle_download_archive_trashed(download_id, result, &sender),
            AppCmdMsg::PrioritySaved(result) => self.handle_cmd_priority_saved(result, &sender),
            AppCmdMsg::OverridesRefreshed(result) => {
                self.handle_cmd_overrides_refreshed(result, &sender)
            }
            AppCmdMsg::PluginOrderSaved(result) => self.handle_cmd_plugin_order_saved(result),
            AppCmdMsg::ModNexusMetadataRefreshed { mod_id, result } => {
                self.handle_cmd_mod_nexus_metadata_refreshed(mod_id, result)
            }
            AppCmdMsg::ProfileSwitched(result) => self.handle_cmd_profile_switched(result, &sender),
            AppCmdMsg::ProfileCreated(result) => self.handle_cmd_profile_created(result, &sender),
            AppCmdMsg::ProfileCloned(result) => self.handle_cmd_profile_cloned(result, &sender),
            AppCmdMsg::ProfileRenamed(result) => self.handle_cmd_profile_renamed(result),
            AppCmdMsg::ProfileDeleted(result) => self.handle_cmd_profile_deleted(result, &sender),
            AppCmdMsg::ToolSaved(result) => self.handle_cmd_tool_saved(result),
            AppCmdMsg::ToolDeleted(result) => self.handle_cmd_tool_deleted(result),
            AppCmdMsg::ToolWorkingDirSaved(result) => {
                self.handle_cmd_tool_working_dir_saved(result)
            }
            AppCmdMsg::ToolLaunched(result) => self.handle_cmd_tool_launched(result),
            AppCmdMsg::ToolLaunchCancelled(name) => self.handle_cmd_tool_launch_cancelled(name),
            AppCmdMsg::ModMerged(result) => self.handle_cmd_mod_merged(result, &sender),
            AppCmdMsg::NxmDownloadComplete(id, result) => {
                self.handle_cmd_nxm_download_complete(id, result, &sender)
            }
            AppCmdMsg::NexusMetadataFetched(download_id, result) => {
                self.handle_cmd_nexus_metadata_fetched(download_id, result, &sender)
            }
            AppCmdMsg::UpdatesChecked(result) => self.handle_cmd_updates_checked(result, &sender),
            AppCmdMsg::DownloadsDirUpdated(dir) => self.handle_cmd_downloads_dir_updated(dir),
            AppCmdMsg::DownloadsScanned(result) => {
                self.handle_cmd_downloads_scanned(result, &sender)
            }
            AppCmdMsg::ExternalScanDone(result) => self.handle_cmd_external_scan_done(result),
            AppCmdMsg::ManagedPluginsAdopted(result) => {
                self.handle_cmd_managed_plugins_adopted(result, &sender)
            }
            AppCmdMsg::BackupRestored(result) => self.handle_cmd_backup_restored(result, &sender),
            AppCmdMsg::VanillaBaselineReset(result) => {
                self.handle_cmd_vanilla_baseline_reset(result, &sender)
            }
            AppCmdMsg::VanillaEntriesUpdated(result) => {
                self.handle_cmd_vanilla_entries_updated(result, &sender)
            }
            AppCmdMsg::EmptyModCreated(result) => {
                self.handle_cmd_empty_mod_created(result, &sender)
            }
            AppCmdMsg::ModFilesRescanned(result) => {
                self.handle_cmd_mod_files_rescanned(result, &sender)
            }
            #[cfg(feature = "loot")]
            AppCmdMsg::LootSortDone(game_id, result) => {
                self.handle_cmd_loot_sort_done(game_id, result, &sender)
            }
            #[cfg(feature = "loot")]
            AppCmdMsg::LootOrderApplied(result, post_action) => {
                self.handle_cmd_loot_order_applied(result, post_action, &sender)
            }
            AppCmdMsg::ModFilesLoaded(files) => self.handle_cmd_mod_files_loaded(files),
            AppCmdMsg::SaveModeToggled(result) => {
                self.handle_cmd_save_mode_toggled(result, &sender)
            }
            AppCmdMsg::SavesSynced(result) => self.handle_cmd_saves_synced(result),
            AppCmdMsg::LastDeployedProfileLoaded(id) => {
                self.handle_cmd_last_deployed_profile_loaded(id)
            }
            AppCmdMsg::AppUpdateResult(result) => self.handle_cmd_app_update_result(result),
            AppCmdMsg::FomodSelectionsLoaded(selections) => {
                self.handle_cmd_fomod_selections_loaded(selections, root, &sender)
            }
            AppCmdMsg::GamesPersisted => self.handle_cmd_games_persisted(&sender),
            AppCmdMsg::OrderSnapshotsLoaded(mod_snapshots, plugin_snapshots) => {
                self.handle_cmd_order_snapshots_loaded(mod_snapshots, plugin_snapshots, &sender)
            }
            AppCmdMsg::ModOrderSnapshotSaved(result) => {
                self.handle_cmd_mod_order_snapshot_saved(result, &sender)
            }
            AppCmdMsg::PluginOrderSnapshotSaved(result) => {
                self.handle_cmd_plugin_order_snapshot_saved(result, &sender)
            }
            AppCmdMsg::ModOrderSnapshotRestored(result) => {
                self.handle_cmd_mod_order_snapshot_restored(result, &sender)
            }
            AppCmdMsg::PluginOrderSnapshotRestored(result) => {
                self.handle_cmd_plugin_order_snapshot_restored(result, &sender)
            }
            AppCmdMsg::OrderSnapshotDeleted(result) => {
                self.handle_cmd_order_snapshot_deleted(result, &sender)
            }
            AppCmdMsg::NexusAvatarLoaded(bytes) => self.handle_cmd_nexus_avatar_loaded(bytes),
            AppCmdMsg::NexusUserRefreshed(username, avatar_url, is_premium) => {
                self.handle_cmd_nexus_user_refreshed(username, avatar_url, is_premium, &sender)
            }
        }
    }
}
