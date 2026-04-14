use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::FactoryVecDeque;
use relm4::prelude::*;

use crate::core::game;
use crate::core::proton_manager;
use crate::models::game::GameEngine;
use crate::models::tool::Tool;

// ---------------------------------------------------------------------------
// ToolRow — factory component for the "Configured Tools" list
// ---------------------------------------------------------------------------

pub struct ToolRow {
    pub tool_id: String,
    pub name: String,
    pub exe_path: String,
    pub working_dir: String,
    pub icon_name: String,
}

impl ToolRow {
    fn wdir_tooltip(&self) -> String {
        if self.working_dir.is_empty() {
            let parent = PathBuf::from(&self.exe_path)
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            format!("Working dir: {parent} (exe folder)\nClick to change")
        } else {
            format!("Working dir: {}\nClick to change", self.working_dir)
        }
    }
}

#[derive(Debug)]
pub enum ToolRowMsg {
    UpdateWorkingDir(String),
}

#[derive(Debug)]
pub enum ToolRowOutput {
    Remove(DynamicIndex),
    ChangeWorkingDir(DynamicIndex),
}

#[relm4::factory(pub)]
impl FactoryComponent for ToolRow {
    type Init = Tool;
    type Input = ToolRowMsg;
    type Output = ToolRowOutput;
    type CommandOutput = ();
    type ParentWidget = gtk::ListBox;

    view! {
        adw::ActionRow {
            set_title: &self.name,
            set_subtitle: &self.exe_path,
            add_prefix = &gtk::Image::from_icon_name(&self.icon_name) {},

            add_suffix = &gtk::Button::from_icon_name("folder-symbolic") {
                set_valign: gtk::Align::Center,
                add_css_class: "flat",
                #[watch] set_tooltip_text: Some(&self.wdir_tooltip()),
                connect_clicked[sender, index] => move |_| {
                    sender.output(ToolRowOutput::ChangeWorkingDir(index.clone())).unwrap();
                },
            },

            add_suffix = &gtk::Button::from_icon_name("user-trash-symbolic") {
                set_valign: gtk::Align::Center,
                add_css_class: "flat",
                set_tooltip_text: Some("Remove tool"),
                connect_clicked[sender, index] => move |_| {
                    sender.output(ToolRowOutput::Remove(index.clone())).unwrap();
                },
            },
        }
    }

    fn init_model(tool: Tool, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        ToolRow {
            tool_id: tool.id,
            name: tool.name,
            exe_path: tool.exe_path,
            working_dir: tool.working_dir,
            icon_name: tool.icon_name,
        }
    }

    fn update(&mut self, msg: Self::Input, _sender: FactorySender<Self>) {
        match msg {
            ToolRowMsg::UpdateWorkingDir(dir) => {
                self.working_dir = dir;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ToolManager — dialog component
// ---------------------------------------------------------------------------

pub struct ToolManager {
    game_id: String,
    game_path: PathBuf,
    wine_prefix: Option<PathBuf>,
    deploy_dir: Option<PathBuf>,
    game_engine: GameEngine,
    tools: FactoryVecDeque<ToolRow>,
    preset_box: gtk::ListBox,
    /// Widget containing the Proton runtime selector section (built once at init).
    runtime_section: gtk::Box,
    /// Currently active ProtonGE tag, updated when the user changes the selection.
    active_runtime: Option<String>,
}

#[derive(Debug)]
pub enum ToolManagerMsg {
    AddCustom,
    ExeChosen(std::path::PathBuf),
    /// Add a preset tool. If it has a known exe path that exists, use it directly;
    /// otherwise open a file dialog.
    AddPreset(usize),
    PresetExeChosen(usize, PathBuf),
    Remove(usize),
    /// Open a folder dialog to change the working directory for tool at `idx`.
    ChangeWorkingDir(usize),
    WorkingDirChosen(usize, PathBuf),
    /// Set the active ProtonGE runtime to the given version tag.
    SetRuntime(String),
    Close,
}

#[derive(Debug)]
pub enum ToolManagerOutput {
    ToolAdded(Tool),
    ToolRemoved(String),
    /// Emitted when the user changes the working directory of an existing tool.
    /// Carries `(tool_id, new_working_dir)`.
    ToolWorkingDirChanged(String, String),
    Closed,
}

impl ToolManager {
    fn rebuild_presets(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.preset_box.first_child() {
            self.preset_box.remove(&child);
        }

        let presets = game::tool_presets_for(&self.game_engine);
        for (i, preset) in presets.iter().enumerate() {
            // Skip presets already added (match by name, case-insensitive)
            if self
                .tools
                .iter()
                .any(|t| t.name.eq_ignore_ascii_case(preset.name))
            {
                continue;
            }

            let row = adw::ActionRow::new();
            row.set_title(preset.name);
            row.add_prefix(&gtk::Image::from_icon_name(preset.icon_name));

            // Auto-detect tool path
            let resolved = game::detect_tool_path(
                preset,
                &self.game_path,
                self.wine_prefix.as_deref(),
                self.deploy_dir.as_deref(),
            );

            if let Some(ref path) = resolved {
                row.set_subtitle(&format!("Found: {}", path.display()));
            } else {
                row.set_subtitle("Not found — browse to locate");
            }

            let add_btn = gtk::Button::from_icon_name("list-add-symbolic");
            add_btn.set_valign(gtk::Align::Center);
            add_btn.add_css_class("flat");
            add_btn.set_tooltip_text(Some("Add tool"));

            let input_sender = sender.input_sender().clone();
            add_btn.connect_clicked(move |_| {
                input_sender.send(ToolManagerMsg::AddPreset(i)).unwrap();
            });

            row.add_suffix(&add_btn);
            self.preset_box.append(&row);
        }
    }

    fn add_tool(&mut self, tool: Tool, sender: &ComponentSender<Self>) {
        let _ = sender.output(ToolManagerOutput::ToolAdded(tool.clone()));
        self.tools.guard().push_back(tool);
        self.rebuild_presets(sender);
    }

    fn default_working_dir(exe_path: &std::path::Path) -> String {
        exe_path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[relm4::component(pub)]
impl Component for ToolManager {
    type Init = (
        String,
        Vec<Tool>,
        PathBuf,
        Option<PathBuf>,
        GameEngine,
        Option<PathBuf>,
    );
    type Input = ToolManagerMsg;
    type Output = ToolManagerOutput;
    type CommandOutput = ();

    view! {
        adw::Window {
            set_title: Some("Manage Tools"),
            set_default_size: (500, -1),
            set_modal: true,

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        set_title: "Manage Tools",
                        set_subtitle: "External modding tools",
                    },
                },

                gtk::ScrolledWindow {
                    set_vexpand: true,
                    set_hscrollbar_policy: gtk::PolicyType::Never,
                    set_propagate_natural_height: true,

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_margin_all: 12,
                        set_spacing: 12,

                        // Proton runtime selector section (built imperatively at init)
                        #[local_ref]
                        runtime_section -> gtk::Box {},

                        // Preset catalog section
                        gtk::Label {
                            set_label: "Available Tools",
                            set_halign: gtk::Align::Start,
                            add_css_class: "heading",
                        },

                        #[local_ref]
                        preset_box -> gtk::ListBox {
                            set_selection_mode: gtk::SelectionMode::None,
                            add_css_class: "boxed-list",
                        },

                        // Configured tools section
                        gtk::Label {
                            set_label: "Configured Tools",
                            set_halign: gtk::Align::Start,
                            add_css_class: "heading",
                            set_margin_top: 4,
                        },

                        #[local_ref]
                        tool_list -> gtk::ListBox {
                            set_selection_mode: gtk::SelectionMode::None,
                            add_css_class: "boxed-list",
                            set_show_separators: true,
                        },

                        // Custom tool button
                        gtk::Button {
                            set_label: "Add Custom Tool…",
                            set_halign: gtk::Align::Start,
                            add_css_class: "flat",
                            connect_clicked => ToolManagerMsg::AddCustom,
                        },

                        gtk::Label {
                            set_label: "Tip: your mod folders are accessible from tools at M:\\ — e.g. configure NPC Plugin Chooser 2 to look for mods in M:\\",
                            set_halign: gtk::Align::Start,
                            set_wrap: true,
                            add_css_class: "dim-label",
                            set_margin_top: 4,
                        },
                    },
                },
            },

            connect_close_request[sender] => move |_| {
                sender.input(ToolManagerMsg::Close);
                glib::Propagation::Proceed
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let (game_id, init_tools, game_path, wine_prefix, game_engine, deploy_dir) = init;

        let mut tools = FactoryVecDeque::<ToolRow>::builder()
            .launch(gtk::ListBox::new())
            .forward(sender.input_sender(), |output| match output {
                ToolRowOutput::Remove(idx) => ToolManagerMsg::Remove(idx.current_index()),
                ToolRowOutput::ChangeWorkingDir(idx) => {
                    ToolManagerMsg::ChangeWorkingDir(idx.current_index())
                }
            });

        {
            let mut guard = tools.guard();
            for tool in init_tools {
                guard.push_back(tool);
            }
        }

        let preset_box = gtk::ListBox::new();

        // Build the Proton runtime selector section.
        let installed_runtimes = proton_manager::installed_versions();
        let active_runtime = proton_manager::active_runtime_tag();
        let runtime_section = build_runtime_section(
            &installed_runtimes,
            active_runtime.as_deref(),
            sender.input_sender(),
        );

        let model = ToolManager {
            game_id,
            game_path,
            wine_prefix,
            deploy_dir,
            game_engine,
            tools,
            preset_box,
            runtime_section,
            active_runtime,
        };

        let tool_list = model.tools.widget();
        let preset_box = &model.preset_box;
        let runtime_section = &model.runtime_section;
        let widgets = view_output!();

        model.rebuild_presets(&sender);

        root.present();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>, root: &Self::Root) {
        match msg {
            ToolManagerMsg::AddCustom => {
                let dialog = gtk::FileDialog::builder()
                    .title("Select Tool Executable")
                    .modal(true)
                    .build();

                let filter = gtk::FileFilter::new();
                for pat in [
                    "*.exe", "*.EXE", "*.bat", "*.BAT", "*.cmd", "*.CMD", "*.jar", "*.JAR",
                ] {
                    filter.add_pattern(pat);
                }
                filter.set_name(Some("Tool files (*.exe, *.bat, *.cmd, *.jar)"));
                let filters = gio::ListStore::new::<gtk::FileFilter>();
                let all_filter = gtk::FileFilter::new();
                all_filter.add_pattern("*");
                all_filter.set_name(Some("All files"));
                filters.append(&filter);
                filters.append(&all_filter);
                dialog.set_filters(Some(&filters));

                let input_sender = sender.input_sender().clone();
                dialog.open(Some(root), None::<&gio::Cancellable>, move |result| {
                    if let Ok(file) = result
                        && let Some(path) = file.path()
                    {
                        input_sender.send(ToolManagerMsg::ExeChosen(path)).unwrap();
                    }
                });
            }
            ToolManagerMsg::ExeChosen(path) => {
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();

                let tool = Tool {
                    id: uuid::Uuid::new_v4().to_string(),
                    game_id: self.game_id.clone(),
                    name,
                    working_dir: Self::default_working_dir(&path),
                    exe_path: path.to_string_lossy().to_string(),
                    icon_name: "application-x-executable-symbolic".to_string(),
                    custom_args: String::new(),
                    sort_order: self.tools.len() as i32,
                };

                self.add_tool(tool, &sender);
            }
            ToolManagerMsg::AddPreset(idx) => {
                let presets = game::tool_presets_for(&self.game_engine);
                let Some(preset) = presets.get(idx) else {
                    return;
                };

                // Try auto-detecting the exe path
                let resolved = game::detect_tool_path(
                    preset,
                    &self.game_path,
                    self.wine_prefix.as_deref(),
                    self.deploy_dir.as_deref(),
                );

                if let Some(exe_path) = resolved {
                    let tool = Tool {
                        id: uuid::Uuid::new_v4().to_string(),
                        game_id: self.game_id.clone(),
                        name: preset.name.to_string(),
                        working_dir: Self::default_working_dir(&exe_path),
                        exe_path: exe_path.to_string_lossy().to_string(),
                        icon_name: preset.icon_name.to_string(),
                        custom_args: preset.default_args.to_string(),
                        sort_order: self.tools.len() as i32,
                    };
                    self.add_tool(tool, &sender);
                } else {
                    // Open file dialog for the user to locate the exe
                    let dialog = gtk::FileDialog::builder()
                        .title(format!("Locate {}", preset.name))
                        .modal(true)
                        .build();

                    let filter = gtk::FileFilter::new();
                    for pat in [
                        "*.exe", "*.EXE", "*.bat", "*.BAT", "*.cmd", "*.CMD", "*.jar", "*.JAR",
                    ] {
                        filter.add_pattern(pat);
                    }
                    filter.set_name(Some("Tool files (*.exe, *.bat, *.cmd, *.jar)"));
                    let filters = gio::ListStore::new::<gtk::FileFilter>();
                    let all_filter = gtk::FileFilter::new();
                    all_filter.add_pattern("*");
                    all_filter.set_name(Some("All files"));
                    filters.append(&filter);
                    filters.append(&all_filter);
                    dialog.set_filters(Some(&filters));

                    // Start in the game directory
                    let initial = gio::File::for_path(&self.game_path);
                    dialog.set_initial_folder(Some(&initial));

                    let input_sender = sender.input_sender().clone();
                    dialog.open(Some(root), None::<&gio::Cancellable>, move |result| {
                        if let Ok(file) = result
                            && let Some(path) = file.path()
                        {
                            input_sender
                                .send(ToolManagerMsg::PresetExeChosen(idx, path))
                                .unwrap();
                        }
                    });
                }
            }
            ToolManagerMsg::PresetExeChosen(idx, path) => {
                let presets = game::tool_presets_for(&self.game_engine);
                let Some(preset) = presets.get(idx) else {
                    return;
                };

                let tool = Tool {
                    id: uuid::Uuid::new_v4().to_string(),
                    game_id: self.game_id.clone(),
                    name: preset.name.to_string(),
                    working_dir: Self::default_working_dir(&path),
                    exe_path: path.to_string_lossy().to_string(),
                    icon_name: preset.icon_name.to_string(),
                    custom_args: preset.default_args.to_string(),
                    sort_order: self.tools.len() as i32,
                };
                self.add_tool(tool, &sender);
            }
            ToolManagerMsg::ChangeWorkingDir(idx) => {
                let Some(row) = self.tools.get(idx) else {
                    return;
                };

                let dialog = gtk::FileDialog::builder()
                    .title(format!("Working Directory for {}", row.name))
                    .modal(true)
                    .build();

                // Start the picker at the current working dir (or exe parent).
                let start_path = if !row.working_dir.is_empty() {
                    PathBuf::from(&row.working_dir)
                } else {
                    PathBuf::from(&row.exe_path)
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| self.game_path.clone())
                };
                dialog.set_initial_folder(Some(&gio::File::for_path(&start_path)));

                let input_sender = sender.input_sender().clone();
                dialog.select_folder(Some(root), None::<&gio::Cancellable>, move |result| {
                    if let Ok(folder) = result
                        && let Some(path) = folder.path()
                    {
                        input_sender
                            .send(ToolManagerMsg::WorkingDirChosen(idx, path))
                            .unwrap();
                    }
                });
            }
            ToolManagerMsg::WorkingDirChosen(idx, path) => {
                let tool_id = self.tools.get(idx).map(|r| r.tool_id.clone());
                let dir = path.to_string_lossy().into_owned();
                self.tools
                    .send(idx, ToolRowMsg::UpdateWorkingDir(dir.clone()));
                if let Some(id) = tool_id {
                    let _ = sender.output(ToolManagerOutput::ToolWorkingDirChanged(id, dir));
                }
            }
            ToolManagerMsg::Remove(idx) => {
                let tool_id = self.tools.get(idx).map(|r| r.tool_id.clone());
                self.tools.guard().remove(idx);
                if let Some(id) = tool_id {
                    let _ = sender.output(ToolManagerOutput::ToolRemoved(id));
                }
                self.rebuild_presets(&sender);
            }
            ToolManagerMsg::SetRuntime(tag) => match proton_manager::set_active_runtime(&tag) {
                Ok(()) => self.active_runtime = Some(tag),
                Err(e) => eprintln!("deployd: failed to set active runtime: {e}"),
            },
            ToolManagerMsg::Close => {
                let _ = sender.output(ToolManagerOutput::Closed);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Build helpers
// ---------------------------------------------------------------------------

/// Build the Proton runtime selector section widget.
///
/// If no runtimes are installed, shows a dimmed label directing the user to
/// Settings. Otherwise renders an `adw::ComboRow` that lets the user switch
/// the active runtime; selecting an entry immediately updates the `current`
/// symlink via `proton_manager::set_active_runtime`.
fn build_runtime_section(
    installed: &[String],
    active: Option<&str>,
    sender: &relm4::Sender<ToolManagerMsg>,
) -> gtk::Box {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 8);

    let heading = gtk::Label::new(Some("Proton Runtime"));
    heading.set_halign(gtk::Align::Start);
    heading.add_css_class("heading");
    section.append(&heading);

    if installed.is_empty() {
        let notice = gtk::Label::new(Some("No ProtonGE runtimes installed — add one in Settings"));
        notice.set_halign(gtk::Align::Start);
        notice.add_css_class("dim-label");
        notice.set_wrap(true);
        section.append(&notice);
    } else {
        let tags: Vec<&str> = installed.iter().map(String::as_str).collect();
        let string_list = gtk::StringList::new(&tags);

        let combo = adw::ComboRow::new();
        combo.set_title("Active Runtime");
        combo.set_subtitle("Proton version used to launch tools");
        combo.set_model(Some(&string_list));

        let selected_idx = active
            .and_then(|tag| installed.iter().position(|r| r == tag))
            .unwrap_or(0);
        combo.set_selected(selected_idx as u32);

        let sender = sender.clone();
        let installed_owned: Vec<String> = installed.to_vec();
        combo.connect_selected_item_notify(move |row| {
            let idx = row.selected() as usize;
            if let Some(tag) = installed_owned.get(idx) {
                sender
                    .send(ToolManagerMsg::SetRuntime(tag.clone()))
                    .unwrap();
            }
        });

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.add_css_class("boxed-list");
        list.append(&combo);
        section.append(&list);
    }

    section
}
