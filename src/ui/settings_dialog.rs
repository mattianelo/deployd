use std::path::PathBuf;

use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::nexus_api::NexusClient;
use crate::core::proton_manager;
use crate::core::tracker::Tracker;
use crate::models::proton_release::ProtonRelease;
use crate::utils::paths;

#[derive(Debug, Clone, PartialEq)]
pub enum LoginSource {
    Sso,
    Manual,
}

pub struct SettingsDialog {
    tracker: Tracker,
    api_key_entry: gtk::Entry,
    status_label: gtk::Label,
    test_button: gtk::Button,
    save_button: gtk::Button,
    login_button: gtk::Button,
    logout_button: gtk::Button,
    sso_in_progress: bool,
    has_key: bool,
    login_source: Option<LoginSource>,
    downloads_dir: String,
    // Runtimes
    installed_list: gtk::ListBox,
    available_list: gtk::ListBox,
    runtime_status_label: gtk::Label,
    fetch_runtimes_btn: gtk::Button,
    installing_runtime: bool,
    /// Available releases fetched from GitHub.
    available_releases: Vec<ProtonRelease>,
}

#[derive(Debug)]
pub enum SettingsMsg {
    TestKey,
    Save,
    NexusLogin,
    Logout,
    Close,
    BrowseDownloadsDir,
    DownloadsDirChosen(PathBuf),
    ManageGames,
    // Runtimes
    FetchRuntimes,
    InstallRuntime { tag: String, url: String },
    RemoveRuntime(String),
    SetActiveRuntime(String),
    RuntimeProgress { downloaded: u64, total: u64 },
}

#[derive(Debug)]
pub enum SettingsCmdMsg {
    KeyValidated(Result<String, String>),
    KeySaved(Result<(), String>),
    KeyLoaded(Option<String>, Option<LoginSource>),
    SsoResult(Result<String, String>),
    LoggedOut(Result<(), String>),
    DownloadsDirLoaded(Option<String>),
    DownloadsDirSaved(Result<(), String>),
    // Runtimes
    RuntimesFetched(Result<Vec<ProtonRelease>, String>),
    RuntimeInstalled {
        tag: String,
        result: Result<(), String>,
    },
}

#[derive(Debug)]
pub enum SettingsDialogOutput {
    Closed,
    /// Emitted whenever the active API key changes (login, logout, manual save).
    ApiKeyChanged,
    /// User wants to open the game setup dialog.
    ManageGames,
}

#[relm4::component(pub)]
impl Component for SettingsDialog {
    type Init = Tracker;
    type Input = SettingsMsg;
    type Output = SettingsDialogOutput;
    type CommandOutput = SettingsCmdMsg;

    view! {
        adw::PreferencesWindow {
            set_title: Some("Settings"),
            set_search_enabled: false,
            set_default_size: (550, 720),

            add = &adw::PreferencesPage {

                // Nexus Mods section
                add = &adw::PreferencesGroup {
                    set_title: "Nexus Mods",

                    // Login / logout row
                    add = &gtk::Box {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_spacing: 8,
                        set_margin_top: 8,
                        set_margin_bottom: 4,

                        #[local_ref]
                        login_button -> gtk::Button {
                            set_label: "Login with Nexus",
                            add_css_class: "suggested-action",
                            set_hexpand: true,
                            #[watch]
                            set_sensitive: !model.has_key && !model.sso_in_progress,
                            connect_clicked => SettingsMsg::NexusLogin,
                        },

                        #[local_ref]
                        logout_button -> gtk::Button {
                            set_label: "Log Out",
                            add_css_class: "destructive-action",
                            set_hexpand: true,
                            #[watch]
                            set_visible: model.has_key,
                            connect_clicked => SettingsMsg::Logout,
                        },
                    },

                    // Manual API key entry (hidden when using SSO; suffix added imperatively)
                    #[name = "api_key_row"]
                    add = &adw::ActionRow {
                        set_title: "API Key",
                        #[watch]
                        set_visible: model.login_source != Some(LoginSource::Sso),
                    },

                    add = &gtk::Box {
                        set_spacing: 8,
                        set_margin_bottom: 4,
                        #[watch]
                        set_visible: model.login_source != Some(LoginSource::Sso),

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

                // Runtimes section — installed versions
                add = &adw::PreferencesGroup {
                    set_title: "Installed Runtimes",

                    #[local_ref]
                    installed_list -> gtk::ListBox {
                        add_css_class: "boxed-list",
                        set_selection_mode: gtk::SelectionMode::None,
                    },
                },

                // Runtimes section — available to install
                add = &adw::PreferencesGroup {
                    set_title: "Available ProtonGE Releases",

                    add = &gtk::Box {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_spacing: 8,
                        set_margin_top: 4,
                        set_margin_bottom: 4,

                        #[local_ref]
                        fetch_runtimes_btn -> gtk::Button {
                            set_label: "Fetch from GitHub",
                            add_css_class: "flat",
                            #[watch]
                            set_sensitive: !model.installing_runtime,
                            connect_clicked => SettingsMsg::FetchRuntimes,
                        },

                        #[local_ref]
                        runtime_status_label -> gtk::Label {
                            set_halign: gtk::Align::Start,
                            set_hexpand: true,
                            set_ellipsize: gtk::pango::EllipsizeMode::End,
                        },
                    },

                    #[local_ref]
                    available_list -> gtk::ListBox {
                        add_css_class: "boxed-list",
                        set_selection_mode: gtk::SelectionMode::None,
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
        tracker: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let api_key_entry = gtk::Entry::builder()
            .hexpand(true)
            .valign(gtk::Align::Center)
            .visibility(false)
            .placeholder_text("Paste API key here")
            .build();
        let status_label = gtk::Label::new(None);
        let test_button = gtk::Button::new();
        let save_button = gtk::Button::new();
        let login_button = gtk::Button::new();
        let logout_button = gtk::Button::new();

        let installed_list = gtk::ListBox::new();
        let available_list = gtk::ListBox::new();
        let runtime_status_label = gtk::Label::new(None);
        let fetch_runtimes_btn = gtk::Button::new();

        let default_dir = paths::default_downloads_dir().to_string_lossy().to_string();

        let model = SettingsDialog {
            tracker,
            api_key_entry,
            status_label,
            test_button,
            save_button,
            login_button,
            logout_button,
            sso_in_progress: false,
            has_key: false,
            login_source: None,
            downloads_dir: default_dir,
            installed_list,
            available_list,
            runtime_status_label,
            fetch_runtimes_btn,
            installing_runtime: false,
            available_releases: vec![],
        };

        let status_label = &model.status_label;
        let test_button = &model.test_button;
        let save_button = &model.save_button;
        let login_button = &model.login_button;
        let logout_button = &model.logout_button;
        let installed_list = &model.installed_list;
        let available_list = &model.available_list;
        let runtime_status_label = &model.runtime_status_label;
        let fetch_runtimes_btn = &model.fetch_runtimes_btn;
        let widgets = view_output!();

        // API key entry suffix
        widgets.api_key_row.add_suffix(&model.api_key_entry);
        widgets
            .api_key_row
            .set_activatable_widget(Some(&model.api_key_entry));

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

        // Load existing API key + login source together
        let t = model.tracker.clone();
        sender.oneshot_command(async move {
            let key = t.get_setting("nexus_api_key").await.ok().flatten();
            let source = t
                .get_setting("nexus_login_source")
                .await
                .ok()
                .flatten()
                .as_deref()
                .and_then(|s| match s {
                    "sso" => Some(LoginSource::Sso),
                    "manual" => Some(LoginSource::Manual),
                    _ => None,
                });
            SettingsCmdMsg::KeyLoaded(key, source)
        });

        // Load existing downloads dir
        let t2 = model.tracker.clone();
        sender.oneshot_command(async move {
            let dir = t2.get_setting("downloads_dir").await.ok().flatten();
            SettingsCmdMsg::DownloadsDirLoaded(dir)
        });

        // Populate installed runtimes immediately.
        model.rebuild_installed_list(&sender);

        root.present();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            SettingsMsg::TestKey => {
                let key = self.api_key_entry.text().to_string();
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
                let key = self.api_key_entry.text().to_string();
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
                    let _ = tracker
                        .set_setting(
                            "nexus_login_source",
                            if key.is_empty() { "" } else { "manual" },
                        )
                        .await;

                    // Validate and cache premium status
                    if !key.is_empty() {
                        let client = NexusClient::new(key);
                        if let Ok((user, _)) = client.validate_key().await {
                            let _ = tracker
                                .set_setting(
                                    "nexus_is_premium",
                                    if user.is_premium { "true" } else { "false" },
                                )
                                .await;
                        }
                    }

                    SettingsCmdMsg::KeySaved(Ok(()))
                });
            }
            SettingsMsg::NexusLogin => {
                if self.sso_in_progress {
                    return;
                }
                self.sso_in_progress = true;
                self.status_label
                    .set_label("Opening browser for Nexus login…");
                self.status_label.remove_css_class("error");
                self.status_label.remove_css_class("success");
                self.status_label.set_visible(true);

                let tracker = self.tracker.clone();
                sender.oneshot_command(async move {
                    match crate::core::nexus_api::sso_login().await {
                        Ok(api_key) => {
                            let _ = tracker.set_setting("nexus_api_key", &api_key).await;
                            let _ = tracker.set_setting("nexus_login_source", "sso").await;

                            let client = NexusClient::new(api_key.clone());
                            if let Ok((user, _)) = client.validate_key().await {
                                let _ = tracker
                                    .set_setting(
                                        "nexus_is_premium",
                                        if user.is_premium { "true" } else { "false" },
                                    )
                                    .await;
                            }

                            SettingsCmdMsg::SsoResult(Ok(api_key))
                        }
                        Err(e) => SettingsCmdMsg::SsoResult(Err(e.to_string())),
                    }
                });
            }
            SettingsMsg::Logout => {
                let tracker = self.tracker.clone();
                self.logout_button.set_sensitive(false);
                sender.oneshot_command(async move {
                    let r = tracker
                        .set_setting("nexus_api_key", "")
                        .await
                        .and(tracker.set_setting("nexus_login_source", "").await)
                        .and(tracker.set_setting("nexus_is_premium", "false").await)
                        .map_err(|e| e.to_string());
                    SettingsCmdMsg::LoggedOut(r)
                });
            }
            SettingsMsg::Close => {
                let _ = sender.output(SettingsDialogOutput::Closed);
            }
            SettingsMsg::BrowseDownloadsDir => {
                let dialog = gtk::FileDialog::builder()
                    .title("Select Downloads Folder")
                    .modal(true)
                    .build();

                let input_sender = sender.input_sender().clone();
                dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        input_sender
                            .send(SettingsMsg::DownloadsDirChosen(path))
                            .unwrap();
                    }
                });
            }
            SettingsMsg::ManageGames => {
                let _ = sender.output(SettingsDialogOutput::ManageGames);
                root.close();
            }
            SettingsMsg::DownloadsDirChosen(path) => {
                let dir_str = path.to_string_lossy().to_string();
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

            // ── Runtimes ─────────────────────────────────────────────────────
            SettingsMsg::FetchRuntimes => {
                self.runtime_status_label.set_label("Fetching…");
                self.fetch_runtimes_btn.set_sensitive(false);
                sender.oneshot_command(async move {
                    SettingsCmdMsg::RuntimesFetched(
                        proton_manager::list_releases()
                            .await
                            .map_err(|e| e.to_string()),
                    )
                });
            }

            SettingsMsg::InstallRuntime { tag, url } => {
                if self.installing_runtime {
                    return;
                }
                self.installing_runtime = true;
                self.runtime_status_label
                    .set_label(&format!("Downloading {tag}…"));

                let input = sender.input_sender().clone();
                let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<(u64, u64)>(32);

                // Forward progress messages to the UI thread.
                let input_fwd = input.clone();
                relm4::spawn(async move {
                    while let Some((downloaded, total)) = progress_rx.recv().await {
                        let _ = input_fwd.send(SettingsMsg::RuntimeProgress { downloaded, total });
                    }
                });

                let tag_clone = tag.clone();
                sender.oneshot_command(async move {
                    let result = proton_manager::install_release(&tag, &url, progress_tx)
                        .await
                        .map_err(|e| e.to_string());
                    SettingsCmdMsg::RuntimeInstalled {
                        tag: tag_clone,
                        result,
                    }
                });
            }

            SettingsMsg::RemoveRuntime(tag) => {
                let result = proton_manager::remove_release(&tag).map_err(|e| e.to_string());
                // Synchronous removal — no need for async.
                self.rebuild_installed_list(&sender);
                self.rebuild_available_list(&sender);
                if let Err(e) = result {
                    self.runtime_status_label
                        .set_label(&format!("Remove failed: {e}"));
                }
            }

            SettingsMsg::SetActiveRuntime(tag) => {
                match proton_manager::set_active_runtime(&tag) {
                    Ok(()) => {
                        self.runtime_status_label
                            .set_label(&format!("Active runtime: {tag}"));
                    }
                    Err(e) => {
                        self.runtime_status_label
                            .set_label(&format!("Could not activate: {e}"));
                    }
                }
                self.rebuild_installed_list(&sender);
            }

            SettingsMsg::RuntimeProgress { downloaded, total } => {
                let label = if total > 0 {
                    let pct = downloaded * 100 / total;
                    let mb_done = downloaded / 1_048_576;
                    let mb_total = total / 1_048_576;
                    format!("Downloading… {pct}% ({mb_done}/{mb_total} MB)")
                } else {
                    let mb = downloaded / 1_048_576;
                    format!("Downloading… {mb} MB")
                };
                self.runtime_status_label.set_label(&label);
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
                        let key = self.api_key_entry.text().to_string();
                        self.has_key = !key.is_empty();
                        self.login_source = if self.has_key {
                            Some(LoginSource::Manual)
                        } else {
                            None
                        };
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
            SettingsCmdMsg::SsoResult(result) => {
                self.sso_in_progress = false;
                match result {
                    Ok(_api_key) => {
                        self.has_key = true;
                        self.login_source = Some(LoginSource::Sso);
                        self.status_label
                            .set_label("Logged in via Nexus SSO — key saved.");
                        self.status_label.remove_css_class("error");
                        self.status_label.add_css_class("success");
                        let _ = sender.output(SettingsDialogOutput::ApiKeyChanged);
                    }
                    Err(e) => {
                        self.status_label.set_label(&format!("Login failed: {e}"));
                        self.status_label.remove_css_class("success");
                        self.status_label.add_css_class("error");
                    }
                }
                self.status_label.set_visible(true);
            }
            SettingsCmdMsg::LoggedOut(result) => {
                self.logout_button.set_sensitive(true);
                match result {
                    Ok(()) => {
                        self.api_key_entry.set_text("");
                        self.has_key = false;
                        self.login_source = None;
                        self.status_label.set_label("Logged out.");
                        self.status_label.remove_css_class("error");
                        self.status_label.add_css_class("success");
                        let _ = sender.output(SettingsDialogOutput::ApiKeyChanged);
                    }
                    Err(e) => {
                        self.status_label.set_label(&format!("Logout failed: {e}"));
                        self.status_label.remove_css_class("success");
                        self.status_label.add_css_class("error");
                    }
                }
                self.status_label.set_visible(true);
            }
            SettingsCmdMsg::KeyLoaded(key, source) => {
                self.login_source = source;
                if let Some(ref key) = key {
                    self.has_key = !key.is_empty();
                    if self.login_source != Some(LoginSource::Sso) {
                        self.api_key_entry.set_text(key);
                    }
                }
            }
            SettingsCmdMsg::DownloadsDirLoaded(dir) => {
                if let Some(dir) = dir {
                    self.downloads_dir = dir;
                }
            }
            SettingsCmdMsg::DownloadsDirSaved(result) => {
                if let Err(e) = result {
                    eprintln!("deployd: failed to save downloads dir: {e}");
                }
            }

            // ── Runtimes ─────────────────────────────────────────────────────
            SettingsCmdMsg::RuntimesFetched(result) => {
                self.fetch_runtimes_btn.set_sensitive(true);
                match result {
                    Ok(releases) => {
                        let n = releases.len();
                        self.available_releases = releases;
                        self.runtime_status_label
                            .set_label(&format!("{n} releases fetched."));
                        self.rebuild_available_list(&sender);
                        // Refresh installed list in case some are now marked installed.
                        self.rebuild_installed_list(&sender);
                    }
                    Err(e) => {
                        self.runtime_status_label
                            .set_label(&format!("Fetch failed: {e}"));
                    }
                }
            }
            SettingsCmdMsg::RuntimeInstalled { tag, result } => {
                self.installing_runtime = false;
                match result {
                    Ok(()) => {
                        self.runtime_status_label
                            .set_label(&format!("{tag} installed."));
                        // Auto-activate if this is the first runtime.
                        if proton_manager::active_runtime_tag().is_none() {
                            let _ = proton_manager::set_active_runtime(&tag);
                        }
                        self.rebuild_installed_list(&sender);
                        self.rebuild_available_list(&sender);
                    }
                    Err(e) => {
                        self.runtime_status_label
                            .set_label(&format!("Install failed: {e}"));
                    }
                }
            }
        }
    }
}

impl SettingsDialog {
    /// Rebuild the "Installed Runtimes" list from what is currently on disk.
    fn rebuild_installed_list(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.installed_list.first_child() {
            self.installed_list.remove(&child);
        }

        let versions = proton_manager::installed_versions();
        let active = proton_manager::active_runtime_tag();

        if versions.is_empty() {
            let empty_row = adw::ActionRow::new();
            empty_row.set_title("No runtimes installed");
            empty_row
                .set_subtitle("Use \"Fetch from GitHub\" to browse available ProtonGE versions");
            self.installed_list.append(&empty_row);
            return;
        }

        for tag in versions {
            let row = adw::ActionRow::new();
            row.set_title(&tag);

            if active.as_deref() == Some(&tag) {
                row.set_subtitle("Active");
                let check = gtk::Image::from_icon_name("emblem-default-symbolic");
                check.set_valign(gtk::Align::Center);
                row.add_prefix(&check);
            } else {
                let activate_btn = gtk::Button::with_label("Set Active");
                activate_btn.set_valign(gtk::Align::Center);
                activate_btn.add_css_class("flat");
                {
                    let input = sender.input_sender().clone();
                    let t = tag.clone();
                    activate_btn.connect_clicked(move |_| {
                        input.send(SettingsMsg::SetActiveRuntime(t.clone())).ok();
                    });
                }
                row.add_suffix(&activate_btn);
            }

            let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            remove_btn.set_valign(gtk::Align::Center);
            remove_btn.add_css_class("flat");
            remove_btn.set_tooltip_text(Some("Remove"));
            {
                let input = sender.input_sender().clone();
                let t = tag.clone();
                remove_btn.connect_clicked(move |_| {
                    input.send(SettingsMsg::RemoveRuntime(t.clone())).ok();
                });
            }
            row.add_suffix(&remove_btn);

            self.installed_list.append(&row);
        }
    }

    /// Rebuild the "Available ProtonGE Releases" list from the last fetched data.
    fn rebuild_available_list(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.available_list.first_child() {
            self.available_list.remove(&child);
        }

        if self.available_releases.is_empty() {
            return;
        }

        for release in &self.available_releases {
            let row = adw::ActionRow::new();
            row.set_title(&release.tag);

            if release.installed {
                row.set_subtitle("Installed");
                let check = gtk::Image::from_icon_name("emblem-default-symbolic");
                check.set_valign(gtk::Align::Center);
                row.add_prefix(&check);
            } else {
                let install_btn = gtk::Button::with_label("Install");
                install_btn.set_valign(gtk::Align::Center);
                install_btn.add_css_class("suggested-action");
                install_btn.add_css_class("flat");
                {
                    let input = sender.input_sender().clone();
                    let tag = release.tag.clone();
                    let url = release.download_url.clone();
                    install_btn.connect_clicked(move |_| {
                        input
                            .send(SettingsMsg::InstallRuntime {
                                tag: tag.clone(),
                                url: url.clone(),
                            })
                            .ok();
                    });
                }
                row.add_suffix(&install_btn);
            }

            self.available_list.append(&row);
        }
    }
}
