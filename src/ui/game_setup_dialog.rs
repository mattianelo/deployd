use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::app::messages::GameConfig;
use crate::core::game;
use crate::core::tracker::PersistedGame;
use crate::models::game::{Game, GameEngine};

/// One row in the games list: a manually added game.
#[derive(Debug, Clone)]
struct GameEntry {
    game: Game,
    /// Whether the user has included this game (checked).
    enabled: bool,
}

pub struct GameSetupDialog {
    entries: Vec<GameEntry>,
    /// Page switcher — "list" vs "add".
    stack: gtk::Stack,
    /// Whether the "add game" page is currently shown.
    add_page_visible: bool,
    /// ListBox for game rows.
    games_list: gtk::ListBox,
    /// Container for the games list section (hidden when empty).
    games_section: gtk::Box,
    /// State for the "Add Game" form.
    new_game_type_idx: usize,
    new_path: Option<PathBuf>,
    new_prefix: Option<PathBuf>,
    new_path_entry: gtk::Entry,
    new_prefix_entry: gtk::Entry,
    add_btn: gtk::Button,
    /// Filtered list of known game options shown in the "Add Game" dropdown.
    known_opts: Vec<game::KnownGameOption>,
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
    /// Remove a game entry.
    RemoveGame(usize),
    /// Switch to the "Add Game" form.
    AddGameClicked,
    /// Navigate back to the game list.
    BackClicked,
    /// The known-game-type dropdown selection changed.
    GameTypeSelected(u32),
    /// Browse for the new game's installation folder.
    BrowseNewPath,
    NewPathChosen(PathBuf),
    /// Browse for the new game's wine prefix.
    BrowseNewPrefix,
    NewPrefixChosen(PathBuf),
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
    /// Rebuild the games list box from `self.entries`.
    fn rebuild_games(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.games_list.first_child() {
            self.games_list.remove(&child);
        }

        self.games_section.set_visible(!self.entries.is_empty());

        for (idx, entry) in self.entries.iter().enumerate() {
            self.games_list
                .append(&Self::build_entry_row(idx, entry, sender));
        }
    }

    /// Build a single game row widget.
    fn build_entry_row(
        idx: usize,
        entry: &GameEntry,
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

        // Remove button.
        let remove_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        remove_btn.set_valign(gtk::Align::Center);
        remove_btn.add_css_class("flat");
        remove_btn.set_tooltip_text(Some("Remove"));
        {
            let input = sender.input_sender().clone();
            remove_btn.connect_clicked(move |_| {
                input.send(GameSetupMsg::RemoveGame(idx)).ok();
            });
        }
        row.add_suffix(&remove_btn);

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
            prefix_row.set_subtitle("Not set");
        }

        let prefix_btn = gtk::Button::from_icon_name("folder-symbolic");
        prefix_btn.set_valign(gtk::Align::Center);
        prefix_btn.add_css_class("flat");
        prefix_btn.set_tooltip_text(Some("Browse…"));
        {
            let input = sender.input_sender().clone();
            prefix_btn.connect_clicked(move |_| {
                input.send(GameSetupMsg::BrowsePrefix(idx)).ok();
            });
        }
        prefix_row.add_suffix(&prefix_btn);
        row.add_row(&prefix_row);

        row
    }

    /// Update the sensitivity of the "Add Game" button — both path and prefix are required.
    fn update_add_btn(&self) {
        self.add_btn
            .set_sensitive(self.new_path.is_some() && self.new_prefix.is_some());
    }
}

#[relm4::component(pub)]
impl Component for GameSetupDialog {
    /// (detected games — always empty, persisted games)
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
                        set_visible: model.add_page_visible,
                        connect_clicked => GameSetupMsg::BackClicked,
                    },

                    pack_end = &gtk::Button {
                        set_label: "OK",
                        add_css_class: "suggested-action",
                        #[watch]
                        set_visible: !model.add_page_visible,
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

                                #[local_ref]
                                games_section -> gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_spacing: 6,

                                    gtk::Label {
                                        set_label: "Your Games",
                                        set_halign: gtk::Align::Start,
                                        add_css_class: "heading",
                                    },

                                    #[local_ref]
                                    games_list -> gtk::ListBox {
                                        add_css_class: "boxed-list",
                                        set_selection_mode: gtk::SelectionMode::None,
                                    },
                                },

                                gtk::Button {
                                    set_label: "Add a Game…",
                                    set_halign: gtk::Align::Start,
                                    add_css_class: "pill",
                                    connect_clicked => GameSetupMsg::AddGameClicked,
                                },
                            },
                        },
                    },

                    // ── Add page ─────────────────────────────────────────────
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

                                    #[name = "game_type_combo"]
                                    adw::ComboRow {
                                        set_title: "Game",
                                        connect_selected_notify[sender] => move |row| {
                                            sender.input(GameSetupMsg::GameTypeSelected(
                                                row.selected(),
                                            ));
                                        },
                                    },
                                },

                                gtk::Label {
                                    set_label: "Directories",
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
                                        set_subtitle: "Required",

                                        add_suffix = &gtk::Button::from_icon_name("folder-symbolic") {
                                            set_valign: gtk::Align::Center,
                                            add_css_class: "flat",
                                            connect_clicked => GameSetupMsg::BrowseNewPrefix,
                                        },

                                        #[local_ref]
                                        add_suffix = new_prefix_entry -> gtk::Entry {
                                            set_valign: gtk::Align::Center,
                                            set_width_chars: 20,
                                            set_editable: false,
                                            set_placeholder_text: Some("Not set"),
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

        // Merge detected (now always empty) and persisted custom games.
        let mut entries: Vec<GameEntry> = detected_games
            .into_iter()
            .map(|g| GameEntry {
                game: g,
                enabled: true,
            })
            .collect();

        for pg in persisted_custom {
            let engine = if pg.engine == "redengine" {
                GameEngine::REDEngine
            } else if pg.engine == "eclipse" {
                GameEngine::Eclipse
            } else if pg.engine == "aurora" {
                GameEngine::Aurora
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
            });
        }

        let stack = gtk::Stack::new();
        let games_list = gtk::ListBox::new();
        let games_section = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let new_path_entry = gtk::Entry::new();
        let new_prefix_entry = gtk::Entry::new();
        let add_btn = gtk::Button::with_label("Add Game");

        let known_opts: Vec<game::KnownGameOption> =
            game::known_game_options().into_iter().collect();

        let model = GameSetupDialog {
            entries,
            stack,
            add_page_visible: false,
            games_list,
            games_section,
            new_game_type_idx: 0,
            new_path: None,
            new_prefix: None,
            new_path_entry,
            new_prefix_entry,
            add_btn,
            known_opts,
        };

        let stack = &model.stack;
        let games_list = &model.games_list;
        let games_section = &model.games_section;
        let new_path_entry = &model.new_path_entry;
        let new_prefix_entry = &model.new_prefix_entry;
        let add_btn = &model.add_btn;

        let widgets = view_output!();

        // Populate the game-type combo with the filtered options list.
        let labels: Vec<String> = model
            .known_opts
            .iter()
            .map(|o| o.title.to_string())
            .collect();
        let strs: Vec<&str> = labels.iter().map(String::as_str).collect();
        widgets
            .game_type_combo
            .set_model(Some(&gtk::StringList::new(&strs)));

        model.stack.page(&widgets.list_page).set_name("list");
        model.stack.page(&widgets.add_page).set_name("add");

        model.rebuild_games(&sender);
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
                self.rebuild_games(&sender);
            }

            GameSetupMsg::BrowsePath(idx) => {
                let input = sender.input_sender().clone();
                sender.oneshot_command(async move {
                    if let Ok(Some(path)) =
                        crate::utils::portal::select_folder("Select Game Folder").await
                    {
                        let _ = input.send(GameSetupMsg::PathChosen(idx, path));
                    }
                });
            }

            GameSetupMsg::PathChosen(idx, path) => {
                if let Some(entry) = self.entries.get_mut(idx) {
                    entry.game.path = path;
                }
                self.rebuild_games(&sender);
            }

            GameSetupMsg::BrowsePrefix(idx) => {
                let input = sender.input_sender().clone();
                sender.oneshot_command(async move {
                    if let Ok(Some(path)) =
                        crate::utils::portal::select_folder("Select Wine Prefix Folder").await
                    {
                        let _ = input.send(GameSetupMsg::PrefixChosen(idx, path));
                    }
                });
            }

            GameSetupMsg::PrefixChosen(idx, path) => {
                if let Some(entry) = self.entries.get_mut(idx) {
                    entry.game.wine_prefix = Some(path);
                }
                self.rebuild_games(&sender);
            }

            GameSetupMsg::RemoveGame(idx) => {
                self.entries.remove(idx);
                self.rebuild_games(&sender);
            }

            GameSetupMsg::AddGameClicked => {
                self.new_path = None;
                self.new_prefix = None;
                self.new_game_type_idx = 0;
                self.new_path_entry.set_text("");
                self.new_prefix_entry.set_text("");
                self.update_add_btn();
                self.stack.set_visible_child_name("add");
                self.add_page_visible = true;
            }

            GameSetupMsg::BackClicked => {
                self.stack.set_visible_child_name("list");
                self.add_page_visible = false;
            }

            GameSetupMsg::GameTypeSelected(idx) => {
                self.new_game_type_idx = idx as usize;
            }

            GameSetupMsg::BrowseNewPath => {
                let input = sender.input_sender().clone();
                sender.oneshot_command(async move {
                    if let Ok(Some(path)) =
                        crate::utils::portal::select_folder("Select Game Installation Folder")
                            .await
                    {
                        let _ = input.send(GameSetupMsg::NewPathChosen(path));
                    }
                });
            }

            GameSetupMsg::NewPathChosen(path) => {
                self.new_path_entry.set_text(&path.to_string_lossy());
                self.new_path = Some(path);
                self.update_add_btn();
            }

            GameSetupMsg::BrowseNewPrefix => {
                let input = sender.input_sender().clone();
                sender.oneshot_command(async move {
                    if let Ok(Some(path)) =
                        crate::utils::portal::select_folder("Select Wine Prefix Folder").await
                    {
                        let _ = input.send(GameSetupMsg::NewPrefixChosen(path));
                    }
                });
            }

            GameSetupMsg::NewPrefixChosen(path) => {
                self.new_prefix_entry.set_text(&path.to_string_lossy());
                self.new_prefix = Some(path);
                self.update_add_btn();
            }

            GameSetupMsg::ConfirmAdd => {
                let Some(path) = self.new_path.take() else {
                    return;
                };
                let Some(prefix) = self.new_prefix.take() else {
                    return;
                };
                let Some(opt) = self.known_opts.get(self.new_game_type_idx) else {
                    return;
                };
                let engine = opt.engine.clone();
                let game = Game {
                    id: opt.deployd_id.to_string(),
                    title: opt.title.to_string(),
                    path,
                    data_subdir: opt.data_subdir.to_string(),
                    engine,
                    wine_prefix: Some(prefix),
                };
                self.entries.push(GameEntry {
                    game,
                    enabled: true,
                });
                self.new_path_entry.set_text("");
                self.new_prefix_entry.set_text("");
                self.update_add_btn();
                self.rebuild_games(&sender);
                self.stack.set_visible_child_name("list");
                self.add_page_visible = false;
            }

            GameSetupMsg::Confirm => {
                let enabled: Vec<GameConfig> = self
                    .entries
                    .iter()
                    .filter(|e| e.enabled)
                    .map(|e| GameConfig {
                        game: e.game.clone(),
                        custom: true,
                    })
                    .collect();
                let hidden_ids: Vec<String> = self
                    .entries
                    .iter()
                    .filter(|e| !e.enabled)
                    .map(|e| e.game.id.clone())
                    .collect();
                let _ = sender.output(GameSetupOutput::Confirmed {
                    enabled,
                    hidden_ids,
                });
                root.close();
            }

            GameSetupMsg::Cancel => {
                let _ = sender.output(GameSetupOutput::Closed);
                root.close();
            }
        }
    }
}
