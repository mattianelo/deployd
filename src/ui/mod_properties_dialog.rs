use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::models::manifest::ModFile;
use crate::models::mod_entry::{InstallTarget, ModEntry};

pub struct ModPropertiesInit {
    pub mod_entry: ModEntry,
    /// Whether the selected game is a Bethesda game. Non-Bethesda games (e.g.
    /// REDEngine) have no Data/Root distinction, so the install-target toggles
    /// are hidden.
    pub is_bethesda: bool,
    /// Whether the selected game uses the Aurora engine (Witcher 1). Shows the
    /// same Data/Root toggles as Bethesda but with Aurora-specific labels.
    pub is_aurora: bool,
    /// Resolved cache root for this game (used to locate the mod's cache folder).
    pub cache_root: std::path::PathBuf,
    /// Files this mod provides that win over lower-priority mods.
    pub override_files: Vec<String>,
    /// Files from this mod that are overridden by higher-priority mods.
    pub overridden_files: Vec<String>,
    /// Names of mods this mod overrides.
    pub conflicting_mod_names: Vec<String>,
    /// Names of mods that override files from this mod.
    pub conflicted_by_mod_names: Vec<String>,
}

pub struct ModPropertiesDialog {
    mod_id: String,
    cache_root: std::path::PathBuf,
    name: String,
    notes: String,
    nexus_mod_id: Option<i64>,
    nexus_mod_id_original: Option<i64>,
    nexus_mod_id_text: String,
    nexus_id_invalid: bool,
    install_target: InstallTarget,
    version: Option<String>,
    author: Option<String>,
    installed_at: Option<String>,
    /// Whether the selected game is a Bethesda game.
    is_bethesda: bool,
    /// Whether the selected game uses the Aurora engine (Witcher 1).
    is_aurora: bool,
    /// (game_rel_lowercase as stored in DB, display_path without leading "../")
    files: Vec<(String, String)>,
    /// Desired per-file targets, indexed parallel to `files`.
    file_targets: Vec<InstallTarget>,
    files_loading: bool,
    files_visible: bool,
    /// Direct handle to root — hidden synchronously in update() before any output
    /// to prevent button clicks after the parent drops the controller.
    window: adw::Window,
    /// Stored handle to the file list widget so LoadFiles can populate it imperatively.
    files_list: gtk::ListBox,
    /// Stored handle to the "Set all" row so LoadFiles can append controls to it.
    set_all_row: gtk::Box,
    /// Files this mod provides that win over lower-priority mods.
    override_files: Vec<String>,
    /// Files from this mod that are overridden by higher-priority mods.
    overridden_files: Vec<String>,
    /// Names of mods this mod overrides.
    conflicting_mod_names: Vec<String>,
    /// Names of mods that override files from this mod.
    conflicted_by_mod_names: Vec<String>,
    conflicts_visible: bool,
}

#[derive(Debug)]
pub enum ModPropertiesMsg {
    NameChanged(String),
    NotesChanged(String),
    NexusModIdChanged(String),
    SetFileTarget(usize, InstallTarget),
    SetAllFileTargets(InstallTarget),
    ToggleFiles,
    ToggleConflicts,
    /// Received from app once the async DB query for this mod's files completes.
    LoadFiles(Vec<ModFile>),
    Apply,
    Cancel,
    OpenFolder,
    ScanCacheClicked,
}

#[derive(Debug)]
pub enum ModPropertiesOutput {
    Applied {
        name: String,
        notes: String,
        nexus_mod_id: Option<i64>,
        nexus_id_changed: bool,
        install_target: InstallTarget,
        /// Maps current game_rel_lowercase → desired InstallTarget for every file.
        file_targets: HashMap<String, InstallTarget>,
    },
    Cancelled,
    ScanCache {
        mod_id: String,
    },
}

#[relm4::component(pub)]
impl SimpleComponent for ModPropertiesDialog {
    type Init = ModPropertiesInit;
    type Input = ModPropertiesMsg;
    type Output = ModPropertiesOutput;

    view! {
        adw::Window {
            set_title: Some("Mod Properties"),
            set_default_size: (980, 820),
            set_modal: true,

            adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        set_title: "Properties",
                        set_subtitle: &model.name,
                    },
                },

                #[wrap(Some)]
                set_content = &gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_hscrollbar_policy: gtk::PolicyType::Never,

                    adw::Clamp {
                        set_maximum_size: 920,
                        set_margin_top: 12,
                        set_margin_bottom: 12,
                        set_margin_start: 12,
                        set_margin_end: 12,

                    gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 12,

                        adw::PreferencesGroup {
                            // Spinner shown while files are loading
                            add = &gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_spacing: 8,
                                #[watch]
                                set_visible: model.files_loading,

                                gtk::Spinner {
                                    #[watch]
                                    set_spinning: model.files_loading,
                                },

                                gtk::Label {
                                    set_label: "Loading file list…",
                                    add_css_class: "dim-label",
                                },
                            },

                            // File section shown once loaded
                            add = &gtk::Box {
                                set_orientation: gtk::Orientation::Vertical,
                                set_spacing: 4,
                                #[watch]
                                set_visible: !model.files_loading,

                                gtk::Button {
                                    #[watch]
                                    set_label: &if model.files_visible {
                                        format!("Per-file targets ({}) ▲", model.files.len())
                                    } else {
                                        format!("Per-file targets ({}) ▼", model.files.len())
                                    },
                                    add_css_class: "flat",
                                    set_halign: gtk::Align::Start,
                                    #[watch]
                                    set_visible: !model.files.is_empty(),
                                    connect_clicked => ModPropertiesMsg::ToggleFiles,
                                },

                                gtk::Label {
                                    set_label: "No files tracked for this mod.",
                                    add_css_class: "dim-label",
                                    set_halign: gtk::Align::Start,
                                    #[watch]
                                    set_visible: model.files.is_empty(),
                                },

                                gtk::Revealer {
                                    #[watch]
                                    set_reveal_child: model.files_visible && !model.files.is_empty(),

                                    gtk::Box {
                                        set_orientation: gtk::Orientation::Vertical,
                                        set_spacing: 6,

                                        // "Set all" row — populated imperatively in LoadFiles handler
                                        #[name = "set_all_row"]
                                        gtk::Box {
                                            set_orientation: gtk::Orientation::Horizontal,
                                            set_spacing: 8,
                                            set_halign: gtk::Align::Start,
                                        },

                                        gtk::ScrolledWindow {
                                            set_min_content_height: 360,
                                            set_max_content_height: 560,
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

                        adw::PreferencesGroup {
                            #[watch]
                            set_visible: !model.override_files.is_empty() || !model.overridden_files.is_empty(),

                            add = &gtk::Button {
                                #[watch]
                                set_label: &{
                                    let total = model.override_files.len() + model.overridden_files.len();
                                    if model.conflicts_visible {
                                        format!("Conflicts ({total}) ▲")
                                    } else {
                                        format!("Conflicts ({total}) ▼")
                                    }
                                },
                                add_css_class: "flat",
                                set_halign: gtk::Align::Start,
                                connect_clicked => ModPropertiesMsg::ToggleConflicts,
                            },

                            add = &gtk::Revealer {
                                #[watch]
                                set_reveal_child: model.conflicts_visible,

                                gtk::Box {
                                    set_orientation: gtk::Orientation::Vertical,
                                    set_spacing: 8,

                                    // Overrides subsection
                                    gtk::Box {
                                        set_orientation: gtk::Orientation::Vertical,
                                        set_spacing: 4,
                                        #[watch]
                                        set_visible: !model.override_files.is_empty(),

                                        #[name = "overrides_label"]
                                        gtk::Label {
                                            set_halign: gtk::Align::Start,
                                            set_wrap: true,
                                            add_css_class: "heading",
                                        },

                                        gtk::ScrolledWindow {
                                            set_min_content_height: 180,
                                            set_max_content_height: 280,
                                            set_propagate_natural_height: true,
                                            set_hscrollbar_policy: gtk::PolicyType::Never,

                                            #[name = "overrides_list"]
                                            gtk::ListBox {
                                                set_selection_mode: gtk::SelectionMode::None,
                                                add_css_class: "boxed-list",
                                            },
                                        },
                                    },

                                    // Overridden by subsection
                                    gtk::Box {
                                        set_orientation: gtk::Orientation::Vertical,
                                        set_spacing: 4,
                                        #[watch]
                                        set_visible: !model.overridden_files.is_empty(),

                                        #[name = "overridden_label"]
                                        gtk::Label {
                                            set_halign: gtk::Align::Start,
                                            set_wrap: true,
                                            add_css_class: "heading",
                                        },

                                        gtk::ScrolledWindow {
                                            set_min_content_height: 180,
                                            set_max_content_height: 280,
                                            set_propagate_natural_height: true,
                                            set_hscrollbar_policy: gtk::PolicyType::Never,

                                            #[name = "overridden_list"]
                                            gtk::ListBox {
                                                set_selection_mode: gtk::SelectionMode::None,
                                                add_css_class: "boxed-list",
                                            },
                                        },
                                    },
                                },
                            },
                        },

                        adw::PreferencesGroup {
                            set_title: "Notes",

                            add = &gtk::ScrolledWindow {
                                set_min_content_height: 64,
                                set_max_content_height: 120,
                                set_hscrollbar_policy: gtk::PolicyType::Never,
                                add_css_class: "card",

                                #[name = "notes_view"]
                                gtk::TextView {
                                    set_wrap_mode: gtk::WrapMode::WordChar,
                                    set_top_margin: 6,
                                    set_bottom_margin: 6,
                                    set_left_margin: 6,
                                    set_right_margin: 6,
                                },
                            },
                        },

                        adw::PreferencesGroup {
                            set_title: "Cache Folder",

                            add = &adw::ActionRow {
                                set_title: "Open Folder",
                                set_subtitle: "Open this mod's cache folder in the file manager",

                                add_suffix = &gtk::Button {
                                    set_icon_name: "folder-open-symbolic",
                                    set_tooltip_text: Some("Open Folder"),
                                    set_valign: gtk::Align::Center,
                                    add_css_class: "flat",
                                    connect_clicked => ModPropertiesMsg::OpenFolder,
                                },
                            },

                            add = &adw::ActionRow {
                                set_title: "Rescan Cache",
                                set_subtitle: "Register all files currently in the cache folder as mod files",

                                add_suffix = &gtk::Button {
                                    set_icon_name: "view-refresh-symbolic",
                                    set_tooltip_text: Some("Rescan Cache"),
                                    set_valign: gtk::Align::Center,
                                    add_css_class: "flat",
                                    connect_clicked => ModPropertiesMsg::ScanCacheClicked,
                                },
                            },
                        },

                        adw::PreferencesGroup {
                            set_title: "Details",
                            #[name = "name_entry"]
                            add = &adw::EntryRow {
                                set_title: "Name",
                                set_text: &model.name,
                            },

                            #[name = "nexus_mod_id_entry"]
                            add = &adw::EntryRow {
                                set_title: "Nexus Mod ID",
                                set_text: &model.nexus_mod_id_text,
                            },

                            add = &gtk::Label {
                                set_label: "Enter a numeric Nexus mod ID or Nexus mod URL.",
                                add_css_class: "error",
                                set_halign: gtk::Align::Start,
                                #[watch]
                                set_visible: model.nexus_id_invalid,
                            },

                            add = &adw::ActionRow {
                                set_title: "Version",
                                set_subtitle: model.version.as_deref().unwrap_or("Unknown"),
                            },

                            add = &adw::ActionRow {
                                set_title: "Author",
                                set_subtitle: model.author.as_deref().unwrap_or("Unknown"),
                            },

                            add = &adw::ActionRow {
                                set_title: "Installed",
                                set_subtitle: model.installed_at.as_deref().unwrap_or("Unknown").split('T').next().unwrap_or("Unknown"),
                            },
                        },

                        },
                    },
                },

                add_bottom_bar = &gtk::ActionBar {
                    pack_start = &gtk::Button {
                        set_label: "Cancel",
                        connect_clicked => ModPropertiesMsg::Cancel,
                    },

                    pack_end = &gtk::Button {
                        set_label: "Apply",
                        add_css_class: "suggested-action",
                        #[watch]
                        set_sensitive: !model.files_loading,
                        connect_clicked => ModPropertiesMsg::Apply,
                    },
                },
            },

            connect_close_request[sender] => move |window| {
                window.set_visible(false);
                sender.input(ModPropertiesMsg::Cancel);
                glib::Propagation::Stop
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let ModPropertiesInit {
            mod_entry,
            is_bethesda,
            is_aurora,
            cache_root,
            override_files,
            overridden_files,
            conflicting_mod_names,
            conflicted_by_mod_names,
        } = init;
        let mut model = ModPropertiesDialog {
            mod_id: mod_entry.id,
            cache_root,
            name: mod_entry.name,
            notes: mod_entry.notes.unwrap_or_default(),
            nexus_mod_id: mod_entry.nexus_mod_id,
            nexus_mod_id_original: mod_entry.nexus_mod_id,
            nexus_mod_id_text: mod_entry
                .nexus_mod_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            nexus_id_invalid: false,
            install_target: mod_entry.install_target,
            version: mod_entry.version,
            author: mod_entry.author,
            installed_at: mod_entry.installed_at,
            is_bethesda,
            is_aurora,
            files: Vec::new(),
            file_targets: Vec::new(),
            files_loading: true,
            files_visible: false,
            window: root.clone(),
            // Placeholder widgets replaced with real widget clones after view_output!().
            files_list: gtk::ListBox::new(),
            set_all_row: gtk::Box::new(gtk::Orientation::Horizontal, 8),
            override_files,
            overridden_files,
            conflicting_mod_names,
            conflicted_by_mod_names,
            conflicts_visible: false,
        };

        let widgets = view_output!();

        // Replace placeholders with refs to the actual view widgets so that
        // the LoadFiles handler can append rows to them from update().
        model.files_list = widgets.files_list.clone();
        model.set_all_row = widgets.set_all_row.clone();

        // Populate conflict lists (data is known at init time).
        let wins_over = if model.conflicting_mod_names.is_empty() {
            String::new()
        } else {
            format!(" — wins over: {}", model.conflicting_mod_names.join(", "))
        };
        widgets.overrides_label.set_label(&format!(
            "Overrides ({}){}",
            model.override_files.len(),
            wins_over,
        ));
        for f in &model.override_files {
            let row = adw::ActionRow::new();
            row.set_title(&gtk::glib::markup_escape_text(f));
            row.set_title_lines(1);
            row.set_activatable(false);
            row.set_tooltip_text(Some(f));
            row.add_css_class("property");
            widgets.overrides_list.append(&row);
        }

        let lost_to = if model.conflicted_by_mod_names.is_empty() {
            String::new()
        } else {
            format!(" — loses to: {}", model.conflicted_by_mod_names.join(", "))
        };
        widgets.overridden_label.set_label(&format!(
            "Overridden by ({}){}",
            model.overridden_files.len(),
            lost_to,
        ));
        for f in &model.overridden_files {
            let row = adw::ActionRow::new();
            row.set_title(&gtk::glib::markup_escape_text(f));
            row.set_title_lines(1);
            row.set_activatable(false);
            row.set_tooltip_text(Some(f));
            row.add_css_class("property");
            widgets.overridden_list.append(&row);
        }

        {
            let input_sender = sender.input_sender().clone();
            widgets.name_entry.connect_changed(move |entry| {
                let _ = input_sender.send(ModPropertiesMsg::NameChanged(entry.text().to_string()));
            });
        }

        {
            let input_sender = sender.input_sender().clone();
            widgets.nexus_mod_id_entry.connect_changed(move |entry| {
                let _ = input_sender.send(ModPropertiesMsg::NexusModIdChanged(
                    entry.text().to_string(),
                ));
            });
        }

        {
            let buffer = widgets.notes_view.buffer();
            buffer.set_text(&model.notes);
            let input_sender = sender.input_sender().clone();
            buffer.connect_changed(move |buf| {
                let text = buf
                    .text(&buf.start_iter(), &buf.end_iter(), false)
                    .to_string();
                let _ = input_sender.send(ModPropertiesMsg::NotesChanged(text));
            });
        }

        glib::idle_add_local_once({
            let root = root.clone();
            move || root.present()
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            ModPropertiesMsg::NameChanged(name) => {
                self.name = name;
            }
            ModPropertiesMsg::NotesChanged(notes) => {
                self.notes = notes;
            }
            ModPropertiesMsg::NexusModIdChanged(raw) => {
                self.nexus_mod_id_text = raw;
                self.nexus_id_invalid = false;
            }
            ModPropertiesMsg::SetFileTarget(idx, target) => {
                if let Some(t) = self.file_targets.get_mut(idx) {
                    *t = target;
                }
            }
            ModPropertiesMsg::SetAllFileTargets(target) => {
                for t in &mut self.file_targets {
                    *t = target.clone();
                }
                self.install_target = target;
            }
            ModPropertiesMsg::ToggleFiles => {
                self.files_visible = !self.files_visible;
            }
            ModPropertiesMsg::ToggleConflicts => {
                self.conflicts_visible = !self.conflicts_visible;
            }
            ModPropertiesMsg::LoadFiles(files) => {
                // Build model state from loaded ModFile list.
                for f in &files {
                    let db_path = f.game_rel_lowercase.clone();
                    let display_path = db_path.strip_prefix("../").unwrap_or(&db_path).to_string();
                    let target = if db_path.starts_with("../") {
                        InstallTarget::Root
                    } else {
                        InstallTarget::Data
                    };
                    self.files.push((db_path, display_path));
                    self.file_targets.push(target);
                }

                // Build the per-file ListBox rows imperatively (same as PreInstallDialog).
                let data_btns: Rc<RefCell<Vec<gtk::ToggleButton>>> =
                    Rc::new(RefCell::new(Vec::new()));
                let root_btns: Rc<RefCell<Vec<gtk::ToggleButton>>> =
                    Rc::new(RefCell::new(Vec::new()));

                for (idx, (_, display_path)) in self.files.iter().enumerate() {
                    let row = adw::ActionRow::new();
                    row.set_title(&gtk::glib::markup_escape_text(display_path));
                    row.set_title_lines(1);
                    row.set_activatable(false);
                    row.set_tooltip_text(Some(display_path.as_str()));
                    row.add_css_class("property");

                    // Per-file Data/Root toggles for Bethesda and Aurora games.
                    if self.is_bethesda || self.is_aurora {
                        let btn_data = gtk::ToggleButton::new();
                        btn_data.set_label("D");
                        btn_data.set_tooltip_text(Some("Deploy to Data directory"));
                        btn_data.set_active(self.file_targets[idx] == InstallTarget::Data);
                        btn_data.add_css_class("dr-btn");

                        let btn_root = gtk::ToggleButton::new();
                        btn_root.set_label("R");
                        btn_root.set_tooltip_text(Some("Deploy to game root directory"));
                        btn_root.set_group(Some(&btn_data));
                        btn_root.set_active(self.file_targets[idx] == InstallTarget::Root);
                        btn_root.add_css_class("dr-btn");

                        {
                            let s = sender.input_sender().clone();
                            btn_data.connect_clicked(move |b| {
                                if b.is_active() {
                                    s.send(ModPropertiesMsg::SetFileTarget(
                                        idx,
                                        InstallTarget::Data,
                                    ))
                                    .ok();
                                }
                            });
                        }
                        {
                            let s = sender.input_sender().clone();
                            btn_root.connect_clicked(move |b| {
                                if b.is_active() {
                                    s.send(ModPropertiesMsg::SetFileTarget(
                                        idx,
                                        InstallTarget::Root,
                                    ))
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

                    self.files_list.append(&row);
                }

                // Build the "Set all" row when there are files (Bethesda and Aurora).
                if !self.files.is_empty() && (self.is_bethesda || self.is_aurora) {
                    let legend_text = if self.is_aurora {
                        "D = Data/ directory · R = game root (System, Launcher, Register)"
                    } else {
                        "D = Data directory · R = game root"
                    };
                    let legend = gtk::Label::new(Some(legend_text));
                    legend.add_css_class("dim-label");
                    legend.set_hexpand(true);
                    legend.set_halign(gtk::Align::Start);

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
                            for btn in data_btns.borrow().iter() {
                                btn.set_active(true);
                            }
                            s.send(ModPropertiesMsg::SetAllFileTargets(InstallTarget::Data))
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
                            s.send(ModPropertiesMsg::SetAllFileTargets(InstallTarget::Root))
                                .ok();
                        });
                    }

                    let all_btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    all_btn_box.add_css_class("linked");
                    all_btn_box.append(&btn_all_data);
                    all_btn_box.append(&btn_all_root);

                    self.set_all_row.append(&legend);
                    self.set_all_row.append(&set_all_label);
                    self.set_all_row.append(&all_btn_box);
                }

                // Sync install_target from per-file state only when files are present.
                // When file_targets is empty (no DB records), the original mod
                // install_target (set from mod_entry in init) must be preserved —
                // overwriting it here would corrupt Root mods that have no file records.
                if !self.file_targets.is_empty() {
                    let all_root = self.file_targets.iter().all(|t| *t == InstallTarget::Root);
                    self.install_target = if all_root {
                        InstallTarget::Root
                    } else {
                        InstallTarget::Data
                    };
                }

                // Auto-expand the file list and mark loading as done.
                self.files_visible = true;
                self.files_loading = false;
            }
            ModPropertiesMsg::Apply => {
                let raw_nexus_id = self.nexus_mod_id_text.trim();
                let parsed_nexus_id = if raw_nexus_id.is_empty() {
                    None
                } else {
                    match crate::app::free_fns::parse_nexus_mod_id_from_input(raw_nexus_id) {
                        Some(id) => Some(id),
                        None => {
                            self.nexus_id_invalid = true;
                            return;
                        }
                    }
                };
                self.nexus_mod_id = parsed_nexus_id;
                self.window.set_visible(false);
                let file_targets: HashMap<String, InstallTarget> = self
                    .files
                    .iter()
                    .zip(self.file_targets.iter())
                    .map(|((db_path, _), target)| (db_path.clone(), target.clone()))
                    .collect();
                let _ = sender.output(ModPropertiesOutput::Applied {
                    name: self.name.clone(),
                    notes: self.notes.clone(),
                    nexus_mod_id: self.nexus_mod_id,
                    nexus_id_changed: self.nexus_mod_id != self.nexus_mod_id_original,
                    install_target: self.install_target.clone(),
                    file_targets,
                });
            }
            ModPropertiesMsg::Cancel => {
                self.window.set_visible(false);
                let _ = sender.output(ModPropertiesOutput::Cancelled);
            }
            ModPropertiesMsg::OpenFolder => {
                let cache_dir =
                    crate::utils::paths::mod_cache_dir_in(&self.cache_root, &self.mod_id);
                let _ = std::fs::create_dir_all(&cache_dir);
                let _ = open::that(&cache_dir);
            }
            ModPropertiesMsg::ScanCacheClicked => {
                let _ = sender.output(ModPropertiesOutput::ScanCache {
                    mod_id: self.mod_id.clone(),
                });
            }
        }
    }
}
