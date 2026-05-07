use std::path::PathBuf;

use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::nexus_api::NexusClient;
use crate::core::tracker::Tracker;
use crate::utils::paths;

pub struct SettingsDialog {
    tracker: Tracker,
    is_logged_in: bool,
    api_key_row: adw::PasswordEntryRow,
    status_label: gtk::Label,
    test_button: gtk::Button,
    save_button: gtk::Button,
    downloads_dir: String,
}

#[derive(Debug)]
pub enum SettingsMsg {
    TestKey,
    Save,
    Close,
    BrowseDownloadsDir,
    DownloadsDirChosen(PathBuf),
    ManageGames,
    SetColorScheme(u32),
    ToggleCompactPlugins(bool),
    ToggleCompactMods(bool),
}

#[derive(Debug)]
pub enum SettingsCmdMsg {
    KeyValidated(Result<String, String>),
    KeySaved(Result<(), String>),
    KeyLoaded(Option<String>),
    DownloadsDirLoaded(Option<String>),
    DownloadsDirSaved(Result<(), String>),
}

#[derive(Debug)]
pub enum SettingsDialogOutput {
    Closed,
    /// Emitted whenever the active API key changes (manual save).
    ApiKeyChanged,
    /// User wants to open the game setup dialog.
    ManageGames,
    /// User toggled compact plugin row mode.
    SetCompactPluginRows(bool),
    /// User toggled compact mod row mode.
    SetCompactModRows(bool),
    /// User changed the color scheme (0=System, 1=Light, 2=Dark).
    ColorSchemeChanged(u32),
}

#[relm4::component(pub)]
impl Component for SettingsDialog {
    /// (tracker, is_logged_in, compact_plugin_rows, compact_mod_rows, color_scheme_idx)
    type Init = (Tracker, bool, bool, bool, u32);
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

                    #[name = "compact_switch_row"]
                    add = &adw::SwitchRow {
                        set_title: "Compact Plugin List",
                        set_subtitle: "Reduce row height in the Plugin Order panel",
                        connect_active_notify[sender] => move |row| {
                            sender.input(SettingsMsg::ToggleCompactPlugins(row.is_active()));
                        },
                    },

                    #[name = "compact_mod_switch_row"]
                    add = &adw::SwitchRow {
                        set_title: "Compact Mod List",
                        set_subtitle: "Reduce row height in the Mod Order panel",
                        connect_active_notify[sender] => move |row| {
                            sender.input(SettingsMsg::ToggleCompactMods(row.is_active()));
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
        (tracker, is_logged_in, compact_plugin_rows, compact_mod_rows, color_scheme_idx): Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let api_key_row = adw::PasswordEntryRow::new();
        let status_label = gtk::Label::new(None);
        let test_button = gtk::Button::new();
        let save_button = gtk::Button::new();

        let default_dir = paths::default_downloads_dir().to_string_lossy().to_string();

        let model = SettingsDialog {
            tracker,
            is_logged_in,
            api_key_row,
            status_label,
            test_button,
            save_button,
            downloads_dir: default_dir,
        };

        let api_key_row = &model.api_key_row;
        let status_label = &model.status_label;
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
            let key = t.get_setting("nexus_api_key").await.ok().flatten();
            let source = t.get_setting("nexus_login_source").await.ok().flatten();
            // Don't populate the entry for SSO keys — they were obtained via the headerbar flow
            if source.as_deref() == Some("sso") {
                SettingsCmdMsg::KeyLoaded(None)
            } else {
                SettingsCmdMsg::KeyLoaded(key)
            }
        });

        // Load existing downloads dir
        let t2 = model.tracker.clone();
        sender.oneshot_command(async move {
            let dir = t2.get_setting("downloads_dir").await.ok().flatten();
            SettingsCmdMsg::DownloadsDirLoaded(dir)
        });

        // Initialise appearance controls with persisted values.
        widgets.color_scheme_combo.set_selected(color_scheme_idx);
        widgets.compact_switch_row.set_active(compact_plugin_rows);
        widgets.compact_mod_switch_row.set_active(compact_mod_rows);

        root.present();

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
                    let _ = tracker
                        .set_setting(
                            "nexus_login_source",
                            if key.is_empty() { "" } else { "manual" },
                        )
                        .await;

                    // Validate and cache premium status + user info
                    if !key.is_empty() {
                        let client = NexusClient::new(key);
                        if let Ok((user, _)) = client.validate_key().await {
                            let _ = tracker
                                .set_setting(
                                    "nexus_is_premium",
                                    if user.is_premium { "true" } else { "false" },
                                )
                                .await;
                            let _ = tracker.save_nexus_user(&user).await;
                        }
                    }

                    SettingsCmdMsg::KeySaved(Ok(()))
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
                dialog.select_folder(Some(root), None::<&gtk::gio::Cancellable>, move |result| {
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
            SettingsMsg::SetColorScheme(idx) => {
                let _ = sender.output(SettingsDialogOutput::ColorSchemeChanged(idx));
            }
            SettingsMsg::ToggleCompactPlugins(compact) => {
                let _ = sender.output(SettingsDialogOutput::SetCompactPluginRows(compact));
            }
            SettingsMsg::ToggleCompactMods(compact) => {
                let _ = sender.output(SettingsDialogOutput::SetCompactModRows(compact));
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
            SettingsCmdMsg::KeyLoaded(key) => {
                if let Some(ref key) = key
                    && !key.is_empty()
                {
                    self.api_key_row.set_text(key);
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
        }
    }
}
