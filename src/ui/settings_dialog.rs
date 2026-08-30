use std::path::PathBuf;

use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::nexus_api::NexusClient;
use crate::core::tracker::Tracker;
use crate::utils::paths;
use crate::utils::snap::{self, SelectedFolderKind, SelectedFolderRecovery};

pub struct SettingsDialog {
    tracker: Tracker,
    is_logged_in: bool,
    api_key_row: adw::PasswordEntryRow,
    status_label: gtk::Label,
    downloads_status_label: gtk::Label,
    test_button: gtk::Button,
    save_button: gtk::Button,
    downloads_dir: String,
    can_preview_appimage_export: bool,
}

#[derive(Debug)]
pub enum SettingsMsg {
    TestKey,
    Save,
    Close,
    BrowseDownloadsDir,
    DownloadsDirChosen(DownloadsFolderSelection),
    PreviewAppImageExport,
    ManageGames,
    SetColorScheme(u32),
}

#[derive(Debug)]
pub enum SettingsCmdMsg {
    KeyValidated(Result<String, String>),
    KeySaved(Result<(), String>),
    KeyLoaded(Result<Option<String>, String>),
    DownloadsDirSelected(Result<Option<DownloadsFolderSelection>, String>),
    DownloadsDirLoaded(Result<Option<String>, String>),
    DownloadsDirSaved(Result<(), String>),
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DownloadsFolderSelection {
    path: PathBuf,
    portal_host_path: Option<PathBuf>,
}

async fn select_downloads_folder() -> anyhow::Result<Option<DownloadsFolderSelection>> {
    let Some(path) = crate::utils::portal::select_folder("Select Downloads Folder").await? else {
        return Ok(None);
    };
    let portal_host_path = crate::utils::portal::document_portal_host_path(&path).await;

    Ok(Some(DownloadsFolderSelection {
        path,
        portal_host_path,
    }))
}

fn downloads_dir_selection(
    result: anyhow::Result<Option<DownloadsFolderSelection>>,
) -> Result<Option<DownloadsFolderSelection>, String> {
    match result {
        Ok(path) => Ok(path),
        Err(error)
            if matches!(
                error.downcast_ref::<ashpd::Error>(),
                Some(ashpd::Error::Response(
                    ashpd::desktop::ResponseError::Cancelled
                ))
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(format!(
            "Failed to open the downloads folder picker: {error}"
        )),
    }
}

fn needs_removable_media_connection(
    selection: &DownloadsFolderSelection,
    recovery: Option<SelectedFolderRecovery>,
) -> bool {
    recovery == Some(SelectedFolderRecovery::ConnectRemovableMedia)
        || selection
            .portal_host_path
            .as_deref()
            .is_some_and(snap::is_removable_media_path)
}

#[derive(Debug)]
pub enum SettingsDialogOutput {
    Closed,
    /// Emitted whenever the active API key changes (manual save).
    ApiKeyChanged,
    /// User wants to open the game setup dialog.
    ManageGames,
    /// User wants to preview an AppImage export bundle.
    PreviewAppImageExport,
    /// User changed the color scheme (0=System, 1=Light, 2=Dark).
    ColorSchemeChanged(u32),
}

#[relm4::component(pub)]
impl Component for SettingsDialog {
    /// (tracker, is_logged_in, color_scheme_idx, can_preview_appimage_export)
    type Init = (Tracker, bool, u32, bool);
    type Input = SettingsMsg;
    type Output = SettingsDialogOutput;
    type CommandOutput = SettingsCmdMsg;

    view! {
        adw::PreferencesWindow {
            set_title: Some("Settings"),
            set_search_enabled: false,
            set_default_size: (550, 660),

            add = &adw::PreferencesPage {

                // Nexus Mods section — manual API key entry for power users (hidden when logged in)
                add = &adw::PreferencesGroup {
                    set_title: "Nexus Mods",
                    #[watch]
                    set_visible: !model.is_logged_in,
                    set_description: Some("Use the account button in the title bar to log in or out via SSO. To use a manual API key instead, enter it below."),

                    #[local_ref]
                    add = api_key_row -> adw::PasswordEntryRow {
                        set_title: "Manual API Key",
                    },

                    add = &gtk::Box {
                        set_spacing: 8,
                        set_margin_bottom: 4,

                        #[local_ref]
                        test_button -> gtk::Button {
                            set_label: "Test",
                            add_css_class: "flat",
                            connect_clicked => SettingsMsg::TestKey,
                        },

                        #[local_ref]
                        save_button -> gtk::Button {
                            set_label: "Save",
                            add_css_class: "suggested-action",
                            connect_clicked => SettingsMsg::Save,
                        },
                    },

                    add = &gtk::Box {
                        set_margin_bottom: 8,

                        #[local_ref]
                        status_label -> gtk::Label {
                            set_halign: gtk::Align::Start,
                            set_wrap: true,
                            set_visible: false,
                        },
                    },
                },

                // Downloads section
                add = &adw::PreferencesGroup {
                    set_title: "Downloads",

                    add = &adw::ActionRow {
                        set_title: "Downloads Folder",
                        #[watch]
                        set_subtitle: &model.downloads_dir,
                        set_subtitle_lines: 1,

                        add_suffix = &gtk::Button {
                            set_icon_name: "folder-open-symbolic",
                            set_tooltip_text: Some("Browse"),
                            set_valign: gtk::Align::Center,
                            add_css_class: "flat",
                            connect_clicked => SettingsMsg::BrowseDownloadsDir,
                        },
                    },

                    #[local_ref]
                    add = downloads_status_label -> gtk::Label {
                        set_halign: gtk::Align::Start,
                        set_wrap: true,
                        set_visible: false,
                    },
                },

                // Games section
                add = &adw::PreferencesGroup {
                    set_title: "Games",

                    add = &adw::ActionRow {
                        set_title: "Manage Games",
                        set_subtitle: "Configure directories and Wine prefix per game",
                        set_activatable: true,
                        connect_activated => SettingsMsg::ManageGames,

                        add_suffix = &gtk::Image::from_icon_name("go-next-symbolic") {
                            set_valign: gtk::Align::Center,
                        },
                    },
                },

                // Migration section
                add = &adw::PreferencesGroup {
                    set_title: "Migration",
                    #[watch]
                    set_visible: model.can_preview_appimage_export,

                    add = &adw::ActionRow {
                        set_title: "Preview AppImage Export",
                        set_subtitle: "Inspect a migration bundle before importing it",
                        set_activatable: true,
                        connect_activated => SettingsMsg::PreviewAppImageExport,

                        add_suffix = &gtk::Image::from_icon_name("document-open-symbolic") {
                            set_valign: gtk::Align::Center,
                        },
                    },
                },

                // Appearance section
                add = &adw::PreferencesGroup {
                    set_title: "Appearance",

                    #[name = "color_scheme_combo"]
                    add = &adw::ComboRow {
                        set_title: "Color Scheme",
                        set_model: Some(&gtk::StringList::new(&["System", "Light", "Dark"])),
                        connect_selected_notify[sender] => move |row| {
                            sender.input(SettingsMsg::SetColorScheme(row.selected()));
                        },
                    },

                },

                // About section
                add = &adw::PreferencesGroup {
                    set_title: "About",

                    add = &adw::ActionRow {
                        set_title: "Deployd",
                        set_subtitle: concat!("Version ", env!("CARGO_PKG_VERSION")),
                    },

                    #[name = "about_kofi_row"]
                    add = &adw::ActionRow {
                        set_title: "Support Development",
                        set_subtitle: "ko-fi.com/mattianelo",
                    },
                },
            },

            connect_close_request[sender] => move |_| {
                sender.input(SettingsMsg::Close);
                glib::Propagation::Proceed
            },
        }
    }

    fn init(
        (tracker, is_logged_in, color_scheme_idx, can_preview_appimage_export): Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let api_key_row = adw::PasswordEntryRow::new();
        let status_label = gtk::Label::new(None);
        let downloads_status_label = gtk::Label::new(None);
        let test_button = gtk::Button::new();
        let save_button = gtk::Button::new();

        let default_dir = paths::default_downloads_dir().to_string_lossy().to_string();

        let model = SettingsDialog {
            tracker,
            is_logged_in,
            api_key_row,
            status_label,
            downloads_status_label,
            test_button,
            save_button,
            downloads_dir: default_dir,
            can_preview_appimage_export,
        };

        let api_key_row = &model.api_key_row;
        let status_label = &model.status_label;
        let downloads_status_label = &model.downloads_status_label;
        let test_button = &model.test_button;
        let save_button = &model.save_button;
        let widgets = view_output!();

        // Ko-fi support row suffix
        let kofi_image = gtk::Image::from_resource("/io/mattianelo/Deployd/kofi-logo.svg");
        kofi_image.set_pixel_size(24);
        kofi_image.set_valign(gtk::Align::Center);

        let kofi_link =
            gtk::LinkButton::with_label("https://ko-fi.com/mattianelo", "Support on Ko-fi");
        kofi_link.set_valign(gtk::Align::Center);
        kofi_link.add_css_class("flat");

        let kofi_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        kofi_box.set_valign(gtk::Align::Center);
        kofi_box.append(&kofi_image);
        kofi_box.append(&kofi_link);

        widgets.about_kofi_row.add_suffix(&kofi_box);

        // Load existing API key (manual keys only; SSO keys are not shown here)
        let t = model.tracker.clone();
        sender.oneshot_command(async move {
            let result = async {
                let key = t
                    .get_setting("nexus_api_key")
                    .await
                    .map_err(|error| error.to_string())?;
                let source = t
                    .get_setting("nexus_login_source")
                    .await
                    .map_err(|error| error.to_string())?;
                Ok(if source.as_deref() == Some("sso") {
                    None
                } else {
                    key
                })
            }
            .await;
            SettingsCmdMsg::KeyLoaded(result)
        });

        // Load existing downloads dir
        let t2 = model.tracker.clone();
        sender.oneshot_command(async move {
            SettingsCmdMsg::DownloadsDirLoaded(
                t2.get_setting("downloads_dir")
                    .await
                    .map_err(|error| error.to_string()),
            )
        });

        // Initialise appearance controls with persisted values.
        widgets.color_scheme_combo.set_selected(color_scheme_idx);

        gtk::glib::idle_add_local_once({
            let root = root.clone();
            move || root.present()
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            SettingsMsg::TestKey => {
                let key = self.api_key_row.text().to_string();
                if key.is_empty() {
                    self.status_label.set_label("Please enter an API key.");
                    self.status_label.remove_css_class("success");
                    self.status_label.add_css_class("error");
                    self.status_label.set_visible(true);
                    return;
                }

                self.status_label.set_label("Validating…");
                self.status_label.remove_css_class("error");
                self.status_label.remove_css_class("success");
                self.status_label.set_visible(true);
                self.test_button.set_sensitive(false);

                sender.oneshot_command(async move {
                    let client = NexusClient::new(key);
                    match client.validate_key().await {
                        Ok((user, _rate_limits)) => {
                            let premium = if user.is_premium { " (Premium)" } else { "" };
                            SettingsCmdMsg::KeyValidated(Ok(format!(
                                "Valid — {}{}",
                                user.name, premium
                            )))
                        }
                        Err(e) => SettingsCmdMsg::KeyValidated(Err(e.to_string())),
                    }
                });
            }
            SettingsMsg::Save => {
                let key = self.api_key_row.text().to_string();
                let tracker = self.tracker.clone();

                self.save_button.set_sensitive(false);
                self.status_label.set_label("Saving…");
                self.status_label.remove_css_class("error");
                self.status_label.remove_css_class("success");
                self.status_label.set_visible(true);

                sender.oneshot_command(async move {
                    if let Err(e) = tracker.set_setting("nexus_api_key", &key).await {
                        return SettingsCmdMsg::KeySaved(Err(e.to_string()));
                    }
                    if let Err(error) = tracker
                        .set_setting(
                            "nexus_login_source",
                            if key.is_empty() { "" } else { "manual" },
                        )
                        .await
                    {
                        return SettingsCmdMsg::KeySaved(Err(error.to_string()));
                    }

                    // Validate and cache premium status + user info
                    if !key.is_empty() {
                        let client = NexusClient::new(key);
                        if let Ok((user, _)) = client.validate_key().await {
                            if let Err(error) = tracker
                                .set_setting(
                                    "nexus_is_premium",
                                    if user.is_premium { "true" } else { "false" },
                                )
                                .await
                            {
                                return SettingsCmdMsg::KeySaved(Err(error.to_string()));
                            }
                            if let Err(error) = tracker.save_nexus_user(&user).await {
                                return SettingsCmdMsg::KeySaved(Err(error.to_string()));
                            }
                        }
                    }

                    SettingsCmdMsg::KeySaved(Ok(()))
                });
            }
            SettingsMsg::Close => {
                let _ = sender.output(SettingsDialogOutput::Closed);
            }
            SettingsMsg::BrowseDownloadsDir => {
                sender.oneshot_command(async {
                    SettingsCmdMsg::DownloadsDirSelected(downloads_dir_selection(
                        select_downloads_folder().await,
                    ))
                });
            }
            SettingsMsg::ManageGames => {
                let _ = sender.output(SettingsDialogOutput::ManageGames);
                root.close();
            }
            SettingsMsg::PreviewAppImageExport => {
                let _ = sender.output(SettingsDialogOutput::PreviewAppImageExport);
                root.close();
            }
            SettingsMsg::SetColorScheme(idx) => {
                let _ = sender.output(SettingsDialogOutput::ColorSchemeChanged(idx));
            }
            SettingsMsg::DownloadsDirChosen(selection) => {
                if let Err(error) = snap::validate_selected_folder(
                    &selection.path,
                    SelectedFolderKind::DownloadsFolder,
                ) {
                    if needs_removable_media_connection(&selection, error.recovery()) {
                        self.downloads_status_label.set_visible(false);
                        present_removable_media_dialog(root);
                    } else {
                        self.downloads_status_label.set_label(&error.to_string());
                        self.downloads_status_label.remove_css_class("success");
                        self.downloads_status_label.add_css_class("error");
                        self.downloads_status_label.set_visible(true);
                    }
                    return;
                }
                self.downloads_status_label.set_visible(false);
                let dir_str = selection.path.to_string_lossy().to_string();
                self.downloads_dir = dir_str.clone();

                let tracker = self.tracker.clone();
                sender.oneshot_command(async move {
                    SettingsCmdMsg::DownloadsDirSaved(
                        tracker
                            .set_setting("downloads_dir", &dir_str)
                            .await
                            .map_err(|e| e.to_string()),
                    )
                });
            }
        }
    }

    fn update_cmd(
        &mut self,
        msg: Self::CommandOutput,
        sender: ComponentSender<Self>,
        _root: &Self::Root,
    ) {
        match msg {
            SettingsCmdMsg::KeyValidated(result) => {
                self.test_button.set_sensitive(true);
                match result {
                    Ok(info) => {
                        self.status_label.set_label(&info);
                        self.status_label.remove_css_class("error");
                        self.status_label.add_css_class("success");
                    }
                    Err(e) => {
                        self.status_label.set_label(&format!("Error: {e}"));
                        self.status_label.remove_css_class("success");
                        self.status_label.add_css_class("error");
                    }
                }
                self.status_label.set_visible(true);
            }
            SettingsCmdMsg::KeySaved(result) => {
                self.save_button.set_sensitive(true);
                match result {
                    Ok(()) => {
                        self.status_label.set_label("Saved.");
                        self.status_label.remove_css_class("error");
                        self.status_label.add_css_class("success");
                        let _ = sender.output(SettingsDialogOutput::ApiKeyChanged);
                    }
                    Err(e) => {
                        self.status_label.set_label(&format!("Save failed: {e}"));
                        self.status_label.remove_css_class("success");
                        self.status_label.add_css_class("error");
                    }
                }
                self.status_label.set_visible(true);
            }
            SettingsCmdMsg::KeyLoaded(result) => match result {
                Ok(Some(key)) if !key.is_empty() => self.api_key_row.set_text(&key),
                Ok(_) => {}
                Err(error) => {
                    self.status_label
                        .set_label(&format!("Failed to load Nexus settings: {error}"));
                    self.status_label.add_css_class("error");
                    self.status_label.set_visible(true);
                }
            },
            SettingsCmdMsg::DownloadsDirSelected(result) => match result {
                Ok(Some(path)) => sender.input(SettingsMsg::DownloadsDirChosen(path)),
                Ok(None) => {}
                Err(error) => {
                    self.downloads_status_label.set_label(&error);
                    self.downloads_status_label.remove_css_class("success");
                    self.downloads_status_label.add_css_class("error");
                    self.downloads_status_label.set_visible(true);
                }
            },
            SettingsCmdMsg::DownloadsDirLoaded(result) => match result {
                Ok(Some(dir)) => self.downloads_dir = dir,
                Ok(None) => {}
                Err(error) => {
                    self.status_label
                        .set_label(&format!("Failed to load downloads settings: {error}"));
                    self.status_label.add_css_class("error");
                    self.status_label.set_visible(true);
                }
            },
            SettingsCmdMsg::DownloadsDirSaved(result) => {
                if let Err(error) = result {
                    self.downloads_status_label
                        .set_label(&format!("Failed to save downloads settings: {error}"));
                    self.downloads_status_label.remove_css_class("success");
                    self.downloads_status_label.add_css_class("error");
                    self.downloads_status_label.set_visible(true);
                }
            }
        }
    }
}

fn present_removable_media_dialog(root: &adw::PreferencesWindow) {
    let dialog = adw::AlertDialog::builder()
        .heading("Connect External Drive Access")
        .body(snap::removable_media_connection_message())
        .build();
    dialog.add_response("close", "Close");
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");

    let command_entry = gtk::Entry::builder()
        .text(snap::REMOVABLE_MEDIA_CONNECT_COMMAND)
        .editable(false)
        .hexpand(true)
        .build();
    command_entry.add_css_class("monospace");

    let copy_button = gtk::Button::builder()
        .icon_name("edit-copy-symbolic")
        .tooltip_text("Copy command")
        .build();
    copy_button.connect_clicked(move |_| {
        if let Some(display) = gtk::gdk::Display::default() {
            display
                .clipboard()
                .set_text(snap::REMOVABLE_MEDIA_CONNECT_COMMAND);
        }
    });

    let command_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    command_box.append(&command_entry);
    command_box.append(&copy_button);
    dialog.set_extra_child(Some(&command_box));
    dialog.present(Some(root));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use anyhow::anyhow;

    use super::{
        DownloadsFolderSelection, downloads_dir_selection, needs_removable_media_connection,
    };
    use crate::utils::snap::SelectedFolderRecovery;

    #[test]
    fn returns_folder_selected_through_portal() {
        let path = PathBuf::from("/run/user/1000/doc/abcd/Downloads");
        let selection = DownloadsFolderSelection {
            path: path.clone(),
            portal_host_path: Some(PathBuf::from("/media/alex/External/Downloads")),
        };

        let selected =
            downloads_dir_selection(Ok(Some(selection))).expect("portal selection should succeed");

        assert_eq!(selected.map(|selection| selection.path), Some(path));
    }

    #[test]
    fn preserves_cancelled_portal_selection() {
        let error = ashpd::Error::from(ashpd::desktop::ResponseError::Cancelled);

        let selected =
            downloads_dir_selection(Err(error.into())).expect("cancellation should not fail");

        assert_eq!(selected, None);
    }

    #[test]
    fn reports_portal_failure() {
        let error = downloads_dir_selection(Err(anyhow!("portal unavailable")))
            .expect_err("portal failure should be reported");

        assert_eq!(
            error,
            "Failed to open the downloads folder picker: portal unavailable"
        );
    }

    #[test]
    fn prompts_when_broken_portal_route_targets_external_drive() {
        let selection = DownloadsFolderSelection {
            path: PathBuf::from("/run/user/1000/doc/abcd/Downloads"),
            portal_host_path: Some(PathBuf::from("/media/alex/External/Downloads")),
        };

        assert!(needs_removable_media_connection(&selection, None));
    }

    #[test]
    fn keeps_non_external_portal_failures_inline() {
        let selection = DownloadsFolderSelection {
            path: PathBuf::from("/run/user/1000/doc/abcd/Downloads"),
            portal_host_path: Some(PathBuf::from("/home/alex/Downloads")),
        };

        assert!(!needs_removable_media_connection(&selection, None));
    }

    #[test]
    fn prompts_for_direct_removable_media_recovery() {
        let selection = DownloadsFolderSelection {
            path: PathBuf::from("/media/alex/External/Downloads"),
            portal_host_path: None,
        };

        assert!(needs_removable_media_connection(
            &selection,
            Some(SelectedFolderRecovery::ConnectRemovableMedia)
        ));
    }
}
