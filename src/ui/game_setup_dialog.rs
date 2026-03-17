use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::app::messages::GameConfig;
use crate::core::game;
use crate::models::game::{Game, GameEngine};
use crate::core::tracker::PersistedGame;

/// One row in the games list: a confirmed or custom-added game.
#[derive(Debug, Clone)]
struct GameEntry {
    game: Game,
    /// Whether the user has included this game (checked).
    enabled: bool,
    /// `true` if manually added by the user (not auto-detected).
    custom: bool,
}

pub struct GameSetupDialog {
    entries: Vec<GameEntry>,
    /// Page switcher — "list" vs "add".
    stack: gtk::Stack,
    /// ListBox for auto-detected game rows.
    detected_list: gtk::ListBox,
    /// ListBox for custom game rows.
    custom_list: gtk::ListBox,
    /// Container for the custom list section (hidden when empty).
    custom_section: gtk::Box,
    /// State for the "Add Custom Game" form.
    new_game_type_idx: usize,
    new_path: Option<PathBuf>,
    new_prefix: Option<PathBuf>,
    new_path_entry: gtk::Entry,
    new_prefix_entry: gtk::Entry,
    add_btn: gtk::Button,
}

#[derive(Debug)]
pub enum GameSetupMsg {
    /// Toggle a game's enabled state by list index.
    ToggleEnabled(usize),
    /// Browse for a new game folder for entry at index.
    BrowsePath(usize),
    PathChosen(usize, PathBuf),
    /// Browse for a wine prefix override for entry at index.
    BrowsePrefix(usize),
    PrefixChosen(usize, PathBuf),
    /// Clear the wine prefix for entry at index.
    ClearPrefix(usize),
    /// Remove a custom game entry.
    RemoveCustom(usize),
    /// Switch to the "Add Custom Game" form.
    AddCustomClicked,
    /// Navigate back to the game list.
    BackClicked,
    /// The known-game-type dropdown selection changed.
    GameTypeSelected(u32),
    /// Browse for the new game's installation folder.
    BrowseNewPath,
    NewPathChosen(PathBuf),
    /// Browse for the new game's wine prefix (optional).
    BrowseNewPrefix,
    NewPrefixChosen(PathBuf),
    /// Clear the pending new game's wine prefix.
    ClearNewPrefix,
    /// Commit the pending "add" form.
    ConfirmAdd,
    /// User confirmed: emit the final game list.
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub enum GameSetupOutput {
    /// `enabled` — games to keep; `hidden_ids` — IDs of unchecked games to stop managing.
    Confirmed {
        enabled: Vec<GameConfig>,
        hidden_ids: Vec<String>,
    },
    Closed,
}

impl GameSetupDialog {
    /// Rebuild the detected-games list box from `self.entries` where `!custom`.
    fn rebuild_detected(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.detected_list.first_child() {
            self.detected_list.remove(&child);
        }

        let detected: Vec<(usize, &GameEntry)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.custom)
            .collect();

        if detected.is_empty() {
            let lbl = gtk::Label::new(Some("No games were detected automatically."));
            lbl.add_css_class("dim-label");
            lbl.set_margin_top(12);
            lbl.set_margin_bottom(12);
            self.detected_list.append(&lbl);
            return;
        }

        for (idx, entry) in detected {
            self.detected_list
                .append(&Self::build_entry_row(idx, entry, false, sender));
        }
    }

    /// Rebuild the custom-games list box from `self.entries` where `custom`.
    fn rebuild_custom(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.custom_list.first_child() {
            self.custom_list.remove(&child);
        }

        let custom: Vec<(usize, &GameEntry)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.custom)
            .collect();

        self.custom_section.set_visible(!custom.is_empty());

        for (idx, entry) in custom {
            self.custom_list
                .append(&Self::build_entry_row(idx, entry, true, sender));
        }
    }

    /// Build a single game row widget.
    fn build_entry_row(
        idx: usize,
        entry: &GameEntry,
        show_remove: bool,
        sender: &ComponentSender<Self>,
    ) -> adw::ExpanderRow {
        let row = adw::ExpanderRow::new();
        row.set_title(&entry.game.title);
        row.set_subtitle(&entry.game.path.to_string_lossy());

        // Enabled toggle as a check button prefix.
        let check = gtk::CheckButton::new();
        check.set_active(entry.enabled);
        check.set_valign(gtk::Align::Center);
        check.set_tooltip_text(Some("Include this game"));
        {
            let input = sender.input_sender().clone();
            check.connect_toggled(move |_| {
                input.send(GameSetupMsg::ToggleEnabled(idx)).ok();
            });
        }
        row.add_prefix(&check);

        // Remove button (custom games only).
        if show_remove {
            let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            remove_btn.set_valign(gtk::Align::Center);
            remove_btn.add_css_class("flat");
            remove_btn.set_tooltip_text(Some("Remove"));
            let input = sender.input_sender().clone();
            remove_btn.connect_clicked(move |_| {
                input.send(GameSetupMsg::RemoveCustom(idx)).ok();
            });
            row.add_suffix(&remove_btn);
        }

        // ── Expanded content ────────────────────────────────────────────────

        // Game folder row.
        let path_row = adw::ActionRow::new();
        path_row.set_title("Game Folder");
        path_row.set_subtitle(&entry.game.path.to_string_lossy());

        let path_btn = gtk::Button::from_icon_name("folder-symbolic");
        path_btn.set_valign(gtk::Align::Center);
        path_btn.add_css_class("flat");
        path_btn.set_tooltip_text(Some("Browse…"));
        {
            let input = sender.input_sender().clone();
            path_btn.connect_clicked(move |_| {
                input.send(GameSetupMsg::BrowsePath(idx)).ok();
            });
        }
        path_row.add_suffix(&path_btn);
        row.add_row(&path_row);

        // Wine prefix row.
        let prefix_row = adw::ActionRow::new();
        prefix_row.set_title("Wine Prefix");
        if let Some(ref pfx) = entry.game.wine_prefix {
            prefix_row.set_subtitle(&pfx.to_string_lossy());
        } else {
            prefix_row.set_subtitle("Auto-detect");
        }

        let prefix_btn = gtk::Button::from_icon_name("folder-symbolic");
        prefix_btn.set_valign(gtk::Align::Center);
        prefix_btn.add_css_class("flat");
        prefix_btn.set_tooltip_text(Some("Set custom prefix…"));
        {
            let input = sender.input_sender().clone();
            prefix_btn.connect_clicked(move |_| {
                input.send(GameSetupMsg::BrowsePrefix(idx)).ok();
            });
        }
        prefix_row.add_suffix(&prefix_btn);

        if entry.game.wine_prefix.is_some() {
            let clear_btn = gtk::Button::from_icon_name("edit-clear-symbolic");
            clear_btn.set_valign(gtk::Align::Center);
            clear_btn.add_css_class("flat");
            clear_btn.set_tooltip_text(Some("Clear (auto-detect)"));
            let input = sender.input_sender().clone();
            clear_btn.connect_clicked(move |_| {
                input.send(GameSetupMsg::ClearPrefix(idx)).ok();
            });
            prefix_row.add_suffix(&clear_btn);
        }
        row.add_row(&prefix_row);

        row
    }

    /// Update the sensitivity of the "Add Game" button on the add-form page.
    fn update_add_btn(&self) {
        self.add_btn.set_sensitive(self.new_path.is_some());
    }
}

#[relm4::component(pub)]
impl Component for GameSetupDialog {
    /// (auto-detected games, persisted custom games)
    type Init = (Vec<Game>, Vec<PersistedGame>);
    type Input = GameSetupMsg;
    type Output = GameSetupOutput;
    type CommandOutput = ();

    view! {
        adw::Window {
            set_title: Some("Manage Games"),
            set_default_size: (520, 480),
            set_modal: true,
            connect_close_request[sender] => move |_| {
                sender.input(GameSetupMsg::Cancel);
                glib::Propagation::Proceed
            },

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        set_title: "Manage Games",
                    },

                    pack_start = &gtk::Button {
                        set_label: "Back",
                        add_css_class: "flat",
                        #[watch]
                        set_visible: model.stack.visible_child_name().as_deref() == Some("add"),
                        connect_clicked => GameSetupMsg::BackClicked,
                    },

                    pack_end = &gtk::Button {
                        set_label: "OK",
                        add_css_class: "suggested-action",
                        #[watch]
                        set_visible: model.stack.visible_child_name().as_deref() == Some("list"),
                        connect_clicked => GameSetupMsg::Confirm,
                    },
                },

                #[local_ref]
                stack -> gtk::Stack {
                    set_vexpand: true,
                    set_transition_type: gtk::StackTransitionType::SlideLeftRight,

                    // ── List page ────────────────────────────────────────────
                    #[name = "list_page"]
                    gtk::ScrolledWindow {
                        set_hscrollbar_policy: gtk::PolicyType::Never,
                        set_vexpand: true,

                        adw::Clamp {
                            set_maximum_size: 540,
                            set_margin_all: 12,

                            gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_spacing: 12,

                                gtk::Label {
                                    set_label: "Detected Games",
                                    set_halign: gtk::Align::Start,
                                    add_css_class: "heading",
                                },

                                #[local_ref]
                                detected_list -> gtk::ListBox {
                                    add_css_class: "boxed-list",
                                    set_selection_mode: gtk::SelectionMode::None,
                                },

                                #[local_ref]
                                custom_section -> gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_spacing: 6,

                                    gtk::Label {
                                        set_label: "Custom Games",
                                        set_halign: gtk::Align::Start,
                                        add_css_class: "heading",
                                    },

                                    #[local_ref]
                                    custom_list -> gtk::ListBox {
                                        add_css_class: "boxed-list",
                                        set_selection_mode: gtk::SelectionMode::None,
                                    },
                                },

                                gtk::Button {
                                    set_label: "Add a Game from Custom Directory…",
                                    set_halign: gtk::Align::Start,
                                    add_css_class: "pill",
                                    connect_clicked => GameSetupMsg::AddCustomClicked,
                                },
                            },
                        },
                    },

                    // ── Add-custom page ──────────────────────────────────────
                    #[name = "add_page"]
                    gtk::ScrolledWindow {
                        set_hscrollbar_policy: gtk::PolicyType::Never,

                        adw::Clamp {
                            set_maximum_size: 540,
                            set_margin_all: 12,

                            gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_spacing: 12,

                                gtk::Label {
                                    set_label: "Game Type",
                                    set_halign: gtk::Align::Start,
                                    add_css_class: "heading",
                                },

                                gtk::ListBox {
                                    add_css_class: "boxed-list",
                                    set_selection_mode: gtk::SelectionMode::None,

                                    adw::ComboRow {
                                        set_title: "Game",
                                        set_model: Some(&{
                                            let opts = game::known_game_options();
                                            let labels: Vec<String> = opts
                                                .iter()
                                                .map(|o| {
                                                    if o.experimental {
                                                        format!("{} ({}) (Experimental)", o.title, o.store)
                                                    } else {
                                                        format!("{} ({})", o.title, o.store)
                                                    }
                                                })
                                                .collect();
                                            let strs: Vec<&str> =
                                                labels.iter().map(String::as_str).collect();
                                            gtk::StringList::new(&strs)
                                        }),
                                        connect_selected_notify[sender] => move |row| {
                                            sender.input(GameSetupMsg::GameTypeSelected(
                                                row.selected(),
                                            ));
                                        },
                                    },
                                },

                                gtk::Label {
                                    set_label: "Installation Folder",
                                    set_halign: gtk::Align::Start,
                                    add_css_class: "heading",
                                },

                                gtk::ListBox {
                                    add_css_class: "boxed-list",
                                    set_selection_mode: gtk::SelectionMode::None,

                                    adw::ActionRow {
                                        set_title: "Game Folder",
                                        set_subtitle: "Required",

                                        add_suffix = &gtk::Button::from_icon_name("folder-symbolic") {
                                            set_valign: gtk::Align::Center,
                                            add_css_class: "flat",
                                            connect_clicked => GameSetupMsg::BrowseNewPath,
                                        },

                                        #[local_ref]
                                        add_suffix = new_path_entry -> gtk::Entry {
                                            set_valign: gtk::Align::Center,
                                            set_width_chars: 20,
                                            set_editable: false,
                                            set_placeholder_text: Some("Not set"),
                                        },
                                    },

                                    adw::ActionRow {
                                        set_title: "Wine Prefix",
                                        set_subtitle: "Optional — leave empty to auto-detect",

                                        add_suffix = &gtk::Button::from_icon_name("folder-symbolic") {
                                            set_valign: gtk::Align::Center,
                                            add_css_class: "flat",
                                            connect_clicked => GameSetupMsg::BrowseNewPrefix,
                                        },

                                        add_suffix = &gtk::Button::from_icon_name("edit-clear-symbolic") {
                                            set_valign: gtk::Align::Center,
                                            add_css_class: "flat",
                                            set_tooltip_text: Some("Clear"),
                                            connect_clicked => GameSetupMsg::ClearNewPrefix,
                                        },

                                        #[local_ref]
                                        add_suffix = new_prefix_entry -> gtk::Entry {
                                            set_valign: gtk::Align::Center,
                                            set_width_chars: 20,
                                            set_editable: false,
                                            set_placeholder_text: Some("Auto-detect"),
                                        },
                                    },
                                },

                                #[local_ref]
                                add_btn -> gtk::Button {
                                    set_label: "Add Game",
                                    add_css_class: "suggested-action",
                                    set_halign: gtk::Align::End,
                                    set_sensitive: false,
                                    connect_clicked => GameSetupMsg::ConfirmAdd,
                                },
                            },
                        },
                    },
                },
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let (detected_games, persisted_custom) = init;

        let mut entries: Vec<GameEntry> = detected_games
            .into_iter()
            .map(|g| GameEntry {
                game: g,
                enabled: true,
                custom: false,
            })
            .collect();

        for pg in persisted_custom {
            let engine = if pg.engine == "redengine" {
                GameEngine::REDEngine
            } else if pg.engine == "eclipse" {
                GameEngine::Eclipse
            } else {
                GameEngine::Bethesda
            };
            entries.push(GameEntry {
                game: Game {
                    id: pg.id,
                    title: pg.title,
                    path: pg.path,
                    data_subdir: pg.data_subdir,
                    engine,
                    wine_prefix: pg.wine_prefix,
                },
                enabled: true,
                custom: true,
            });
        }

        let stack = gtk::Stack::new();
        let detected_list = gtk::ListBox::new();
        let custom_list = gtk::ListBox::new();
        let custom_section = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let new_path_entry = gtk::Entry::new();
        let new_prefix_entry = gtk::Entry::new();
        let add_btn = gtk::Button::with_label("Add Game");

        let model = GameSetupDialog {
            entries,
            stack,
            detected_list,
            custom_list,
            custom_section,
            new_game_type_idx: 0,
            new_path: None,
            new_prefix: None,
            new_path_entry,
            new_prefix_entry,
            add_btn,
        };

        let stack = &model.stack;
        let detected_list = &model.detected_list;
        let custom_list = &model.custom_list;
        let custom_section = &model.custom_section;
        let new_path_entry = &model.new_path_entry;
        let new_prefix_entry = &model.new_prefix_entry;
        let add_btn = &model.add_btn;

        let widgets = view_output!();

        // Give the stack pages their lookup names so set_visible_child_name works.
        model.stack.page(&widgets.list_page).set_name("list");
        model.stack.page(&widgets.add_page).set_name("add");

        model.rebuild_detected(&sender);
        model.rebuild_custom(&sender);

        // Set the initial visible page.
        model.stack.set_visible_child_name("list");

        root.present();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            GameSetupMsg::ToggleEnabled(idx) => {
                if let Some(entry) = self.entries.get_mut(idx) {
                    entry.enabled = !entry.enabled;
                }
                self.rebuild_detected(&sender);
                self.rebuild_custom(&sender);
            }

            GameSetupMsg::BrowsePath(idx) => {
                let initial = self
                    .entries
                    .get(idx)
                    .map(|e| e.game.path.clone())
                    .unwrap_or_default();
                let dialog = gtk::FileDialog::builder()
                    .title("Select Game Folder")
                    .modal(true)
                    .build();
                if initial.is_dir() {
                    dialog.set_initial_folder(Some(&gio::File::for_path(&initial)));
                }
                let input = sender.input_sender().clone();
                dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        input.send(GameSetupMsg::PathChosen(idx, path)).ok();
                    }
                });
            }

            GameSetupMsg::PathChosen(idx, path) => {
                if let Some(entry) = self.entries.get_mut(idx) {
                    entry.game.path = path;
                }
                self.rebuild_detected(&sender);
                self.rebuild_custom(&sender);
            }

            GameSetupMsg::BrowsePrefix(idx) => {
                let dialog = gtk::FileDialog::builder()
                    .title("Select Wine Prefix Folder")
                    .modal(true)
                    .build();
                if let Some(entry) = self.entries.get(idx) {
                    if let Some(ref pfx) = entry.game.wine_prefix {
                        dialog.set_initial_folder(Some(&gio::File::for_path(pfx)));
                    }
                }
                let input = sender.input_sender().clone();
                dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        input.send(GameSetupMsg::PrefixChosen(idx, path)).ok();
                    }
                });
            }

            GameSetupMsg::PrefixChosen(idx, path) => {
                if let Some(entry) = self.entries.get_mut(idx) {
                    entry.game.wine_prefix = Some(path);
                }
                self.rebuild_detected(&sender);
                self.rebuild_custom(&sender);
            }

            GameSetupMsg::ClearPrefix(idx) => {
                if let Some(entry) = self.entries.get_mut(idx) {
                    entry.game.wine_prefix = None;
                }
                self.rebuild_detected(&sender);
                self.rebuild_custom(&sender);
            }

            GameSetupMsg::RemoveCustom(idx) => {
                self.entries.remove(idx);
                self.rebuild_detected(&sender);
                self.rebuild_custom(&sender);
            }

            GameSetupMsg::AddCustomClicked => {
                self.new_path = None;
                self.new_prefix = None;
                self.new_game_type_idx = 0;
                self.new_path_entry.set_text("");
                self.new_prefix_entry.set_text("");
                self.update_add_btn();
                self.stack.set_visible_child_name("add");
            }

            GameSetupMsg::BackClicked => {
                self.stack.set_visible_child_name("list");
            }

            GameSetupMsg::GameTypeSelected(idx) => {
                self.new_game_type_idx = idx as usize;
            }

            GameSetupMsg::BrowseNewPath => {
                let dialog = gtk::FileDialog::builder()
                    .title("Select Game Installation Folder")
                    .modal(true)
                    .build();
                let input = sender.input_sender().clone();
                dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        input.send(GameSetupMsg::NewPathChosen(path)).ok();
                    }
                });
            }

            GameSetupMsg::NewPathChosen(path) => {
                self.new_path_entry.set_text(&path.to_string_lossy());
                self.new_path = Some(path);
                self.update_add_btn();
            }

            GameSetupMsg::BrowseNewPrefix => {
                let dialog = gtk::FileDialog::builder()
                    .title("Select Wine Prefix Folder")
                    .modal(true)
                    .build();
                let input = sender.input_sender().clone();
                dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        input.send(GameSetupMsg::NewPrefixChosen(path)).ok();
                    }
                });
            }

            GameSetupMsg::NewPrefixChosen(path) => {
                self.new_prefix_entry.set_text(&path.to_string_lossy());
                self.new_prefix = Some(path);
            }

            GameSetupMsg::ClearNewPrefix => {
                self.new_prefix_entry.set_text("");
                self.new_prefix = None;
            }

            GameSetupMsg::ConfirmAdd => {
                let Some(path) = self.new_path.take() else {
                    return;
                };
                let opts = game::known_game_options();
                let Some(opt) = opts.get(self.new_game_type_idx) else {
                    return;
                };
                let engine = match opt.engine {
                    GameEngine::REDEngine => GameEngine::REDEngine,
                    GameEngine::Eclipse => GameEngine::Eclipse,
                    GameEngine::Bethesda => GameEngine::Bethesda,
                };
                let game = Game {
                    id: opt.deployd_id.to_string(),
                    title: opt.title.to_string(),
                    path,
                    data_subdir: opt.data_subdir.to_string(),
                    engine,
                    wine_prefix: self.new_prefix.take(),
                };
                self.entries.push(GameEntry {
                    game,
                    enabled: true,
                    custom: true,
                });
                self.new_path_entry.set_text("");
                self.new_prefix_entry.set_text("");
                self.update_add_btn();
                self.rebuild_custom(&sender);
                self.stack.set_visible_child_name("list");
            }

            GameSetupMsg::Confirm => {
                let enabled: Vec<GameConfig> = self
                    .entries
                    .iter()
                    .filter(|e| e.enabled)
                    .map(|e| GameConfig {
                        game: e.game.clone(),
                        custom: e.custom,
                    })
                    .collect();
                let hidden_ids: Vec<String> = self
                    .entries
                    .iter()
                    .filter(|e| !e.enabled)
                    .map(|e| e.game.id.clone())
                    .collect();
                let _ = sender.output(GameSetupOutput::Confirmed { enabled, hidden_ids });
                root.close();
            }

            GameSetupMsg::Cancel => {
                let _ = sender.output(GameSetupOutput::Closed);
                root.close();
            }
        }
    }
}
