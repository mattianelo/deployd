use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::{DynamicIndex, FactoryComponent, FactorySender};
use relm4::prelude::*;

use crate::models::mod_entry::ModEntry;

// ---------------------------------------------------------------------------
// Init types
// ---------------------------------------------------------------------------

/// Init data for a mod row (the non-separator case).
pub struct ModRowInit {
    pub mod_entry: ModEntry,
    pub priority_label: String,
    pub overrides: usize,
    pub overridden_by: usize,
    pub override_files: Vec<String>,
    pub overridden_files: Vec<String>,
    pub conflicting_mod_names: Vec<String>,
    pub conflicted_by_mod_names: Vec<String>,
    /// True when the source archive is outside the downloads folder.
    /// Controls visibility of the "Reinstall from archive" button.
    pub reinstall_from_file: bool,
}

/// Preset group colors. Stored as short name strings; rendered via CSS classes.
pub const GROUP_COLOR_NAMES: &[&str] =
    &["red", "orange", "yellow", "green", "teal", "blue", "purple", "pink"];

/// What kind of list item this is.
pub enum ModListItemKind {
    /// A collapsible group separator header.
    Separator {
        group_id: String,
        name: String,
        collapsed: bool,
        color: Option<String>,
    },
    /// A regular mod row.
    Mod(Box<ModRowInit>),
}

pub struct ModListItemInit {
    pub kind: ModListItemKind,
    pub visible: bool,
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

pub struct ModListItem {
    pub kind: ModListItemKind,
    /// Controlled by search filter and group collapse logic.
    pub visible: bool,
    pub selection_mode: bool,
    pub selected: bool,
    /// Shared with the row's DragSource; set to true only in selection mode.
    pub drag_enabled: Rc<Cell<bool>>,
}

impl std::fmt::Debug for ModListItem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModListItem")
            .field("visible", &self.visible)
            .field("is_separator", &self.is_separator())
            .finish()
    }
}

impl ModListItem {
    pub fn is_separator(&self) -> bool {
        matches!(self.kind, ModListItemKind::Separator { .. })
    }

    pub fn is_collapsed(&self) -> bool {
        matches!(
            self.kind,
            ModListItemKind::Separator {
                collapsed: true,
                ..
            }
        )
    }

    pub fn group_name(&self) -> &str {
        if let ModListItemKind::Separator { name, .. } = &self.kind {
            name.as_str()
        } else {
            ""
        }
    }

    /// CSS classes for the group color dot widget.
    pub fn group_color_classes(&self) -> Vec<&'static str> {
        let mut classes = vec!["group-color-dot"];
        if let ModListItemKind::Separator { color: Some(c), .. } = &self.kind {
            match c.as_str() {
                "red"    => classes.push("color-red"),
                "orange" => classes.push("color-orange"),
                "yellow" => classes.push("color-yellow"),
                "green"  => classes.push("color-green"),
                "teal"   => classes.push("color-teal"),
                "blue"   => classes.push("color-blue"),
                "purple" => classes.push("color-purple"),
                "pink"   => classes.push("color-pink"),
                _        => {}
            }
        }
        classes
    }

    /// Returns the mod name for search filtering (empty for separators).
    pub fn mod_name(&self) -> &str {
        if let ModListItemKind::Mod(init) = &self.kind {
            init.mod_entry.name.as_str()
        } else {
            ""
        }
    }

    /// Returns the mod ID for database operations (None for separators).
    pub fn mod_id(&self) -> Option<&str> {
        if let ModListItemKind::Mod(init) = &self.kind {
            Some(init.mod_entry.id.as_str())
        } else {
            None
        }
    }

    /// Returns a shared reference to the ModRowInit (None for separators).
    pub fn mod_row(&self) -> Option<&ModRowInit> {
        if let ModListItemKind::Mod(init) = &self.kind {
            Some(init)
        } else {
            None
        }
    }

    /// Returns a mutable reference to the underlying ModEntry (None for separators).
    pub fn mod_entry_mut(&mut self) -> Option<&mut crate::models::mod_entry::ModEntry> {
        if let ModListItemKind::Mod(init) = &mut self.kind {
            Some(&mut init.mod_entry)
        } else {
            None
        }
    }

    /// Returns a mutable reference to the ModRowInit (None for separators).
    pub fn mod_row_mut(&mut self) -> Option<&mut ModRowInit> {
        if let ModListItemKind::Mod(init) = &mut self.kind {
            Some(init)
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ModListItemOutput {
    // From mod rows
    RenameMod(DynamicIndex, String),
    OpenProperties(DynamicIndex),
    Reinstall(DynamicIndex),
    // From separator rows
    ToggleGroupCollapse(DynamicIndex),
    DeleteGroup(DynamicIndex),
    RenameGroup(DynamicIndex, String),
    SetGroupColor(DynamicIndex, Option<String>),
    SetSelected(DynamicIndex, bool),
}

// ---------------------------------------------------------------------------
// Factory component
// ---------------------------------------------------------------------------

#[relm4::factory(pub)]
impl FactoryComponent for ModListItem {
    type Init = ModListItemInit;
    type Input = ();
    type Output = ModListItemOutput;
    type CommandOutput = ();
    type ParentWidget = gtk::ListBox;

    view! {
        root = gtk::ListBoxRow {
            set_selectable: false,
            #[watch]
            set_activatable: self.selection_mode && !self.is_separator(),
            #[watch]
            set_visible: self.visible,
            #[watch]
            set_sensitive: self.selection_mode || match &self.kind {
                ModListItemKind::Mod(r) => r.mod_entry.enabled,
                ModListItemKind::Separator { .. } => true,
            },
            #[watch]
            set_css_classes: match &self.kind {
                ModListItemKind::Mod(r) if r.mod_entry.enabled && self.selected => &["mod-row", "mod-row-enabled", "mod-row-selected"],
                ModListItemKind::Mod(r) if r.mod_entry.enabled                  => &["mod-row", "mod-row-enabled"],
                ModListItemKind::Mod(_) if self.selected                        => &["mod-row", "mod-row-selected"],
                ModListItemKind::Mod(_)                                         => &["mod-row"],
                ModListItemKind::Separator { .. }                               => &["mod-separator-row"],
            },

            // Outer vertical box — one child visible at a time
            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                // ── SEPARATOR HEADER ─────────────────────────────────────
                gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_spacing: 6,
                    set_margin_start: 8,
                    set_margin_end: 8,
                    set_margin_top: 3,
                    set_margin_bottom: 3,
                    #[watch]
                    set_visible: self.is_separator(),
                    add_css_class: "dim-label",

                    gtk::Box {
                        #[watch]
                        set_css_classes: &self.group_color_classes(),
                    },

                    gtk::Label {
                        #[watch]
                        set_label: self.group_name(),
                        set_hexpand: true,
                        set_halign: gtk::Align::Start,
                        set_ellipsize: gtk::pango::EllipsizeMode::End,
                    },

                    gtk::Button {
                        #[watch]
                        set_icon_name: if self.is_collapsed() { "go-next-symbolic" } else { "go-down-symbolic" },
                        set_tooltip_text: Some("Collapse / expand"),
                        set_valign: gtk::Align::Center,
                        add_css_class: "flat",
                        add_css_class: "circular",
                        connect_clicked[sender, index] => move |_| {
                            sender.output(ModListItemOutput::ToggleGroupCollapse(index.clone())).unwrap();
                        }
                    },

                    gtk::Button {
                        set_icon_name: "user-trash-symbolic",
                        set_tooltip_text: Some("Delete group"),
                        set_valign: gtk::Align::Center,
                        add_css_class: "flat",
                        add_css_class: "circular",
                        connect_clicked[sender, index] => move |_| {
                            sender.output(ModListItemOutput::DeleteGroup(index.clone())).unwrap();
                        }
                    },
                },

                // ── MOD ROW ──────────────────────────────────────────────
                #[name = "mod_action_row"]
                adw::ActionRow {
                    #[watch]
                    set_visible: !self.is_separator(),
                    #[watch]
                    set_title: &if let ModListItemKind::Mod(r) = &self.kind {
                        gtk::glib::markup_escape_text(&r.mod_entry.name).to_string()
                    } else {
                        String::new()
                    },
                    #[watch]
                    set_subtitle: &if let ModListItemKind::Mod(r) = &self.kind {
                        gtk::glib::markup_escape_text(&format_nexus_subtitle(
                            r.mod_entry.version.as_deref(),
                            r.mod_entry.author.as_deref(),
                        )).to_string()
                    } else {
                        String::new()
                    },
                    set_title_lines: 1,
                    set_subtitle_lines: 1,
                    #[watch]
                    set_tooltip_text: Some(&if let ModListItemKind::Mod(r) = &self.kind {
                        format_mod_tooltip(&r.mod_entry)
                    } else {
                        String::new()
                    }),

                    add_prefix = &gtk::CheckButton {
                        #[watch]
                        set_visible: self.selection_mode,
                        #[watch]
                        set_active: self.selected,
                        set_can_focus: false,
                        set_valign: gtk::Align::Center,
                        connect_toggled[sender, index] => move |btn| {
                            sender.output(ModListItemOutput::SetSelected(index.clone(), btn.is_active())).ok();
                        },
                    },

                    add_suffix = &gtk::Button {
                        set_icon_name: "view-refresh-symbolic",
                        set_tooltip_text: Some("Reinstall from archive"),
                        set_valign: gtk::Align::Center,
                        add_css_class: "flat",
                        #[watch]
                        set_visible: matches!(&self.kind, ModListItemKind::Mod(r) if r.reinstall_from_file),
                        connect_clicked[sender, index] => move |_| {
                            sender.output(ModListItemOutput::Reinstall(index.clone())).unwrap();
                        }
                    },

                    add_suffix = &gtk::Image {
                        set_icon_name: Some("software-update-available-symbolic"),
                        #[watch]
                        set_tooltip_text: Some(&if let ModListItemKind::Mod(r) = &self.kind {
                            format!("Update available: v{}", r.mod_entry.latest_version.as_deref().unwrap_or("?"))
                        } else {
                            String::new()
                        }),
                        #[watch]
                        set_visible: matches!(&self.kind, ModListItemKind::Mod(r) if has_update(&r.mod_entry)),
                        add_css_class: "accent",
                        set_valign: gtk::Align::Center,
                    },

                    add_suffix = &gtk::Label {
                        #[watch]
                        set_label: if let ModListItemKind::Mod(r) = &self.kind { r.priority_label.as_str() } else { "" },
                        add_css_class: "dim-label",
                        add_css_class: "caption",
                        set_valign: gtk::Align::Center,
                    },

                    add_suffix = &gtk::Image {
                        set_icon_name: Some("media-record-symbolic"),
                        #[watch]
                        set_tooltip_text: Some(&if let ModListItemKind::Mod(r) = &self.kind {
                            format_conflict_tooltip_combined(r)
                        } else {
                            String::new()
                        }),
                        #[watch]
                        set_visible: matches!(&self.kind, ModListItemKind::Mod(r) if r.overrides > 0 || r.overridden_by > 0),
                        #[watch]
                        set_css_classes: if let ModListItemKind::Mod(r) = &self.kind {
                            if r.overridden_by > 0 { &["warning"] } else { &["success"] }
                        } else {
                            &["success"]
                        },
                        set_valign: gtk::Align::Center,
                    },

                    add_suffix = &gtk::Image {
                        set_icon_name: Some("emblem-documents-symbolic"),
                        #[watch]
                        set_tooltip_text: Some(&if let ModListItemKind::Mod(r) = &self.kind {
                            r.mod_entry.notes.as_deref().unwrap_or("").to_string()
                        } else {
                            String::new()
                        }),
                        #[watch]
                        set_visible: matches!(
                            &self.kind,
                            ModListItemKind::Mod(r) if r.mod_entry.notes.as_ref().is_some_and(|n| !n.is_empty())
                        ),
                        set_valign: gtk::Align::Center,
                    },
                },
            },
        }
    }

    fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        Self {
            kind: init.kind,
            visible: init.visible,
            selection_mode: false,
            selected: false,
            drag_enabled: Rc::new(Cell::new(false)),
        }
    }

    fn init_widgets(
        &mut self,
        index: &DynamicIndex,
        root: Self::Root,
        _returned_widget: &<Self::ParentWidget as relm4::factory::FactoryView>::ReturnedWidget,
        sender: FactorySender<Self>,
    ) -> Self::Widgets {
        let root_ref = root.clone();
        let widgets = view_output!();

        if self.is_separator() {
            // Drag source so groups can be repositioned in the list
            let drag_source = gtk::DragSource::new();
            drag_source.set_actions(gtk::gdk::DragAction::MOVE);
            let idx = index.clone();
            let drag_enabled = self.drag_enabled.clone();
            drag_source.connect_prepare(move |_src, _x, _y| {
                if !drag_enabled.get() { return None; }
                let current = idx.current_index();
                Some(gtk::gdk::ContentProvider::for_value(
                    &format!("group:{current}").to_value(),
                ))
            });
            root_ref.add_controller(drag_source);

            // Rename / color button with popover for group separators
            let group_name = self.group_name().to_string();
            let entry = gtk::Entry::builder()
                .text(&group_name)
                .hexpand(true)
                .build();
            let apply_btn = gtk::Button::builder()
                .label("Rename")
                .css_classes(["suggested-action"])
                .build();
            let rename_row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(4)
                .build();
            rename_row.append(&entry);
            rename_row.append(&apply_btn);

            // Build popover first so swatch closures can close it on click.
            let popover_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .margin_start(6)
                .margin_end(6)
                .margin_top(6)
                .margin_bottom(6)
                .build();
            let popover = gtk::Popover::builder().child(&popover_box).build();

            // Color palette row
            let color_row = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(4)
                .build();
            // "None" clear button
            {
                let clear_btn = gtk::Button::builder()
                    .icon_name("edit-clear-symbolic")
                    .tooltip_text("No color")
                    .css_classes(["flat", "circular"])
                    .build();
                let idx = index.clone();
                let s = sender.clone();
                let p = popover.clone();
                clear_btn.connect_clicked(move |_| {
                    s.output(ModListItemOutput::SetGroupColor(idx.clone(), None)).ok();
                    p.popdown();
                });
                color_row.append(&clear_btn);
            }
            for &color_name in crate::ui::mod_list::GROUP_COLOR_NAMES {
                let swatch = gtk::Button::builder()
                    .tooltip_text(color_name)
                    .css_classes(["color-swatch", color_name])
                    .build();
                let idx = index.clone();
                let s = sender.clone();
                let p = popover.clone();
                let name = color_name.to_string();
                swatch.connect_clicked(move |_| {
                    s.output(ModListItemOutput::SetGroupColor(idx.clone(), Some(name.clone()))).ok();
                    p.popdown();
                });
                color_row.append(&swatch);
            }
            popover_box.append(&color_row);
            popover_box.append(&rename_row);

            let rename_btn = gtk::MenuButton::builder()
                .icon_name("document-edit-symbolic")
                .tooltip_text("Rename group / set color")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .popover(&popover)
                .build();

            let idx = index.clone();
            let sender2 = sender.clone();
            let popover_ref = popover.clone();
            apply_btn.connect_clicked(move |_| {
                let new_name = entry.text().to_string();
                if !new_name.is_empty() {
                    sender2
                        .output(ModListItemOutput::RenameGroup(idx.clone(), new_name))
                        .unwrap();
                }
                popover_ref.popdown();
            });

            // Insert rename button between the collapse and delete buttons.
            // Widget tree: root → outer_box (vertical) → first child = separator_box
            // separator_box children: Label, Button(collapse), Button(delete)
            if let Some(outer) = root_ref.child().and_downcast::<gtk::Box>()
                && let Some(sep_box) = outer.first_child().and_downcast::<gtk::Box>()
            {
                // Insert before the delete button (last child)
                if let Some(delete_btn) = sep_box.last_child() {
                    sep_box.insert_child_after(&rename_btn, delete_btn.prev_sibling().as_ref());
                }
            }
        } else {
            // Drag source for reordering mod rows
            let drag_source = gtk::DragSource::new();
            drag_source.set_actions(gtk::gdk::DragAction::MOVE);
            let idx = index.clone();
            let drag_enabled = self.drag_enabled.clone();
            drag_source.connect_prepare(move |_src, _x, _y| {
                if !drag_enabled.get() { return None; }
                let current = idx.current_index();
                Some(gtk::gdk::ContentProvider::for_value(
                    &format!("mod:{current}").to_value(),
                ))
            });
            root_ref.add_controller(drag_source);

            // Rename button with popover for mod rows
            let mod_name = if let ModListItemKind::Mod(r) = &self.kind {
                r.mod_entry.name.clone()
            } else {
                String::new()
            };
            let entry = gtk::Entry::builder().text(&mod_name).hexpand(true).build();
            let apply_btn = gtk::Button::builder()
                .label("Rename")
                .css_classes(["suggested-action"])
                .build();
            let popover_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(4)
                .margin_start(4)
                .margin_end(4)
                .margin_top(4)
                .margin_bottom(4)
                .build();
            popover_box.append(&entry);
            popover_box.append(&apply_btn);
            let popover = gtk::Popover::builder().child(&popover_box).build();

            let rename_btn = gtk::MenuButton::builder()
                .icon_name("document-edit-symbolic")
                .tooltip_text("Rename mod")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .popover(&popover)
                .build();

            let idx2 = index.clone();
            let sender2 = sender.clone();
            let popover_ref = popover.clone();
            apply_btn.connect_clicked(move |_| {
                let new_name = entry.text().to_string();
                if !new_name.is_empty() {
                    sender2
                        .output(ModListItemOutput::RenameMod(idx2.clone(), new_name))
                        .unwrap();
                }
                popover_ref.popdown();
            });

            // Append rename and properties buttons to the mod row box.
            // root -> outer_box (vertical) -> last child = mod_row_box
            let props_btn = gtk::Button::builder()
                .icon_name("document-properties-symbolic")
                .tooltip_text("Properties")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();

            let idx4 = index.clone();
            let sender4 = sender.clone();
            props_btn.connect_clicked(move |_| {
                sender4
                    .output(ModListItemOutput::OpenProperties(idx4.clone()))
                    .unwrap();
            });

            widgets.mod_action_row.add_suffix(&rename_btn);
            widgets.mod_action_row.add_suffix(&props_btn);

            // Right-click gesture → open Properties dialog
            let right_click = gtk::GestureClick::new();
            right_click.set_button(3);
            let idx3 = index.clone();
            let sender3 = sender.clone();
            right_click.connect_pressed(move |gesture, _, _, _| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                if let Some(widget) = gesture.widget() {
                    widget.unset_state_flags(gtk::StateFlags::PRELIGHT);
                }
                sender3
                    .output(ModListItemOutput::OpenProperties(idx3.clone()))
                    .unwrap();
            });
            root_ref.add_controller(right_click);
        }

        widgets
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn format_install_date(date: &Option<String>) -> String {
    match date {
        Some(d) => d.split('T').next().unwrap_or(d).to_string(),
        None => "Unknown date".to_string(),
    }
}

fn format_nexus_subtitle(version: Option<&str>, author: Option<&str>) -> String {
    match (version, author) {
        (Some(v), Some(a)) => format!("v{v} by {a}"),
        (Some(v), None) => format!("v{v}"),
        (None, Some(a)) => format!("by {a}"),
        (None, None) => String::new(),
    }
}

fn has_update(entry: &ModEntry) -> bool {
    if let (Some(current), Some(latest)) = (&entry.version, &entry.latest_version) {
        !current.is_empty() && !latest.is_empty() && current != latest
    } else {
        false
    }
}

fn format_mod_tooltip(entry: &ModEntry) -> String {
    let mut parts = vec![entry.name.clone()];
    if let Some(desc) = &entry.nexus_description
        && !desc.is_empty()
    {
        parts.push(desc.clone());
    }
    parts.push(format!(
        "Installed: {}",
        format_install_date(&entry.installed_at)
    ));
    parts.join("\n")
}

fn format_conflict_tooltip(
    label: &str,
    count: usize,
    files: &[String],
    mod_names: &[String],
) -> String {
    const MAX_FILES: usize = 10;
    let mut tooltip = format!("{label} {count} file(s)");
    if !mod_names.is_empty() {
        tooltip.push_str(" \u{2014} ");
        tooltip.push_str(&mod_names.join(", "));
    }
    if !files.is_empty() {
        tooltip.push(':');
        for f in files.iter().take(MAX_FILES) {
            tooltip.push_str(&format!("\n  {f}"));
        }
        if files.len() > MAX_FILES {
            tooltip.push_str(&format!("\n  ...and {} more", files.len() - MAX_FILES));
        }
    }
    tooltip
}

fn format_conflict_tooltip_combined(r: &ModRowInit) -> String {
    let mut parts = Vec::new();
    if r.overrides > 0 {
        parts.push(format_conflict_tooltip(
            "Overrides",
            r.overrides,
            &r.override_files,
            &r.conflicting_mod_names,
        ));
    }
    if r.overridden_by > 0 {
        parts.push(format_conflict_tooltip(
            "Overridden in",
            r.overridden_by,
            &r.overridden_files,
            &r.conflicted_by_mod_names,
        ));
    }
    parts.join("\n\n")
}
