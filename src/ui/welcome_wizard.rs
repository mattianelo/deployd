use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::app::messages::GameConfig;
use crate::core::game;
use crate::models::game::Game;
use crate::utils::snap::{self, SelectedFolderKind};

pub struct WelcomeWizard {
    known_opts: Vec<game::KnownGameOption>,
    /// Whether each known game is checked by the user.
    selected: Vec<bool>,
    /// User-browsed installation folder for each known game.
    install_paths: Vec<Option<PathBuf>>,
    /// User-browsed Wine prefix for each known game.
    wine_prefixes: Vec<Option<PathBuf>>,
    // Widget handles for imperative rebuilds.
    navigation_view: adw::NavigationView,
    games_list: gtk::ListBox,
    dirs_list: gtk::ListBox,
    next_btn: gtk::Button,
    finish_btn: gtk::Button,
}

#[derive(Debug)]
pub enum WelcomeWizardMsg {
    GetStarted,
    /// Toggle selection of the game at the given index.
    ToggleGame(usize),
    /// Browse for the installation folder of game at index.
    BrowseInstallPath(usize),
    InstallPathChosen(usize, PathBuf),
    /// Browse for the Wine prefix of game at index.
    BrowseWinePrefix(usize),
    WinePrefixChosen(usize, PathBuf),
    NextToDirectories,
    BackToGames,
    Finish,
    Cancel,
}

#[derive(Debug)]
pub enum WelcomeWizardOutput {
    Confirmed {
        enabled: Vec<GameConfig>,
        hidden_ids: Vec<String>,
    },
    Skipped,
}

impl WelcomeWizard {
    /// Returns the indices of selected games that still need an install path.
    fn missing_paths(&self) -> Vec<usize> {
        self.selected
            .iter()
            .enumerate()
            .filter(|&(i, &sel)| sel && self.install_paths[i].is_none())
            .map(|(i, _)| i)
            .collect()
    }

    /// Rebuild the game-selection list box from current state.
    fn rebuild_games_list(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.games_list.first_child() {
            self.games_list.remove(&child);
        }

        for (idx, opt) in self.known_opts.iter().enumerate() {
            let row = adw::ActionRow::new();
            row.set_title(opt.title);

            let check = gtk::CheckButton::new();
            check.set_active(self.selected[idx]);
            check.set_valign(gtk::Align::Center);
            {
                let input = sender.input_sender().clone();
                check.connect_toggled(move |_| {
                    input.send(WelcomeWizardMsg::ToggleGame(idx)).ok();
                });
            }
            row.add_prefix(&check);
            row.set_activatable_widget(Some(&check));

            self.games_list.append(&row);
        }

        let any_selected = self.selected.iter().any(|&s| s);
        self.next_btn.set_sensitive(any_selected);
    }

    /// Rebuild the directory-configuration list box for selected games.
    fn rebuild_dirs_list(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.dirs_list.first_child() {
            self.dirs_list.remove(&child);
        }

        let selected_indices: Vec<usize> = self
            .selected
            .iter()
            .enumerate()
            .filter(|&(_, &s)| s)
            .map(|(i, _)| i)
            .collect();

        for idx in selected_indices {
            let opt = &self.known_opts[idx];

            let expander = adw::ExpanderRow::new();
            expander.set_title(opt.title);
            expander.set_expanded(true);

            // Installation folder row.
            let path_row = adw::ActionRow::new();
            path_row.set_title("Installation Folder");
            path_row.set_subtitle(
                self.install_paths[idx]
                    .as_deref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Not set — required".to_owned())
                    .as_str(),
            );

            let browse_btn = gtk::Button::from_icon_name("folder-symbolic");
            browse_btn.set_valign(gtk::Align::Center);
            browse_btn.add_css_class("flat");
            browse_btn.set_tooltip_text(Some("Browse…"));
            {
                let input = sender.input_sender().clone();
                browse_btn.connect_clicked(move |_| {
                    input.send(WelcomeWizardMsg::BrowseInstallPath(idx)).ok();
                });
            }
            path_row.add_suffix(&browse_btn);
            expander.add_row(&path_row);

            // Wine prefix row.
            let prefix_row = adw::ActionRow::new();
            prefix_row.set_title("Wine Prefix");
            prefix_row.set_subtitle(
                self.wine_prefixes[idx]
                    .as_deref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Not set — required".to_owned())
                    .as_str(),
            );

            let prefix_btn = gtk::Button::from_icon_name("folder-symbolic");
            prefix_btn.set_valign(gtk::Align::Center);
            prefix_btn.add_css_class("flat");
            prefix_btn.set_tooltip_text(Some("Browse…"));
            {
                let input = sender.input_sender().clone();
                prefix_btn.connect_clicked(move |_| {
                    input.send(WelcomeWizardMsg::BrowseWinePrefix(idx)).ok();
                });
            }
            prefix_row.add_suffix(&prefix_btn);
            expander.add_row(&prefix_row);

            self.dirs_list.append(&expander);
        }

        let all_set = self.missing_paths().is_empty();
        self.finish_btn.set_sensitive(all_set);
    }

    fn show_path_error(root: &adw::Window, message: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading("Folder Not Available")
            .body(message)
            .build();
        dialog.add_response("close", "Close");
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("close");
        dialog.present(Some(root));
    }
}

#[relm4::component(pub)]
impl Component for WelcomeWizard {
    type Init = ();
    type Input = WelcomeWizardMsg;
    type Output = WelcomeWizardOutput;
    type CommandOutput = ();

    view! {
        adw::Window {
            set_title: Some("Welcome to Deployd"),
            set_default_size: (560, 520),
            set_modal: true,
            set_deletable: true,
            connect_close_request[sender] => move |_| {
                sender.input(WelcomeWizardMsg::Cancel);
                glib::Propagation::Proceed
            },

            #[local_ref]
            navigation_view -> adw::NavigationView { }
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let known_opts: Vec<game::KnownGameOption> = game::known_game_options();
        let n = known_opts.len();

        // ── Games list (rebuilt imperatively) ────────────────────────────────
        let games_list = gtk::ListBox::new();
        games_list.add_css_class("boxed-list");
        games_list.set_selection_mode(gtk::SelectionMode::None);

        // ── Dirs list (rebuilt imperatively) ─────────────────────────────────
        let dirs_list = gtk::ListBox::new();
        dirs_list.add_css_class("boxed-list");
        dirs_list.set_selection_mode(gtk::SelectionMode::None);

        // ── Next button ───────────────────────────────────────────────────────
        let next_btn = gtk::Button::with_label("Next");
        next_btn.add_css_class("suggested-action");
        next_btn.set_sensitive(false);
        {
            let s = sender.input_sender().clone();
            next_btn.connect_clicked(move |_| {
                s.send(WelcomeWizardMsg::NextToDirectories).ok();
            });
        }

        // ── Finish button ─────────────────────────────────────────────────────
        let finish_btn = gtk::Button::with_label("Finish");
        finish_btn.add_css_class("suggested-action");
        finish_btn.set_sensitive(false);
        {
            let s = sender.input_sender().clone();
            finish_btn.connect_clicked(move |_| {
                s.send(WelcomeWizardMsg::Finish).ok();
            });
        }

        let navigation_view = adw::NavigationView::new();

        // ── Welcome page ──────────────────────────────────────────────────────
        let get_started_btn = gtk::Button::with_label("Get Started");
        get_started_btn.add_css_class("suggested-action");
        get_started_btn.add_css_class("pill");
        get_started_btn.set_halign(gtk::Align::Center);
        {
            let s = sender.input_sender().clone();
            get_started_btn.connect_clicked(move |_| {
                s.send(WelcomeWizardMsg::GetStarted).ok();
            });
        }

        let welcome_status = adw::StatusPage::new();
        welcome_status.set_icon_name(Some("deployd"));
        welcome_status.set_title("Welcome to Deployd");
        welcome_status.set_description(Some(
            "A mod manager for Bethesda and REDEngine games.\n\
             Choose the games you want to manage and point Deployd\n\
             to their installation folders to get started.",
        ));
        welcome_status.set_child(Some(&get_started_btn));

        let skip_btn = gtk::Button::with_label("Skip");
        skip_btn.add_css_class("flat");
        {
            let s = sender.input_sender().clone();
            skip_btn.connect_clicked(move |_| {
                s.send(WelcomeWizardMsg::Cancel).ok();
            });
        }
        let welcome_header = adw::HeaderBar::new();
        welcome_header.set_show_back_button(false);
        welcome_header.pack_end(&skip_btn);

        let welcome_toolbar = adw::ToolbarView::new();
        welcome_toolbar.add_top_bar(&welcome_header);
        welcome_toolbar.set_content(Some(&welcome_status));

        let welcome_page = adw::NavigationPage::new(&welcome_toolbar, "Welcome");
        welcome_page.set_tag(Some("welcome"));

        // ── Games page ────────────────────────────────────────────────────────
        let games_group = adw::PreferencesGroup::new();
        games_group.set_title("Games");
        games_group.set_description(Some("Choose the games you want to manage."));
        games_group.add(&games_list);

        let games_vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        games_vbox.append(&games_group);

        let games_clamp = adw::Clamp::new();
        games_clamp.set_maximum_size(540);
        games_clamp.set_margin_top(12);
        games_clamp.set_margin_bottom(12);
        games_clamp.set_margin_start(12);
        games_clamp.set_margin_end(12);
        games_clamp.set_child(Some(&games_vbox));

        let games_scrolled = gtk::ScrolledWindow::new();
        games_scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        games_scrolled.set_vexpand(true);
        games_scrolled.set_child(Some(&games_clamp));

        let games_header = adw::HeaderBar::new();
        games_header.set_show_back_button(false);
        games_header.pack_end(&next_btn);

        let games_toolbar = adw::ToolbarView::new();
        games_toolbar.add_top_bar(&games_header);
        games_toolbar.set_content(Some(&games_scrolled));

        let games_page = adw::NavigationPage::new(&games_toolbar, "Select Games");
        games_page.set_tag(Some("games"));

        // ── Directories page ──────────────────────────────────────────────────
        let dirs_group = adw::PreferencesGroup::new();
        dirs_group.set_title("Directories");
        dirs_group.set_description(Some(
            "Set the installation folder and Wine prefix for each selected game.",
        ));
        dirs_group.add(&dirs_list);

        let dirs_vbox = gtk::Box::new(gtk::Orientation::Vertical, 12);
        dirs_vbox.append(&dirs_group);

        let dirs_clamp = adw::Clamp::new();
        dirs_clamp.set_maximum_size(540);
        dirs_clamp.set_margin_top(12);
        dirs_clamp.set_margin_bottom(12);
        dirs_clamp.set_margin_start(12);
        dirs_clamp.set_margin_end(12);
        dirs_clamp.set_child(Some(&dirs_vbox));

        let dirs_scrolled = gtk::ScrolledWindow::new();
        dirs_scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
        dirs_scrolled.set_vexpand(true);
        dirs_scrolled.set_child(Some(&dirs_clamp));

        let back_btn = gtk::Button::with_label("Back");
        back_btn.add_css_class("flat");
        {
            let s = sender.input_sender().clone();
            back_btn.connect_clicked(move |_| {
                s.send(WelcomeWizardMsg::BackToGames).ok();
            });
        }

        let dirs_header = adw::HeaderBar::new();
        dirs_header.set_show_back_button(false);
        dirs_header.pack_start(&back_btn);
        dirs_header.pack_end(&finish_btn);

        let dirs_toolbar = adw::ToolbarView::new();
        dirs_toolbar.add_top_bar(&dirs_header);
        dirs_toolbar.set_content(Some(&dirs_scrolled));

        let dirs_page = adw::NavigationPage::new(&dirs_toolbar, "Set Directories");
        dirs_page.set_tag(Some("directories"));

        // ── Wire navigation view ──────────────────────────────────────────────
        // push() adds and shows the welcome page as the root.
        navigation_view.push(&welcome_page);
        // add() makes games and directories available for push_by_tag().
        navigation_view.add(&games_page);
        navigation_view.add(&dirs_page);

        let model = WelcomeWizard {
            known_opts,
            selected: vec![false; n],
            install_paths: vec![None; n],
            wine_prefixes: vec![None; n],
            navigation_view,
            games_list,
            dirs_list,
            next_btn,
            finish_btn,
        };

        let navigation_view = &model.navigation_view;
        let widgets = view_output!();

        model.rebuild_games_list(&sender);

        gtk::glib::idle_add_local_once({
            let root = root.clone();
            move || root.present()
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            WelcomeWizardMsg::GetStarted => {
                self.navigation_view.push_by_tag("games");
            }

            WelcomeWizardMsg::ToggleGame(idx) => {
                if let Some(sel) = self.selected.get_mut(idx) {
                    *sel = !*sel;
                }
                let any = self.selected.iter().any(|&s| s);
                self.next_btn.set_sensitive(any);
            }

            WelcomeWizardMsg::NextToDirectories => {
                self.rebuild_dirs_list(&sender);
                self.navigation_view.push_by_tag("directories");
            }

            WelcomeWizardMsg::BackToGames => {
                self.navigation_view.pop();
            }

            WelcomeWizardMsg::BrowseInstallPath(idx) => {
                let input = sender.input_sender().clone();
                sender.oneshot_command(async move {
                    if let Ok(Some(path)) =
                        crate::utils::portal::select_folder("Select Installation Folder").await
                    {
                        let _ = input.send(WelcomeWizardMsg::InstallPathChosen(idx, path));
                    }
                });
            }

            WelcomeWizardMsg::InstallPathChosen(idx, path) => {
                if let Err(message) =
                    snap::validate_selected_folder(&path, SelectedFolderKind::GameFolder)
                {
                    Self::show_path_error(root, &message);
                    return;
                }
                if let Some(slot) = self.install_paths.get_mut(idx) {
                    *slot = Some(path);
                }
                self.rebuild_dirs_list(&sender);
            }

            WelcomeWizardMsg::BrowseWinePrefix(idx) => {
                let input = sender.input_sender().clone();
                sender.oneshot_command(async move {
                    if let Ok(Some(path)) =
                        crate::utils::portal::select_folder("Select Wine Prefix Folder").await
                    {
                        let _ = input.send(WelcomeWizardMsg::WinePrefixChosen(idx, path));
                    }
                });
            }

            WelcomeWizardMsg::WinePrefixChosen(idx, path) => {
                if let Err(message) =
                    snap::validate_selected_folder(&path, SelectedFolderKind::WinePrefix)
                {
                    Self::show_path_error(root, &message);
                    return;
                }
                if let Some(slot) = self.wine_prefixes.get_mut(idx) {
                    *slot = Some(path);
                }
                self.rebuild_dirs_list(&sender);
            }

            WelcomeWizardMsg::Finish => {
                let enabled: Vec<GameConfig> = self
                    .known_opts
                    .iter()
                    .enumerate()
                    .filter_map(|(i, opt)| {
                        if !self.selected.get(i).copied().unwrap_or(false) {
                            return None;
                        }
                        let path = self.install_paths.get(i)?.clone()?;
                        let engine = opt.engine.clone();
                        Some(GameConfig {
                            game: Game {
                                id: opt.deployd_id.to_string(),
                                title: opt.title.to_string(),
                                path,
                                data_subdir: opt.data_subdir.to_string(),
                                engine,
                                wine_prefix: self.wine_prefixes.get(i).cloned().flatten(),
                            },
                            custom: true,
                        })
                    })
                    .collect();

                let _ = sender.output(WelcomeWizardOutput::Confirmed {
                    enabled,
                    hidden_ids: vec![],
                });
                root.close();
            }

            WelcomeWizardMsg::Cancel => {
                let _ = sender.output(WelcomeWizardOutput::Skipped);
                root.close();
            }
        }
    }
}
