use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::installer;
use crate::models::game::GameEngine;
use crate::models::mod_entry::InstallTarget;

pub struct PreInstallDialog {
    mod_name: String,
    /// (dest_rel path string, auto-detected target) — used to build final HashMap on confirm.
    file_preview: Vec<(String, InstallTarget)>,
    is_fomod: bool,
    /// Per-file mutable target state, indexed by file_preview position.
    file_targets: Vec<InstallTarget>,
    files_visible: bool,
    /// Whether the selected game is a Bethesda game. REDEngine games hide the
    /// per-file Root/Data toggle since the concept doesn't apply to them.
    is_bethesda: bool,
}

#[derive(Debug)]
pub enum PreInstallDialogMsg {
    NameChanged(String),
    SetFileTarget(usize, InstallTarget),
    SetAllTargets(InstallTarget),
    ToggleFiles,
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub enum PreInstallDialogOutput {
    /// name + per-file target map (keys = dest_rel strings from file_preview).
    /// Empty map means "auto-detect all" (used for FOMOD mods).
    Confirmed(String, HashMap<String, InstallTarget>),
    Cancelled,
}

pub struct PreInstallDialogInit {
    pub mod_name: String,
    /// (dest_rel path string, auto-detected target) pairs.
    pub file_preview: Vec<(String, InstallTarget)>,
    pub is_fomod: bool,
    /// Set to true for Bethesda games to show per-file Root/Data toggles.
    /// REDEngine games have no Data/Root distinction (data_subdir = ".").
    pub is_bethesda: bool,
}

#[relm4::component(pub)]
impl SimpleComponent for PreInstallDialog {
    type Init = PreInstallDialogInit;
    type Input = PreInstallDialogMsg;
    type Output = PreInstallDialogOutput;

    view! {
        adw::Window {
            set_title: Some("Install Mod"),
            set_default_size: (480, -1),
            set_resizable: false,
            set_modal: true,

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        set_title: "Install Mod",
                    },
                },

                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_margin_all: 16,
                    set_spacing: 12,

                    gtk::Label {
                        set_label: "Mod Name",
                        set_halign: gtk::Align::Start,
                        add_css_class: "heading",
                    },

                    #[name = "name_entry"]
                    gtk::Entry {
                        set_text: &model.mod_name,
                        set_hexpand: true,
                    },

                    gtk::Label {
                        #[watch]
                        set_visible: model.is_fomod,
                        set_label: "FOMOD installer will follow after confirming.",
                        add_css_class: "dim-label",
                        set_halign: gtk::Align::Start,
                    },

                    // Collapsible file list with per-file Root/Data toggles.
                    // Only shown for Bethesda normal (non-FOMOD) mods with files.
                    // Hidden for REDEngine games: data_subdir="." so there is no
                    // separate Root vs Data distinction — all files deploy to game root.
                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_spacing: 4,
                        #[watch]
                        set_visible: model.is_bethesda && !model.is_fomod && !model.file_preview.is_empty(),

                        gtk::Button {
                            #[watch]
                            set_label: &if model.files_visible {
                                format!("Files ({}) ▲", model.file_preview.len())
                            } else {
                                format!("Files ({}) ▼", model.file_preview.len())
                            },
                            add_css_class: "flat",
                            set_halign: gtk::Align::Start,
                            connect_clicked => PreInstallDialogMsg::ToggleFiles,
                        },

                        gtk::Revealer {
                            #[watch]
                            set_reveal_child: model.files_visible,

                            gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_spacing: 6,

                                // "Set all" row and legend — populated imperatively in init()
                                #[name = "set_all_row"]
                                gtk::Box {
                                    set_orientation: gtk::Orientation::Horizontal,
                                    set_spacing: 8,
                                    set_halign: gtk::Align::Start,
                                },

                                gtk::ScrolledWindow {
                                    set_max_content_height: 280,
                                    set_propagate_natural_height: true,
                                    set_hscrollbar_policy: gtk::PolicyType::Never,

                                    #[name = "files_list"]
                                    gtk::ListBox {
                                        set_selection_mode: gtk::SelectionMode::None,
                                        add_css_class: "boxed-list",
                                    },
                                },
                            },
                        },
                    },

                    // Action buttons
                    gtk::Box {
                        set_orientation: gtk::Orientation::Horizontal,
                        set_halign: gtk::Align::End,
                        set_spacing: 8,
                        set_margin_top: 4,

                        gtk::Button {
                            set_label: "Cancel",
                            connect_clicked => PreInstallDialogMsg::Cancel,
                        },

                        gtk::Button {
                            #[watch]
                            set_label: if model.is_fomod { "Continue" } else { "Install" },
                            add_css_class: "suggested-action",
                            connect_clicked => PreInstallDialogMsg::Confirm,
                        },
                    },
                },
            },

            connect_close_request[sender] => move |_| {
                sender.input(PreInstallDialogMsg::Cancel);
                glib::Propagation::Proceed
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let file_targets: Vec<InstallTarget> =
            init.file_preview.iter().map(|(_, t)| t.clone()).collect();

        let model = PreInstallDialog {
            mod_name: init.mod_name,
            file_preview: init.file_preview,
            is_fomod: init.is_fomod,
            file_targets,
            files_visible: false,
            is_bethesda: init.is_bethesda,
        };

        let widgets = view_output!();

        // Connect name entry changed signal
        {
            let input_sender = sender.input_sender().clone();
            widgets.name_entry.connect_changed(move |entry| {
                input_sender
                    .send(PreInstallDialogMsg::NameChanged(entry.text().to_string()))
                    .unwrap();
            });
        }

        // Shared button refs so the "Set all" buttons can toggle them directly.
        // Using Rc<RefCell> because GTK closures are single-threaded.
        let data_btns: Rc<RefCell<Vec<gtk::ToggleButton>>> = Rc::new(RefCell::new(Vec::new()));
        let root_btns: Rc<RefCell<Vec<gtk::ToggleButton>>> = Rc::new(RefCell::new(Vec::new()));

        // Populate file list with per-row Root/Data toggles
        for (idx, (path, initial_target)) in model.file_preview.iter().enumerate() {
            let row = adw::ActionRow::new();
            row.set_title(path);

            let btn_data = gtk::ToggleButton::new();
            btn_data.set_label("D");
            btn_data.set_tooltip_text(Some("Deploy to Data directory"));
            btn_data.set_active(*initial_target == InstallTarget::Data);

            let btn_root = gtk::ToggleButton::new();
            btn_root.set_label("R");
            btn_root.set_tooltip_text(Some("Deploy to game root directory"));
            btn_root.set_group(Some(&btn_data));
            btn_root.set_active(*initial_target == InstallTarget::Root);

            {
                let s = sender.input_sender().clone();
                btn_data.connect_clicked(move |b| {
                    if b.is_active() {
                        s.send(PreInstallDialogMsg::SetFileTarget(idx, InstallTarget::Data))
                            .ok();
                    }
                });
            }
            {
                let s = sender.input_sender().clone();
                btn_root.connect_clicked(move |b| {
                    if b.is_active() {
                        s.send(PreInstallDialogMsg::SetFileTarget(idx, InstallTarget::Root))
                            .ok();
                    }
                });
            }

            data_btns.borrow_mut().push(btn_data.clone());
            root_btns.borrow_mut().push(btn_root.clone());

            let toggle_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            toggle_box.add_css_class("linked");
            toggle_box.append(&btn_data);
            toggle_box.append(&btn_root);
            row.add_suffix(&toggle_box);

            widgets.files_list.append(&row);
        }

        // Populate the "Set all" row (only meaningful when there are files)
        if !model.file_preview.is_empty() {
            let legend = gtk::Label::new(Some(
                "D = Data directory · R = game root (script extenders, ENB)",
            ));
            legend.add_css_class("dim-label");
            legend.set_hexpand(true);
            legend.set_halign(gtk::Align::Start);
            legend.set_xalign(0.0);

            let set_all_label = gtk::Label::new(Some("Set all:"));
            set_all_label.add_css_class("dim-label");

            let btn_all_data = gtk::Button::with_label("D");
            btn_all_data.set_tooltip_text(Some("Set all files to Data directory"));
            btn_all_data.add_css_class("flat");
            {
                let data_btns = data_btns.clone();
                let s = sender.input_sender().clone();
                btn_all_data.connect_clicked(move |_| {
                    // Activating the Data button in each linked group also deactivates Root.
                    for btn in data_btns.borrow().iter() {
                        btn.set_active(true);
                    }
                    s.send(PreInstallDialogMsg::SetAllTargets(InstallTarget::Data))
                        .ok();
                });
            }

            let btn_all_root = gtk::Button::with_label("R");
            btn_all_root.set_tooltip_text(Some("Set all files to game root directory"));
            btn_all_root.add_css_class("flat");
            {
                let root_btns = root_btns.clone();
                let s = sender.input_sender().clone();
                btn_all_root.connect_clicked(move |_| {
                    for btn in root_btns.borrow().iter() {
                        btn.set_active(true);
                    }
                    s.send(PreInstallDialogMsg::SetAllTargets(InstallTarget::Root))
                        .ok();
                });
            }

            let all_btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            all_btn_box.add_css_class("linked");
            all_btn_box.append(&btn_all_data);
            all_btn_box.append(&btn_all_root);

            widgets.set_all_row.append(&legend);
            widgets.set_all_row.append(&set_all_label);
            widgets.set_all_row.append(&all_btn_box);
        }

        root.present();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            PreInstallDialogMsg::NameChanged(name) => {
                self.mod_name = name;
            }
            PreInstallDialogMsg::SetFileTarget(idx, target) => {
                if let Some(t) = self.file_targets.get_mut(idx) {
                    *t = target;
                }
            }
            PreInstallDialogMsg::SetAllTargets(target) => {
                for t in &mut self.file_targets {
                    *t = target.clone();
                }
            }
            PreInstallDialogMsg::ToggleFiles => {
                self.files_visible = !self.files_visible;
            }
            PreInstallDialogMsg::Confirm => {
                let targets: HashMap<String, InstallTarget> = self
                    .file_preview
                    .iter()
                    .zip(self.file_targets.iter())
                    .map(|((path, _), t)| (path.clone(), t.clone()))
                    .collect();
                let _ = sender.output(PreInstallDialogOutput::Confirmed(
                    self.mod_name.clone(),
                    targets,
                ));
            }
            PreInstallDialogMsg::Cancel => {
                let _ = sender.output(PreInstallDialogOutput::Cancelled);
            }
        }
    }
}

/// Build a file preview list from a file list (destination relative paths),
/// with auto-detected install targets for each file.
///
/// `rules` should be the game-specific rules (from `rules_for_game`). They are
/// applied to each path before display and auto-detection so that, e.g., a
/// `Data/NVSE/nvse_config.ini` archive path is shown as `NVSE/nvse_config.ini`
/// — matching exactly what the installer will deploy — preventing users from
/// incorrectly overriding the target to avoid a phantom Data/Data nesting.
///
/// `engine` is used to apply engine-specific path routing (e.g. Aurora mods
/// get the `Override/` prefix and wrapper-stripping applied so the preview
/// shows where files will actually land).  For non-Bethesda games the
/// Root/Data auto-detection is skipped; all files default to `Data`.
pub fn file_preview_from_list(
    file_list: &[(PathBuf, PathBuf)],
    rules: &[crate::core::rules::Rule],
    engine: GameEngine,
) -> Vec<(String, InstallTarget)> {
    // Apply engine-specific routing so the preview reflects actual deploy paths.
    let is_bethesda = engine == GameEngine::Bethesda;
    let routed = installer::route_paths_for_preview(engine, file_list.to_vec());

    let mut entries: Vec<(String, InstallTarget)> = routed
        .into_iter()
        .map(|(_, dest)| {
            let raw = dest.to_string_lossy();
            // Apply game rules — same transformation the installer performs.
            let s = crate::core::rules::apply_rules(rules, &raw).replace('\\', "/");
            let target = if s.starts_with("../") || raw.starts_with("../") {
                InstallTarget::Root
            } else if is_bethesda {
                installer::auto_detect_install_target(&s)
            } else {
                // Root/Data distinction does not apply to non-Bethesda games.
                InstallTarget::Data
            };
            (s, target)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}
