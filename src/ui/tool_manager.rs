use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game;
use crate::models::game::GameEngine;
use crate::models::tool::Tool;

pub struct ToolManager {
    game_id: String,
    game_path: PathBuf,
    wine_prefix: Option<PathBuf>,
    game_engine: GameEngine,
    tools: Vec<Tool>,
    list_box: gtk::ListBox,
    preset_box: gtk::ListBox,
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
    fn rebuild_list(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.list_box.first_child() {
            self.list_box.remove(&child);
        }

        for (i, tool) in self.tools.iter().enumerate() {
            let row = adw::ActionRow::new();
            row.set_title(&tool.name);
            row.set_subtitle(&tool.exe_path);
            row.add_prefix(&gtk::Image::from_icon_name(&tool.icon_name));

            // Folder button to change working directory.
            let folder_btn = gtk::Button::from_icon_name("folder-symbolic");
            folder_btn.set_valign(gtk::Align::Center);
            folder_btn.add_css_class("flat");
            let wdir_label = if tool.working_dir.is_empty() {
                format!(
                    "Working dir: {} (exe folder)\nClick to change",
                    PathBuf::from(&tool.exe_path)
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                )
            } else {
                format!("Working dir: {}\nClick to change", tool.working_dir)
            };
            folder_btn.set_tooltip_text(Some(&wdir_label));

            let input_sender = sender.input_sender().clone();
            folder_btn.connect_clicked(move |_| {
                input_sender
                    .send(ToolManagerMsg::ChangeWorkingDir(i))
                    .unwrap();
            });

            let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
            delete_btn.set_valign(gtk::Align::Center);
            delete_btn.add_css_class("flat");
            delete_btn.set_tooltip_text(Some("Remove tool"));

            let input_sender = sender.input_sender().clone();
            delete_btn.connect_clicked(move |_| {
                input_sender.send(ToolManagerMsg::Remove(i)).unwrap();
            });

            row.add_suffix(&folder_btn);
            row.add_suffix(&delete_btn);
            self.list_box.append(&row);
        }

        if self.tools.is_empty() {
            let placeholder = gtk::Label::new(Some("No tools configured yet."));
            placeholder.add_css_class("dim-label");
            placeholder.set_margin_top(16);
            placeholder.set_margin_bottom(16);
            self.list_box.append(&placeholder);
        }
    }

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
            let resolved =
                game::detect_tool_path(preset, &self.game_path, self.wine_prefix.as_deref());

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
        self.tools.push(tool.clone());
        self.rebuild_list(sender);
        self.rebuild_presets(sender);
        let _ = sender.output(ToolManagerOutput::ToolAdded(tool));
    }

    /// Derive a sensible default working directory from an exe path:
    /// the directory that contains the exe.
    fn default_working_dir(exe_path: &std::path::Path) -> String {
        exe_path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

#[relm4::component(pub)]
impl Component for ToolManager {
    type Init = (String, Vec<Tool>, PathBuf, Option<PathBuf>, GameEngine);
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
                        list_box -> gtk::ListBox {
                            set_selection_mode: gtk::SelectionMode::None,
                            add_css_class: "boxed-list",
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
        let (game_id, tools, game_path, wine_prefix, game_engine) = init;
        let list_box = gtk::ListBox::new();
        let preset_box = gtk::ListBox::new();

        let model = ToolManager {
            game_id,
            game_path,
            wine_prefix,
            game_engine,
            tools,
            list_box,
            preset_box,
        };

        let list_box = &model.list_box;
        let preset_box = &model.preset_box;
        let widgets = view_output!();

        model.rebuild_list(&sender);
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
                let resolved =
                    game::detect_tool_path(preset, &self.game_path, self.wine_prefix.as_deref());

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
                let Some(tool) = self.tools.get(idx) else {
                    return;
                };

                let dialog = gtk::FileDialog::builder()
                    .title(format!("Working Directory for {}", tool.name))
                    .modal(true)
                    .build();

                // Start the picker at the current working dir (or exe parent).
                let start_path = if !tool.working_dir.is_empty() {
                    PathBuf::from(&tool.working_dir)
                } else {
                    PathBuf::from(&tool.exe_path)
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
                let Some(tool) = self.tools.get_mut(idx) else {
                    return;
                };
                let tool_id = tool.id.clone();
                tool.working_dir = path.to_string_lossy().into_owned();
                self.rebuild_list(&sender);
                let _ = sender.output(ToolManagerOutput::ToolWorkingDirChanged(
                    tool_id,
                    path.to_string_lossy().into_owned(),
                ));
            }
            ToolManagerMsg::Remove(idx) => {
                if idx < self.tools.len() {
                    let tool_id = self.tools[idx].id.clone();
                    self.tools.remove(idx);
                    self.rebuild_list(&sender);
                    self.rebuild_presets(&sender);
                    let _ = sender.output(ToolManagerOutput::ToolRemoved(tool_id));
                }
            }
            ToolManagerMsg::Close => {
                let _ = sender.output(ToolManagerOutput::Closed);
            }
        }
    }
}
