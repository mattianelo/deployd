use std::collections::HashMap;
use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::app::cache;
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
    /// Custom cache dirs per game_id (passed in at init, updated on change).
    game_cache_dirs: HashMap<String, PathBuf>,
    /// Navigation view — "list" vs "add".
    navigation_view: adw::NavigationView,
    /// ListBox for game rows.
    games_list: gtk::ListBox,
    /// Container for the games list section (hidden when empty).
    games_section: adw::PreferencesGroup,
    /// State for the "Add Game" form.
    new_game_type_idx: usize,
    new_path: Option<PathBuf>,
    new_prefix: Option<PathBuf>,
    new_path_entry: adw::EntryRow,
    new_prefix_entry: adw::EntryRow,
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
    /// Browse for a custom cache folder for the game at index.
    BrowseCacheDir(usize),
    CacheDirChosen(usize, PathBuf),
    /// Reset the custom cache dir for the game at index back to the default.
    ResetCacheDir(usize),
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
    /// User selected a new cache directory for a game; the App runs the move.
    CacheDirChangeRequested {
        game_id: String,
        new_dir: PathBuf,
    },
    /// User reset a game's cache dir to the global default.
    CacheDirResetRequested {
        game_id: String,
    },
}

impl GameSetupDialog {
    /// Rebuild the games list box from `self.entries`.
    fn rebuild_games(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.games_list.first_child() {
            self.games_list.remove(&child);
        }

        self.games_section.set_visible(!self.entries.is_empty());

        for (idx, entry) in self.entries.iter().enumerate() {
            let cache_dir = self.game_cache_dirs.get(&entry.game.id).cloned();
            self.games_list
                .append(&Self::build_entry_row(idx, entry, cache_dir, sender));
        }
    }

    /// Build a single game row widget.
    fn build_entry_row(
        idx: usize,
        entry: &GameEntry,
        cache_dir: Option<PathBuf>,
        sender: &ComponentSender<Self>,
    ) -> adw::ExpanderRow {
        let row = adw::ExpanderRow::new();
        row.set_title(&gtk::glib::markup_escape_text(&entry.game.title));
        row.set_subtitle(&gtk::glib::markup_escape_text(
            entry.game.path.to_string_lossy().as_ref(),
        ));

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
        path_row.set_subtitle(&gtk::glib::markup_escape_text(
            entry.game.path.to_string_lossy().as_ref(),
        ));

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
            prefix_row.set_subtitle(&gtk::glib::markup_escape_text(
                pfx.to_string_lossy().as_ref(),
            ));
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

        // Cache folder row.
        let cache_row = adw::ActionRow::new();
        cache_row.set_title("Cache Folder");
        cache_row.set_subtitle_lines(1);
        let cache_subtitle = cache::display_cache_root(cache_dir.as_ref());
        cache_row.set_subtitle(&gtk::glib::markup_escape_text(&cache_subtitle));

        let cache_browse_btn = gtk::Button::from_icon_name("folder-symbolic");
        cache_browse_btn.set_valign(gtk::Align::Center);
        cache_browse_btn.add_css_class("flat");
        cache_browse_btn.set_tooltip_text(Some("Browse…"));
        {
            let input = sender.input_sender().clone();
            cache_browse_btn.connect_clicked(move |_| {
                input.send(GameSetupMsg::BrowseCacheDir(idx)).ok();
            });
        }
        cache_row.add_suffix(&cache_browse_btn);

        // "Reset" button — only shown when a custom dir is set.
        let cache_reset_btn = gtk::Button::from_icon_name("edit-clear-symbolic");
        cache_reset_btn.set_valign(gtk::Align::Center);
        cache_reset_btn.add_css_class("flat");
        cache_reset_btn.set_tooltip_text(Some("Reset to default"));
        cache_reset_btn.set_visible(cache_dir.is_some());
        {
            let input = sender.input_sender().clone();
            cache_reset_btn.connect_clicked(move |_| {
                input.send(GameSetupMsg::ResetCacheDir(idx)).ok();
            });
        }
        cache_row.add_suffix(&cache_reset_btn);

        row.add_row(&cache_row);

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
    /// (detected games — always empty, persisted games, custom cache dirs)
    type Init = (Vec<Game>, Vec<PersistedGame>, HashMap<String, PathBuf>);
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

            #[local_ref]
            navigation_view -> adw::NavigationView { }
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let (detected_games, persisted_custom, game_cache_dirs) = init;

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

        let known_opts: Vec<game::KnownGameOption> =
            game::known_game_options().into_iter().collect();

        // ── Shared widgets stored in model ────────────────────────────────────
        let games_list = gtk::ListBox::new();
        games_list.add_css_class("boxed-list");
        games_list.set_selection_mode(gtk::SelectionMode::None);

        let games_section = adw::PreferencesGroup::new();

        let new_path_entry = adw::EntryRow::new();
        new_path_entry.set_title("Game Folder");
        new_path_entry.set_editable(false);

        let new_prefix_entry = adw::EntryRow::new();
        new_prefix_entry.set_title("Wine Prefix");
        new_prefix_entry.set_editable(false);

        let add_btn = gtk::Button::with_label("Add Game");
        add_btn.add_css_class("suggested-action");
        add_btn.set_halign(gtk::Align::End);
        add_btn.set_sensitive(false);
        {
            let s = sender.input_sender().clone();
            add_btn.connect_clicked(move |_| {
                s.send(GameSetupMsg::ConfirmAdd).ok();
            });
        }

        let navigation_view = adw::NavigationView::new();

        // ── List page ─────────────────────────────────────────────────────────
        games_section.set_title("Your Games");
        games_section.add(&games_list);

        let add_game_btn = gtk::Button::with_label("Add a Game…");
        add_game_btn.set_halign(gtk::Align::Start);
        add_game_btn.add_css_class("pill");
        {
            let s = sender.input_sender().clone();
            add_game_btn.connect_clicked(move |_| {
                s.send(GameSetupMsg::AddGameClicked).ok();
            });
        }

        let list_vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        list_vbox.append(&games_section);
        list_vbox.append(&add_game_btn);

        let list_clamp = adw::Clamp::new();
        list_clamp.set_maximum_size(540);
        list_clamp.set_margin_top(12);
        list_clamp.set_margin_bottom(12);
        list_clamp.set_margin_start(12);
        list_clamp.set_margin_end(12);
        list_clamp.set_child(Some(&list_vbox));

        let list_scrolled = gtk::ScrolledWindow::new();
        list_scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        list_scrolled.set_vexpand(true);
        list_scrolled.set_child(Some(&list_clamp));

        let cancel_btn = gtk::Button::with_label("Cancel");
        cancel_btn.add_css_class("flat");
        {
            let s = sender.input_sender().clone();
            cancel_btn.connect_clicked(move |_| {
                s.send(GameSetupMsg::Cancel).ok();
            });
        }

        let ok_btn = gtk::Button::with_label("OK");
        ok_btn.add_css_class("suggested-action");
        {
            let s = sender.input_sender().clone();
            ok_btn.connect_clicked(move |_| {
                s.send(GameSetupMsg::Confirm).ok();
            });
        }

        let list_header = adw::HeaderBar::new();
        list_header.set_show_back_button(false);
        list_header.pack_start(&cancel_btn);
        list_header.pack_end(&ok_btn);
        list_header.set_title_widget(Some(&adw::WindowTitle::new("Manage Games", "")));

        let list_toolbar = adw::ToolbarView::new();
        list_toolbar.add_top_bar(&list_header);
        list_toolbar.set_content(Some(&list_scrolled));

        let list_page = adw::NavigationPage::new(&list_toolbar, "Manage Games");
        list_page.set_tag(Some("list"));

        // ── Add page ──────────────────────────────────────────────────────────
        let game_type_combo = adw::ComboRow::new();
        game_type_combo.set_title("Game");
        let labels: Vec<String> = known_opts.iter().map(|o| o.title.to_string()).collect();
        let strs: Vec<&str> = labels.iter().map(String::as_str).collect();
        game_type_combo.set_model(Some(&gtk::StringList::new(&strs)));
        {
            let s = sender.input_sender().clone();
            game_type_combo.connect_selected_notify(move |row| {
                s.send(GameSetupMsg::GameTypeSelected(row.selected())).ok();
            });
        }

        let type_group = adw::PreferencesGroup::new();
        type_group.set_title("Game Type");
        type_group.add(&game_type_combo);

        let path_browse_btn = gtk::Button::from_icon_name("folder-symbolic");
        path_browse_btn.set_valign(gtk::Align::Center);
        path_browse_btn.add_css_class("flat");
        {
            let s = sender.input_sender().clone();
            path_browse_btn.connect_clicked(move |_| {
                s.send(GameSetupMsg::BrowseNewPath).ok();
            });
        }
        new_path_entry.add_suffix(&path_browse_btn);

        let prefix_browse_btn = gtk::Button::from_icon_name("folder-symbolic");
        prefix_browse_btn.set_valign(gtk::Align::Center);
        prefix_browse_btn.add_css_class("flat");
        {
            let s = sender.input_sender().clone();
            prefix_browse_btn.connect_clicked(move |_| {
                s.send(GameSetupMsg::BrowseNewPrefix).ok();
            });
        }
        new_prefix_entry.add_suffix(&prefix_browse_btn);

        let dir_group = adw::PreferencesGroup::new();
        dir_group.set_title("Directories");
        dir_group.add(&new_path_entry);
        dir_group.add(&new_prefix_entry);

        let add_vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        add_vbox.append(&type_group);
        add_vbox.append(&dir_group);
        add_vbox.append(&add_btn);

        let add_clamp = adw::Clamp::new();
        add_clamp.set_maximum_size(540);
        add_clamp.set_margin_top(12);
        add_clamp.set_margin_bottom(12);
        add_clamp.set_margin_start(12);
        add_clamp.set_margin_end(12);
        add_clamp.set_child(Some(&add_vbox));

        let add_scrolled = gtk::ScrolledWindow::new();
        add_scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        add_scrolled.set_child(Some(&add_clamp));

        let add_back_btn = gtk::Button::with_label("Back");
        add_back_btn.add_css_class("flat");
        {
            let s = sender.input_sender().clone();
            add_back_btn.connect_clicked(move |_| {
                s.send(GameSetupMsg::BackClicked).ok();
            });
        }

        let add_header = adw::HeaderBar::new();
        add_header.set_show_back_button(false);
        add_header.pack_start(&add_back_btn);
        add_header.set_title_widget(Some(&adw::WindowTitle::new("Add a Game", "")));

        let add_toolbar = adw::ToolbarView::new();
        add_toolbar.add_top_bar(&add_header);
        add_toolbar.set_content(Some(&add_scrolled));

        let add_page = adw::NavigationPage::new(&add_toolbar, "Add a Game");
        add_page.set_tag(Some("add"));

        navigation_view.push(&list_page);
        navigation_view.add(&add_page);

        let model = GameSetupDialog {
            entries,
            game_cache_dirs,
            navigation_view,
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

        let navigation_view = &model.navigation_view;
        let widgets = view_output!();

        model.rebuild_games(&sender);

        gtk::glib::idle_add_local_once({
            let root = root.clone();
            move || root.present()
        });

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

            GameSetupMsg::BrowseCacheDir(idx) => {
                let input = sender.input_sender().clone();
                sender.oneshot_command(async move {
                    if let Ok(Some(path)) =
                        crate::utils::portal::select_folder("Select Cache Folder").await
                    {
                        let _ = input.send(GameSetupMsg::CacheDirChosen(idx, path));
                    }
                });
            }

            GameSetupMsg::CacheDirChosen(idx, path) => {
                let Some(entry) = self.entries.get(idx) else {
                    return;
                };
                let game_id = entry.game.id.clone();
                self.game_cache_dirs.insert(game_id.clone(), path.clone());
                self.rebuild_games(&sender);
                let _ = sender.output(GameSetupOutput::CacheDirChangeRequested {
                    game_id,
                    new_dir: path,
                });
            }

            GameSetupMsg::ResetCacheDir(idx) => {
                let Some(entry) = self.entries.get(idx) else {
                    return;
                };
                let game_id = entry.game.id.clone();
                self.game_cache_dirs.remove(&game_id);
                self.rebuild_games(&sender);
                let _ = sender.output(GameSetupOutput::CacheDirResetRequested { game_id });
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
                self.navigation_view.push_by_tag("add");
            }

            GameSetupMsg::BackClicked => {
                self.navigation_view.pop();
            }

            GameSetupMsg::GameTypeSelected(idx) => {
                self.new_game_type_idx = idx as usize;
            }

            GameSetupMsg::BrowseNewPath => {
                let input = sender.input_sender().clone();
                sender.oneshot_command(async move {
                    if let Ok(Some(path)) =
                        crate::utils::portal::select_folder("Select Game Installation Folder").await
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
                self.navigation_view.pop();
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
