use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::app::messages::GameConfig;
use crate::core::game;
use crate::models::game::Game;

pub struct WelcomeWizard {
    known_opts: Vec<game::KnownGameOption>,
    /// Whether each known game is checked by the user.
    selected: Vec<bool>,
    /// User-browsed installation folder for each known game.
    install_paths: Vec<Option<PathBuf>>,
    /// User-browsed Wine prefix for each known game.
    wine_prefixes: Vec<Option<PathBuf>>,
    /// Current page name shown in the stack.
    current_page: &'static str,
    // Widget handles for imperative rebuilds.
    stack: gtk::Stack,
    games_list: gtk::ListBox,
    dirs_list: gtk::ListBox,
    next_btn: gtk::Button,
    back_dirs_btn: gtk::Button,
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
            // Make the whole row activate the checkbox.
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

        // Finish is enabled only when every selected game has an install path.
        let all_set = self.missing_paths().is_empty();
        self.finish_btn.set_sensitive(all_set);
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

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        set_title: "Welcome to Deployd",
                    },

                    pack_start = &gtk::Button {
                        set_label: "Back",
                        add_css_class: "flat",
                        #[watch]
                        set_visible: model.current_page == "directories",
                        connect_clicked => WelcomeWizardMsg::BackToGames,
                    },
                },

                #[local_ref]
                stack -> gtk::Stack {
                    set_vexpand: true,
                    set_transition_type: gtk::StackTransitionType::SlideLeftRight,

                    // ── Welcome page ─────────────────────────────────────────
                    #[name = "welcome_page"]
                    adw::StatusPage {
                        set_icon_name: Some("deployd"),
                        set_title: "Welcome to Deployd",
                        set_description: Some("A mod manager for Bethesda and REDEngine games.\nChoose the games you want to manage and point Deployd\nto their installation folders to get started."),

                        gtk::Button {
                            set_label: "Get Started",
                            add_css_class: "suggested-action",
                            add_css_class: "pill",
                            set_halign: gtk::Align::Center,
                            connect_clicked => WelcomeWizardMsg::GetStarted,
                        },
                    },

                    // ── Game selection page ───────────────────────────────────
                    #[name = "games_page"]
                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 0,

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
                                        set_label: "Which games do you want to manage?",
                                        set_halign: gtk::Align::Start,
                                        add_css_class: "heading",
                                    },

                                    #[local_ref]
                                    games_list -> gtk::ListBox {
                                        add_css_class: "boxed-list",
                                        set_selection_mode: gtk::SelectionMode::None,
                                    },
                                },
                            },
                        },

                        // Footer bar with Next button.
                        gtk::Box {
                            set_orientation: gtk::Orientation::Horizontal,
                            set_margin_start: 12,
                            set_margin_end: 12,
                            set_margin_top: 6,
                            set_margin_bottom: 12,
                            set_halign: gtk::Align::End,

                            #[local_ref]
                            next_btn -> gtk::Button {
                                set_label: "Next",
                                add_css_class: "suggested-action",
                                set_sensitive: false,
                                connect_clicked => WelcomeWizardMsg::NextToDirectories,
                            },
                        },
                    },

                    // ── Directory configuration page ──────────────────────────
                    #[name = "dirs_page"]
                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 0,

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
                                        set_label: "Where are these games installed?",
                                        set_halign: gtk::Align::Start,
                                        add_css_class: "heading",
                                    },

                                    #[local_ref]
                                    dirs_list -> gtk::ListBox {
                                        add_css_class: "boxed-list",
                                        set_selection_mode: gtk::SelectionMode::None,
                                    },
                                },
                            },
                        },

                        // Footer bar with Back / Finish.
                        gtk::Box {
                            set_orientation: gtk::Orientation::Horizontal,
                            set_margin_start: 12,
                            set_margin_end: 12,
                            set_margin_top: 6,
                            set_margin_bottom: 12,
                            set_halign: gtk::Align::End,
                            set_spacing: 6,

                            #[local_ref]
                            back_dirs_btn -> gtk::Button {
                                set_label: "Back",
                                add_css_class: "flat",
                                connect_clicked => WelcomeWizardMsg::BackToGames,
                            },

                            #[local_ref]
                            finish_btn -> gtk::Button {
                                set_label: "Finish",
                                add_css_class: "suggested-action",
                                set_sensitive: false,
                                connect_clicked => WelcomeWizardMsg::Finish,
                            },
                        },
                    },
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let known_opts: Vec<game::KnownGameOption> = game::known_game_options();
        let n = known_opts.len();

        let stack = gtk::Stack::new();
        let games_list = gtk::ListBox::new();
        let dirs_list = gtk::ListBox::new();
        let next_btn = gtk::Button::with_label("Next");
        let back_dirs_btn = gtk::Button::with_label("Back");
        let finish_btn = gtk::Button::with_label("Finish");

        let model = WelcomeWizard {
            known_opts,
            selected: vec![false; n],
            install_paths: vec![None; n],
            wine_prefixes: vec![None; n],
            current_page: "welcome",
            stack,
            games_list,
            dirs_list,
            next_btn,
            back_dirs_btn,
            finish_btn,
        };

        let stack = &model.stack;
        let games_list = &model.games_list;
        let dirs_list = &model.dirs_list;
        let next_btn = &model.next_btn;
        let back_dirs_btn = &model.back_dirs_btn;
        let finish_btn = &model.finish_btn;

        let widgets = view_output!();

        model.stack.page(&widgets.welcome_page).set_name("welcome");
        model.stack.page(&widgets.games_page).set_name("games");
        model.stack.page(&widgets.dirs_page).set_name("directories");
        model.stack.set_visible_child_name("welcome");

        model.rebuild_games_list(&sender);

        root.present();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            WelcomeWizardMsg::GetStarted => {
                self.current_page = "games";
                self.stack.set_visible_child_name("games");
            }

            WelcomeWizardMsg::ToggleGame(idx) => {
                if let Some(sel) = self.selected.get_mut(idx) {
                    *sel = !*sel;
                }
                let any = self.selected.iter().any(|&s| s);
                self.next_btn.set_sensitive(any);
                // No full rebuild needed — the checkbutton already updated its own visual.
            }

            WelcomeWizardMsg::NextToDirectories => {
                self.current_page = "directories";
                self.rebuild_dirs_list(&sender);
                self.stack.set_visible_child_name("directories");
            }

            WelcomeWizardMsg::BackToGames => {
                self.current_page = "games";
                self.rebuild_games_list(&sender);
                self.stack.set_visible_child_name("games");
            }

            WelcomeWizardMsg::BrowseInstallPath(idx) => {
                let dialog = gtk::FileDialog::builder()
                    .title("Select Installation Folder")
                    .modal(true)
                    .build();
                if let Some(Some(p)) = self.install_paths.get(idx)
                    && p.is_dir()
                {
                    dialog.set_initial_folder(Some(&gio::File::for_path(p)));
                }
                let input = sender.input_sender().clone();
                dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        input
                            .send(WelcomeWizardMsg::InstallPathChosen(idx, path))
                            .ok();
                    }
                });
            }

            WelcomeWizardMsg::InstallPathChosen(idx, path) => {
                if let Some(slot) = self.install_paths.get_mut(idx) {
                    *slot = Some(path);
                }
                self.rebuild_dirs_list(&sender);
            }

            WelcomeWizardMsg::BrowseWinePrefix(idx) => {
                let dialog = gtk::FileDialog::builder()
                    .title("Select Wine Prefix Folder")
                    .modal(true)
                    .build();
                if let Some(Some(p)) = self.wine_prefixes.get(idx)
                    && p.is_dir()
                {
                    dialog.set_initial_folder(Some(&gio::File::for_path(p)));
                }
                let input = sender.input_sender().clone();
                dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        input
                            .send(WelcomeWizardMsg::WinePrefixChosen(idx, path))
                            .ok();
                    }
                });
            }

            WelcomeWizardMsg::WinePrefixChosen(idx, path) => {
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
                    .filter(|&(i, _)| self.selected[i] && self.install_paths[i].is_some())
                    .map(|(i, opt)| {
                        let engine = opt.engine.clone();
                        GameConfig {
                            game: Game {
                                id: opt.deployd_id.to_string(),
                                title: opt.title.to_string(),
                                path: self.install_paths[i].clone().unwrap(),
                                data_subdir: opt.data_subdir.to_string(),
                                engine,
                                wine_prefix: self.wine_prefixes[i].clone(),
                            },
                            custom: true,
                        }
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
