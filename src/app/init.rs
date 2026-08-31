use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;

use crate::core::game;
use crate::core::tracker::Tracker;
use crate::models::download::{DownloadFilter, DownloadSort};
use crate::models::game::{Game, GameEngine};
use crate::ui::bottom_status::BottomStatus;
use crate::ui::download_row::{DownloadRow, DownloadRowOutput};
use crate::ui::header::{Header, HeaderInit, HeaderOutput, HeaderState};
use crate::ui::mod_list::ModListItemOutput;
use crate::ui::plugin_list::PluginRowOutput;
use crate::utils::paths;

use crate::ui::drag::{clear_drop_indicators, update_drop_indicator, wire_deselect};

use super::session::{GameLoadMode, load_game_data};
use super::state::{
    DownloadState, ModState, PluginState, SessionState, ShellState, ToolState, UiState,
};
use super::types::{InitData, ModFilter, PostLootAction, SearchScope};
use super::{App, AppCmdMsg, AppMsg};

/// Builds the initial App model, wires up factory lists, constructs UI helpers
/// (profile rename popover, search bar), and registers the global NXM sender.
/// Returns the model, game identifiers used during initialization, and the
/// search bar needed as a `#[local_ref]` value by `view_output!()`.
pub(super) fn build_model(
    nxm_link: Option<String>,
    sender: &ComponentSender<App>,
) -> (App, Vec<String>, Vec<Game>, gtk::SearchBar) {
    let mods =
        FactoryVecDeque::builder()
            .launch_default()
            .forward(sender.input_sender(), |output| match output {
                ModListItemOutput::Reinstall(index) => {
                    AppMsg::Mods(crate::app::messages::ModsMsg::ReinstallMod(index))
                }
                ModListItemOutput::OpenProperties(index) => {
                    AppMsg::Mods(crate::app::messages::ModsMsg::OpenModProperties(index))
                }
                ModListItemOutput::ToggleGroupCollapse(index) => {
                    AppMsg::Mods(crate::app::messages::ModsMsg::ToggleGroupCollapse(index))
                }
                ModListItemOutput::DeleteGroup(index) => {
                    AppMsg::Mods(crate::app::messages::ModsMsg::DeleteGroup(index))
                }
                ModListItemOutput::RenameGroup(index, name) => {
                    AppMsg::Mods(crate::app::messages::ModsMsg::RenameGroup(index, name))
                }
                ModListItemOutput::SetGroupColor(index, color) => {
                    AppMsg::Mods(crate::app::messages::ModsMsg::SetGroupColor(index, color))
                }
                ModListItemOutput::SetSelected(index, selected) => AppMsg::Mods(
                    crate::app::messages::ModsMsg::SetModRowSelected(index, selected),
                ),
            });

    let plugins =
        FactoryVecDeque::builder()
            .launch_default()
            .forward(sender.input_sender(), |output| match output {
                PluginRowOutput::SetSelected(index, selected) => AppMsg::Plugins(
                    crate::app::messages::PluginsMsg::SetPluginRowSelected(index, selected),
                ),
            });

    let downloads: FactoryVecDeque<DownloadRow> = FactoryVecDeque::builder()
        .launch_default()
        .forward(sender.input_sender(), |output| match output {
            DownloadRowOutput::Install(index) => {
                AppMsg::Downloads(crate::app::messages::DownloadsMsg::InstallDownload(index))
            }
            DownloadRowOutput::Reinstall(index) => {
                AppMsg::Downloads(crate::app::messages::DownloadsMsg::ReinstallDownload(index))
            }
            DownloadRowOutput::FetchMetadata(index) => AppMsg::Downloads(
                crate::app::messages::DownloadsMsg::FetchDownloadMetadata(index),
            ),
            DownloadRowOutput::ClearMetadata(index) => AppMsg::Downloads(
                crate::app::messages::DownloadsMsg::ClearDownloadMetadata(index),
            ),
            DownloadRowOutput::Rename(index) => {
                AppMsg::Downloads(crate::app::messages::DownloadsMsg::RenameDownload(index))
            }
            DownloadRowOutput::Pause(index) => {
                AppMsg::Downloads(crate::app::messages::DownloadsMsg::PauseDownload(index))
            }
            DownloadRowOutput::Resume(index) => {
                AppMsg::Downloads(crate::app::messages::DownloadsMsg::ResumeDownload(index))
            }
            DownloadRowOutput::Delete(index) => {
                AppMsg::Downloads(crate::app::messages::DownloadsMsg::DeleteDownload(index))
            }
            DownloadRowOutput::HideDownload(index) => {
                AppMsg::Downloads(crate::app::messages::DownloadsMsg::HideDownload(index))
            }
        });

    // Games start empty; they are populated from the DB (persisted games) or via
    // the welcome wizard / Manage Games dialog.
    let games: Vec<Game> = game::detect_games();

    // Build dropdown model (StringList allows dynamic updates after wizard completes)
    let game_names: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
    let game_model = gtk::StringList::new(&game_names);
    let game_dropdown = gtk::DropDown::new(Some(game_model.clone()), None::<gtk::Expression>);

    // Constrain the dropdown button label so it never widens the window
    let game_factory = gtk::SignalListItemFactory::new();
    game_factory.connect_setup(|_, item| {
        let label = gtk::Label::builder()
            .max_width_chars(25)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .xalign(0.0_f32)
            .build();
        if let Some(list_item) = item.downcast_ref::<gtk::ListItem>() {
            list_item.set_child(Some(&label));
        }
    });
    game_factory.connect_bind(|_, item| {
        let Some(list_item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(s) = list_item.item().and_downcast::<gtk::StringObject>()
            && let Some(lbl) = list_item.child().and_downcast::<gtk::Label>()
        {
            lbl.set_text(&s.string());
        }
    });
    game_dropdown.set_factory(Some(&game_factory));

    // Popup list shows full titles without truncation
    let game_list_factory = gtk::SignalListItemFactory::new();
    game_list_factory.connect_setup(|_, item| {
        let label = gtk::Label::builder().xalign(0.0_f32).build();
        if let Some(list_item) = item.downcast_ref::<gtk::ListItem>() {
            list_item.set_child(Some(&label));
        }
    });
    game_list_factory.connect_bind(|_, item| {
        let Some(list_item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        if let Some(s) = list_item.item().and_downcast::<gtk::StringObject>()
            && let Some(lbl) = list_item.child().and_downcast::<gtk::Label>()
        {
            lbl.set_text(&s.string());
        }
    });
    game_dropdown.set_list_factory(Some(&game_list_factory));

    let profile_model = gtk::StringList::new(&[]);
    let profile_dropdown = gtk::DropDown::new(Some(profile_model.clone()), None::<gtk::Expression>);

    let game_ids: Vec<String> = games.iter().map(|g| g.id.clone()).collect();
    let games_for_init = games.clone();

    let tool_buttons_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    let profile_rename_entry = gtk::Entry::builder().hexpand(true).build();
    let mod_scroll = gtk::ScrolledWindow::new();
    let plugin_scroll = gtk::ScrolledWindow::new();
    let downloads_scroll = gtk::ScrolledWindow::new();
    let download_list: gtk::ListBox = downloads.widget().clone();

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

    let notification_list = gtk::ListBox::new();
    let deploy_options_btn = gtk::MenuButton::new();
    let notifications_menu_btn = gtk::MenuButton::new();
    let overflow_menu_btn = gtk::MenuButton::new();
    let profile_menu_btn = gtk::MenuButton::new();
    let save_mode_btn = gtk::Button::new();
    let sync_saves_btn = gtk::Button::new();
    let nexus_user_btn = gtk::MenuButton::new();
    let nexus_avatar_widget = adw::Avatar::new(24, None, true);

    let header = Header::builder()
        .launch(HeaderInit {
            state: HeaderState {
                nexus_username: None,
                nexus_is_premium: false,
                has_games: !games.is_empty(),
                initializing: true,
                profile_count: 0,
                save_mode_label: "Saves: Global".to_string(),
                game_has_save_management: false,
                can_sync_saves: false,
                is_busy: false,
                busy_message: "Working...".to_string(),
                deploying: false,
                needs_deploy: false,
                notification_count: 0,
                notification_badge: String::new(),
                external_changes_count: 0,
                app_update_version: None,
                running_as_appimage: std::env::var("APPIMAGE").is_ok(),
                global_active_count: 0,
                downloads_visible: false,
                search_active: false,
            },
            nexus_user_btn: nexus_user_btn.clone(),
            nexus_avatar_widget: nexus_avatar_widget.clone(),
            game_dropdown: game_dropdown.clone(),
            profile_dropdown: profile_dropdown.clone(),
            profile_menu_btn: profile_menu_btn.clone(),
            profile_rename_btn: profile_rename_btn.clone(),
            save_mode_btn,
            sync_saves_btn,
            deploy_options_btn: deploy_options_btn.clone(),
            overflow_menu_btn: overflow_menu_btn.clone(),
            notifications_menu_btn: notifications_menu_btn.clone(),
            notification_list: notification_list.clone(),
            tool_buttons_box: tool_buttons_box.clone(),
        })
        .forward(sender.input_sender(), |output| match output {
            HeaderOutput::NexusLogoutClicked => {
                AppMsg::Shell(crate::app::messages::ShellMsg::NexusLogoutClicked)
            }
            HeaderOutput::NexusLoginClicked => {
                AppMsg::Shell(crate::app::messages::ShellMsg::NexusLoginClicked)
            }
            HeaderOutput::GameSelected(index) => {
                AppMsg::Games(crate::app::messages::GamesMsg::GameSelected(index))
            }
            HeaderOutput::RemoveCurrentGame => {
                AppMsg::Games(crate::app::messages::GamesMsg::RemoveCurrentGame)
            }
            HeaderOutput::ProfileSelected(index) => {
                AppMsg::Games(crate::app::messages::GamesMsg::ProfileSelected(index))
            }
            HeaderOutput::NewProfileClicked => {
                AppMsg::Games(crate::app::messages::GamesMsg::NewProfileClicked)
            }
            HeaderOutput::CloneProfileClicked => {
                AppMsg::Games(crate::app::messages::GamesMsg::CloneProfileClicked)
            }
            HeaderOutput::DeleteProfileClicked => {
                AppMsg::Games(crate::app::messages::GamesMsg::DeleteProfileClicked)
            }
            HeaderOutput::ToggleProfileSaveMode => {
                AppMsg::Games(crate::app::messages::GamesMsg::ToggleProfileSaveMode)
            }
            HeaderOutput::SyncSaves => AppMsg::Games(crate::app::messages::GamesMsg::SyncSaves),
            HeaderOutput::ManageSaveBackups => {
                AppMsg::Games(crate::app::messages::GamesMsg::ManageSaveBackups)
            }
            HeaderOutput::DeployClicked => {
                AppMsg::Shell(crate::app::messages::ShellMsg::DeployClicked)
            }
            HeaderOutput::OpenDeploymentFolder => {
                AppMsg::Shell(crate::app::messages::ShellMsg::OpenDeploymentFolder)
            }
            HeaderOutput::PurgeClicked => {
                AppMsg::Shell(crate::app::messages::ShellMsg::PurgeClicked)
            }
            HeaderOutput::CreateEmptyMod => {
                AppMsg::Mods(crate::app::messages::ModsMsg::CreateEmptyMod)
            }
            HeaderOutput::ResetVanillaBaseline => {
                AppMsg::Plugins(crate::app::messages::PluginsMsg::ResetVanillaBaseline)
            }
            HeaderOutput::ManageToolsClicked => {
                AppMsg::Tools(crate::app::messages::ToolsMsg::ManageToolsClicked)
            }
            HeaderOutput::SettingsClicked => {
                AppMsg::Games(crate::app::messages::GamesMsg::SettingsClicked)
            }
            HeaderOutput::AbsorbExternalFiles => {
                AppMsg::Mods(crate::app::messages::ModsMsg::AbsorbExternalFiles)
            }
            HeaderOutput::SelfUpdateDownload => {
                AppMsg::Shell(crate::app::messages::ShellMsg::SelfUpdateDownload)
            }
            HeaderOutput::ClearNotifications => {
                AppMsg::Shell(crate::app::messages::ShellMsg::ClearNotifications)
            }
            HeaderOutput::SetDownloadsVisible(visible) => AppMsg::Downloads(
                crate::app::messages::DownloadsMsg::SetDownloadsVisible(visible),
            ),
            HeaderOutput::SearchToggled(active) => {
                AppMsg::Shell(crate::app::messages::ShellMsg::SearchToggled(active))
            }
        });
    let downloads_pane =
        super::downloads::pane::launch(downloads_scroll.clone(), download_list, sender);
    let bottom_status = BottomStatus::builder()
        .launch(crate::ui::bottom_status::BottomStatusState {
            initializing: true,
            mod_status: "0 of 0 mods".to_string(),
            plugin_status: "0 of 0 plugins".to_string(),
            conflict_status: "0 conflicts".to_string(),
            has_conflicts: false,
            rate_limit_status: String::new(),
            rate_limit_visible: false,
            rate_limit_warning: false,
            needs_deploy: false,
            has_games: !games.is_empty(),
        })
        .detach();

    let model = App {
        shell: ShellState {
            deploying: false,
            needs_deploy: false,
            status_msg: None,
            work_status: None,
            search_active: false,
            search_text: String::new(),
            pending_search_text: None,
            search_debounce: None,
            search_scope: SearchScope::All,
            nexus_username: None,
            nexus_avatar_url: None,
            nexus_is_premium: false,
            app_update_version: None,
            app_update_url: None,
            running_as_appimage: std::env::var("APPIMAGE").is_ok(),
            color_scheme_idx: 0,
        },
        session: SessionState {
            initializing: true,
            tracker: None,
            games,
            selected_game_idx: 0,
            profiles: vec![],
            active_profile_idx: 0,
            updating_profiles: false,
            pending_save_profile_idx: None,
            game_cache_dirs: HashMap::new(),
            pending_new_game_ids: vec![],
            last_deployed_profile_id: None,
        },
        mods: ModState {
            rows: mods,
            collapsed_groups: HashSet::new(),
            selection_active: false,
            selection_dirty: false,
            selected: HashSet::new(),
            filter: ModFilter::All,
            pending_external_files: Vec::new(),
            external_changes_count: 0,
            pending_scroll_restore: None,
            scroll: mod_scroll,
            snapshots: Vec::new(),
            snapshot_save_entry: gtk::Entry::new(),
            snapshots_list: gtk::ListBox::new(),
        },
        plugins: PluginState {
            rows: plugins,
            selection_active: false,
            selection_dirty: false,
            selected: HashSet::new(),
            scroll: plugin_scroll,
            #[cfg(feature = "loot")]
            dirty: HashMap::new(),
            pending_post_loot_action: PostLootAction::None,
            show_vanilla: false,
            managed_count: 0,
            vanilla_names: Vec::new(),
            vanilla_derived: HashSet::new(),
            masters: HashMap::new(),
            snapshots: Vec::new(),
            snapshot_save_entry: gtk::Entry::new(),
            snapshots_list: gtk::ListBox::new(),
        },
        install: Default::default(),
        tools: ToolState {
            entries: vec![],
            launch_cancel: None,
            launch_session: None,
            proton_setup: false,
        },
        ui: UiState {
            header,
            bottom_status,
            toast_overlay: adw::ToastOverlay::new(),
            notification_sender: sender.input_sender().clone(),
            notification_list,
            notification_count: 0,
            profile_model,
            profile_dropdown,
            game_model,
            game_dropdown,
            pre_install_dialog: None,
            fomod_dialog: None,
            downloads_pane,
            tool_buttons_box,
            tool_manager_dialog: None,
            tool_launch_dialog: None,
            game_setup_dialog: None,
            welcome_wizard: None,
            settings_dialog: None,
            pending_migration_import: None,
            mod_properties_dialog: None,
            absorb_dialog: None,
            profile_rename_entry: profile_rename_entry.clone(),
            deploy_options_btn,
            notifications_menu_btn,
            overflow_menu_btn,
            profile_menu_btn,
            nexus_user_btn,
            nexus_avatar_widget,
        },
        download: DownloadState {
            rows: downloads,
            all: Vec::new(),
            visible: false,
            metadata_previous_status: HashMap::new(),
            active_count: 0,
            global_active_count: 0,
            directory: paths::default_downloads_dir(),
            initial_scan_done: false,
            sort: DownloadSort::Default,
            filter: DownloadFilter::All,
            show_hidden: false,
            scroll: downloads_scroll,
            rate_limit: None,
            pending_nxm: nxm_link,
        },
    };

    {
        let entry_ref = model.ui.profile_rename_entry.clone();
        let sender = sender.input_sender().clone();
        let popover = rename_popover.clone();
        let profile_menu_btn = model.ui.profile_menu_btn.clone();
        {
            let entry_ref = entry_ref.clone();
            let sender = sender.clone();
            let popover = popover.clone();
            let profile_menu_btn = profile_menu_btn.clone();
            rename_apply.connect_clicked(move |_| {
                let new_name = entry_ref.text().to_string();
                if !new_name.is_empty() {
                    let _ = sender.send(AppMsg::Games(
                        crate::app::messages::GamesMsg::RenameProfile(new_name),
                    ));
                }
                popover.popdown();
                profile_menu_btn.popdown();
            });
        }
        entry_ref.connect_activate(move |e| {
            let new_name = e.text().to_string();
            if !new_name.is_empty() {
                let _ = sender.send(AppMsg::Games(
                    crate::app::messages::GamesMsg::RenameProfile(new_name),
                ));
            }
            popover.popdown();
            profile_menu_btn.popdown();
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
            let _ = sender.send(AppMsg::Shell(
                crate::app::messages::ShellMsg::SearchChanged(entry.text().to_string()),
            ));
        });
    }
    {
        let sender = sender.input_sender().clone();
        scope_dropdown.connect_selected_notify(move |dd| {
            let _ = sender.send(AppMsg::Shell(
                crate::app::messages::ShellMsg::SearchScopeChanged(dd.selected()),
            ));
        });
    }

    (model, game_ids, games_for_init, search_bar)
}

/// Returns the insertion index for a drag-drop using half-row precision: cursor in the
/// bottom half of a row inserts *after* it; top half inserts *before* it.
fn half_row_index(row: &gtk::ListBoxRow, y: f64, list_len: usize) -> usize {
    use gtk::prelude::ListBoxRowExt;
    let alloc = row.allocation();
    let mid = alloc.y() + alloc.height() / 2;
    let idx = row.index() as usize;
    if (y as i32) >= mid {
        (idx + 1).min(list_len)
    } else {
        idx
    }
}

/// Attaches drag-and-drop `DropTarget` controllers to the mod and plugin list widgets.
pub(super) fn wire_drag_drop(
    sender: &ComponentSender<App>,
    mod_list: &gtk::ListBox,
    plugin_list: &gtk::ListBox,
    mod_scroll: &gtk::ScrolledWindow,
) {
    // Shared timeout handle for drag-scroll. None = not scrolling.
    let scroll_source: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));

    let mod_drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    let mod_sender = sender.input_sender().clone();
    let scroll_drop = scroll_source.clone();
    mod_drop.connect_drop(move |target, value, _x, y| {
        if let Some(id) = scroll_drop.borrow_mut().take() {
            id.remove();
        }
        let Some(widget) = target.widget() else {
            return false;
        };
        let Ok(list_box) = widget.downcast::<gtk::ListBox>() else {
            return false;
        };
        clear_drop_indicators(&list_box);
        let Ok(data) = value.get::<String>() else {
            return false;
        };
        let len = list_box.observe_children().n_items() as usize;
        // Resolve the target row; if the cursor is past the last row treat it as end-of-list.
        let row_at_y = list_box.row_at_y(y as i32).or_else(|| {
            len.checked_sub(1)
                .and_then(|i| list_box.row_at_index(i as i32))
        });
        // A group separator was dragged — just reposition it.
        if let Some(from) = data
            .strip_prefix("group:")
            .and_then(|s| s.parse::<usize>().ok())
        {
            if let Some(row) = row_at_y {
                let to = half_row_index(&row, y, len);
                if from != to {
                    let _ = mod_sender.send(AppMsg::Mods(
                        crate::app::messages::ModsMsg::MoveGroupTo(from, to),
                    ));
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
        if let Some(row) = row_at_y {
            let to = half_row_index(&row, y, len);
            let mut selected: Vec<usize> = list_box
                .selected_rows()
                .iter()
                .map(|r| gtk::prelude::ListBoxRowExt::index(r) as usize)
                .collect();
            list_box.unselect_all();
            if selected.contains(&from) && selected.len() > 1 {
                selected.sort_unstable();
                let _ = mod_sender.send(AppMsg::Mods(
                    crate::app::messages::ModsMsg::MoveSelectedModsTo { selected, from, to },
                ));
            } else if from != to {
                let _ = mod_sender.send(AppMsg::Mods(crate::app::messages::ModsMsg::MoveModTo(
                    from, to,
                )));
            }
        }
        true
    });
    let scroll_motion = scroll_source.clone();
    let vadj_motion = mod_scroll.vadjustment();
    mod_drop.connect_motion(move |target, _x, y| {
        if let Some(widget) = target.widget() {
            let Ok(list_box) = widget.clone().downcast::<gtk::ListBox>() else {
                return gtk::gdk::DragAction::MOVE;
            };
            update_drop_indicator(&list_box, y);

            const EDGE: f64 = 40.0;
            const MAX_STEP: f64 = 8.0;
            let height = widget.height() as f64;
            let delta = if y < EDGE {
                // Near top — scroll up (negative)
                -MAX_STEP * (1.0 - y / EDGE)
            } else if y > height - EDGE {
                // Near bottom — scroll down (positive)
                MAX_STEP * (1.0 - (height - y) / EDGE)
            } else {
                0.0
            };

            if delta.abs() < 0.5 {
                // Not near an edge — cancel any running scroll timeout
                if let Some(id) = scroll_motion.borrow_mut().take() {
                    id.remove();
                }
            } else if scroll_motion.borrow().is_none() {
                // Start a new repeating timeout that nudges the adjustment
                let vadj = vadj_motion.clone();
                let scroll_ref = scroll_motion.clone();
                let id = glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
                    let cur = vadj.value();
                    let next = (cur + delta).clamp(vadj.lower(), vadj.upper() - vadj.page_size());
                    vadj.set_value(next);
                    if scroll_ref.borrow().is_none() {
                        glib::ControlFlow::Break
                    } else {
                        glib::ControlFlow::Continue
                    }
                });
                *scroll_motion.borrow_mut() = Some(id);
            }
        }
        gtk::gdk::DragAction::MOVE
    });
    let scroll_leave = scroll_source.clone();
    mod_drop.connect_leave(move |target| {
        if let Some(id) = scroll_leave.borrow_mut().take() {
            id.remove();
        }
        if let Some(widget) = target.widget()
            && let Ok(list_box) = widget.downcast::<gtk::ListBox>()
        {
            clear_drop_indicators(&list_box);
        }
    });
    mod_list.add_controller(mod_drop);

    let plugin_drop = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
    let plugin_sender = sender.input_sender().clone();
    plugin_drop.connect_drop(move |target, value, _x, y| {
        let Some(widget) = target.widget() else {
            return false;
        };
        let Ok(list_box) = widget.downcast::<gtk::ListBox>() else {
            return false;
        };
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
            let len = list_box.observe_children().n_items() as usize;
            let to = half_row_index(&row, y, len);
            list_box.unselect_all();
            if from != to {
                let _ = plugin_sender.send(AppMsg::Plugins(
                    crate::app::messages::PluginsMsg::MovePluginTo(from, to),
                ));
            }
        }
        true
    });
    plugin_drop.connect_motion(|target, _x, y| {
        if let Some(widget) = target.widget()
            && let Ok(list_box) = widget.downcast::<gtk::ListBox>()
        {
            update_drop_indicator(&list_box, y);
        }
        gtk::gdk::DragAction::MOVE
    });
    plugin_drop.connect_leave(|target| {
        if let Some(widget) = target.widget()
            && let Ok(list_box) = widget.downcast::<gtk::ListBox>()
        {
            clear_drop_indicators(&list_box);
        }
    });
    plugin_list.add_controller(plugin_drop);

    wire_deselect(mod_list);
    wire_deselect(plugin_list);

    let sel_sender = sender.input_sender().clone();
    mod_list.connect_row_activated(move |_, row| {
        let _ = sel_sender.send(AppMsg::Mods(
            crate::app::messages::ModsMsg::ToggleModRowSelected(row.index() as usize),
        ));
    });

    let sel_sender = sender.input_sender().clone();
    plugin_list.connect_row_activated(move |_, row| {
        let _ = sel_sender.send(AppMsg::Plugins(
            crate::app::messages::PluginsMsg::TogglePluginRowSelected(row.index() as usize),
        ));
    });
}

/// Asynchronously opens the database, loads game data, and fetches initial
/// settings.  Returns an `AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::Initialized)` variant ready to dispatch.
pub(super) async fn load_init_data() -> AppCmdMsg {
    let init = async {
        let db_path = paths::db_path().map_err(|e| e.to_string())?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let pending = paths::pending_restore_path().map_err(|e| e.to_string())?;
        let marker = paths::post_restore_marker_path().map_err(|e| e.to_string())?;
        crate::core::restore::apply_staged_database_restore(&db_path, &pending, &marker)
            .map_err(|e| e.to_string())?;

        let db_url = format!("sqlite://{}?mode=rwc", db_path.display());
        let open_report = Tracker::open(&db_url).await.map_err(|e| e.to_string())?;
        let tracker = open_report.tracker;
        let mut startup_warnings = open_report.warnings;

        // Determine which game to select: prefer last_game_id from settings.
        // detect_games() returns empty, so load persisted games from DB to find
        // the game to initialise with.
        let last_game_id = super::startup::optional(
            tracker.get_setting("last_game_id").await,
            "Could not load the last selected game",
            &mut startup_warnings,
        )
        .flatten();
        let persisted_games = tracker
            .load_persisted_games()
            .await
            .map_err(|e| format!("Failed to load configured games: {e}"))?;

        let selected_persisted = last_game_id
            .as_deref()
            .and_then(|id| persisted_games.iter().find(|g| g.id == id))
            .or_else(|| persisted_games.first());

        let init_game_id = selected_persisted.map(|g| g.id.clone());

        let init_game: Option<Game> = selected_persisted.map(|p| Game {
            id: p.id.clone(),
            title: p.title.clone(),
            path: p.path.clone(),
            // Prefer the canonical value from KNOWN_GAMES so that updates to the
            // constant (e.g. Witcher 1 changing from "." to "Data") take effect
            // without requiring the user to re-add the game.
            data_subdir: game::known_data_subdir(&p.id)
                .map(|s| s.to_string())
                .unwrap_or_else(|| p.data_subdir.clone()),
            engine: match p.engine.as_str() {
                "redengine" => GameEngine::REDEngine,
                "eclipse" => GameEngine::Eclipse,
                "aurora" => GameEngine::Aurora,
                _ => GameEngine::Bethesda,
            },
            wine_prefix: p.wine_prefix.clone(),
        });

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
            vanilla_plugin_master_counts,
            vanilla_derived_plugins,
            access_warnings,
            plugin_scan_complete,
        ) = if let Some(game) = &init_game {
            let loaded = load_game_data(&tracker, game, GameLoadMode::OpenGame).await?;
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
                loaded.vanilla_plugin_master_counts,
                loaded.vanilla_derived_plugins,
                loaded.access_warnings,
                loaded.plugin_scan_complete,
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
                Default::default(),
                Default::default(),
                vec![],
                true,
            )
        };

        let downloads_dir = tracker
            .get_setting("downloads_dir")
            .await
            .map_err(|e| format!("Failed to load the downloads directory: {e}"))?
            .map(PathBuf::from);

        let game_cache_dirs = tracker
            .load_game_cache_dirs()
            .await
            .map_err(|e| format!("Failed to load game cache directories: {e}"))?;

        let download_entries = tracker
            .load_download_entries()
            .await
            .map_err(|e| e.to_string())?;

        // Fetch fresh rate limits and user info from the API; fall back to DB-cached values
        let (rate_limit_info, nexus_username, nexus_avatar_url, nexus_is_premium) =
            if let Some(api_key) = tracker
                .get_setting("nexus_api_key")
                .await
                .map_err(|e| format!("Failed to load the Nexus API key: {e}"))?
                .filter(|k| !k.is_empty())
            {
                let client = crate::core::nexus_api::NexusClient::new(api_key);
                match client.validate_key().await {
                    Ok((user, rl)) => {
                        if let Err(error) = tracker.save_nexus_user(&user).await {
                            startup_warnings
                                .push(format!("Could not cache Nexus user details: {error}"));
                        }
                        let premium = user.is_premium;
                        let avatar = user.profile_url.clone();
                        (rl, Some(user.name), avatar, premium)
                    }
                    _ => {
                        let rl = super::startup::optional(
                            tracker.load_rate_limits().await,
                            "Could not load cached Nexus rate limits",
                            &mut startup_warnings,
                        )
                        .flatten();
                        let (name, avatar) = super::startup::optional(
                            tracker.load_nexus_user().await,
                            "Could not load cached Nexus user details",
                            &mut startup_warnings,
                        )
                        .unwrap_or((None, None));
                        let premium = tracker
                            .get_setting("nexus_is_premium")
                            .await
                            .map_err(|e| format!("Failed to load Nexus account status: {e}"))?
                            .map(|v| v == "true")
                            .unwrap_or(false);
                        (rl, name, avatar, premium)
                    }
                }
            } else {
                let rl = super::startup::optional(
                    tracker.load_rate_limits().await,
                    "Could not load cached Nexus rate limits",
                    &mut startup_warnings,
                )
                .flatten();
                (rl, None, None, false)
            };

        let hidden_game_ids = tracker
            .load_hidden_game_ids()
            .await
            .map_err(|e| format!("Failed to load hidden games: {e}"))?;

        let wizard_shown = tracker
            .get_setting("welcome_wizard_shown")
            .await
            .map_err(|e| format!("Failed to load welcome state: {e}"))?
            .is_some();
        let first_launch = !wizard_shown && persisted_games.is_empty();

        let last_deployed_profile_id = if let Some(game) = &init_game {
            tracker
                .get_setting(&format!("last_deployed_profile_{}", game.id))
                .await
                .map_err(|e| format!("Failed to load deployed profile state: {e}"))?
        } else {
            None
        };

        let color_scheme_idx = tracker
            .get_setting("color_scheme")
            .await
            .map_err(|e| format!("Failed to load the color scheme: {e}"))?
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);

        // Consume the post-restore marker if present.
        let restore_marker = paths::post_restore_marker_path().map_err(|e| e.to_string())?;
        let (restored_from_backup, marker_warning) =
            crate::core::restore::consume_restore_marker(&restore_marker);
        if let Some(warning) = marker_warning {
            startup_warnings.push(warning);
        }

        startup_warnings.extend(access_warnings);

        Ok::<_, String>(InitData {
            tracker,
            mods,
            plugins,
            plugin_masters,
            overrides,
            profiles,
            active_profile_idx: active_idx,
            tools,
            init_game_id,
            downloads_dir,
            game_cache_dirs,
            download_entries,
            rate_limit_info,
            vanilla_plugins,
            groups,
            vanilla_plugin_master_counts,
            vanilla_derived_plugins,
            persisted_games,
            hidden_game_ids,
            last_deployed_profile_id,
            first_launch,
            nexus_username,
            nexus_avatar_url,
            nexus_is_premium,
            color_scheme_idx,
            restored_from_backup,
            access_warnings: startup_warnings,
            plugin_scan_complete,
        })
    };
    AppCmdMsg::Shell(crate::app::messages::ShellCmdMsg::Initialized(Box::new(
        init.await,
    )))
}
