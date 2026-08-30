mod appearance;
mod cache_handlers;
mod deploy;
mod dispatch;
mod downloads;
mod external;
mod init;
mod install;
mod install_file_id;
mod messages;
mod mods;
mod notifications;
mod order_snapshots;
mod plugins;
mod presentation;
mod profiles;
mod progress;
mod search;
mod session;
mod startup;
mod timing;
mod tools;
mod types;

pub(crate) use self::messages::DownloadsMsg;
pub(crate) use self::messages::{AppCmdMsg, AppMsg};

use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use self::types::ModFilter;

mod state;
pub(crate) use state::App;

#[relm4::component(pub(crate))]
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
                sender.input(AppMsg::Shell(crate::app::messages::ShellMsg::CloseRequested));
                glib::Propagation::Stop
            },

            adw::ToolbarView {
                #[local_ref]
                add_top_bar = header -> adw::HeaderBar {},

                #[wrap(Some)]
                set_content = &gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,

                #[local_ref]
                search_bar -> gtk::SearchBar {
                    #[watch]
                    set_search_mode: model.shell.search_active,
                },

                adw::Clamp {
                    #[watch]
                    set_visible: model.session.initializing,
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
                    set_visible: !model.session.initializing,
                    #[watch]
                    set_show_sidebar: model.download.visible,
                    set_sidebar_position: gtk::PackType::End,
                    set_max_sidebar_width: 700.0,
                    set_min_sidebar_width: 250.0,
                    set_collapsed: false,

                    #[wrap(Some)]
                    #[local_ref]
                    set_sidebar = downloads_pane -> adw::ToolbarView {},
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
                                set_visible: !model.mods.selection_active,

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
                                        set_css_classes: if matches!(model.mods.filter, ModFilter::All) {
                                            &["pill", "filter-chip", "suggested-action"]
                                        } else {
                                            &["pill", "filter-chip"]
                                        },
                                        #[watch]
                                        set_label: &format!("All ({})", model.total_mods_count()),
                                        connect_clicked => AppMsg::Mods(crate::app::messages::ModsMsg::SetModFilter(ModFilter::All)),
                                    },

                                    gtk::Button {
                                        #[watch]
                                        set_css_classes: if matches!(model.mods.filter, ModFilter::Enabled) {
                                            &["pill", "filter-chip", "suggested-action"]
                                        } else {
                                            &["pill", "filter-chip"]
                                        },
                                        #[watch]
                                        set_label: &format!("Enabled ({})", model.enabled_mods_count()),
                                        connect_clicked => AppMsg::Mods(crate::app::messages::ModsMsg::SetModFilter(ModFilter::Enabled)),
                                    },

                                    gtk::Button {
                                        #[watch]
                                        set_css_classes: if matches!(model.mods.filter, ModFilter::Issues) {
                                            &["pill", "filter-chip", "suggested-action"]
                                        } else {
                                            &["pill", "filter-chip"]
                                        },
                                        #[watch]
                                        set_label: &format!("Conflicts ({})", model.issues_mods_count()),
                                        connect_clicked => AppMsg::Mods(crate::app::messages::ModsMsg::SetModFilter(ModFilter::Issues)),
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
                                        connect_clicked => AppMsg::Install(crate::app::messages::InstallMsg::InstallClicked),
                                    },

                                    gtk::Button {
                                        set_icon_name: "selection-mode-symbolic",
                                        set_tooltip_text: Some("Select mods"),
                                        add_css_class: "flat",
                                        connect_clicked => AppMsg::Mods(crate::app::messages::ModsMsg::EnterModSelectionMode),
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
                                                    sender.input(AppMsg::Mods(crate::app::messages::ModsMsg::EnableAllMods));
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
                                                    sender.input(AppMsg::Mods(crate::app::messages::ModsMsg::DisableAllMods));
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
                                                    sender.input(AppMsg::Mods(crate::app::messages::ModsMsg::CreateGroup("New Group".to_string())));
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
                                                        sender.input(AppMsg::Mods(crate::app::messages::ModsMsg::SaveModOrderSnapshot(name)));
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
                                set_visible: model.mods.selection_active,

                                gtk::Label {
                                    #[watch]
                                    set_label: &format!("{} selected", model.mods.selected.len()),
                                    set_hexpand: true,
                                    set_halign: gtk::Align::Start,
                                    add_css_class: "heading",
                                },

                                gtk::Button {
                                    #[watch]
                                    set_label: if model.mods.selection_dirty { "Done" } else { "Cancel" },
                                    connect_clicked => AppMsg::Mods(crate::app::messages::ModsMsg::ExitModSelectionMode),
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
                                set_visible: model.has_no_mods() && matches!(model.mods.filter, ModFilter::All),
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
                                set_visible: matches!(model.mods.filter, ModFilter::Enabled)
                                    && model.enabled_mods_count() == 0
                                    && model.total_mods_count() > 0,
                                set_icon_name: Some("checkbox-checked-symbolic"),
                                set_title: "No Enabled Mods",
                                set_description: Some("Select mods and use the action bar to enable them."),
                            },
                            adw::StatusPage {
                                #[watch]
                                set_visible: matches!(model.mods.filter, ModFilter::Issues)
                                    && model.issues_mods_count() == 0
                                    && model.total_mods_count() > 0,
                                set_icon_name: Some("emblem-ok-symbolic"),
                                set_title: "No Conflicts",
                                set_description: Some("No mods override each other's files."),
                            },

                            gtk::ActionBar {
                                #[watch]
                                set_revealed: model.mods.selection_active,

                                pack_start = &gtk::Button {
                                    set_label: "Enable",
                                    connect_clicked => AppMsg::Mods(crate::app::messages::ModsMsg::EnableSelectedMods),
                                },

                                pack_start = &gtk::Button {
                                    set_label: "Disable",
                                    connect_clicked => AppMsg::Mods(crate::app::messages::ModsMsg::DisableSelectedMods),
                                },

                                pack_end = &gtk::Button {
                                    set_label: "Remove",
                                    add_css_class: "destructive-action",
                                    connect_clicked => AppMsg::Mods(crate::app::messages::ModsMsg::RemoveSelectedMods),
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
                                set_visible: !model.plugins.selection_active,

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
                                    set_active: model.plugins.show_vanilla,
                                    connect_clicked => AppMsg::Plugins(crate::app::messages::PluginsMsg::ToggleShowVanillaPlugins),
                                },

                                gtk::Button {
                                    set_icon_name: "view-sort-ascending-symbolic",
                                    set_tooltip_text: Some("Sort with LOOT"),
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    set_margin_end: 4,
                                    connect_clicked => AppMsg::Plugins(crate::app::messages::PluginsMsg::SortWithLoot),
                                },

                                gtk::Button {
                                    set_icon_name: "selection-mode-symbolic",
                                    set_tooltip_text: Some("Select plugins"),
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    set_margin_end: 4,
                                    connect_clicked => AppMsg::Plugins(crate::app::messages::PluginsMsg::EnterPluginSelectionMode),
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
                                                    sender.input(AppMsg::Plugins(crate::app::messages::PluginsMsg::EnableAllPlugins));
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
                                                    sender.input(AppMsg::Plugins(crate::app::messages::PluginsMsg::DisableAllPlugins));
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
                                                        sender.input(AppMsg::Plugins(crate::app::messages::PluginsMsg::SavePluginOrderSnapshot(name)));
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
                                set_visible: model.plugins.selection_active,

                                gtk::Label {
                                    #[watch]
                                    set_label: &format!("{} selected", model.plugins.selected.len()),
                                    set_hexpand: true,
                                    set_halign: gtk::Align::Start,
                                    add_css_class: "heading",
                                },

                                gtk::Button {
                                    #[watch]
                                    set_label: if model.plugins.selection_dirty { "Done" } else { "Cancel" },
                                    connect_clicked => AppMsg::Plugins(crate::app::messages::PluginsMsg::ExitPluginSelectionMode),
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
                                set_visible: model.plugins.managed_count == 0,
                                set_icon_name: Some("application-x-addon-symbolic"),
                                set_title: "No Plugins",
                                set_description: Some("Plugin files (.esp/.esm/.esl) will appear here"),
                            },

                            gtk::ActionBar {
                                #[watch]
                                set_revealed: model.plugins.selection_active,

                                pack_start = &gtk::Button {
                                    set_label: "Enable",
                                    connect_clicked => AppMsg::Plugins(crate::app::messages::PluginsMsg::EnableSelectedPlugins),
                                },

                                pack_start = &gtk::Button {
                                    set_label: "Disable",
                                    connect_clicked => AppMsg::Plugins(crate::app::messages::PluginsMsg::DisableSelectedPlugins),
                                },
                            },
                            },
                        },
                    }
                    }
                    }
                },

                #[local_ref]
                bottom_status -> gtk::Box {},
                },
            }
        }
    }

    fn init(
        nxm_link: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let (model, _game_ids, _games_for_init, search_bar) = init::build_model(nxm_link, &sender);

        let header = model.ui.header.widget();
        let toast_overlay = &model.ui.toast_overlay;
        let mod_list = model.mods.rows.widget();
        let plugin_list = model.plugins.rows.widget();
        let downloads_pane = model.ui.downloads_pane.widget();
        let bottom_status = model.ui.bottom_status.widget();
        let mod_scroll = &model.mods.scroll;
        let plugin_scroll = &model.plugins.scroll;
        let mod_snapshot_save_entry = &model.mods.snapshot_save_entry;
        let plugin_snapshot_save_entry = &model.plugins.snapshot_save_entry;
        let mod_snapshots_list = &model.mods.snapshots_list;
        let plugin_snapshots_list = &model.plugins.snapshots_list;

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

        init::wire_drag_drop(&sender, mod_list, plugin_list, &model.mods.scroll);

        sender.oneshot_command(async move { init::load_init_data().await });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        self.dispatch_input(msg, sender, root);
        self.ui.header.sender().send(self.header_state()).ok();
        self.ui
            .downloads_pane
            .sender()
            .send(self.downloads_pane_state())
            .ok();
        self.ui
            .bottom_status
            .sender()
            .send(self.bottom_status_state())
            .ok();
    }

    fn update_cmd(
        &mut self,
        msg: Self::CommandOutput,
        sender: ComponentSender<Self>,
        root: &Self::Root,
    ) {
        self.dispatch_command(msg, sender, root);
        self.ui.header.sender().send(self.header_state()).ok();
        self.ui
            .downloads_pane
            .sender()
            .send(self.downloads_pane_state())
            .ok();
        self.ui
            .bottom_status
            .sender()
            .send(self.bottom_status_state())
            .ok();
    }
}
