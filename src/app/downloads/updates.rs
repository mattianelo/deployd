use relm4::prelude::*;

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
