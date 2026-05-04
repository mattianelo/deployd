pub mod backup;
pub mod cache;
pub mod cache_handlers;
pub mod deploy;
pub mod downloads;
pub mod external;
pub mod free_fns;
pub mod helpers;
pub mod init;
pub mod install;
mod launch;
pub mod messages;
pub mod mods;
pub mod notifications;
pub mod order_snapshots;
pub mod plugins;
pub mod profiles;
pub mod types;

pub use self::messages::{AppCmdMsg, AppMsg};

use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use self::types::{DownloadFilter, ModFilter};

mod state;
pub use state::App;

#[relm4::component(pub)]
impl Component for App {
    type Init = Option<String>;
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = AppCmdMsg;

    view! {
        adw::ApplicationWindow {
            set_title: Some("Deployd"),
            set_default_size: (1100, 680),
            connect_close_request[sender] => move |_| {
                sender.input(AppMsg::CloseRequested);
                glib::Propagation::Stop
            },

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                adw::HeaderBar {
                    set_centering_policy: adw::CenteringPolicy::Loose,
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        set_title: "Deployd",
                    },

                    #[local_ref]
                    pack_start = nexus_user_btn -> gtk::MenuButton {
                        add_css_class: "flat",
                        set_always_show_arrow: false,
                        set_tooltip_text: Some("Nexus Mods account"),
                        #[wrap(Some)]
                        set_child = &gtk::Box {
                            #[local_ref]
                            nexus_avatar_widget -> adw::Avatar {
                                set_size: 24,
                                set_show_initials: true,
                                #[watch]
                                set_text: model.nexus_username.as_deref(),
                            },
                        },
                        #[wrap(Some)]
                        set_popover = &gtk::Popover {
                            #[wrap(Some)]
                            set_child = &gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_spacing: 8,
                                set_margin_all: 12,
                                set_width_request: 200,

                                // Logged-in state
                                gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_spacing: 4,
                                    #[watch]
                                    set_visible: model.nexus_username.is_some(),

                                    gtk::Label {
                                        #[watch]
                                        set_label: model.nexus_username.as_deref().unwrap_or(""),
                                        add_css_class: "title-4",
                                        set_halign: gtk::Align::Start,
                                        set_ellipsize: gtk::pango::EllipsizeMode::End,
                                    },

                                    gtk::Label {
                                        #[watch]
                                        set_label: if model.nexus_is_premium { "Premium" } else { "Free" },
                                        add_css_class: "caption",
                                        set_halign: gtk::Align::Start,
                                    },

                                    gtk::Separator {},

                                    gtk::Button {
                                        set_label: "Log Out",
                                        add_css_class: "destructive-action",
                                        connect_clicked[sender] => move |_| {
                                            sender.input(AppMsg::NexusLogoutClicked);
                                        },
                                    },
                                },

                                // Logged-out state
                                gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_spacing: 4,
                                    #[watch]
                                    set_visible: model.nexus_username.is_none(),

                                    gtk::Label {
                                        set_label: "Not connected",
                                        add_css_class: "dim-label",
                                        set_halign: gtk::Align::Start,
                                    },

                                    gtk::Button {
                                        set_label: "Login with Nexus",
                                        add_css_class: "suggested-action",
                                        connect_clicked[sender] => move |_| {
                                            sender.input(AppMsg::NexusLoginClicked);
                                        },
                                    },
                                },
                            },
                        },
                    },

                    #[local_ref]
                    pack_start = game_dropdown -> gtk::DropDown {
                        set_selected: 0,
                        #[watch]
                        set_visible: model.has_games() && !model.initializing,
                        connect_selected_notify[sender] => move |dd| {
                            sender.input(AppMsg::GameSelected(dd.selected()));
                        }
                    },

                    pack_start = &gtk::Button {
                        set_icon_name: "window-close-symbolic",
                        set_tooltip_text: Some("Stop managing this game"),
                        add_css_class: "flat",
                        #[watch]
                        set_visible: model.has_games() && !model.initializing,
                        connect_clicked[sender] => move |_| {
                            sender.input(AppMsg::RemoveCurrentGame);
                        },
                    },

                    #[local_ref]
                    pack_start = profile_dropdown -> gtk::DropDown {
                        set_tooltip_text: Some("Active profile"),
                        #[watch]
                        set_visible: model.has_games() && !model.initializing,
                        connect_selected_notify[sender] => move |dd| {
                            sender.input(AppMsg::ProfileSelected(dd.selected()));
                        }
                    },

                    // Profile management MenuButton — icon-only, opens action popover
                    #[local_ref]
                    pack_start = profile_menu_btn -> gtk::MenuButton {
                        set_icon_name: "view-more-symbolic",
                        set_tooltip_text: Some("Profile options"),
                        #[watch]
                        set_visible: model.has_games() && !model.initializing,
                        add_css_class: "flat",
                        #[wrap(Some)]
                        set_popover = &gtk::Popover {
                            #[wrap(Some)]
                            set_child = &gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_spacing: 4,
                                set_margin_all: 8,

                                gtk::Box {
                                    set_orientation: gtk::Orientation::Horizontal,
                                    set_spacing: 4,
                                    set_halign: gtk::Align::Center,

                                    gtk::Button {
                                        set_icon_name: "list-add-symbolic",
                                        set_tooltip_text: Some("New empty profile (all mods disabled)"),
                                        add_css_class: "flat",
                                        connect_clicked => AppMsg::NewProfileClicked,
                                    },

                                    gtk::Button {
                                        set_icon_name: "edit-copy-symbolic",
                                        set_tooltip_text: Some("Clone current profile"),
                                        add_css_class: "flat",
                                        connect_clicked => AppMsg::CloneProfileClicked,
                                    },

                                    #[local_ref]
                                    profile_rename_btn -> gtk::MenuButton {
                                        set_icon_name: "document-edit-symbolic",
                                        set_tooltip_text: Some("Rename profile"),
                                        add_css_class: "flat",
                                        #[watch]
                                        set_sensitive: model.profiles.len() > 1,
                                    },

                                    gtk::Button {
                                        set_icon_name: "user-trash-symbolic",
                                        set_tooltip_text: Some("Delete profile"),
                                        add_css_class: "flat",
                                        #[watch]
                                        set_sensitive: model.profiles.len() > 1,
                                        connect_clicked => AppMsg::DeleteProfileClicked,
                                    },

                                    gtk::Button {
                                        set_icon_name: "document-save-symbolic",
                                        set_tooltip_text: Some("Export active profile to file"),
                                        add_css_class: "flat",
                                        set_visible: cfg!(feature = "experimental"),
                                        connect_clicked => AppMsg::ExportProfileClicked,
                                    },

                                    gtk::Button {
                                        set_icon_name: "document-open-symbolic",
                                        set_tooltip_text: Some("Import profile from file"),
                                        add_css_class: "flat",
                                        set_visible: cfg!(feature = "experimental"),
                                        connect_clicked => AppMsg::ImportProfileClicked,
                                    },
                                },

                                #[local_ref]
                                save_mode_btn -> gtk::Button {
                                    #[watch]
                                    set_label: model.save_mode_label().as_str(),
                                    #[watch]
                                    set_visible: model.game_has_save_management(),
                                    set_tooltip_text: Some("Toggle per-profile save file isolation"),
                                    add_css_class: "flat",
                                    connect_clicked => AppMsg::ToggleProfileSaveMode,
                                },

                                #[local_ref]
                                sync_saves_btn -> gtk::Button {
                                    set_icon_name: "view-refresh-symbolic",
                                    #[watch]
                                    set_visible: model.can_sync_saves(),
                                    #[watch]
                                    set_sensitive: !model.is_busy(),
                                    set_tooltip_text: Some("Sync saves: update profile snapshot from game save directory"),
                                    add_css_class: "flat",
                                    connect_clicked => AppMsg::SyncSaves,
                                },
                            }
                        },
                    },

                    pack_start = &gtk::Box {
                        set_orientation: gtk::Orientation::Horizontal,
                        #[watch]
                        set_visible: model.has_games() && !model.initializing && model.is_busy(),

                        gtk::Box {
                            set_orientation: gtk::Orientation::Horizontal,
                            set_spacing: 6,
                            set_margin_start: 8,
                            set_margin_end: 8,
                            set_valign: gtk::Align::Center,

                            gtk::Spinner {
                                set_spinning: true,
                            },
                            gtk::Label {
                                add_css_class: "caption",
                                #[watch]
                                set_label: model.status_msg.as_deref().unwrap_or("Extracting..."),
                            },
                        },
                    },

                    pack_end = &gtk::Button {
                        set_label: "Launch",
                        set_icon_name: "media-playback-start-symbolic",
                        set_tooltip_text: Some("Launch game via script extender"),
                        #[watch]
                        set_visible: cfg!(feature = "experimental") && model.script_extender_present(),
                        #[watch]
                        set_sensitive: !model.is_busy(),
                        connect_clicked => AppMsg::LaunchGameClicked,
                    },

                    pack_end = &gtk::Box {
                        add_css_class: "linked",

                        gtk::Button {
                            #[watch]
                            set_label: if model.deploying { "Deploying\u{2026}" } else { "Deploy" },
                            #[watch]
                            set_css_classes: if model.needs_deploy {
                                &["suggested-action"]
                            } else {
                                &[]
                            },
                            set_tooltip_text: Some("Deploy mods to game folder"),
                            #[watch]
                            set_sensitive: !model.is_busy() && model.has_games(),
                            connect_clicked => AppMsg::DeployClicked,
                        },

                        #[local_ref]
                        deploy_options_btn -> gtk::MenuButton {
                            set_icon_name: "pan-down-symbolic",
                            set_tooltip_text: Some("Deploy options"),
                            #[watch]
                            set_css_classes: if model.needs_deploy {
                                &["suggested-action"]
                            } else {
                                &[]
                            },
                            #[watch]
                            set_sensitive: !model.is_busy() && model.has_games(),
                            #[wrap(Some)]
                            set_popover = &gtk::Popover {
                                #[wrap(Some)]
                                set_child = &gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_spacing: 2,
                                    set_margin_all: 4,

                                    gtk::Button {
                                        set_icon_name: "folder-open-symbolic",
                                        set_label: "Open deployment folder",
                                        add_css_class: "flat",
                                        connect_clicked => AppMsg::OpenDeploymentFolder,
                                    },

                                    gtk::Separator {},

                                    gtk::Button {
                                        set_label: "Purge deployment",
                                        add_css_class: "flat",
                                        connect_clicked => AppMsg::PurgeClicked,
                                    },
                                },
                            },
                        },
                    },

                    // Overflow menu — secondary/infrequent actions
                    #[local_ref]
                    pack_end = overflow_menu_btn -> gtk::MenuButton {
                        set_icon_name: "view-more-symbolic",
                        set_tooltip_text: Some("More actions"),
                        add_css_class: "flat",
                        #[wrap(Some)]
                        set_popover = &gtk::Popover {
                            #[wrap(Some)]
                            set_child = &gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_spacing: 2,
                                set_margin_all: 4,

                                gtk::Button {
                                    set_icon_name: "folder-new-symbolic",
                                    set_label: "Create Empty Mod",
                                    set_tooltip_text: Some("Create Empty Mod — opens cache folder in file manager"),
                                    add_css_class: "flat",
                                    #[watch]
                                    set_sensitive: !model.is_busy() && model.has_games(),
                                    connect_clicked => AppMsg::CreateEmptyMod,
                                },

                                gtk::Button {
                                    set_icon_name: "view-refresh-symbolic",
                                    set_label: "Reset Vanilla Baseline",
                                    set_tooltip_text: Some("Re-snapshot the current game folder as the new vanilla state — use after a clean game reinstall"),
                                    add_css_class: "flat",
                                    #[watch]
                                    set_sensitive: !model.is_busy() && model.has_games(),
                                    connect_clicked => AppMsg::ResetVanillaBaseline,
                                },

                                gtk::Button {
                                    set_icon_name: "applications-engineering-symbolic",
                                    set_label: "Manage Tools",
                                    set_tooltip_text: Some("Add and configure external modding tools"),
                                    add_css_class: "flat",
                                    #[watch]
                                    set_sensitive: !model.is_busy() && model.has_games(),
                                    connect_clicked => AppMsg::ManageToolsClicked,
                                },

                                gtk::Button {
                                    set_icon_name: "emblem-system-symbolic",
                                    set_label: "Settings",
                                    add_css_class: "flat",
                                    connect_clicked => AppMsg::SettingsClicked,
                                },
                            },
                        },
                    },

                    #[local_ref]
                    pack_end = notifications_menu_btn -> gtk::MenuButton {
                        set_tooltip_text: Some("Notifications"),
                        set_always_show_arrow: false,
                        #[watch]
                        set_css_classes: if model.notifications_count() > 0 {
                            &["flat", "notification-active"]
                        } else {
                            &["flat"]
                        },
                        #[wrap(Some)]
                        set_child = &gtk::Box {
                            set_spacing: 4,
                            gtk::Image {
                                // Always a bell; filled when there are notifications
                                set_icon_name: Some("notification-symbolic"),
                            },
                            gtk::Label {
                                #[watch]
                                set_label: &model.notifications_badge(),
                                #[watch]
                                set_visible: model.notifications_count() > 0,
                                add_css_class: "notification-badge",
                            },
                        },
                        #[wrap(Some)]
                        set_popover = &gtk::Popover {
                            set_width_request: 320,
                            #[wrap(Some)]
                            set_child = &gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_margin_all: 8,
                                set_spacing: 8,

                                adw::StatusPage {
                                    #[watch]
                                    set_visible: model.notifications_count() == 0,
                                    set_icon_name: Some("emblem-ok-symbolic"),
                                    set_title: "All Caught Up",
                                },

                                gtk::ScrolledWindow {
                                    #[watch]
                                    set_visible: model.external_changes_count > 0
                                        || model.app_update_version.is_some(),
                                    set_propagate_natural_height: true,
                                    set_max_content_height: 400,
                                    set_hscrollbar_policy: gtk::PolicyType::Never,

                                    gtk::ListBox {
                                        set_selection_mode: gtk::SelectionMode::None,
                                        add_css_class: "boxed-list",

                                        adw::ActionRow {
                                            #[watch]
                                            set_visible: model.external_changes_count > 0,
                                            set_title: "External Changes",
                                            #[watch]
                                            set_subtitle: &format!(
                                                "{} file{} detected outside mod manager",
                                                model.external_changes_count,
                                                if model.external_changes_count == 1 { "" } else { "s" }
                                            ),
                                            add_prefix = &gtk::Image {
                                                set_icon_name: Some("dialog-warning-symbolic"),
                                                set_valign: gtk::Align::Center,
                                            },
                                            add_suffix = &gtk::Button {
                                                set_label: "Review",
                                                set_valign: gtk::Align::Center,
                                                add_css_class: "suggested-action",
                                                add_css_class: "pill",
                                                connect_clicked[sender] => move |_| {
                                                    sender.input(AppMsg::AbsorbExternalFiles);
                                                },
                                            },
                                        },

                                        adw::ActionRow {
                                            #[watch]
                                            set_visible: model.app_update_version.is_some(),
                                            set_title: "App Update Available",
                                            #[watch]
                                            set_subtitle: model.app_update_version.as_deref().unwrap_or(""),
                                            add_prefix = &gtk::Image {
                                                set_icon_name: Some("software-update-available-symbolic"),
                                                set_valign: gtk::Align::Center,
                                            },
                                            add_suffix = &gtk::Button {
                                                set_label: if model.running_as_appimage { "Download" } else { "View" },
                                                set_valign: gtk::Align::Center,
                                                add_css_class: "suggested-action",
                                                add_css_class: "pill",
                                                connect_clicked[sender] => move |_| {
                                                    sender.input(AppMsg::SelfUpdateDownload);
                                                },
                                            },
                                        },
                                    },
                                },

                                gtk::ScrolledWindow {
                                    #[watch]
                                    set_visible: model.notification_count > 0,
                                    set_propagate_natural_height: true,
                                    set_max_content_height: 300,
                                    set_hscrollbar_policy: gtk::PolicyType::Never,

                                    #[local_ref]
                                    notification_list -> gtk::ListBox {
                                        set_selection_mode: gtk::SelectionMode::None,
                                        add_css_class: "boxed-list",
                                    },
                                },

                            },
                        },
                    },

                    pack_end = &gtk::ToggleButton {
                        #[watch]
                        set_icon_name: if model.global_active_downloads > 0 {
                            "content-loading-symbolic"
                        } else {
                            "folder-download-symbolic"
                        },
                        set_tooltip_text: Some("Downloads"),
                        #[watch]
                        set_active: model.downloads_visible,
                        connect_toggled[sender] => move |btn| {
                            sender.input(AppMsg::SetDownloadsVisible(btn.is_active()));
                        },
                    },

                    pack_end = &gtk::ToggleButton {
                        set_icon_name: "edit-find-symbolic",
                        set_tooltip_text: Some("Search mods (Ctrl+F)"),
                        #[watch]
                        set_active: model.search_active,
                        connect_toggled[sender] => move |btn| {
                            sender.input(AppMsg::SearchToggled(btn.is_active()));
                        },
                    },

                    #[local_ref]
                    pack_end = tool_buttons_box -> gtk::Box {},
                },

                #[local_ref]
                search_bar -> gtk::SearchBar {
                    #[watch]
                    set_search_mode: model.search_active,
                },

                gtk::Box {
                    #[watch]
                    set_visible: model.initializing,
                    set_vexpand: true,
                    set_orientation: gtk::Orientation::Vertical,
                    set_valign: gtk::Align::Center,
                    set_halign: gtk::Align::Center,
                    set_spacing: 12,

                    gtk::Spinner {
                        set_spinning: true,
                        set_size_request: (32, 32),
                    },
                    gtk::Label {
                        set_label: "Loading...",
                        add_css_class: "title-3",
                    },
                },

                adw::OverlaySplitView {
                    set_vexpand: true,
                    #[watch]
                    set_visible: !model.initializing,
                    #[watch]
                    set_show_sidebar: model.downloads_visible,
                    set_sidebar_position: gtk::PackType::End,
                    set_max_sidebar_width: 400.0,
                    set_min_sidebar_width: 300.0,

                    #[wrap(Some)]
                    set_sidebar = &adw::ToolbarView {
                        set_width_request: 300,

                        add_top_bar = &adw::HeaderBar {
                            set_centering_policy: adw::CenteringPolicy::Loose,
                            set_show_back_button: false,
                            set_decoration_layout: Some(""),

                            #[wrap(Some)]
                            set_title_widget = &adw::WindowTitle {
                                set_title: "Downloads",
                            },

                            pack_start = &gtk::DropDown {
                                set_model: Some(&gtk::StringList::new(&["Default", "Name", "Status"])),
                                set_valign: gtk::Align::Center,
                                set_tooltip_text: Some("Sort downloads"),
                                #[watch]
                                set_selected: model.download_sort as u32,
                                connect_selected_notify[sender] => move |dd| {
                                    sender.input(AppMsg::DownloadSortChanged(dd.selected()));
                                },
                            },

                            pack_end = &gtk::Button {
                                set_icon_name: "go-next-rtl-symbolic",
                                set_tooltip_text: Some("Hide downloads panel"),
                                add_css_class: "flat",
                                connect_clicked[sender] => move |_| {
                                    sender.input(AppMsg::SetDownloadsVisible(false));
                                },
                            },

                            pack_end = &gtk::Button {
                                set_icon_name: "folder-open-symbolic",
                                set_tooltip_text: Some("Scan downloads folder"),
                                add_css_class: "flat",
                                connect_clicked => AppMsg::ScanDownloadsFolder,
                            },
                        },

                        #[wrap(Some)]
                        set_content = &gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_margin_start: 8,
                                set_margin_end: 8,
                                set_margin_top: 4,
                                set_margin_bottom: 4,
                                set_spacing: 4,

                                gtk::Button {
                                    #[watch]
                                    set_css_classes: if matches!(model.download_filter, DownloadFilter::All) {
                                        &["pill", "filter-chip", "suggested-action"]
                                    } else {
                                        &["pill", "filter-chip"]
                                    },
                                    set_label: "All",
                                    connect_clicked => AppMsg::SetDownloadFilter(DownloadFilter::All),
                                },

                                gtk::Button {
                                    #[watch]
                                    set_css_classes: if matches!(model.download_filter, DownloadFilter::Active) {
                                        &["pill", "filter-chip", "suggested-action"]
                                    } else {
                                        &["pill", "filter-chip"]
                                    },
                                    #[watch]
                                    set_label: &format!("Active ({})", model.active_downloads_count()),
                                    connect_clicked => AppMsg::SetDownloadFilter(DownloadFilter::Active),
                                },

                                gtk::Button {
                                    #[watch]
                                    set_css_classes: if matches!(model.download_filter, DownloadFilter::Completed) {
                                        &["pill", "filter-chip", "suggested-action"]
                                    } else {
                                        &["pill", "filter-chip"]
                                    },
                                    #[watch]
                                    set_label: &format!("Completed ({})", model.completed_downloads_count()),
                                    connect_clicked => AppMsg::SetDownloadFilter(DownloadFilter::Completed),
                                },
                            },

                            #[local_ref]
                            downloads_scroll -> gtk::ScrolledWindow {
                                set_vexpand: true,
                                set_hscrollbar_policy: gtk::PolicyType::Automatic,

                                #[local_ref]
                                download_list -> gtk::ListBox {
                                    set_selection_mode: gtk::SelectionMode::None,
                                    add_css_class: "boxed-list",
                                    set_margin_all: 8,
                                }
                            },

                            adw::StatusPage {
                                #[watch]
                                set_visible: model.downloads.is_empty(),
                                set_icon_name: Some("folder-download-symbolic"),
                                set_title: "No Downloads",
                                set_description: Some("Click Scan or download from Nexus Mods"),
                            },
                        },
                    },

                    #[wrap(Some)]
                    set_content = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,

                        #[local_ref]
                        toast_overlay -> adw::ToastOverlay {
                            set_vexpand: true,

                            gtk::Paned {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_position: 450,
                        set_shrink_start_child: false,
                        set_shrink_end_child: false,

                        // LEFT PANEL: Mod Order
                        #[wrap(Some)]
                        set_start_child = &gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_margin_top: 8,
                                set_margin_bottom: 4,
                                set_margin_start: 8,
                                set_margin_end: 8,

                                gtk::Label {
                                    set_label: "Mod Order",
                                    add_css_class: "heading",
                                    set_hexpand: true,
                                    set_halign: gtk::Align::Start,
                                },

                                gtk::Box {
                                    add_css_class: "linked",

                                    gtk::Button {
                                        set_label: "All",
                                        set_tooltip_text: Some("Enable all mods"),
                                        connect_clicked => AppMsg::EnableAllMods,
                                    },

                                    gtk::Button {
                                        set_label: "None",
                                        set_tooltip_text: Some("Disable all mods"),
                                        connect_clicked => AppMsg::DisableAllMods,
                                    },
                                },

                                // Mod order snapshots — save and load in one icon button
                                gtk::MenuButton {
                                    set_icon_name: "media-floppy-symbolic",
                                    set_tooltip_text: Some("Mod order snapshots"),
                                    add_css_class: "flat",
                                    set_margin_start: 4,
                                    #[wrap(Some)]
                                    set_popover = &gtk::Popover {
                                        #[wrap(Some)]
                                        set_child = &gtk::Box {
                                            set_orientation: gtk::Orientation::Vertical,
                                            set_spacing: 6,
                                            set_margin_all: 8,

                                            gtk::Label {
                                                set_label: "Save snapshot",
                                                set_halign: gtk::Align::Start,
                                                add_css_class: "caption",
                                                add_css_class: "dim-label",
                                            },

                                            #[local_ref]
                                            mod_snapshot_save_entry -> gtk::Entry {
                                                set_placeholder_text: Some("e.g. Pre-DLC run"),
                                            },

                                            gtk::Button {
                                                set_label: "Save",
                                                add_css_class: "suggested-action",
                                                connect_clicked[sender, mod_snapshot_save_entry] => move |btn| {
                                                    let name = mod_snapshot_save_entry.text().to_string();
                                                    if !name.is_empty() {
                                                        sender.input(AppMsg::SaveModOrderSnapshot(name));
                                                        mod_snapshot_save_entry.set_text("");
                                                        if let Some(w) = btn.ancestor(gtk::Popover::static_type()) {
                                                            w.downcast_ref::<gtk::Popover>().unwrap().popdown();
                                                        }
                                                    }
                                                },
                                            },

                                            gtk::Separator {},

                                            gtk::Label {
                                                set_label: "Restore snapshot",
                                                set_halign: gtk::Align::Start,
                                                add_css_class: "caption",
                                                add_css_class: "dim-label",
                                            },

                                            gtk::ScrolledWindow {
                                                set_min_content_height: 40,
                                                set_max_content_height: 200,
                                                set_hscrollbar_policy: gtk::PolicyType::Never,

                                                #[local_ref]
                                                mod_snapshots_list -> gtk::ListBox {
                                                    add_css_class: "boxed-list",
                                                    set_selection_mode: gtk::SelectionMode::None,
                                                },
                                            },
                                        },
                                    },
                                },

                                // Add Mod — moved here from headerbar
                                gtk::Button {
                                    set_icon_name: "list-add-symbolic",
                                    set_tooltip_text: Some("Add mod from file"),
                                    add_css_class: "flat",
                                    #[watch]
                                    set_sensitive: !model.is_busy(),
                                    #[watch]
                                    set_visible: !model.is_busy(),
                                    connect_clicked => AppMsg::InstallClicked,
                                },

                                gtk::Button {
                                    set_icon_name: "folder-new-symbolic",
                                    set_tooltip_text: Some("Create mod group"),
                                    add_css_class: "flat",
                                    #[watch]
                                    set_sensitive: !model.is_busy(),
                                    #[watch]
                                    set_visible: !model.is_busy(),
                                    connect_clicked[sender] => move |_| {
                                        sender.input(AppMsg::CreateGroup("New Group".to_string()));
                                    },
                                },
                            },

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_margin_start: 8,
                                set_margin_end: 8,
                                set_margin_bottom: 4,
                                set_spacing: 4,

                                gtk::Button {
                                    #[watch]
                                    set_css_classes: if matches!(model.mod_filter, ModFilter::All) {
                                        &["pill", "filter-chip", "suggested-action"]
                                    } else {
                                        &["pill", "filter-chip"]
                                    },
                                    #[watch]
                                    set_label: &format!("All ({})", model.total_mods_count()),
                                    connect_clicked => AppMsg::SetModFilter(ModFilter::All),
                                },

                                gtk::Button {
                                    #[watch]
                                    set_css_classes: if matches!(model.mod_filter, ModFilter::Enabled) {
                                        &["pill", "filter-chip", "suggested-action"]
                                    } else {
                                        &["pill", "filter-chip"]
                                    },
                                    #[watch]
                                    set_label: &format!("Enabled ({})", model.enabled_mods_count()),
                                    connect_clicked => AppMsg::SetModFilter(ModFilter::Enabled),
                                },

                                gtk::Button {
                                    #[watch]
                                    set_css_classes: if matches!(model.mod_filter, ModFilter::Issues) {
                                        &["pill", "filter-chip", "suggested-action"]
                                    } else {
                                        &["pill", "filter-chip"]
                                    },
                                    #[watch]
                                    set_label: &format!("Issues ({})", model.issues_mods_count()),
                                    connect_clicked => AppMsg::SetModFilter(ModFilter::Issues),
                                },
                            },

                            #[local_ref]
                            mod_scroll -> gtk::ScrolledWindow {
                                set_vexpand: true,
                                set_hscrollbar_policy: gtk::PolicyType::Never,

                                #[local_ref]
                                mod_list -> gtk::ListBox {
                                    set_selection_mode: gtk::SelectionMode::None,
                                    add_css_class: "boxed-list",
                                    set_margin_all: 8,
                                }
                            },

                            adw::StatusPage {
                                #[watch]
                                set_visible: model.has_no_mods(),
                                set_icon_name: Some("package-x-generic-symbolic"),
                                #[watch]
                                set_title: if model.has_games() { "No Mods" } else { "No Games Detected" },
                                #[watch]
                                set_description: Some(
                                    if model.has_games() {
                                        "Add mods, then Deploy to install"
                                    } else {
                                        "Install a supported game via Heroic Launcher"
                                    }
                                ),
                            },
                        },

                        // RIGHT PANEL: Plugin Load Order
                        #[wrap(Some)]
                        set_end_child = &gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_margin_top: 8,
                                set_margin_bottom: 4,

                                gtk::Label {
                                    set_label: "Plugin Order",
                                    add_css_class: "heading",
                                    set_hexpand: true,
                                    set_halign: gtk::Align::Start,
                                    set_margin_start: 8,
                                },

                                gtk::Box {
                                    add_css_class: "linked",
                                    set_valign: gtk::Align::Center,

                                    gtk::Button {
                                        set_label: "All",
                                        set_tooltip_text: Some("Enable all plugins"),
                                        connect_clicked => AppMsg::EnableAllPlugins,
                                    },

                                    gtk::Button {
                                        set_label: "None",
                                        set_tooltip_text: Some("Disable all plugins"),
                                        connect_clicked => AppMsg::DisableAllPlugins,
                                    },
                                },

                                // Plugin order snapshots — save and load in one icon button
                                gtk::MenuButton {
                                    set_icon_name: "media-floppy-symbolic",
                                    set_tooltip_text: Some("Plugin order snapshots"),
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    set_margin_start: 4,
                                    #[wrap(Some)]
                                    set_popover = &gtk::Popover {
                                        #[wrap(Some)]
                                        set_child = &gtk::Box {
                                            set_orientation: gtk::Orientation::Vertical,
                                            set_spacing: 6,
                                            set_margin_all: 8,

                                            gtk::Label {
                                                set_label: "Save snapshot",
                                                set_halign: gtk::Align::Start,
                                                add_css_class: "caption",
                                                add_css_class: "dim-label",
                                            },

                                            #[local_ref]
                                            plugin_snapshot_save_entry -> gtk::Entry {
                                                set_placeholder_text: Some("e.g. Vanilla load order"),
                                            },

                                            gtk::Button {
                                                set_label: "Save",
                                                add_css_class: "suggested-action",
                                                connect_clicked[sender, plugin_snapshot_save_entry] => move |btn| {
                                                    let name = plugin_snapshot_save_entry.text().to_string();
                                                    if !name.is_empty() {
                                                        sender.input(AppMsg::SavePluginOrderSnapshot(name));
                                                        plugin_snapshot_save_entry.set_text("");
                                                        if let Some(w) = btn.ancestor(gtk::Popover::static_type()) {
                                                            w.downcast_ref::<gtk::Popover>().unwrap().popdown();
                                                        }
                                                    }
                                                },
                                            },

                                            gtk::Separator {},

                                            gtk::Label {
                                                set_label: "Restore snapshot",
                                                set_halign: gtk::Align::Start,
                                                add_css_class: "caption",
                                                add_css_class: "dim-label",
                                            },

                                            gtk::ScrolledWindow {
                                                set_min_content_height: 40,
                                                set_max_content_height: 200,
                                                set_hscrollbar_policy: gtk::PolicyType::Never,

                                                #[local_ref]
                                                plugin_snapshots_list -> gtk::ListBox {
                                                    add_css_class: "boxed-list",
                                                    set_selection_mode: gtk::SelectionMode::None,
                                                },
                                            },
                                        },
                                    },
                                },

                                gtk::ToggleButton {
                                    set_icon_name: "view-reveal-symbolic",
                                    set_tooltip_text: Some("Show vanilla / DLC plugins"),
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    #[watch]
                                    set_active: model.show_vanilla_plugins,
                                    connect_clicked => AppMsg::ToggleShowVanillaPlugins,
                                },

                                gtk::Button {
                                    set_icon_name: "view-sort-ascending-symbolic",
                                    set_tooltip_text: Some("Sort with LOOT"),
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    set_margin_end: 4,
                                    connect_clicked => AppMsg::SortWithLoot,
                                },
                            },

                            gtk::ScrolledWindow {
                                set_vexpand: true,
                                set_hscrollbar_policy: gtk::PolicyType::Never,

                                #[local_ref]
                                plugin_list -> gtk::ListBox {
                                    set_selection_mode: gtk::SelectionMode::None,
                                    add_css_class: "boxed-list",
                                    set_margin_all: 8,
                                }
                            },

                            adw::StatusPage {
                                #[watch]
                                set_visible: model.managed_plugins_count == 0,
                                set_icon_name: Some("application-x-addon-symbolic"),
                                set_title: "No Plugins",
                                set_description: Some("Plugin files (.esp/.esm/.esl) will appear here"),
                            },
                        },
                    }
                    }
                    }
                },

                gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_margin_start: 10,
                    set_margin_end: 10,
                    set_margin_top: 3,
                    set_margin_bottom: 3,
                    set_spacing: 8,
                    #[watch]
                    set_visible: !model.initializing,

                    gtk::Label {
                        #[watch]
                        set_label: &model.mod_status_label(),
                        add_css_class: "caption",
                        add_css_class: "dim-label",
                    },

                    gtk::Label {
                        set_label: "\u{00b7}",
                        add_css_class: "caption",
                        add_css_class: "dim-label",
                    },

                    gtk::Label {
                        #[watch]
                        set_label: &model.plugin_status_label(),
                        add_css_class: "caption",
                        add_css_class: "dim-label",
                    },

                    gtk::Box { set_hexpand: true },

                    gtk::Label {
                        #[watch]
                        set_label: &model.rate_limit_label(),
                        #[watch]
                        set_visible: model.rate_limit_info.is_some(),
                        add_css_class: "caption",
                        #[watch]
                        set_css_classes: &model.rate_limit_css(),
                    },

                    gtk::Label {
                        #[watch]
                        set_label: if model.needs_deploy {
                            "\u{25cf} Unsaved changes"
                        } else {
                            "\u{2713} Synced"
                        },
                        #[watch]
                        set_css_classes: if model.needs_deploy {
                            &["caption", "warning"]
                        } else {
                            &["caption", "dim-label"]
                        },
                        #[watch]
                        set_visible: model.has_games(),
                    },
                },
            }
        }
    }

    fn init(
        nxm_link: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let (model, _game_ids, _games_for_init, profile_rename_btn, search_bar) =
            init::build_model(nxm_link, &sender);

        let toast_overlay = &model.toast_overlay;
        let notification_list = &model.notification_list;
        let mod_list = model.mods.widget();
        let plugin_list = model.plugins.widget();
        let download_list = model.downloads.widget();
        let profile_dropdown = &model.profile_dropdown;
        let game_dropdown = &model.game_dropdown;
        let tool_buttons_box = &model.tool_buttons_box;
        let mod_scroll = &model.mod_scroll;
        let downloads_scroll = &model.downloads_scroll;
        let deploy_options_btn = &model.deploy_options_btn;
        let notifications_menu_btn = &model.notifications_menu_btn;
        let overflow_menu_btn = &model.overflow_menu_btn;
        let profile_menu_btn = &model.profile_menu_btn;
        let save_mode_btn = &model.save_mode_btn;
        let sync_saves_btn = &model.sync_saves_btn;
        let mod_snapshot_save_entry = &model.mod_snapshot_save_entry;
        let plugin_snapshot_save_entry = &model.plugin_snapshot_save_entry;
        let mod_snapshots_list = &model.mod_snapshots_list;
        let plugin_snapshots_list = &model.plugin_snapshots_list;
        let nexus_user_btn = &model.nexus_user_btn;
        let nexus_avatar_widget = &model.nexus_avatar_widget;

        let widgets = view_output!();

        // NOTE: do NOT set_key_capture_widget(root) here — that routes every window
        // keystroke into the search entry, causing the bar to flicker open/closed
        // whenever the user types while some other widget has focus.

        init::wire_drag_drop(&sender, mod_list, plugin_list, &model.mod_scroll);

        sender.oneshot_command(async move { init::load_init_data().await });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
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
            AppMsg::RemoveMod(idx) => self.handle_remove_mod(idx, &sender),
            AppMsg::ReinstallMod(idx) => self.handle_reinstall_mod(idx, &sender),
            AppMsg::ToggleModEnabled(idx, enabled) => {
                self.handle_toggle_mod_enabled(idx, enabled, &sender)
            }
            AppMsg::MoveModTo(from, to) => self.handle_move_mod_to(from, to, &sender),
            AppMsg::MoveGroupTo(from, to) => self.handle_move_group_to(from, to, &sender),
            AppMsg::MoveSelectedModsTo { selected, from, to } => {
                self.handle_move_selected_mods_to(selected, from, to, &sender)
            }
            AppMsg::MovePluginTo(from, to) => self.handle_move_plugin_to(from, to, &sender),
            AppMsg::MoveSelectedPluginsTo { selected, from, to } => {
                self.handle_move_selected_plugins_to(selected, from, to, &sender)
            }
            AppMsg::TogglePluginEnabled(idx, enabled) => {
                self.handle_toggle_plugin_enabled(idx, enabled, &sender)
            }
            AppMsg::RenameMod(idx, name) => self.handle_rename_mod(idx, name, &sender),
            AppMsg::ProfileSelected(idx) => self.handle_profile_selected(idx, &sender),
            AppMsg::NewProfileClicked => {
                self.profile_menu_btn.popdown();
                self.handle_new_profile_clicked(&sender);
            }
            AppMsg::CloneProfileClicked => {
                self.profile_menu_btn.popdown();
                self.handle_clone_profile_clicked(&sender);
            }
            AppMsg::RenameProfile(name) => self.handle_rename_profile(name, &sender),
            AppMsg::DeleteProfileClicked => {
                self.profile_menu_btn.popdown();
                self.handle_delete_profile_clicked(&sender);
            }
            AppMsg::DeployClicked => self.handle_deploy_clicked(root, &sender),
            AppMsg::DeployConfirmed => self.execute_deploy(&sender),
            AppMsg::PurgeClicked => self.handle_purge_clicked(root, &sender),
            AppMsg::PurgeConfirmed => self.handle_purge_confirmed(&sender),
            AppMsg::GrantGameFolderAccess => self.handle_grant_game_folder_access(root, &sender),
            AppMsg::GameFolderGranted(path) => self.handle_game_folder_granted(path, &sender),
            AppMsg::LaunchTool(name) => self.handle_launch_tool(name, &sender),
            AppMsg::ToolExited(name, error) => self.handle_tool_exited(name, error, &sender),
            AppMsg::LaunchGameClicked => self.handle_launch_game_clicked(&sender),
            AppMsg::GameExited(error) => self.handle_game_exited(error),
            AppMsg::ConfirmProtonSetup(tool_id) => {
                self.handle_confirm_proton_setup(tool_id, root, &sender)
            }
            AppMsg::ProtonSetupConfirmed(tool_id) => {
                self.handle_proton_setup_confirmed(tool_id, &sender)
            }
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
            AppMsg::WelcomeWizardSkipped => {
                self.welcome_wizard = None;
            }
            AppMsg::RemoveGame(id) => self.confirm_remove_game(id, root, &sender),
            AppMsg::RemoveCurrentGame => {
                if let Some(game) = self.games.get(self.selected_game_idx) {
                    let id = game.id.clone();
                    self.confirm_remove_game(id, root, &sender);
                }
            }
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
            AppMsg::NexusApiKeyUpdated => self.handle_nexus_api_key_updated(&sender),
            AppMsg::NxmLinkReceived(link) => self.handle_nxm_link_received(link, &sender),
            AppMsg::CheckUpdatesClicked => self.handle_check_updates(&sender),
            AppMsg::ToggleDownloads => self.handle_toggle_downloads(),
            AppMsg::SetDownloadsVisible(v) => self.handle_set_downloads_visible(v),
            AppMsg::InstallDownload(idx) => self.handle_install_download(idx, &sender),
            AppMsg::ReinstallDownload(idx) => self.handle_reinstall_download(idx, &sender),
            AppMsg::ClearDownloadMetadata(idx) => self.handle_clear_download_metadata(idx, &sender),
            AppMsg::RenameDownload(idx) => self.handle_rename_download(idx, root, &sender),
            AppMsg::ConfirmDownloadRename(id, name) => {
                self.handle_confirm_download_rename(id, name, &sender)
            }
            AppMsg::ConfirmNexusIdEntry(dl_id, mod_id, domain) => {
                self.handle_confirm_nexus_id_entry(dl_id, mod_id, domain, &sender)
            }
            AppMsg::FileIdDialogConfirmed { download_id, file_id, mod_id, domain } => {
                self.handle_file_id_dialog_confirmed(
                    download_id, file_id, mod_id, domain, &sender,
                )
            }
            AppMsg::ShowFileIdDialog { download_id, mod_id, domain, partial_name } => {
                if let Some(name) = partial_name {
                    self.pending_fetched_name = Some(name);
                }
                self.pending_file_id_needed = Some(crate::app::types::FileIdNeeded {
                    download_id,
                    mod_id,
                    domain,
                });
                self.show_file_id_dialog(root, &sender);
            }
            AppMsg::DownloadProgress(id, frac, msg) => self.handle_download_progress(id, frac, msg),
            AppMsg::DownloadNameResolved(id, name, domain, fname, is_primary, file_id, version) => {
                self.handle_download_name_resolved(
                    id, name, domain, fname, is_primary, file_id, version, &sender,
                )
            }
            AppMsg::ArchiveMd5Computed(dl_id, md5) => {
                if let Some(entry) = self.all_downloads.iter_mut().find(|e| e.id == dl_id) {
                    entry.archive_md5 = Some(md5);
                }
                if let Some(tracker) = self.tracker.clone()
                    && let Some(entry) = self.all_downloads.iter().find(|e| e.id == dl_id).cloned()
                {
                    sender.oneshot_command(async move {
                        let _ = tracker.save_download_entry(&entry).await;
                        AppCmdMsg::PrioritySaved(Ok(()))
                    });
                }
            }
            AppMsg::FetchDownloadMetadata(idx) => {
                self.handle_fetch_download_metadata(idx, root, &sender)
            }
            AppMsg::ScanDownloadsFolder => self.handle_scan_downloads_folder(&sender),
            AppMsg::DownloadSortChanged(idx) => self.handle_download_sort_changed(idx),
            AppMsg::SearchToggled(active) => self.handle_search_toggled(active),
            AppMsg::SearchChanged(text) => self.handle_search_changed(text),
            AppMsg::SearchScopeChanged(idx) => self.handle_search_scope_changed(idx),
            AppMsg::RateLimitUpdated(info) => self.handle_rate_limit_updated(info),
            AppMsg::CloseRequested => self.handle_close_requested(root, &sender),
            AppMsg::ConfirmClose => self.handle_confirm_close(root),
            AppMsg::ToggleGroupCollapse(idx) => self.handle_toggle_group_collapse(idx),
            AppMsg::DeleteGroup(idx) => self.handle_delete_group(idx, &sender),
            AppMsg::CreateGroup(name) => self.handle_create_group(name, &sender),
            AppMsg::RenameGroup(idx, name) => self.handle_rename_group(idx, name),
            AppMsg::OpenModProperties(idx) => self.handle_open_mod_properties(idx, root, &sender),
            AppMsg::ModPropertiesApplied {
                mod_id,
                mod_idx,
                name,
                notes,
                install_target,
                file_targets,
            } => self.handle_mod_properties_applied(
                mod_id,
                mod_idx,
                name,
                notes,
                install_target,
                file_targets,
            ),
            AppMsg::ModPropertiesCancelled => self.handle_mod_properties_cancelled(),
            AppMsg::ScanExternalFiles => self.handle_scan_external_files(&sender),
            AppMsg::AbsorbExternalFiles => {
                self.notifications_menu_btn.popdown();
                self.handle_absorb_external_files(root, &sender);
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
            AppMsg::ExportProfileClicked => {
                self.profile_menu_btn.popdown();
                self.handle_export_profile_clicked(root, &sender);
            }
            AppMsg::ImportProfileClicked => {
                self.profile_menu_btn.popdown();
                self.handle_import_profile_clicked(root, &sender);
            }
            AppMsg::ImportProfileFileChosen(path) => {
                self.handle_import_profile_file_chosen(path, &sender)
            }
            AppMsg::ProfileExported(result) => self.handle_profile_exported(result),
            AppMsg::OpenPreInstallDialog => self.handle_open_pre_install_dialog(root, &sender),
            AppMsg::OpenPreInstallDialogReplacing(id, priority) => {
                if self.pending_install.as_ref().is_some_and(|p| p.fomod_config.is_some()) {
                    let old_name = self.mod_name_for_id(&id);
                    if let Some(pending) = &mut self.pending_install {
                        pending.mod_name = old_name;
                    }
                    self.pending_replace_mod_id = Some((id.clone(), priority));
                    self.pending_fetched_name = None;
                    self.pending_file_id_needed = None;
                    let tracker = self.tracker.clone();
                    sender.oneshot_command(async move {
                        let selections = if let Some(t) = tracker {
                            t.get_fomod_selections(&id)
                                .await
                                .ok()
                                .flatten()
                                .and_then(|json| {
                                    let raw: Option<Vec<Vec<Vec<usize>>>> =
                                        serde_json::from_str(&json).ok();
                                    raw.map(|steps| {
                                        steps
                                            .into_iter()
                                            .map(|step| {
                                                step.into_iter()
                                                    .map(|g| {
                                                        g.into_iter()
                                                            .collect::<std::collections::HashSet<usize>>()
                                                    })
                                                    .collect::<Vec<_>>()
                                            })
                                            .collect::<Vec<_>>()
                                    })
                                })
                        } else {
                            None
                        };
                        AppCmdMsg::FomodSelectionsLoaded(selections)
                    });
                } else {
                    self.handle_open_pre_install_dialog_replacing(id, priority, root, &sender)
                }
            }
            AppMsg::SortWithLoot => self.handle_sort_with_loot(&sender),
            AppMsg::EnableAllMods => self.handle_enable_all_mods(&sender),
            AppMsg::DisableAllMods => self.handle_disable_all_mods(&sender),
            AppMsg::EnableAllPlugins => self.handle_enable_all_plugins(&sender),
            AppMsg::DisableAllPlugins => self.handle_disable_all_plugins(&sender),
            AppMsg::ToggleShowVanillaPlugins => self.handle_toggle_show_vanilla_plugins(),
            AppMsg::ShowToast(msg) => self.handle_show_toast(msg),
            AppMsg::NotificationDismissed => {
                self.notification_count = self.notification_count.saturating_sub(1);
            }
            AppMsg::ClearNotifications => {
                while let Some(child) = self.notification_list.first_child() {
                    self.notification_list.remove(&child);
                }
                self.notification_count = 0;
            }
            AppMsg::ToggleProfileSaveMode => {
                self.profile_menu_btn.popdown();
                self.handle_toggle_profile_save_mode(&sender);
            }
            AppMsg::SyncSaves => {
                self.profile_menu_btn.popdown();
                self.handle_sync_saves(&sender);
            }
            AppMsg::AppUpdateAvailable(version, url) => {
                self.app_update_version = Some(format!("Deployd {version} is available"));
                self.app_update_url = Some(url);
            }
            AppMsg::OpenUpdatePage => {
                let url = self
                    .app_update_url
                    .as_deref()
                    .unwrap_or(crate::core::update_check::NEXUS_PAGE_URL);
                let _ = open::that(url);
            }
            AppMsg::SelfUpdateDownload => {
                self.notifications_menu_btn.popdown();
                self.handle_self_update_download(&sender);
            }
            AppMsg::SaveModOrderSnapshot(name) => {
                self.handle_save_mod_order_snapshot(name, &sender)
            }
            AppMsg::SavePluginOrderSnapshot(name) => {
                self.handle_save_plugin_order_snapshot(name, &sender)
            }
            AppMsg::LoadModOrderSnapshot(id) => self.handle_load_mod_order_snapshot(id, &sender),
            AppMsg::LoadPluginOrderSnapshot(id) => {
                self.handle_load_plugin_order_snapshot(id, &sender)
            }
            AppMsg::DeleteModOrderSnapshot(id) => self.handle_delete_order_snapshot(id, &sender),
            AppMsg::DeletePluginOrderSnapshot(id) => self.handle_delete_order_snapshot(id, &sender),
            AppMsg::SetModFilter(filter) => {
                self.mod_filter = filter;
                self.apply_search_filter();
            }
            AppMsg::SetDownloadFilter(filter) => {
                self.download_filter = filter;
                self.apply_search_filter();
            }
            AppMsg::OpenDeploymentFolder => {
                self.deploy_options_btn.popdown();
                if let Some(game) = self.selected_game() {
                    let uri = format!("file://{}", game.path.display());
                    let _ = gtk::gio::AppInfo::launch_default_for_uri(
                        &uri,
                        None::<&gtk::gio::AppLaunchContext>,
                    );
                }
            }
            AppMsg::PauseDownload(idx) => self.handle_pause_download(idx),
            AppMsg::ResumeDownload(idx) => self.handle_resume_download(idx, &sender),
            AppMsg::SetCompactPluginRows(compact) => self.handle_set_compact_plugin_rows(compact),
            AppMsg::SetCompactModRows(compact) => self.handle_set_compact_mod_rows(compact),
            AppMsg::SetColorScheme(idx) => self.handle_set_color_scheme(idx),
            AppMsg::NexusLoginClicked => self.handle_nexus_login_clicked(&sender),
            AppMsg::NexusLogoutClicked => self.handle_nexus_logout_clicked(&sender),
            AppMsg::CreateFullBackupClicked => {
                self.handle_create_full_backup_clicked(root, &sender)
            }
            AppMsg::RestoreFromBackupClicked => {
                self.handle_restore_from_backup_clicked(root, &sender)
            }
            AppMsg::RestoreBackupFileChosen(path) => {
                self.handle_restore_backup_file_chosen(path, root, &sender)
            }
            AppMsg::StageFullRestore(path) => self.handle_stage_full_restore(path, &sender),
            AppMsg::ImportProfilesFromBackup(path) => {
                self.handle_import_profiles_from_backup(path, &sender)
            }
            AppMsg::FullBackupCreated(result) => self.handle_full_backup_created(result),
        }
    }

    fn update_cmd(
        &mut self,
        msg: Self::CommandOutput,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        match msg {
            AppCmdMsg::Initialized(result) => self.handle_cmd_initialized(result, &sender),
            AppCmdMsg::PendingMetadataFetched(name) => {
                self.pending_fetched_name = Some(name);
            }
            AppCmdMsg::PendingFileNameUnresolved { partial_name, download_id, mod_id, domain } => {
                self.pending_fetched_name = Some(partial_name);
                self.pending_file_id_needed =
                    Some(crate::app::types::FileIdNeeded { download_id, mod_id, domain });
            }
            AppCmdMsg::FileIdFetched { combined_name, download_id, version, file_id } => {
                self.pending_file_id_needed = None;
                if let Some(dl_id) = download_id {
                    // Standalone (right-click) path: update the download entry directly.
                    if let Some(name) = combined_name {
                        self.handle_download_name_resolved(
                            dl_id.clone(), name, None, None, false, file_id, version, &sender,
                        );
                    }
                    self.show_toast("Metadata updated");
                    // Auto-trigger the next unresolved download for the same mod so the
                    // user doesn't have to manually right-click each one.
                    let resolved_mod_id = self
                        .all_downloads
                        .iter()
                        .find(|e| e.id == dl_id)
                        .and_then(|e| e.nexus_ids.as_ref())
                        .map(|ids| ids.mod_id);
                    if let Some(mod_id) = resolved_mod_id {
                        let sibling = self
                            .all_downloads
                            .iter()
                            .find(|e| {
                                e.id != dl_id
                                    && e.nexus_ids.as_ref().map(|ids| ids.mod_id) == Some(mod_id)
                                    && e.nexus_ids
                                        .as_ref()
                                        .is_some_and(|ids| ids.file_id == 0)
                                    && !e.metadata_fetched
                            })
                            .map(|e| e.id.clone());
                        if let Some(next_id) = sibling {
                            self.start_nexus_metadata_fetch(next_id, &sender);
                        }
                    }
                } else {
                    // Install path: hand off to the pre-install dialog flow.
                    if let Some(name) = combined_name {
                        self.pending_fetched_name = Some(name);
                    }
                    let _ = sender.input_sender().send(AppMsg::OpenPreInstallDialog);
                }
            }
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
            AppCmdMsg::PrioritySaved(result) => self.handle_cmd_priority_saved(result, &sender),
            AppCmdMsg::OverridesRefreshed(result) => {
                self.handle_cmd_overrides_refreshed(result, &sender)
            }
            AppCmdMsg::PluginOrderSaved(result) => self.handle_cmd_plugin_order_saved(result),
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
            AppCmdMsg::GameLaunched(result) => self.handle_cmd_game_launched(result),
            AppCmdMsg::ModMerged(result) => self.handle_cmd_mod_merged(result, &sender),
            AppCmdMsg::NxmDownloadComplete(id, result) => {
                self.handle_cmd_nxm_download_complete(id, result, &sender)
            }
            AppCmdMsg::NexusMetadataFetched(dl_id, result) => {
                self.handle_cmd_nexus_metadata_fetched(dl_id, result, &sender)
            }
            AppCmdMsg::UpdatesChecked(result) => self.handle_cmd_updates_checked(result, &sender),
            AppCmdMsg::DownloadsDirUpdated(dir) => self.handle_cmd_downloads_dir_updated(dir),
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
            AppCmdMsg::ProfileImported(result) => self.handle_cmd_profile_imported(result, &sender),
            AppCmdMsg::EmptyModCreated(result) => {
                self.handle_cmd_empty_mod_created(result, &sender)
            }
            AppCmdMsg::ModFilesRescanned(result) => {
                self.handle_cmd_mod_files_rescanned(result, &sender)
            }
            #[cfg(feature = "loot")]
            AppCmdMsg::LootSortDone(result) => self.handle_cmd_loot_sort_done(result, &sender),
            AppCmdMsg::ModFilesLoaded(files) => self.handle_cmd_mod_files_loaded(files),
            AppCmdMsg::SaveModeToggled(result) => {
                self.handle_cmd_save_mode_toggled(result, &sender)
            }
            AppCmdMsg::SavesSynced(result) => self.handle_cmd_saves_synced(result),
            AppCmdMsg::LastDeployedProfileLoaded(id) => self.last_deployed_profile_id = id,
            AppCmdMsg::FullRestoreStaged(result) => {
                self.handle_cmd_full_restore_staged(result, root)
            }
            AppCmdMsg::ProfilesImportedFromBackup(result) => {
                self.handle_cmd_profiles_imported_from_backup(result, &sender);
            }
            AppCmdMsg::AppUpdateResult(result) => self.handle_cmd_app_update_result(result),
            AppCmdMsg::ProtonDownloaded { result, tool_id } => {
                self.handle_proton_downloaded(result, tool_id, &sender)
            }
            AppCmdMsg::FomodSelectionsLoaded(selections) => {
                self.pending_fomod_selections = selections;
                self.open_pre_install_dialog(root, &sender);
            }
            AppCmdMsg::GamesPersisted => {
                // Reset the selection sentinel so handle_game_selected's same-index
                // guard does not skip the reload when the index was already 0.
                self.selected_game_idx = usize::MAX;
                sender.input(AppMsg::GameSelected(0));
            }
            AppCmdMsg::OrderSnapshotsLoaded(mod_snaps, plugin_snaps) => {
                self.mod_order_snapshots = mod_snaps;
                self.plugin_order_snapshots = plugin_snaps;
                self.rebuild_snapshot_lists(&sender);
            }
            AppCmdMsg::ModOrderSnapshotSaved(result) => {
                if let Err(e) = result {
                    self.push_notification(&format!("Failed to save snapshot: {e}"));
                } else {
                    self.show_toast("Mod order snapshot saved");
                    self.reload_order_snapshots(&sender);
                }
            }
            AppCmdMsg::PluginOrderSnapshotSaved(result) => {
                if let Err(e) = result {
                    self.push_notification(&format!("Failed to save snapshot: {e}"));
                } else {
                    self.show_toast("Plugin order snapshot saved");
                    self.reload_order_snapshots(&sender);
                }
            }
            AppCmdMsg::ModOrderSnapshotRestored(result) => match result {
                Ok(data) => {
                    self.apply_loaded_data(data, &sender);
                    self.show_toast("Mod order restored");
                }
                Err(e) => self.push_notification(&format!("Failed to restore snapshot: {e}")),
            },
            AppCmdMsg::PluginOrderSnapshotRestored(result) => match result {
                Ok(data) => {
                    self.apply_loaded_data(data, &sender);
                    self.show_toast("Plugin order restored");
                }
                Err(e) => self.push_notification(&format!("Failed to restore snapshot: {e}")),
            },
            AppCmdMsg::OrderSnapshotDeleted(result) => {
                if let Err(e) = result {
                    self.push_notification(&format!("Failed to delete snapshot: {e}"));
                }
                self.reload_order_snapshots(&sender);
            }
            AppCmdMsg::NexusAvatarLoaded(bytes) => {
                crate::dlog!("[avatar] NexusAvatarLoaded: {:?}", bytes.as_ref().map(|b| b.len()));
                if let Some(bytes) = bytes {
                    match gtk::gdk::Texture::from_bytes(&gtk::glib::Bytes::from_owned(bytes)) {
                        Ok(texture) => {
                            crate::dlog!("[avatar] texture created, setting custom image");
                            self.nexus_avatar_widget.set_custom_image(Some(&texture));
                        }
                        Err(e) => {
                            crate::dlog!("[avatar] Texture::from_bytes failed: {e}");
                        }
                    }
                }
            }
            AppCmdMsg::NexusUserRefreshed(username, avatar_url, is_premium) => {
                crate::dlog!(
                    "[avatar] NexusUserRefreshed: username={:?} avatar_url={:?}",
                    username,
                    avatar_url,
                );
                self.nexus_username = username.clone();
                self.nexus_avatar_url = avatar_url.clone();
                self.nexus_is_premium = is_premium;
                self.nexus_avatar_widget.set_text(username.as_deref());
                self.nexus_avatar_widget.set_custom_image(None::<&gtk::gdk::Texture>);
                if let Some(url) = avatar_url {
                    sender.oneshot_command(async move {
                        AppCmdMsg::NexusAvatarLoaded(
                            crate::app::free_fns::fetch_avatar_bytes(&url).await,
                        )
                    });
                } else {
                    crate::dlog!("[avatar] NexusUserRefreshed: no avatar URL, showing initials");
                }
            }
        }
    }
}
