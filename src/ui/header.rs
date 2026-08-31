use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeaderState {
    pub(crate) nexus_username: Option<String>,
    pub(crate) nexus_is_premium: bool,
    pub(crate) has_games: bool,
    pub(crate) initializing: bool,
    pub(crate) profile_count: usize,
    pub(crate) save_mode_label: String,
    pub(crate) game_has_save_management: bool,
    pub(crate) can_sync_saves: bool,
    pub(crate) is_busy: bool,
    pub(crate) busy_message: String,
    pub(crate) deploying: bool,
    pub(crate) needs_deploy: bool,
    pub(crate) notification_count: usize,
    pub(crate) notification_badge: String,
    pub(crate) external_changes_count: usize,
    pub(crate) app_update_version: Option<String>,
    pub(crate) running_as_appimage: bool,
    pub(crate) global_active_count: usize,
    pub(crate) downloads_visible: bool,
    pub(crate) search_active: bool,
}

pub(crate) struct HeaderInit {
    pub(crate) state: HeaderState,
    pub(crate) nexus_user_btn: gtk::MenuButton,
    pub(crate) nexus_avatar_widget: adw::Avatar,
    pub(crate) game_dropdown: gtk::DropDown,
    pub(crate) profile_dropdown: gtk::DropDown,
    pub(crate) profile_menu_btn: gtk::MenuButton,
    pub(crate) profile_rename_btn: gtk::MenuButton,
    pub(crate) save_mode_btn: gtk::Button,
    pub(crate) sync_saves_btn: gtk::Button,
    pub(crate) deploy_options_btn: gtk::MenuButton,
    pub(crate) overflow_menu_btn: gtk::MenuButton,
    pub(crate) notifications_menu_btn: gtk::MenuButton,
    pub(crate) notification_list: gtk::ListBox,
    pub(crate) tool_buttons_box: gtk::Box,
}

pub(crate) struct Header {
    state: HeaderState,
}

#[derive(Debug)]
pub(crate) enum HeaderOutput {
    NexusLogoutClicked,
    NexusLoginClicked,
    GameSelected(u32),
    RemoveCurrentGame,
    ProfileSelected(u32),
    NewProfileClicked,
    CloneProfileClicked,
    DeleteProfileClicked,
    ToggleProfileSaveMode,
    SyncSaves,
    ManageSaveBackups,
    DeployClicked,
    OpenDeploymentFolder,
    PurgeClicked,
    CreateEmptyMod,
    ResetVanillaBaseline,
    ManageToolsClicked,
    SettingsClicked,
    AbsorbExternalFiles,
    SelfUpdateDownload,
    ClearNotifications,
    SetDownloadsVisible(bool),
    SearchToggled(bool),
}

#[relm4::component(pub(crate))]
impl SimpleComponent for Header {
    type Init = HeaderInit;
    type Input = HeaderState;
    type Output = HeaderOutput;

    view! {
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
                        set_text: model.state.nexus_username.as_deref(),
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
                            set_visible: model.state.nexus_username.is_some(),

                            gtk::Label {
                                #[watch]
                                set_label: model.state.nexus_username.as_deref().unwrap_or(""),
                                add_css_class: "title-4",
                                set_halign: gtk::Align::Start,
                                set_ellipsize: gtk::pango::EllipsizeMode::End,
                            },

                            gtk::Label {
                                #[watch]
                                set_label: if model.state.nexus_is_premium { "Premium" } else { "Free" },
                                add_css_class: "caption",
                                set_halign: gtk::Align::Start,
                            },

                            gtk::Separator {},

                            gtk::Button {
                                set_label: "Log Out",
                                add_css_class: "destructive-action",
                                connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::NexusLogoutClicked).ok();
                                },
                            },
                        },

                        // Logged-out state
                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 4,
                            #[watch]
                            set_visible: model.state.nexus_username.is_none(),

                            gtk::Label {
                                set_label: "Not connected",
                                add_css_class: "dim-label",
                                set_halign: gtk::Align::Start,
                            },

                            gtk::Button {
                                set_label: "Login with Nexus",
                                add_css_class: "suggested-action",
                                connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::NexusLoginClicked).ok();
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
                set_visible: model.state.has_games && !model.state.initializing,
                connect_selected_notify[sender] => move |dd| {
                    sender.output(HeaderOutput::GameSelected(dd.selected())).ok();
                }
            },

            pack_start = &gtk::Button {
                set_icon_name: "window-close-symbolic",
                set_tooltip_text: Some("Stop managing this game"),
                add_css_class: "flat",
                #[watch]
                set_visible: model.state.has_games && !model.state.initializing,
                connect_clicked[sender] => move |_| {
                    sender.output(HeaderOutput::RemoveCurrentGame).ok();
                },
            },

            #[local_ref]
            pack_start = profile_dropdown -> gtk::DropDown {
                set_tooltip_text: Some("Active profile"),
                add_css_class: "flat",
                #[watch]
                set_visible: model.state.has_games && !model.state.initializing,
                connect_selected_notify[sender] => move |dd| {
                    sender.output(HeaderOutput::ProfileSelected(dd.selected())).ok();
                }
            },

            // Profile management MenuButton — icon-only, opens action popover
            #[local_ref]
            pack_start = profile_menu_btn -> gtk::MenuButton {
                set_icon_name: "view-more-symbolic",
                set_tooltip_text: Some("Profile options"),
                #[watch]
                set_visible: model.state.has_games && !model.state.initializing,
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
                                connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::NewProfileClicked).ok();
                                },
                            },

                            gtk::Button {
                                set_icon_name: "edit-copy-symbolic",
                                set_tooltip_text: Some("Clone current profile"),
                                add_css_class: "flat",
                                connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::CloneProfileClicked).ok();
                                },
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
                                set_sensitive: model.state.profile_count > 1,
                                connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::DeleteProfileClicked).ok();
                                },
                            },

                        },

                        #[local_ref]
                        save_mode_btn -> gtk::Button {
                            #[watch]
                            set_label: model.state.save_mode_label.as_str(),
                            #[watch]
                            set_visible: model.state.game_has_save_management,
                            set_tooltip_text: Some("Toggle per-profile save file isolation"),
                            add_css_class: "flat",
                            connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::ToggleProfileSaveMode).ok();
                                },
                        },

                        #[local_ref]
                        sync_saves_btn -> gtk::Button {
                            set_icon_name: "view-refresh-symbolic",
                            #[watch]
                            set_visible: model.state.can_sync_saves,
                            #[watch]
                            set_sensitive: !model.state.is_busy,
                            set_tooltip_text: Some("Sync saves: update profile snapshot from game save directory"),
                            add_css_class: "flat",
                            connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::SyncSaves).ok();
                                },
                        },

                        gtk::Button {
                            set_label: "Manage save backups…",
                            #[watch]
                            set_visible: model.state.game_has_save_management,
                            add_css_class: "flat",
                            connect_clicked[sender] => move |_| {
                                sender.output(HeaderOutput::ManageSaveBackups).ok();
                            },
                        },
                    }
                },
            },

            pack_start = &gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                #[watch]
                set_visible: model.state.has_games && !model.state.initializing && model.state.is_busy,

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
                        set_label: model.state.busy_message.as_str(),
                    },
                },
            },

            pack_end = &gtk::Box {
                add_css_class: "linked",

                gtk::Button {
                    #[watch]
                    set_label: if model.state.deploying { "Deploying\u{2026}" } else { "Deploy" },
                    #[watch]
                    set_css_classes: if model.state.needs_deploy {
                        &["suggested-action"]
                    } else {
                        &[]
                    },
                    set_tooltip_text: Some("Deploy mods to game folder"),
                    #[watch]
                    set_sensitive: !model.state.is_busy && model.state.has_games,
                    connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::DeployClicked).ok();
                                },
                },

                #[local_ref]
                deploy_options_btn -> gtk::MenuButton {
                    set_icon_name: "pan-down-symbolic",
                    set_tooltip_text: Some("Deploy options"),
                    #[watch]
                    set_css_classes: if model.state.needs_deploy {
                        &["suggested-action"]
                    } else {
                        &[]
                    },
                    #[watch]
                    set_sensitive: !model.state.is_busy && model.state.has_games,
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
                                connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::OpenDeploymentFolder).ok();
                                },
                            },

                            gtk::Separator {},

                            gtk::Button {
                                set_label: "Purge deployment",
                                add_css_class: "flat",
                                connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::PurgeClicked).ok();
                                },
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
                            set_sensitive: !model.state.is_busy && model.state.has_games,
                            connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::CreateEmptyMod).ok();
                                },
                        },

                        gtk::Button {
                            set_icon_name: "view-refresh-symbolic",
                            set_label: "Reset Vanilla Baseline",
                            set_tooltip_text: Some("Re-snapshot the current game folder as the new vanilla state — use after a clean game reinstall"),
                            add_css_class: "flat",
                            #[watch]
                            set_sensitive: !model.state.is_busy && model.state.has_games,
                            connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::ResetVanillaBaseline).ok();
                                },
                        },

                        gtk::Button {
                            set_icon_name: "applications-engineering-symbolic",
                            set_label: "Manage Tools",
                            set_tooltip_text: Some("Add and configure external modding tools"),
                            add_css_class: "flat",
                            #[watch]
                            set_sensitive: !model.state.is_busy && model.state.has_games,
                            connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::ManageToolsClicked).ok();
                                },
                        },

                        gtk::Button {
                            set_icon_name: "emblem-system-symbolic",
                            set_label: "Settings",
                            add_css_class: "flat",
                            connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::SettingsClicked).ok();
                                },
                        },
                    },
                },
            },

            #[local_ref]
            pack_end = notifications_menu_btn -> gtk::MenuButton {
                set_tooltip_text: Some("Notifications"),
                set_always_show_arrow: false,
                #[watch]
                set_css_classes: if model.state.notification_count > 0 {
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
                        set_label: model.state.notification_badge.as_str(),
                        #[watch]
                        set_visible: model.state.notification_count > 0,
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
                            set_visible: model.state.notification_count == 0,
                            set_icon_name: Some("emblem-ok-symbolic"),
                            set_title: "All Caught Up",
                        },

                        gtk::ScrolledWindow {
                            #[watch]
                            set_visible: model.state.external_changes_count > 0
                                || model.state.app_update_version.is_some(),
                            set_propagate_natural_height: true,
                            set_max_content_height: 400,
                            set_hscrollbar_policy: gtk::PolicyType::Never,

                            gtk::ListBox {
                                set_selection_mode: gtk::SelectionMode::None,
                                add_css_class: "boxed-list",

                                adw::ActionRow {
                                    #[watch]
                                    set_visible: model.state.external_changes_count > 0,
                                    set_title: "External Changes",
                                    #[watch]
                                    set_subtitle: &format!(
                                        "{} file{} detected outside mod manager",
                                        model.state.external_changes_count,
                                        if model.state.external_changes_count == 1 { "" } else { "s" }
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
                                            sender.output(HeaderOutput::AbsorbExternalFiles).ok();
                                        },
                                    },
                                },

                                adw::ActionRow {
                                    #[watch]
                                    set_visible: model.state.app_update_version.is_some(),
                                    set_title: "App Update Available",
                                    #[watch]
                                    set_subtitle: model.state.app_update_version.as_deref().unwrap_or(""),
                                    add_prefix = &gtk::Image {
                                        set_icon_name: Some("software-update-available-symbolic"),
                                        set_valign: gtk::Align::Center,
                                    },
                                    add_suffix = &gtk::Button {
                                        set_label: if model.state.running_as_appimage { "Download" } else { "View" },
                                        set_valign: gtk::Align::Center,
                                        add_css_class: "suggested-action",
                                        add_css_class: "pill",
                                        connect_clicked[sender] => move |_| {
                                            sender.output(HeaderOutput::SelfUpdateDownload).ok();
                                        },
                                    },
                                },
                            },
                        },

                        gtk::Box {
                            #[watch]
                            set_visible: model.state.notification_count > 0,
                            set_orientation: gtk::Orientation::Horizontal,
                            set_halign: gtk::Align::End,

                            gtk::Button {
                                set_label: "Clear All",
                                add_css_class: "flat",
                                set_tooltip_text: Some("Dismiss all notifications"),
                                connect_clicked[sender] => move |_| {
                                    sender.output(HeaderOutput::ClearNotifications).ok();
                                },
                            },
                        },

                        gtk::ScrolledWindow {
                            #[watch]
                            set_visible: model.state.notification_count > 0,
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
                set_icon_name: if model.state.global_active_count > 0 {
                    "content-loading-symbolic"
                } else {
                    "folder-download-symbolic"
                },
                set_tooltip_text: Some("Downloads"),
                #[watch]
                set_active: model.state.downloads_visible,
                connect_toggled[sender] => move |btn| {
                    sender.output(HeaderOutput::SetDownloadsVisible(btn.is_active())).ok();
                },
            },

            pack_end = &gtk::ToggleButton {
                set_icon_name: "edit-find-symbolic",
                set_tooltip_text: Some("Search mods (Ctrl+F)"),
                #[watch]
                set_active: model.state.search_active,
                connect_toggled[sender] => move |btn| {
                    sender.output(HeaderOutput::SearchToggled(btn.is_active())).ok();
                },
            },

            #[local_ref]
            pack_end = tool_buttons_box -> gtk::Box {},
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let HeaderInit {
            state,
            nexus_user_btn,
            nexus_avatar_widget,
            game_dropdown,
            profile_dropdown,
            profile_menu_btn,
            profile_rename_btn,
            save_mode_btn,
            sync_saves_btn,
            deploy_options_btn,
            overflow_menu_btn,
            notifications_menu_btn,
            notification_list,
            tool_buttons_box,
        } = init;
        let nexus_user_btn = &nexus_user_btn;
        let nexus_avatar_widget = &nexus_avatar_widget;
        let game_dropdown = &game_dropdown;
        let profile_dropdown = &profile_dropdown;
        let profile_menu_btn = &profile_menu_btn;
        let profile_rename_btn = &profile_rename_btn;
        let save_mode_btn = &save_mode_btn;
        let sync_saves_btn = &sync_saves_btn;
        let deploy_options_btn = &deploy_options_btn;
        let overflow_menu_btn = &overflow_menu_btn;
        let notifications_menu_btn = &notifications_menu_btn;
        let notification_list = &notification_list;
        let tool_buttons_box = &tool_buttons_box;
        let model = Self { state };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, state: Self::Input, _sender: ComponentSender<Self>) {
        self.state = state;
    }
}
