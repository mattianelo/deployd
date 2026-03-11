pub mod deploy;
pub mod downloads;
pub mod external;
pub mod free_fns;
pub mod helpers;
pub mod install;
pub mod messages;
pub mod mods;
pub mod plugins;
pub mod profiles;
pub mod types;

pub use self::messages::{AppCmdMsg, AppMsg};
pub(crate) use self::free_fns::load_game_data;

use self::types::{InitData, SearchScope};

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gtk::prelude::*;
use gtk::glib;
use relm4::abstractions::Toaster;
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;

use crate::core::detector::ExternalFile;
use crate::core::game;
use crate::core::tracker::Tracker;
use crate::models::download::DownloadEntry;
use crate::models::game::Game;
use crate::models::plugin::PluginDirtyInfo;
use crate::models::profile::Profile;
use crate::models::tool::Tool;
use crate::ui::absorb_dialog::AbsorbDialog;
use crate::ui::download_row::{DownloadRow, DownloadRowOutput};
use crate::ui::fomod_dialog::FomodDialog;
use crate::ui::mod_list::{ModListItem, ModListItemOutput};
use crate::ui::mod_properties_dialog::ModPropertiesDialog;
use crate::ui::plugin_list::{PluginRow, PluginRowOutput};
use crate::ui::pre_install_dialog::PreInstallDialog;
use crate::ui::game_setup_dialog::GameSetupDialog;
use crate::ui::settings_dialog::SettingsDialog;
use crate::ui::tool_manager::ToolManager;
use crate::utils::paths;

use self::free_fns::{clear_drop_indicators, update_drop_indicator};
use self::types::{DownloadSort, PendingInstall};

pub struct App {
    pub(crate) tracker: Option<Tracker>,
    pub(crate) games: Vec<Game>,
    pub(crate) selected_game_idx: usize,
    pub(crate) mods: FactoryVecDeque<ModListItem>,
    /// Group IDs that are currently collapsed (kept in sync with DB on toggle).
    pub(crate) collapsed_groups: HashSet<String>,
    pub(crate) plugins: FactoryVecDeque<PluginRow>,
    pub(crate) installing: bool,
    pub(crate) deploying: bool,
    pub(crate) needs_deploy: bool,
    /// Profile ID that was active during the last successful deploy for the current game.
    pub(crate) last_deployed_profile_id: Option<String>,
    pub(crate) status_msg: Option<String>,
    /// Progress fraction for file caching phase (None = indeterminate/spinner, Some = ProgressBar).
    pub(crate) install_progress: Option<f64>,
    pub(crate) toaster: Toaster,
    pub(crate) profiles: Vec<Profile>,
    pub(crate) active_profile_idx: usize,
    pub(crate) profile_model: gtk::StringList,
    pub(crate) profile_dropdown: gtk::DropDown,
    /// Suppresses the profile dropdown signal while we programmatically update it.
    pub(crate) updating_profiles: bool,
    /// Backing string model for the game dropdown (allows dynamic updates).
    pub(crate) game_model: gtk::StringList,
    /// Game selection dropdown widget.
    pub(crate) game_dropdown: gtk::DropDown,
    /// Holds extracted archive data while pre-install or FOMOD dialog is open.
    pub(crate) pending_install: Option<PendingInstall>,
    /// Active pre-install rename dialog controller.
    pub(crate) pre_install_dialog: Option<Controller<PreInstallDialog>>,
    /// Active FOMOD dialog controller.
    pub(crate) fomod_dialog: Option<Controller<FomodDialog>>,
    /// External tools configured for the selected game.
    pub(crate) tools: Vec<Tool>,
    /// Container for dynamic tool buttons in the headerbar.
    pub(crate) tool_buttons_box: gtk::Box,
    /// Active tool manager dialog controller.
    pub(crate) tool_manager_dialog: Option<Controller<ToolManager>>,
    /// Active game setup dialog controller.
    pub(crate) game_setup_dialog: Option<Controller<GameSetupDialog>>,
    /// Active settings dialog controller.
    pub(crate) settings_dialog: Option<Controller<SettingsDialog>>,
    /// Active mod properties dialog controller.
    pub(crate) mod_properties_dialog: Option<Controller<ModPropertiesDialog>>,
    /// Active external-file absorption dialog controller.
    pub(crate) absorb_dialog: Option<Controller<AbsorbDialog>>,
    /// Entry widget for profile rename popover.
    pub(crate) profile_rename_entry: gtk::Entry,
    /// Pending NXM link received at startup (processed after initialization).
    pub(crate) pending_nxm: Option<String>,
    /// Nexus IDs to attach to the next PendingInstall (handoff from NXM download).
    pub(crate) pending_nexus_ids: Option<(i64, i64, String)>,
    /// Downloads sidebar entries (game-filtered view).
    pub(crate) downloads: FactoryVecDeque<DownloadRow>,
    /// All downloads across all games (backing store).
    pub(crate) all_downloads: Vec<DownloadEntry>,
    /// Whether the downloads sidebar is visible.
    pub(crate) downloads_visible: bool,
    /// ID of the currently active download (for status updates).
    pub(crate) active_download_id: Option<String>,
    /// Cached count of active downloads for current game (for sidebar view).
    pub(crate) active_download_count: usize,
    /// Count of active downloads across ALL games (for headerbar indicator).
    pub(crate) global_active_downloads: usize,
    /// Cached downloads directory path (avoids async DB reads during scan).
    pub(crate) downloads_dir: PathBuf,
    /// Whether the initial auto-scan has been performed (suppresses toast).
    pub(crate) initial_scan_done: bool,
    /// Whether the search bar is shown.
    pub(crate) search_active: bool,
    /// Current search query text.
    pub(crate) search_text: String,
    /// Which panel(s) the search applies to.
    pub(crate) search_scope: SearchScope,
    /// Latest Nexus API rate limit info.
    pub(crate) rate_limit_info: Option<crate::core::nexus_api::RateLimitInfo>,
    /// Current sort order for the downloads sidebar.
    pub(crate) download_sort: DownloadSort,
    /// Files found in the game folder that are not tracked by any mod.
    pub(crate) pending_external_files: Vec<ExternalFile>,
    /// Number of external files detected (drives the badge button visibility).
    pub(crate) external_changes_count: usize,
    /// When set, (mod_id, old_priority) to remove after the pending install completes (replace flow).
    pub(crate) pending_replace_mod_id: Option<(String, i32)>,
    /// ScrolledWindow wrapping the mod list — held so we can restore scroll position.
    pub(crate) mod_scroll: gtk::ScrolledWindow,
    /// ScrolledWindow wrapping the downloads list — held so we can restore scroll position.
    pub(crate) downloads_scroll: gtk::ScrolledWindow,
    /// Dirty-edit info keyed by lowercase plugin filename, from the LOOT masterlist.
    /// Populated after each LOOT sort; cleared on game switch. CRC-based so entries
    /// drop automatically once the user cleans a plugin and re-sorts.
    #[cfg(feature = "loot")]
    pub(crate) dirty_plugins: HashMap<String, PluginDirtyInfo>,
    /// Button that shows/switches the active profile's save mode.
    pub(crate) save_mode_btn: gtk::Button,
    /// Button to manually sync saves from the game directory to the active profile snapshot.
    pub(crate) sync_saves_btn: gtk::Button,
    /// Whether vanilla/DLC plugins are shown in the plugin panel.
    pub(crate) show_vanilla_plugins: bool,
    /// Number of Deployd-managed plugin rows (vanilla rows come after this index).
    pub(crate) managed_plugins_count: usize,
    /// Sorted vanilla/DLC plugin filenames for the current game (used when toggling visibility).
    pub(crate) vanilla_plugin_names: Vec<String>,
    /// Master dependency map: plugin_id → list of master filenames (from TES4 header).
    /// Used to block drag-and-drop moves that would place a plugin before its masters.
    pub(crate) plugin_masters: HashMap<String, Vec<String>>,
    /// Persistent banner shown when a newer app version is available.
    pub(crate) update_banner: adw::Banner,
    /// URL to open when the user clicks the update banner button.
    pub(crate) update_url: Option<String>,
    /// True when the app is running as an AppImage (APPIMAGE env var is set).
    /// Controls whether the banner button triggers a self-update or opens the browser.
    pub(crate) running_as_appimage: bool,
}

#[relm4::component(pub)]
impl Component for App {
    type Init = Option<String>;
    type Input = AppMsg;
    type Output = ();
    type CommandOutput = AppCmdMsg;

    view! {
        adw::Window {
            set_title: Some("Deployd"),
            set_default_size: (1100, 680),
            connect_close_request[sender] => move |_| {
                sender.input(AppMsg::CloseRequested);
                glib::Propagation::Stop
            },

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        set_title: "Deployd",
                        #[watch]
                        set_subtitle: model.current_game_title(),
                    },

                    #[local_ref]
                    pack_start = game_dropdown -> gtk::DropDown {
                        set_selected: 0,
                        #[watch]
                        set_visible: model.has_games(),
                        connect_selected_notify[sender] => move |dd| {
                            sender.input(AppMsg::GameSelected(dd.selected()));
                        }
                    },

                    pack_start = &gtk::Button {
                        set_icon_name: "view-refresh-symbolic",
                        set_tooltip_text: Some("Rescan for games"),
                        add_css_class: "flat",
                        connect_clicked => AppMsg::RescanGames,
                    },

                    pack_start = &gtk::Button {
                        set_icon_name: "window-close-symbolic",
                        set_tooltip_text: Some("Stop managing this game"),
                        add_css_class: "flat",
                        #[watch]
                        set_visible: model.has_games(),
                        connect_clicked[sender] => move |_| {
                            sender.input(AppMsg::RemoveCurrentGame);
                        },
                    },

                    pack_start = &gtk::Box {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_spacing: 4,
                        #[watch]
                        set_visible: model.has_games(),

                        #[local_ref]
                        profile_dropdown -> gtk::DropDown {
                            set_tooltip_text: Some("Active profile"),
                            connect_selected_notify[sender] => move |dd| {
                                sender.input(AppMsg::ProfileSelected(dd.selected()));
                            }
                        },

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

                        gtk::Button {
                            set_icon_name: "document-save-symbolic",
                            set_tooltip_text: Some("Export active profile to file"),
                            add_css_class: "flat",
                            connect_clicked => AppMsg::ExportProfileClicked,
                        },

                        gtk::Button {
                            set_icon_name: "document-open-symbolic",
                            set_tooltip_text: Some("Import profile from file"),
                            add_css_class: "flat",
                            connect_clicked => AppMsg::ImportProfileClicked,
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
                    },

                    pack_end = &gtk::Button {
                        #[watch]
                        set_label: if model.needs_deploy { "Deploy \u{2022}" } else { "Deploy" },
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

                    pack_end = &gtk::Button {
                        set_icon_name: "edit-clear-all-symbolic",
                        set_tooltip_text: Some("Purge deployed files from game folder"),
                        #[watch]
                        set_sensitive: !model.is_busy() && model.has_games(),
                        connect_clicked => AppMsg::PurgeClicked,
                    },

                    pack_end = &gtk::Button {
                        set_icon_name: "list-add-symbolic",
                        set_tooltip_text: Some("Add Mod"),
                        #[watch]
                        set_sensitive: !model.is_busy() && model.has_games(),
                        connect_clicked => AppMsg::InstallClicked,
                    },

                    pack_end = &gtk::Button {
                        set_icon_name: "folder-new-symbolic",
                        set_tooltip_text: Some("Create Empty Mod — opens cache folder in file manager"),
                        #[watch]
                        set_sensitive: !model.is_busy() && model.has_games(),
                        connect_clicked => AppMsg::CreateEmptyMod,
                    },

                    pack_end = &gtk::Button {
                        set_icon_name: "view-refresh-symbolic",
                        set_tooltip_text: Some("Reset vanilla baseline — use after a clean game reinstall to clear false-positive external detections"),
                        #[watch]
                        set_visible: model.external_changes_count > 0,
                        #[watch]
                        set_sensitive: !model.is_busy(),
                        connect_clicked => AppMsg::ResetVanillaBaseline,
                        add_css_class: "flat",
                    },

                    pack_end = &gtk::Button {
                        #[watch]
                        set_label: &format!("{} external", model.external_changes_count),
                        set_tooltip_text: Some("External files detected — click to create a managed mod from them"),
                        #[watch]
                        set_visible: model.external_changes_count > 0,
                        connect_clicked => AppMsg::AbsorbExternalFiles,
                        add_css_class: "flat",
                    },

                    pack_end = &gtk::Button {
                        set_icon_name: "emblem-system-symbolic",
                        set_tooltip_text: Some("Settings"),
                        connect_clicked => AppMsg::SettingsClicked,
                    },

                    pack_end = &gtk::Button {
                        #[watch]
                        set_icon_name: if model.global_active_downloads > 0 {
                            "content-loading-symbolic"
                        } else {
                            "folder-download-symbolic"
                        },
                        set_tooltip_text: Some("Downloads"),
                        connect_clicked => AppMsg::ToggleDownloads,
                    },

                    pack_end = &gtk::Button {
                        set_icon_name: "software-update-available-symbolic",
                        set_tooltip_text: Some("Check for updates on Nexus"),
                        #[watch]
                        set_sensitive: !model.is_busy() && model.has_games(),
                        connect_clicked => AppMsg::CheckUpdatesClicked,
                    },

                    pack_end = &gtk::Button {
                        set_icon_name: "preferences-other-symbolic",
                        set_tooltip_text: Some("Manage Tools"),
                        #[watch]
                        set_sensitive: model.has_games(),
                        connect_clicked => AppMsg::ManageToolsClicked,
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

                #[local_ref]
                update_banner -> adw::Banner {
                    #[watch]
                    set_button_label: Some(if model.running_as_appimage {
                        "Download Update"
                    } else {
                        "View on Nexus"
                    }),
                    connect_button_clicked[sender] => move |_| {
                        sender.input(AppMsg::SelfUpdateDownload);
                    },
                },

                // Indeterminate spinner (extraction phase)
                gtk::Box {
                    #[watch]
                    set_visible: model.is_busy() && model.install_progress.is_none(),
                    set_orientation: gtk::Orientation::Horizontal,
                    set_spacing: 8,
                    set_margin_all: 8,
                    set_halign: gtk::Align::Center,

                    gtk::Spinner {
                        set_spinning: true,
                    },
                    gtk::Label {
                        #[watch]
                        set_label: model.status_msg.as_deref().unwrap_or("Working..."),
                    },
                },

                // Determinate progress bar (file caching phase)
                gtk::ProgressBar {
                    #[watch]
                    set_visible: model.install_progress.is_some(),
                    #[watch]
                    set_fraction: model.install_progress.unwrap_or(0.0),
                    #[watch]
                    set_text: Some(model.status_msg.as_deref().unwrap_or("Working...")),
                    set_show_text: true,
                    set_margin_start: 16,
                    set_margin_end: 16,
                    set_margin_top: 4,
                    set_margin_bottom: 4,
                },

                adw::OverlaySplitView {
                    set_vexpand: true,
                    #[watch]
                    set_show_sidebar: model.downloads_visible,
                    set_sidebar_position: gtk::PackType::End,
                    set_max_sidebar_width: 400.0,
                    set_min_sidebar_width: 300.0,

                    #[wrap(Some)]
                    set_sidebar = &gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_width_request: 300,

                        gtk::Box {
                            set_orientation: gtk::Orientation::Horizontal,
                            set_margin_all: 8,
                            set_spacing: 8,

                            gtk::Label {
                                set_label: "Downloads",
                                add_css_class: "heading",
                                set_hexpand: true,
                                set_halign: gtk::Align::Start,
                            },

                            gtk::DropDown {
                                set_model: Some(&gtk::StringList::new(&["Default", "Name", "Status"])),
                                set_valign: gtk::Align::Center,
                                set_tooltip_text: Some("Sort downloads"),
                                #[watch]
                                set_selected: model.download_sort as u32,
                                connect_selected_notify[sender] => move |dd| {
                                    sender.input(AppMsg::DownloadSortChanged(dd.selected()));
                                },
                            },

                            gtk::Button {
                                set_icon_name: "folder-open-symbolic",
                                set_tooltip_text: Some("Scan downloads folder"),
                                add_css_class: "flat",
                                connect_clicked => AppMsg::ScanDownloadsFolder,
                            },

                        },

                        gtk::Separator {},

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

                                gtk::Button {
                                    set_label: "All",
                                    set_tooltip_text: Some("Enable all mods"),
                                    add_css_class: "flat",
                                    connect_clicked => AppMsg::EnableAllMods,
                                },

                                gtk::Button {
                                    set_label: "None",
                                    set_tooltip_text: Some("Disable all mods"),
                                    add_css_class: "flat",
                                    connect_clicked => AppMsg::DisableAllMods,
                                },

                                gtk::Button {
                                    set_icon_name: "list-add-symbolic",
                                    set_tooltip_text: Some("New group"),
                                    add_css_class: "flat",
                                    add_css_class: "circular",
                                    connect_clicked[sender] => move |_| {
                                        sender.input(AppMsg::CreateGroup("New Group".to_string()));
                                    }
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

                                gtk::Button {
                                    set_label: "All",
                                    set_tooltip_text: Some("Enable all plugins"),
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    connect_clicked => AppMsg::EnableAllPlugins,
                                },

                                gtk::Button {
                                    set_label: "None",
                                    set_tooltip_text: Some("Disable all plugins"),
                                    add_css_class: "flat",
                                    set_valign: gtk::Align::Center,
                                    connect_clicked => AppMsg::DisableAllPlugins,
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

                // Rate limit status bar
                gtk::Label {
                    #[watch]
                    set_label: &model.rate_limit_label(),
                    #[watch]
                    set_visible: model.rate_limit_info.is_some(),
                    set_halign: gtk::Align::End,
                    set_margin_end: 8,
                    set_margin_top: 2,
                    set_margin_bottom: 2,
                    add_css_class: "caption",
                    add_css_class: "dim-label",
                    #[watch]
                    set_css_classes: &model.rate_limit_css(),
                },
            }
        }
    }

    fn init(
        nxm_link: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let mods =
            FactoryVecDeque::builder()
                .launch_default()
                .forward(sender.input_sender(), |output| match output {
                    ModListItemOutput::Remove(index) => AppMsg::RemoveMod(index),
                    ModListItemOutput::ToggleEnabled(index, enabled) => {
                        AppMsg::ToggleModEnabled(index, enabled)
                    }
                    ModListItemOutput::RenameMod(index, name) => AppMsg::RenameMod(index, name),
                    ModListItemOutput::OpenProperties(index) => AppMsg::OpenModProperties(index),
                    ModListItemOutput::ToggleGroupCollapse(index) => {
                        AppMsg::ToggleGroupCollapse(index)
                    }
                    ModListItemOutput::DeleteGroup(index) => AppMsg::DeleteGroup(index),
                    ModListItemOutput::RenameGroup(index, name) => AppMsg::RenameGroup(index, name),
                });

        let plugins =
            FactoryVecDeque::builder()
                .launch_default()
                .forward(sender.input_sender(), |output| match output {
                    PluginRowOutput::ToggleEnabled(index, enabled) => {
                        AppMsg::TogglePluginEnabled(index, enabled)
                    }
                });

        let downloads =
            FactoryVecDeque::builder()
                .launch_default()
                .forward(sender.input_sender(), |output| match output {
                    DownloadRowOutput::Install(index) => AppMsg::InstallDownload(index),
                    DownloadRowOutput::FetchMetadata(index) => AppMsg::FetchDownloadMetadata(index),
                    DownloadRowOutput::ClearMetadata(index) => AppMsg::ClearDownloadMetadata(index),
                    DownloadRowOutput::Rename(index) => AppMsg::RenameDownload(index),
                });

        let games = game::detect_games();

        // Build dropdown model from detected game titles (StringList allows dynamic updates)
        let game_names: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        let game_model = gtk::StringList::new(&game_names);
        let game_dropdown = gtk::DropDown::new(Some(game_model.clone()), None::<gtk::Expression>);

        let profile_model = gtk::StringList::new(&[]);
        let profile_dropdown =
            gtk::DropDown::new(Some(profile_model.clone()), None::<gtk::Expression>);

        let game_ids: Vec<String> = games.iter().map(|g| g.id.clone()).collect();
        let games_for_init = games.clone();

        let tool_buttons_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let profile_rename_entry = gtk::Entry::builder().hexpand(true).build();
        let mod_scroll = gtk::ScrolledWindow::new();
        let downloads_scroll = gtk::ScrolledWindow::new();

        let model = App {
            tracker: None,
            games,
            selected_game_idx: 0,
            mods,
            plugins,
            installing: false,
            deploying: false,
            needs_deploy: false,
            last_deployed_profile_id: None,
            status_msg: None,
            install_progress: None,
            toaster: Toaster::default(),
            profiles: vec![],
            active_profile_idx: 0,
            profile_model,
            profile_dropdown,
            game_model,
            game_dropdown,
            updating_profiles: false,
            pending_install: None,
            pre_install_dialog: None,
            fomod_dialog: None,
            tools: vec![],
            tool_buttons_box,
            tool_manager_dialog: None,
            game_setup_dialog: None,
            settings_dialog: None,
            mod_properties_dialog: None,
            absorb_dialog: None,
            profile_rename_entry: profile_rename_entry.clone(),
            pending_nxm: nxm_link,
            pending_nexus_ids: None,
            downloads,
            all_downloads: Vec::new(),
            downloads_visible: false,
            active_download_id: None,
            active_download_count: 0,
            global_active_downloads: 0,
            downloads_dir: paths::default_downloads_dir(),
            initial_scan_done: false,
            search_active: false,
            search_text: String::new(),
            search_scope: SearchScope::All,
            rate_limit_info: None,
            collapsed_groups: HashSet::new(),
            download_sort: DownloadSort::Default,
            pending_external_files: Vec::new(),
            external_changes_count: 0,
            pending_replace_mod_id: None,
            mod_scroll,
            downloads_scroll,
            #[cfg(feature = "loot")]
            dirty_plugins: HashMap::new(),
            save_mode_btn: gtk::Button::new(),
            sync_saves_btn: gtk::Button::new(),
            show_vanilla_plugins: false,
            managed_plugins_count: 0,
            vanilla_plugin_names: Vec::new(),
            plugin_masters: HashMap::new(),
            update_banner: adw::Banner::new(""),
            update_url: None,
            running_as_appimage: std::env::var("APPIMAGE").is_ok(),
        };

        // Profile rename popover
        let rename_apply = gtk::Button::builder()
            .label("Rename")
            .css_classes(["suggested-action"])
            .build();
        let rename_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .margin_start(4)
            .margin_end(4)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        rename_box.append(&profile_rename_entry);
        rename_box.append(&rename_apply);
        let rename_popover = gtk::Popover::builder().child(&rename_box).build();
        let profile_rename_btn = gtk::MenuButton::builder().popover(&rename_popover).build();

        {
            let entry_ref = model.profile_rename_entry.clone();
            let sender = sender.input_sender().clone();
            let popover = rename_popover.clone();
            rename_apply.connect_clicked(move |_| {
                let new_name = entry_ref.text().to_string();
                if !new_name.is_empty() {
                    sender.send(AppMsg::RenameProfile(new_name)).unwrap();
                }
                popover.popdown();
            });
        }

        // Store sender globally so the command-line signal handler can forward NXM links
        let _ = crate::NXM_SENDER.set(sender.input_sender().clone());

        // Search bar with entry and scope dropdown
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search mods...")
            .hexpand(true)
            .build();
        let scope_dropdown =
            gtk::DropDown::from_strings(&["All", "Mod Order", "Plugin Order", "Downloads"]);
        scope_dropdown.set_selected(0);
        let search_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .margin_start(8)
            .margin_end(8)
            .margin_top(4)
            .margin_bottom(4)
            .build();
        search_box.append(&search_entry);
        search_box.append(&scope_dropdown);
        let search_bar = gtk::SearchBar::builder()
            .child(&search_box)
            .show_close_button(true)
            .build();
        search_bar.connect_entry(&search_entry);

        {
            let sender = sender.input_sender().clone();
            search_entry.connect_search_changed(move |entry| {
                sender
                    .send(AppMsg::SearchChanged(entry.text().to_string()))
                    .unwrap();
            });
        }
        {
            let sender = sender.input_sender().clone();
            scope_dropdown.connect_selected_notify(move |dd| {
                sender
                    .send(AppMsg::SearchScopeChanged(dd.selected()))
                    .unwrap();
            });
        }

        let toast_overlay = model.toaster.overlay_widget();
        let mod_list = model.mods.widget();
        let plugin_list = model.plugins.widget();
        let download_list = model.downloads.widget();
        let profile_dropdown = &model.profile_dropdown;
        let game_dropdown = &model.game_dropdown;
        let tool_buttons_box = &model.tool_buttons_box;
        let mod_scroll = &model.mod_scroll;
        let downloads_scroll = &model.downloads_scroll;
        let save_mode_btn = &model.save_mode_btn;
        let sync_saves_btn = &model.sync_saves_btn;
        let update_banner = &model.update_banner;
        let widgets = view_output!();

        // NOTE: do NOT set_key_capture_widget(root) here — that routes every window
        // keystroke into the search entry, causing the bar to flicker open/closed
        // whenever the user types while some other widget has focus.

        // Attach DropTarget for mod list drag-and-drop reordering
        let mod_drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
        let mod_sender = sender.input_sender().clone();
        mod_drop.connect_drop(move |target, value, _x, y| {
            let Some(widget) = target.widget() else {
                return false;
            };
            let list_box = widget.downcast::<gtk::ListBox>().unwrap();
            clear_drop_indicators(&list_box);
            let Ok(data) = value.get::<String>() else {
                return false;
            };
            // A group separator was dragged — just reposition it.
            if let Some(from) = data
                .strip_prefix("group:")
                .and_then(|s| s.parse::<usize>().ok())
            {
                if let Some(row) = list_box.row_at_y(y as i32) {
                    let to = gtk::prelude::ListBoxRowExt::index(&row) as usize;
                    if from != to {
                        mod_sender.send(AppMsg::MoveGroupTo(from, to)).unwrap();
                    }
                }
                return true;
            }
            let Some(from) = data
                .strip_prefix("mod:")
                .and_then(|s| s.parse::<usize>().ok())
            else {
                return false;
            };
            if let Some(row) = list_box.row_at_y(y as i32) {
                let to = gtk::prelude::ListBoxRowExt::index(&row) as usize;
                let mut selected: Vec<usize> = list_box
                    .selected_rows()
                    .iter()
                    .map(|r| gtk::prelude::ListBoxRowExt::index(r) as usize)
                    .collect();
                list_box.unselect_all();
                if selected.contains(&from) && selected.len() > 1 {
                    selected.sort_unstable();
                    mod_sender
                        .send(AppMsg::MoveSelectedModsTo { selected, from, to })
                        .unwrap();
                } else if from != to {
                    mod_sender.send(AppMsg::MoveModTo(from, to)).unwrap();
                }
            }
            true
        });
        mod_drop.connect_motion(|target, _x, y| {
            if let Some(widget) = target.widget() {
                let list_box = widget.downcast::<gtk::ListBox>().unwrap();
                update_drop_indicator(&list_box, y);
            }
            gtk::gdk::DragAction::MOVE
        });
        mod_drop.connect_leave(|target| {
            if let Some(widget) = target.widget() {
                let list_box = widget.downcast::<gtk::ListBox>().unwrap();
                clear_drop_indicators(&list_box);
            }
        });
        mod_list.add_controller(mod_drop);

        // Attach DropTarget for plugin list drag-and-drop reordering
        let plugin_drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
        let plugin_sender = sender.input_sender().clone();
        plugin_drop.connect_drop(move |target, value, _x, y| {
            let Some(widget) = target.widget() else {
                return false;
            };
            let list_box = widget.downcast::<gtk::ListBox>().unwrap();
            clear_drop_indicators(&list_box);
            let Ok(data) = value.get::<String>() else {
                return false;
            };
            let Some(from) = data
                .strip_prefix("plugin:")
                .and_then(|s| s.parse::<usize>().ok())
            else {
                return false;
            };
            if let Some(row) = list_box.row_at_y(y as i32) {
                let to = gtk::prelude::ListBoxRowExt::index(&row) as usize;
                let mut selected: Vec<usize> = list_box
                    .selected_rows()
                    .iter()
                    .map(|r| gtk::prelude::ListBoxRowExt::index(r) as usize)
                    .collect();
                list_box.unselect_all();
                if selected.contains(&from) && selected.len() > 1 {
                    selected.sort_unstable();
                    plugin_sender
                        .send(AppMsg::MoveSelectedPluginsTo { selected, from, to })
                        .unwrap();
                } else if from != to {
                    plugin_sender.send(AppMsg::MovePluginTo(from, to)).unwrap();
                }
            }
            true
        });
        plugin_drop.connect_motion(|target, _x, y| {
            if let Some(widget) = target.widget() {
                let list_box = widget.downcast::<gtk::ListBox>().unwrap();
                update_drop_indicator(&list_box, y);
            }
            gtk::gdk::DragAction::MOVE
        });
        plugin_drop.connect_leave(|target| {
            if let Some(widget) = target.widget() {
                let list_box = widget.downcast::<gtk::ListBox>().unwrap();
                clear_drop_indicators(&list_box);
            }
        });
        plugin_list.add_controller(plugin_drop);

        // Kick off async DB initialization
        sender.oneshot_command(async move {
            let init = async {
                let db_path = paths::db_path().map_err(|e| e.to_string())?;
                if let Some(parent) = db_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
                let tracker = Tracker::open(&db_url).await.map_err(|e| e.to_string())?;

                // Determine which game to select: prefer last_game_id from settings
                let last_game_id = tracker.get_setting("last_game_id").await.ok().flatten();
                let selected_game_idx = last_game_id
                    .as_ref()
                    .and_then(|id| game_ids.iter().position(|g| g == id))
                    .unwrap_or(0);

                let (
                    mods,
                    plugins,
                    plugin_masters,
                    overrides,
                    profiles,
                    active_idx,
                    tools,
                    vanilla_plugins,
                    groups,
                ) = if let Some(game) = games_for_init.get(selected_game_idx) {
                    // Ensure a default profile exists
                    tracker
                        .ensure_default_profile(&game.id)
                        .await
                        .map_err(|e| e.to_string())?;
                    let loaded = load_game_data(&tracker, game, false).await?;
                    (
                        loaded.mods,
                        loaded.plugins,
                        loaded.plugin_masters,
                        loaded.overrides,
                        loaded.profiles,
                        loaded.active_profile_idx,
                        loaded.tools,
                        loaded.vanilla_plugins,
                        loaded.groups,
                    )
                } else {
                    (
                        vec![],
                        vec![],
                        Default::default(),
                        Default::default(),
                        vec![],
                        0,
                        vec![],
                        HashSet::new(),
                        vec![],
                    )
                };

                let downloads_dir = tracker
                    .get_setting("downloads_dir")
                    .await
                    .ok()
                    .flatten()
                    .map(PathBuf::from);

                let download_entries = tracker
                    .load_download_entries()
                    .await
                    .map_err(|e| e.to_string())?;

                // Fetch fresh rate limits from the API; fall back to DB-cached values
                let rate_limit_info = if let Some(api_key) = tracker
                    .get_setting("nexus_api_key")
                    .await
                    .ok()
                    .flatten()
                    .filter(|k| !k.is_empty())
                {
                    let client = crate::core::nexus_api::NexusClient::new(api_key);
                    match client.validate_key().await {
                        Ok((_, Some(rl))) => Some(rl),
                        _ => tracker.load_rate_limits().await.unwrap_or(None),
                    }
                } else {
                    tracker.load_rate_limits().await.unwrap_or(None)
                };

                let persisted_games = tracker.load_persisted_games().await.unwrap_or_default();
                let hidden_game_ids = tracker.load_hidden_game_ids().await.unwrap_or_default();

                let last_deployed_profile_id = if let Some(game) =
                    games_for_init.get(selected_game_idx)
                {
                    tracker
                        .get_setting(&format!("last_deployed_profile_{}", game.id))
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                };

                Ok::<_, String>(InitData {
                    tracker,
                    mods,
                    plugins,
                    plugin_masters,
                    overrides,
                    profiles,
                    active_profile_idx: active_idx,
                    tools,
                    selected_game_idx,
                    downloads_dir,
                    download_entries,
                    rate_limit_info,
                    vanilla_plugins,
                    groups,
                    persisted_games,
                    hidden_game_ids,
                    last_deployed_profile_id,
                })
            };
            AppCmdMsg::Initialized(init.await)
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            AppMsg::Noop => {}
            AppMsg::GameSelected(idx) => self.handle_game_selected(idx, &sender),
            AppMsg::InstallClicked => self.handle_install_clicked(root, &sender),
            AppMsg::FileChosen(path) => self.handle_file_chosen(path, &sender),
            AppMsg::PreInstallConfirmed(name, targets) => {
                self.handle_pre_install_confirmed(name, targets, root, &sender)
            }
            AppMsg::PreInstallCancelled => self.handle_pre_install_cancelled(),
            AppMsg::FomodConfirmed(selections) => self.handle_fomod_confirmed(selections, &sender),
            AppMsg::FomodCancelled => self.handle_fomod_cancelled(),
            AppMsg::RemoveMod(idx) => self.handle_remove_mod(idx, &sender),
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
            AppMsg::NewProfileClicked => self.handle_new_profile_clicked(&sender),
            AppMsg::CloneProfileClicked => self.handle_clone_profile_clicked(&sender),
            AppMsg::RenameProfile(name) => self.handle_rename_profile(name, &sender),
            AppMsg::DeleteProfileClicked => self.handle_delete_profile_clicked(&sender),
            AppMsg::DeployClicked => self.handle_deploy_clicked(root, &sender),
            AppMsg::DeployConfirmed => self.execute_deploy(&sender),
            AppMsg::PurgeClicked => self.handle_purge_clicked(root, &sender),
            AppMsg::PurgeConfirmed => self.handle_purge_confirmed(&sender),
            AppMsg::GrantGameFolderAccess => self.handle_grant_game_folder_access(root, &sender),
            AppMsg::GameFolderGranted(path) => self.handle_game_folder_granted(path, &sender),
            AppMsg::LaunchTool(name) => self.handle_launch_tool(name, &sender),
            AppMsg::ToolExited(name) => self.handle_tool_exited(name, &sender),
            AppMsg::ManageToolsClicked => self.handle_manage_tools_clicked(root, &sender),
            AppMsg::ToolAdded(tool) => self.handle_tool_added(tool, &sender),
            AppMsg::ToolRemoved(name) => self.handle_tool_removed(name, &sender),
            AppMsg::ToolWorkingDirChanged(name, dir) => {
                self.handle_tool_working_dir_changed(name, dir, &sender)
            }
            AppMsg::PreInstallMerge(mod_id) => self.handle_pre_install_merge(mod_id, &sender),
            AppMsg::PreInstallCreateNew => self.handle_pre_install_create_new(&sender),
            AppMsg::InstallProgress(frac, msg) => self.handle_install_progress(frac, msg),
            AppMsg::ToolManagerClosed => self.handle_tool_manager_closed(),
            AppMsg::SettingsClicked => self.handle_settings_clicked(root, &sender),
            AppMsg::SettingsClosed => self.handle_settings_closed(&sender),
            AppMsg::ManageGamesClicked => self.handle_manage_games_clicked(root, &sender),
            AppMsg::GamesConfigured(configs, hidden_ids) => {
                self.handle_games_configured(configs, hidden_ids, &sender)
            }
            AppMsg::RemoveGame(id) => self.handle_remove_game(id, &sender),
            AppMsg::RemoveCurrentGame => {
                if let Some(game) = self.games.get(self.selected_game_idx) {
                    let id = game.id.clone();
                    self.handle_remove_game(id, &sender);
                }
            }
            AppMsg::NexusApiKeyUpdated => self.handle_nexus_api_key_updated(),
            AppMsg::NxmLinkReceived(link) => self.handle_nxm_link_received(link, &sender),
            AppMsg::CheckUpdatesClicked => self.handle_check_updates(&sender),
            AppMsg::ToggleDownloads => self.handle_toggle_downloads(),
            AppMsg::InstallDownload(idx) => self.handle_install_download(idx, &sender),
            AppMsg::ClearDownloadMetadata(idx) => self.handle_clear_download_metadata(idx, &sender),
            AppMsg::RenameDownload(idx) => self.handle_rename_download(idx, root, &sender),
            AppMsg::ConfirmDownloadRename(id, name) => {
                self.handle_confirm_download_rename(id, name, &sender)
            }
            AppMsg::DownloadProgress(id, frac, msg) => self.handle_download_progress(id, frac, msg),
            AppMsg::DownloadNameResolved(id, name, domain, fname, is_primary) => {
                self.handle_download_name_resolved(id, name, domain, fname, is_primary, &sender)
            }
            AppMsg::FetchDownloadMetadata(idx) => self.handle_fetch_download_metadata(idx, &sender),
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
                install_target,
                file_targets,
            } => self.handle_mod_properties_applied(
                mod_id,
                mod_idx,
                name,
                install_target,
                file_targets,
            ),
            AppMsg::ModPropertiesCancelled => self.handle_mod_properties_cancelled(),
            AppMsg::ScanExternalFiles => self.handle_scan_external_files(&sender),
            AppMsg::AbsorbExternalFiles => self.handle_absorb_external_files(root, &sender),
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
            AppMsg::ExportProfileClicked => self.handle_export_profile_clicked(root, &sender),
            AppMsg::ImportProfileClicked => self.handle_import_profile_clicked(root, &sender),
            AppMsg::ImportProfileFileChosen(path) => {
                self.handle_import_profile_file_chosen(path, &sender)
            }
            AppMsg::ProfileExported(result) => self.handle_profile_exported(result),
            AppMsg::OpenPreInstallDialog => self.handle_open_pre_install_dialog(root, &sender),
            AppMsg::OpenPreInstallDialogReplacing(id, priority) => {
                self.handle_open_pre_install_dialog_replacing(id, priority, root, &sender)
            }
            AppMsg::SortWithLoot => self.handle_sort_with_loot(&sender),
            AppMsg::RescanGames => self.handle_rescan_games(&sender),
            AppMsg::EnableAllMods => self.handle_enable_all_mods(&sender),
            AppMsg::DisableAllMods => self.handle_disable_all_mods(&sender),
            AppMsg::EnableAllPlugins => self.handle_enable_all_plugins(&sender),
            AppMsg::DisableAllPlugins => self.handle_disable_all_plugins(&sender),
            AppMsg::ToggleShowVanillaPlugins => self.handle_toggle_show_vanilla_plugins(),
            AppMsg::ShowToast(msg) => self.handle_show_toast(msg),
            AppMsg::ToggleProfileSaveMode => self.handle_toggle_profile_save_mode(&sender),
            AppMsg::SyncSaves => self.handle_sync_saves(&sender),
            AppMsg::AppUpdateAvailable(version, url) => {
                self.update_banner
                    .set_title(&format!("Deployd {version} is available"));
                self.update_banner.set_revealed(true);
                self.update_url = Some(url);
            }
            AppMsg::OpenUpdatePage => {
                let url = self
                    .update_url
                    .as_deref()
                    .unwrap_or(crate::core::update_check::NEXUS_PAGE_URL);
                let _ = open::that(url);
            }
            AppMsg::SelfUpdateDownload => self.handle_self_update_download(&sender),
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
            AppCmdMsg::ModsLoaded(result) => self.handle_cmd_mods_loaded(result, &sender),
            AppCmdMsg::ModAdded(result, was_replace) => {
                self.handle_cmd_mod_added(result, was_replace, &sender)
            }
            AppCmdMsg::ModPrepared(result) => self.handle_cmd_mod_prepared(result, root, &sender),
            AppCmdMsg::ModRemoved(result, nexus_ids, mod_name, archive_hash) => {
                self.handle_cmd_mod_removed(result, nexus_ids, mod_name, archive_hash, &sender)
            }
            AppCmdMsg::DeployDone(result) => self.handle_cmd_deploy_done(result, &sender),
            AppCmdMsg::PurgeDone(result) => self.handle_cmd_purge_done(result),
            AppCmdMsg::PrioritySaved(result) => self.handle_cmd_priority_saved(result, &sender),
            AppCmdMsg::PluginOrderSaved(result) => self.handle_cmd_plugin_order_saved(result),
            AppCmdMsg::ProfileSwitched(result) => {
                self.handle_cmd_profile_switched(result, &sender)
            }
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
            AppCmdMsg::ModMerged(result) => self.handle_cmd_mod_merged(result, &sender),
            AppCmdMsg::NxmDownloadComplete(id, result) => {
                self.handle_cmd_nxm_download_complete(id, result, &sender)
            }
            AppCmdMsg::NexusMetadataFetched(result) => {
                self.handle_cmd_nexus_metadata_fetched(result, &sender)
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
            AppCmdMsg::ProfileImported(result) => {
                self.handle_cmd_profile_imported(result, &sender)
            }
            AppCmdMsg::EmptyModCreated(result) => {
                self.handle_cmd_empty_mod_created(result, &sender)
            }
            AppCmdMsg::ModFilesRescanned(result) => {
                self.handle_cmd_mod_files_rescanned(result, &sender)
            }
            #[cfg(feature = "loot")]
            AppCmdMsg::LootSortDone(result) => self.handle_cmd_loot_sort_done(result, &sender),
            AppCmdMsg::GamesRescanned(games) => self.handle_cmd_games_rescanned(games),
            AppCmdMsg::ModFilesLoaded(files) => self.handle_cmd_mod_files_loaded(files),
            AppCmdMsg::SaveModeToggled(result) => {
                self.handle_cmd_save_mode_toggled(result, &sender)
            }
            AppCmdMsg::SavesSynced(result) => self.handle_cmd_saves_synced(result),
            AppCmdMsg::LastDeployedProfileLoaded(id) => self.last_deployed_profile_id = id,
            AppCmdMsg::AppUpdateResult(result) => self.handle_cmd_app_update_result(result),
        }
    }
}
