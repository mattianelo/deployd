pub mod cache;
pub mod cache_handlers;
pub mod deploy;
mod dispatch;
pub mod downloads;
pub mod external;
pub mod free_fns;
pub mod helpers;
pub mod init;
pub mod install;
mod install_file_id;
pub mod messages;
pub mod mods;
pub mod notifications;
pub mod order_snapshots;
pub mod plugins;
pub mod profiles;
pub mod progress;
pub mod timing;
pub mod types;

pub use self::messages::{AppCmdMsg, AppMsg};

use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use self::types::{DownloadFilter, DownloadSort, ModFilter};

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

            adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {
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
                        add_css_class: "flat",
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
                        add_css_class: "flat",
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
                                    },

                                    gtk::Button {
                                        set_icon_name: "user-trash-symbolic",
                                        set_tooltip_text: Some("Delete profile"),
                                        add_css_class: "flat",
                                        #[watch]
                                        set_sensitive: model.profiles.len() > 1,
                                        connect_clicked => AppMsg::DeleteProfileClicked,
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
                                set_label: &model.busy_message(),
                            },
                        },
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

                                gtk::Box {
                                    #[watch]
                                    set_visible: model.notification_count > 0,
                                    set_orientation: gtk::Orientation::Horizontal,
                                    set_halign: gtk::Align::End,

                                    gtk::Button {
                                        set_label: "Clear All",
                                        add_css_class: "flat",
                                        set_tooltip_text: Some("Dismiss all notifications"),
                                        connect_clicked[sender] => move |_| {
                                            sender.input(AppMsg::ClearNotifications);
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

                #[wrap(Some)]
                set_content = &gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,

                #[local_ref]
                search_bar -> gtk::SearchBar {
                    #[watch]
                    set_search_mode: model.search_active,
                },

                adw::Clamp {
                    #[watch]
                    set_visible: model.initializing,
                    set_vexpand: true,
                    set_valign: gtk::Align::Center,
                    set_halign: gtk::Align::Center,

                    gtk::Box {
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
                    }
                },

                adw::OverlaySplitView {
                    set_vexpand: true,
                    #[watch]
                    set_visible: !model.initializing,
                    #[watch]
                    set_show_sidebar: model.downloads_visible,
                    set_sidebar_position: gtk::PackType::End,
                    set_max_sidebar_width: 700.0,
                    set_min_sidebar_width: 250.0,
                    set_collapsed: false,

                    #[wrap(Some)]
                    set_sidebar = &adw::ToolbarView {
                        add_css_class: "plain-panel-bg",

                        add_top_bar = &adw::HeaderBar {
                            set_centering_policy: adw::CenteringPolicy::Loose,
                            set_show_back_button: false,
                            set_decoration_layout: Some(""),

                            #[wrap(Some)]
                            set_title_widget = &gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_spacing: 4,
                                set_halign: gtk::Align::Center,

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

                            pack_start = &gtk::Label {
                                set_label: "Downloads",
                                add_css_class: "heading",
                                set_valign: gtk::Align::Center,
                                set_margin_start: 4,
                            },

                            pack_end = &gtk::MenuButton {
                                set_icon_name: "view-sort-ascending-symbolic",
                                set_tooltip_text: Some("Sort downloads"),
                                add_css_class: "flat",
                                #[wrap(Some)]
                                set_popover = &gtk::Popover {
                                    #[wrap(Some)]
                                    set_child = &gtk::Box {
                                        set_orientation: gtk::Orientation::Vertical,
                                        set_spacing: 6,
                                        set_margin_all: 8,

                                        gtk::Button {
                                            set_label: "Default order",
                                            add_css_class: "flat",
                                            #[watch]
                                            set_sensitive: !matches!(
                                                model.download_sort,
                                                DownloadSort::Default,
                                            ),
                                            connect_clicked[sender] => move |button| {
                                                sender.input(AppMsg::DownloadSortChanged(0));
                                                if let Some(popover) = button
                                                    .ancestor(gtk::Popover::static_type())
                                                    .and_downcast::<gtk::Popover>()
                                                {
                                                    popover.popdown();
                                                }
                                            },
                                        },

                                        gtk::Button {
                                            set_label: "Name",
                                            add_css_class: "flat",
                                            #[watch]
                                            set_sensitive: !matches!(
                                                model.download_sort,
                                                DownloadSort::Name,
                                            ),
                                            connect_clicked[sender] => move |button| {
                                                sender.input(AppMsg::DownloadSortChanged(1));
                                                if let Some(popover) = button
                                                    .ancestor(gtk::Popover::static_type())
                                                    .and_downcast::<gtk::Popover>()
                                                {
                                                    popover.popdown();
                                                }
                                            },
                                        },

                                        gtk::Button {
                                            set_label: "Status",
                                            add_css_class: "flat",
                                            #[watch]
                                            set_sensitive: !matches!(
                                                model.download_sort,
                                                DownloadSort::Status,
                                            ),
                                            connect_clicked[sender] => move |button| {
                                                sender.input(AppMsg::DownloadSortChanged(2));
                                                if let Some(popover) = button
                                                    .ancestor(gtk::Popover::static_type())
                                                    .and_downcast::<gtk::Popover>()
                                                {
                                                    popover.popdown();
                                                }
                                            },
                                        },
                                    },
                                },
                            },

                            pack_end = &gtk::Button {
                                set_icon_name: "folder-open-symbolic",
                                set_tooltip_text: Some("Scan downloads folder"),
                                add_css_class: "flat",
                                connect_clicked => AppMsg::ScanDownloadsFolder,
                            },

                            pack_end = &gtk::ToggleButton {
                                #[watch]
                                set_icon_name: if model.show_hidden_downloads {
                                    "view-conceal-symbolic"
                                } else {
                                    "view-reveal-symbolic"
                                },
                                #[watch]
                                set_tooltip_text: Some(if model.show_hidden_downloads {
                                    "Hide hidden downloads"
                                } else {
                                    "Show hidden downloads"
                                }),
                                add_css_class: "flat",
                                #[watch]
                                set_active: model.show_hidden_downloads,
                                connect_toggled[sender] => move |btn| {
                                    sender.input(AppMsg::SetShowHiddenDownloads(btn.is_active()));
                                },
                            },
                        },

                        #[wrap(Some)]
                        set_content = &adw::Clamp {
                            set_maximum_size: 700,

                            gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                add_css_class: "plain-panel-bg",

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
                    },

                    #[wrap(Some)]
                    set_content = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,

                        #[local_ref]
                        toast_overlay -> adw::ToastOverlay {
                            set_vexpand: true,

                            adw::NavigationSplitView {
                        set_collapsed: false,
                        set_min_sidebar_width: 320.0,
                        set_max_sidebar_width: 1000.0,
                        set_sidebar_width_fraction: 0.5,

                        // LEFT PANEL: Mod Order
                        #[wrap(Some)]
                        set_sidebar = &adw::NavigationPage {
                            set_title: "Mod Order",

                            #[wrap(Some)]
                            set_child = &gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            add_css_class: "plain-panel-bg",

                            // Normal mode header
                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                add_css_class: "headerbar",
                                set_margin_top: 8,
                                set_margin_bottom: 4,
                                #[watch]
                                set_visible: !model.mod_selection_active,

                                gtk::Label {
                                    set_label: "Mod Order",
                                    add_css_class: "heading",
                                    set_halign: gtk::Align::Start,
                                    set_margin_start: 8,
                                },

                                gtk::Box {
                                    set_orientation: gtk::Orientation::Horizontal,
                                    set_spacing: 4,
                                    set_hexpand: true,
                                    set_halign: gtk::Align::Center,
                                    set_valign: gtk::Align::Center,

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
                                        set_label: &format!("Conflicts ({})", model.issues_mods_count()),
                                        connect_clicked => AppMsg::SetModFilter(ModFilter::Issues),
                                    },
                                },

                                gtk::Box {
                                    set_orientation: gtk::Orientation::Horizontal,
                                    set_spacing: 0,
                                    set_halign: gtk::Align::End,

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
                                        set_icon_name: "selection-mode-symbolic",
                                        set_tooltip_text: Some("Select mods"),
                                        add_css_class: "flat",
                                        connect_clicked => AppMsg::EnterModSelectionMode,
                                    },

                                    gtk::MenuButton {
                                        set_icon_name: "view-more-symbolic",
                                        set_tooltip_text: Some("More mod order actions"),
                                        add_css_class: "flat",
                                        set_margin_start: 4,
                                        #[wrap(Some)]
                                        set_popover = &gtk::Popover {
                                            #[wrap(Some)]
                                            set_child = &gtk::Box {
                                                set_orientation: gtk::Orientation::Vertical,
                                                set_spacing: 6,
                                                set_margin_all: 8,

                                            gtk::Button {
                                                set_icon_name: "checkbox-checked-symbolic",
                                                set_label: "Enable all mods",
                                                add_css_class: "flat",
                                                connect_clicked[sender] => move |btn| {
                                                    sender.input(AppMsg::EnableAllMods);
                                                    if let Some(popover) = btn
                                                        .ancestor(gtk::Popover::static_type())
                                                        .and_downcast::<gtk::Popover>()
                                                    {
                                                        popover.popdown();
                                                    }
                                                },
                                            },

                                            gtk::Button {
                                                set_icon_name: "checkbox-symbolic",
                                                set_label: "Disable all mods",
                                                add_css_class: "flat",
                                                connect_clicked[sender] => move |btn| {
                                                    sender.input(AppMsg::DisableAllMods);
                                                    if let Some(popover) = btn
                                                        .ancestor(gtk::Popover::static_type())
                                                        .and_downcast::<gtk::Popover>()
                                                    {
                                                        popover.popdown();
                                                    }
                                                },
                                            },

                                            gtk::Button {
                                                set_icon_name: "folder-new-symbolic",
                                                set_label: "Create mod group",
                                                add_css_class: "flat",
                                                #[watch]
                                                set_sensitive: !model.is_busy(),
                                                #[watch]
                                                set_visible: !model.is_busy(),
                                                connect_clicked[sender] => move |btn| {
                                                    sender.input(AppMsg::CreateGroup("New Group".to_string()));
                                                    if let Some(popover) = btn
                                                        .ancestor(gtk::Popover::static_type())
                                                        .and_downcast::<gtk::Popover>()
                                                    {
                                                        popover.popdown();
                                                    }
                                                },
                                            },

                                            gtk::Separator {},

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
                                                        if let Some(popover) = btn
                                                            .ancestor(gtk::Popover::static_type())
                                                            .and_downcast::<gtk::Popover>()
                                                        {
                                                            popover.popdown();
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
                                },
                            },

                            // Selection mode header
                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_margin_top: 8,
                                set_margin_bottom: 4,
                                set_margin_start: 8,
                                set_margin_end: 8,
                                #[watch]
                                set_visible: model.mod_selection_active,

                                gtk::Label {
                                    #[watch]
                                    set_label: &format!("{} selected", model.selected_mods.len()),
                                    set_hexpand: true,
                                    set_halign: gtk::Align::Start,
                                    add_css_class: "heading",
                                },

                                gtk::Button {
                                    #[watch]
                                    set_label: if model.mod_selection_dirty { "Done" } else { "Cancel" },
                                    connect_clicked => AppMsg::ExitModSelectionMode,
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
                                set_visible: model.has_no_mods() && matches!(model.mod_filter, ModFilter::All),
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
                            adw::StatusPage {
                                #[watch]
                                set_visible: matches!(model.mod_filter, ModFilter::Enabled)
                                    && model.enabled_mods_count() == 0
                                    && model.total_mods_count() > 0,
                                set_icon_name: Some("checkbox-checked-symbolic"),
                                set_title: "No Enabled Mods",
                                set_description: Some("Select mods and use the action bar to enable them."),
                            },
                            adw::StatusPage {
                                #[watch]
                                set_visible: matches!(model.mod_filter, ModFilter::Issues)
                                    && model.issues_mods_count() == 0
                                    && model.total_mods_count() > 0,
                                set_icon_name: Some("emblem-ok-symbolic"),
                                set_title: "No Conflicts",
                                set_description: Some("No mods override each other's files."),
                            },

                            gtk::ActionBar {
                                #[watch]
                                set_revealed: model.mod_selection_active,

                                pack_start = &gtk::Button {
                                    set_label: "Enable",
                                    connect_clicked => AppMsg::EnableSelectedMods,
                                },

                                pack_start = &gtk::Button {
                                    set_label: "Disable",
                                    connect_clicked => AppMsg::DisableSelectedMods,
                                },

                                pack_end = &gtk::Button {
                                    set_label: "Remove",
                                    add_css_class: "destructive-action",
                                    connect_clicked => AppMsg::RemoveSelectedMods,
                                },
                            },
                            },
                        },

                        // RIGHT PANEL: Plugin Load Order
                        #[wrap(Some)]
                        set_content = &adw::NavigationPage {
                            set_title: "Plugin Order",

                            #[wrap(Some)]
                            set_child = &gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,

                            // Normal mode header
                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_margin_top: 8,
                                set_margin_bottom: 4,
                                #[watch]
                                set_visible: !model.plugin_selection_active,

                                gtk::Label {
                                    set_label: "Plugin Order",
                                    add_css_class: "heading",
                                    set_hexpand: true,
                                    set_halign: gtk::Align::Start,
                                    set_margin_start: 8,
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

                                gtk::Button {
                                    set_icon_name: "selection-mode-symbolic",
                                    set_tooltip_text: Some("Select plugins"),
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    set_margin_end: 4,
                                    connect_clicked => AppMsg::EnterPluginSelectionMode,
                                },

                                gtk::MenuButton {
                                    set_icon_name: "view-more-symbolic",
                                    set_tooltip_text: Some("More plugin order actions"),
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    set_margin_end: 4,
                                    #[wrap(Some)]
                                    set_popover = &gtk::Popover {
                                        #[wrap(Some)]
                                        set_child = &gtk::Box {
                                            set_orientation: gtk::Orientation::Vertical,
                                            set_spacing: 6,
                                            set_margin_all: 8,

                                            gtk::Button {
                                                set_icon_name: "checkbox-checked-symbolic",
                                                set_label: "Enable all plugins",
                                                add_css_class: "flat",
                                                connect_clicked[sender] => move |btn| {
                                                    sender.input(AppMsg::EnableAllPlugins);
                                                    if let Some(popover) = btn
                                                        .ancestor(gtk::Popover::static_type())
                                                        .and_downcast::<gtk::Popover>()
                                                    {
                                                        popover.popdown();
                                                    }
                                                },
                                            },

                                            gtk::Button {
                                                set_icon_name: "checkbox-symbolic",
                                                set_label: "Disable all plugins",
                                                add_css_class: "flat",
                                                connect_clicked[sender] => move |btn| {
                                                    sender.input(AppMsg::DisableAllPlugins);
                                                    if let Some(popover) = btn
                                                        .ancestor(gtk::Popover::static_type())
                                                        .and_downcast::<gtk::Popover>()
                                                    {
                                                        popover.popdown();
                                                    }
                                                },
                                            },

                                            gtk::Separator {},

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
                                                        if let Some(popover) = btn
                                                            .ancestor(gtk::Popover::static_type())
                                                            .and_downcast::<gtk::Popover>()
                                                        {
                                                            popover.popdown();
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
                            },

                            // Selection mode header
                            gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_margin_top: 8,
                                set_margin_bottom: 4,
                                set_margin_start: 8,
                                set_margin_end: 8,
                                #[watch]
                                set_visible: model.plugin_selection_active,

                                gtk::Label {
                                    #[watch]
                                    set_label: &format!("{} selected", model.selected_plugins.len()),
                                    set_hexpand: true,
                                    set_halign: gtk::Align::Start,
                                    add_css_class: "heading",
                                },

                                gtk::Button {
                                    #[watch]
                                    set_label: if model.plugin_selection_dirty { "Done" } else { "Cancel" },
                                    connect_clicked => AppMsg::ExitPluginSelectionMode,
                                },
                            },

                            #[local_ref]
                            plugin_scroll -> gtk::ScrolledWindow {
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

                            gtk::ActionBar {
                                #[watch]
                                set_revealed: model.plugin_selection_active,

                                pack_start = &gtk::Button {
                                    set_label: "Enable",
                                    connect_clicked => AppMsg::EnableSelectedPlugins,
                                },

                                pack_start = &gtk::Button {
                                    set_label: "Disable",
                                    connect_clicked => AppMsg::DisableSelectedPlugins,
                                },
                            },
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

                    gtk::Label {
                        set_label: "\u{00b7}",
                        #[watch]
                        set_visible: model.issues_mods_count() > 0,
                        add_css_class: "caption",
                        add_css_class: "dim-label",
                    },

                    gtk::Label {
                        #[watch]
                        set_label: &model.conflict_count_label(),
                        #[watch]
                        set_visible: model.issues_mods_count() > 0,
                        add_css_class: "caption",
                        add_css_class: "warning",
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
        let plugin_scroll = &model.plugin_scroll;
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
        root.set_opacity(0.0);
        gtk::glib::idle_add_local_once({
            let root = root.clone();
            move || {
                root.set_opacity(1.0);
                root.present();
            }
        });

        // NOTE: do NOT set_key_capture_widget(root) here — that routes every window
        // keystroke into the search entry, causing the bar to flicker open/closed
        // whenever the user types while some other widget has focus.

        init::wire_drag_drop(&sender, mod_list, plugin_list, &model.mod_scroll);

        sender.oneshot_command(async move { init::load_init_data().await });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        self.dispatch_input(msg, sender, root);
    }

    fn update_cmd(
        &mut self,
        msg: Self::CommandOutput,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        self.dispatch_command(msg, sender, root);
    }
}
