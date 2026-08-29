use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
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
    /// Per-file inclusion state; false means the file will be skipped on install.
    file_included: Vec<bool>,
    files_visible: bool,
    /// Whether the selected game is a Bethesda game. REDEngine games hide the
    /// per-file Root/Data toggle since the concept doesn't apply to them.
    is_bethesda: bool,
    /// Whether the selected game uses the Aurora engine (Witcher 1). Shows the
    /// Override/Root toggle with Aurora-specific labels and auto-detection.
    is_aurora: bool,
    /// True when any file was auto-detected as Root (exe/dll/asi at archive root).
    has_root_files: bool,
}

#[derive(Debug)]
pub enum PreInstallDialogMsg {
    NameChanged(String),
    SetFileTarget(usize, InstallTarget),
    SetAllTargets(InstallTarget),
    SetFileIncluded(usize, bool),
    ToggleFiles,
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub enum PreInstallDialogOutput {
    /// name + per-file target map (keys = dest_rel strings from file_preview) +
    /// set of dest_rel keys for files the user opted to skip.
    /// Empty map + empty set means "auto-detect all" (used for FOMOD mods).
    Confirmed(String, HashMap<String, InstallTarget>, HashSet<String>),
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
    /// Set to true for Aurora (Witcher 1) to show per-file Override/Root toggles.
    pub is_aurora: bool,
    /// Existing mod names offered as autocomplete suggestions in the name entry.
    pub mod_names: Vec<String>,
}

#[relm4::component(pub)]
impl SimpleComponent for PreInstallDialog {
    type Init = PreInstallDialogInit;
    type Input = PreInstallDialogMsg;
    type Output = PreInstallDialogOutput;

    view! {
        adw::Window {
            set_title: Some("Install Mod"),
            set_default_size: (720, 760),
            set_resizable: true,
            set_modal: true,

            adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        set_title: "Install Mod",
                    },
                },

                #[wrap(Some)]
                set_content = &gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_hscrollbar_policy: gtk::PolicyType::Never,

                    adw::Clamp {
                        set_margin_top: 12,
                        set_margin_bottom: 12,
                        set_margin_start: 12,
                        set_margin_end: 12,

                        gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 12,

                            adw::PreferencesGroup {
                                set_title: "Mod",

                                add = &adw::ActionRow {
                                    set_title: "Name",
                                    set_activatable: false,

                                    #[name = "name_entry"]
                                    add_suffix = &gtk::Entry {
                                        set_text: &model.mod_name,
                                        set_hexpand: true,
                                        set_valign: gtk::Align::Center,
                                    },
                                },
                            },

                            adw::Banner {
                                #[watch]
                                set_revealed: model.is_fomod,
                                set_title: "FOMOD installer will follow after confirming.",
                            },

                            adw::Banner {
                                #[watch]
                                set_revealed: model.has_root_files && (model.is_bethesda || model.is_aurora),
                                set_title: "One or more files were auto-assigned to the game root (exe/dll/asi). If this tool expects all its files in the same folder, use \"Set all → D\".",
                            },

                    // Collapsible file list with per-file Root/Data toggles.
                    // Only shown for Bethesda normal (non-FOMOD) mods with files.
                            // Hidden for REDEngine games: data_subdir="." so there is no
                            // separate Root vs Data distinction — all files deploy to game root.
                            adw::PreferencesGroup {
                                #[watch]
                                set_visible: !model.is_fomod && !model.file_preview.is_empty(),

                                add = &gtk::Button {
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

                                add = &gtk::Revealer {
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
                                            set_max_content_height: 500,
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
                        },
                    },
                },

                add_bottom_bar = &gtk::ActionBar {
                    pack_start = &gtk::Button {
                        set_label: "Cancel",
                        connect_clicked => PreInstallDialogMsg::Cancel,
                    },

                    pack_end = &gtk::Button {
                        #[watch]
                        set_label: if model.is_fomod { "Continue" } else { "Install" },
                        add_css_class: "suggested-action",
                        connect_clicked => PreInstallDialogMsg::Confirm,
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
        let has_root_files = file_targets.contains(&InstallTarget::Root);
        let file_included = vec![true; init.file_preview.len()];

        let model = PreInstallDialog {
            mod_name: init.mod_name,
            file_preview: init.file_preview,
            is_fomod: init.is_fomod,
            file_targets,
            file_included,
            files_visible: true,
            is_bethesda: init.is_bethesda,
            is_aurora: init.is_aurora,
            has_root_files,
        };

        let widgets = view_output!();

        // Connect name entry changed signal
        {
            let input_sender = sender.input_sender().clone();
            widgets.name_entry.connect_changed(move |entry| {
                let _ =
                    input_sender.send(PreInstallDialogMsg::NameChanged(entry.text().to_string()));
            });
        }

        // Wire autocomplete for existing mod names (minimum 2 chars to trigger).
        // EntryCompletion is deprecated since GTK 4.10 but remains the simplest
        // drop-in for single-entry inline suggestions without a full custom widget.
        #[allow(deprecated)]
        if !init.mod_names.is_empty() {
            let store = gtk::ListStore::new(&[glib::Type::STRING]);
            for name in &init.mod_names {
                store.insert_with_values(None, &[(0, name)]);
            }
            let completion = gtk::EntryCompletion::new();
            completion.set_model(Some(&store));
            completion.set_text_column(0);
            completion.set_minimum_key_length(2);
            completion.set_inline_completion(false);
            widgets.name_entry.set_completion(Some(&completion));
        }

        // Shared button refs so the "Set all" buttons can toggle them directly.
        // Using Rc<RefCell> because GTK closures are single-threaded.
        let data_btns: Rc<RefCell<Vec<gtk::ToggleButton>>> = Rc::new(RefCell::new(Vec::new()));
        let root_btns: Rc<RefCell<Vec<gtk::ToggleButton>>> = Rc::new(RefCell::new(Vec::new()));

        // Populate file list with per-row checkboxes and (for Bethesda/Aurora) Data/Root toggles.
        for (idx, (path, initial_target)) in model.file_preview.iter().enumerate() {
            let row = adw::ActionRow::new();
            row.set_title(&gtk::glib::markup_escape_text(path));
            row.set_title_lines(1);
            row.set_activatable(false);
            row.set_tooltip_text(Some(path));

            let check = gtk::CheckButton::new();
            check.set_active(true);
            check.set_tooltip_text(Some("Include this file in the install"));
            {
                let s = sender.input_sender().clone();
                check.connect_toggled(move |b| {
                    s.send(PreInstallDialogMsg::SetFileIncluded(idx, b.is_active()))
                        .ok();
                });
            }
            row.add_prefix(&check);

            if model.is_bethesda || model.is_aurora {
                let btn_data = gtk::ToggleButton::new();
                btn_data.set_label("D");
                btn_data.set_tooltip_text(Some("Deploy to Data directory"));
                btn_data.set_active(*initial_target == InstallTarget::Data);
                btn_data.add_css_class("dr-btn");

                let btn_root = gtk::ToggleButton::new();
                btn_root.set_label("R");
                btn_root.set_tooltip_text(Some("Deploy to game root directory"));
                btn_root.set_group(Some(&btn_data));
                btn_root.set_active(*initial_target == InstallTarget::Root);
                btn_root.add_css_class("dr-btn");

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
            }

            widgets.files_list.append(&row);
        }

        // Populate the "Set all" row — only for Bethesda/Aurora where Data/Root toggles exist.
        if !model.file_preview.is_empty() && (model.is_bethesda || model.is_aurora) {
            let legend_text = if model.is_aurora {
                "D = Data/ directory · R = game root (System, Launcher, Register)"
            } else {
                "D = Data directory · R = game root (script extenders, ENB)"
            };
            let legend = gtk::Label::new(Some(legend_text));
            legend.add_css_class("dim-label");
            legend.set_hexpand(true);
            legend.set_halign(gtk::Align::Start);
            legend.set_xalign(0.0);

            let set_all_label = gtk::Label::new(Some("Set all:"));
            set_all_label.add_css_class("dim-label");

            let btn_all_data = gtk::Button::with_label("D");
            btn_all_data.set_tooltip_text(Some("Set all files to Data directory"));
            btn_all_data.add_css_class("flat");
            btn_all_data.add_css_class("dr-btn");
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
            btn_all_root.add_css_class("dr-btn");
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

        gtk::glib::idle_add_local_once({
            let root = root.clone();
            move || root.present()
        });

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
            PreInstallDialogMsg::SetFileIncluded(idx, included) => {
                if let Some(v) = self.file_included.get_mut(idx) {
                    *v = included;
                }
            }
            PreInstallDialogMsg::ToggleFiles => {
                self.files_visible = !self.files_visible;
            }
            PreInstallDialogMsg::Confirm => {
                let mut targets = HashMap::new();
                let mut excluded = HashSet::new();
                for ((path, _), (target, &included)) in self
                    .file_preview
                    .iter()
                    .zip(self.file_targets.iter().zip(self.file_included.iter()))
                {
                    if included {
                        targets.insert(path.clone(), target.clone());
                    } else {
                        excluded.insert(path.clone());
                    }
                }
                let _ = sender.output(PreInstallDialogOutput::Confirmed(
                    self.mod_name.clone(),
                    targets,
                    excluded,
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
    data_subdir: &str,
) -> Vec<(String, InstallTarget)> {
    let is_bethesda = engine == GameEngine::Bethesda;
    let is_aurora = engine == GameEngine::Aurora;
    // route_paths_for_preview now returns paths unchanged; routing happens at
    // install time once the user's file_targets are known.
    let routed = installer::route_paths_for_preview(engine, data_subdir, file_list.to_vec());

    let data_prefix = format!("{}/", data_subdir.to_lowercase());

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
            } else if is_aurora {
                // Auto-detect Root for game-root sibling dirs (system/, launcher/,
                // register/) after stripping any leading data/ prefix from the path.
                let lower_s = s.to_lowercase();
                let check = if lower_s.starts_with(&data_prefix) {
                    &lower_s[data_prefix.len()..]
                } else {
                    &lower_s[..]
                };
                if check.starts_with("system/")
                    || check.starts_with("launcher/")
                    || check.starts_with("register/")
                {
                    InstallTarget::Root
                } else {
                    InstallTarget::Data
                }
            } else {
                InstallTarget::Data
            };

            // For Aurora, display (and key) uses the data/-stripped path so it
            // matches the key that route_aurora_paths uses for file_targets lookup.
            let display = if is_aurora {
                let lower_s = s.to_lowercase();
                if lower_s.starts_with(&data_prefix) {
                    s[data_prefix.len()..].to_string()
                } else {
                    s
                }
            } else {
                s
            };
            (display, target)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}
