use relm4::prelude::*;

use std::collections::HashMap;

use super::super::App;
use super::super::messages::AppCmdMsg;

#[derive(Debug, PartialEq, Eq)]
enum SelfUpdateTarget {
    ReleasePage,
    AppImage(String),
}

fn self_update_target(appimage_path: Option<String>) -> SelfUpdateTarget {
    match appimage_path {
        Some(path) => SelfUpdateTarget::AppImage(path),
        None => SelfUpdateTarget::ReleasePage,
    }
}

impl App {
    pub(crate) fn refresh_all_installed_nexus_updates(&self, sender: &ComponentSender<Self>) {
        let (Some(tracker), Some(game)) =
            (self.session.tracker.clone(), self.selected_game().cloned())
        else {
            return;
        };
        let input = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let refresh_result: Result<usize, String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .map_err(|error| error.to_string())?
                    .filter(|key| !key.is_empty())
                    .ok_or_else(|| "No Nexus API key configured".to_string())?;
                let mods = tracker
                    .list_mods(&game.id)
                    .await
                    .map_err(|error| error.to_string())?;
                let mut groups: HashMap<(String, i64), Vec<_>> = HashMap::new();
                for entry in mods {
                    if let (Some(domain), Some(mod_id), Some(file_id)) = (
                        entry.nexus_domain.clone(),
                        entry.nexus_mod_id,
                        entry.nexus_file_id,
                    ) && file_id > 0
                    {
                        groups.entry((domain, mod_id)).or_default().push(entry);
                    }
                }
                let client = crate::core::nexus_api::NexusClient::new(api_key);
                let mut failures = 0;
                for ((domain, mod_id), entries) in groups {
                    let files = match client.get_mod_files(&domain, mod_id).await {
                        Ok((files, _)) => files,
                        Err(error) => {
                            failures += 1;
                            eprintln!(
                                "deployd: failed to refresh Nexus updates for {domain}/{mod_id}: {error:#}"
                            );
                            continue;
                        }
                    };
                    for entry in entries {
                        let latest = entry.nexus_file_id.and_then(|file_id| {
                            super::metadata::latest_file_version(
                                &files.files,
                                &files.file_updates,
                                file_id,
                            )
                        });
                        if let Err(error) = tracker
                            .set_mod_latest_version(&entry.id, latest.as_deref())
                            .await
                        {
                            failures += 1;
                            eprintln!("deployd: failed to store Nexus update status: {error:#}");
                        }
                    }
                }
                Ok(failures)
            }
            .await;
            match refresh_result {
                Ok(failures) => {
                    if failures > 0 {
                        let _ = input.send(crate::app::messages::AppMsg::Shell(
                            crate::app::messages::ShellMsg::ShowToast(format!(
                                "Could not refresh updates for {failures} Nexus mod(s)"
                            )),
                        ));
                    }
                }
                Err(error) if error == "No Nexus API key configured" => {}
                Err(error) => {
                    let _ = input.send(crate::app::messages::AppMsg::Shell(
                        crate::app::messages::ShellMsg::ShowToast(format!(
                            "Could not refresh Nexus updates: {error}"
                        )),
                    ));
                }
            }
            let result = crate::app::session::load_game_data(
                &tracker,
                &game,
                crate::app::session::GameLoadMode::Refresh,
            )
            .await;
            crate::app::messages::AppCmdMsg::Games(
                crate::app::messages::GamesCmdMsg::ModsLoaded(result, true),
            )
        });
    }

    pub(crate) fn handle_app_update_available(&mut self, version: String, url: String) {
        self.shell.app_update_version = Some(format!("Deployd {version} is available"));
        self.shell.app_update_url = Some(url);
    }

    pub(crate) fn handle_self_update_clicked(&mut self, sender: &ComponentSender<Self>) {
        self.ui.notifications_menu_btn.popdown();
        self.handle_self_update_download(sender);
    }

    fn handle_self_update_download(&mut self, sender: &ComponentSender<Self>) {
        use crate::core::nexus_api::NexusClient;
        use crate::core::update_check::{NEXUS_DOMAIN, NEXUS_MOD_ID};

        let SelfUpdateTarget::AppImage(appimage_path) =
            self_update_target(std::env::var("APPIMAGE").ok())
        else {
            self.open_update_page();
            return;
        };

        let Some(tracker) = self.session.tracker.clone() else {
            return;
        };

        self.show_toast("Downloading update...");

        sender.oneshot_command(async move {
            let result: Result<(), String> = async {
                let api_key = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .ok()
                    .flatten()
                    .filter(|k| !k.is_empty())
                    .ok_or_else(|| "No Nexus API key — configure it in Settings.".to_string())?;

                let client = NexusClient::new(api_key);

                let (files_resp, _) = client
                    .get_mod_files(NEXUS_DOMAIN, NEXUS_MOD_ID)
                    .await
                    .map_err(|e| e.to_string())?;

                let file = files_resp
                    .files
                    .into_iter()
                    .find(|f| f.is_primary || f.file_name.ends_with(".AppImage"))
                    .ok_or_else(|| "AppImage file not found on the Nexus mod page.".to_string())?;

                let (links, _) = client
                    .get_download_links(NEXUS_DOMAIN, NEXUS_MOD_ID, file.file_id, None, None)
                    .await
                    .map_err(|e| e.to_string())?;

                let url = links
                    .into_iter()
                    .next()
                    .map(|l| l.uri)
                    .ok_or_else(|| "No download link returned by Nexus.".to_string())?;

                let temp_path = format!("{appimage_path}.new");
                let dest = std::path::Path::new(&temp_path);

                client
                    .download_file(&url, dest, |_, _| {})
                    .await
                    .map_err(|e| e.to_string())?;

                // Make the downloaded file executable before replacing the running one.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o755);
                    std::fs::set_permissions(&temp_path, perms).map_err(|e| e.to_string())?;
                }

                std::fs::rename(&temp_path, &appimage_path).map_err(|e| e.to_string())?;

                Ok(())
            }
            .await;
            AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::AppUpdateResult(result))
        });
    }

    pub(crate) fn handle_cmd_app_update_result(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.push_notification(
                    "Update downloaded. Restart deployd to use the new version.",
                );
            }
            Err(error) => {
                self.push_notification(&format!("Update failed: {error}"));
                if error.contains("premium") {
                    self.open_update_page();
                }
            }
        }
    }

    fn open_update_page(&mut self) {
        let url = self
            .shell
            .app_update_url
            .as_deref()
            .unwrap_or(crate::core::update_check::NEXUS_PAGE_URL);
        if let Err(error) = open::that(url) {
            self.push_notification(&format!("Could not open update page: {error}"));
        }
    }
}

pub(crate) async fn refresh_nexus_update_for_mod(
    tracker: &crate::core::tracker::Tracker,
    entry: &crate::models::mod_entry::ModEntry,
) -> Result<(), String> {
    let (Some(domain), Some(nexus_mod_id), Some(nexus_file_id)) = (
        entry.nexus_domain.as_deref(),
        entry.nexus_mod_id,
        entry.nexus_file_id,
    ) else {
        return Ok(());
    };
    if nexus_file_id <= 0 {
        return Ok(());
    }
    let Some(api_key) = tracker
        .get_setting("nexus_api_key")
        .await
        .map_err(|error| error.to_string())?
        .filter(|key| !key.is_empty())
    else {
        return Ok(());
    };
    let client = crate::core::nexus_api::NexusClient::new(api_key);
    let (files, _) = client
        .get_mod_files(domain, nexus_mod_id)
        .await
        .map_err(|error| error.to_string())?;
    let latest =
        super::metadata::latest_file_version(&files.files, &files.file_updates, nexus_file_id);
    tracker
        .set_mod_latest_version(&entry.id, latest.as_deref())
        .await
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{SelfUpdateTarget, self_update_target};

    // @variants: snap
    #[test]
    fn non_appimage_packages_open_release_page() {
        assert_eq!(self_update_target(None), SelfUpdateTarget::ReleasePage);
    }

    // @variants: appimage
    #[test]
    fn appimage_package_replaces_running_image() {
        let path = "/tmp/Deployd.AppImage".to_string();

        assert_eq!(
            self_update_target(Some(path.clone())),
            SelfUpdateTarget::AppImage(path)
        );
    }
}
