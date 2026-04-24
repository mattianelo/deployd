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
}

/// What kind of list item this is.
pub enum ModListItemKind {
    /// A collapsible group separator header.
    Separator {
        group_id: String,
        name: String,
        collapsed: bool,
    },
    /// A regular mod row.
    Mod(Box<ModRowInit>),
}

pub struct ModListItemInit {
    pub kind: ModListItemKind,
}

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

pub struct ModListItem {
    pub kind: ModListItemKind,
    /// Controlled by search filter and group collapse logic.
    pub visible: bool,
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
}

// ---------------------------------------------------------------------------
// Output
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ModListItemOutput {
    // From mod rows
    Remove(DynamicIndex),
    ToggleEnabled(DynamicIndex, bool),
    RenameMod(DynamicIndex, String),
    OpenProperties(DynamicIndex),
    // From separator rows
    ToggleGroupCollapse(DynamicIndex),
    DeleteGroup(DynamicIndex),
    RenameGroup(DynamicIndex, String),
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
            set_visible: self.visible,

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
                gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_spacing: 6,
                    set_margin_start: 8,
                    set_margin_end: 8,
                    set_margin_top: 4,
                    set_margin_bottom: 4,
                    #[watch]
                    set_visible: !self.is_separator(),

                    gtk::CheckButton {
                        #[watch]
                        set_active: if let ModListItemKind::Mod(r) = &self.kind { r.mod_entry.enabled } else { false },
                        connect_toggled[sender, index] => move |btn| {
                            sender.output(ModListItemOutput::ToggleEnabled(index.clone(), btn.is_active())).unwrap();
                        }
                    },

                    gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_hexpand: true,
                        set_valign: gtk::Align::Center,

                        gtk::Label {
                            #[watch]
                            set_label: if let ModListItemKind::Mod(r) = &self.kind { r.mod_entry.name.as_str() } else { "" },
                            set_halign: gtk::Align::Start,
                            set_ellipsize: gtk::pango::EllipsizeMode::End,
                            #[watch]
                            set_tooltip_text: Some(&if let ModListItemKind::Mod(r) = &self.kind {
                                format_mod_tooltip(&r.mod_entry)
                            } else {
                                String::new()
                            }),
                        },

                        gtk::Label {
                            #[watch]
                            set_label: &if let ModListItemKind::Mod(r) = &self.kind {
                                format_nexus_subtitle(
                                    r.mod_entry.version.as_deref(),
                                    r.mod_entry.author.as_deref(),
                                )
                            } else {
                                String::new()
                            },
                            set_halign: gtk::Align::Start,
                            set_ellipsize: gtk::pango::EllipsizeMode::End,
                            add_css_class: "dim-label",
                            add_css_class: "caption",
                            #[watch]
                            set_visible: matches!(&self.kind, ModListItemKind::Mod(r) if r.mod_entry.version.is_some() || r.mod_entry.author.is_some()),
                        },
                    },

                    gtk::Image {
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
                    },

                    gtk::Label {
                        #[watch]
                        set_label: if let ModListItemKind::Mod(r) = &self.kind { r.priority_label.as_str() } else { "" },
                        add_css_class: "dim-label",
                        add_css_class: "caption",
                    },

                    gtk::Image {
                        set_icon_name: Some("media-playlist-shuffle-symbolic"),
                        #[watch]
                        set_tooltip_text: Some(&if let ModListItemKind::Mod(r) = &self.kind {
                            format_conflict_tooltip("Overrides", r.overrides, &r.override_files, &r.conflicting_mod_names)
                        } else {
                            String::new()
                        }),
                        #[watch]
                        set_visible: matches!(&self.kind, ModListItemKind::Mod(r) if r.overrides > 0),
                        add_css_class: "success",
                    },

                    gtk::Image {
                        set_icon_name: Some("dialog-warning-symbolic"),
                        #[watch]
                        set_tooltip_text: Some(&if let ModListItemKind::Mod(r) = &self.kind {
                            format_conflict_tooltip("Overridden in", r.overridden_by, &r.overridden_files, &r.conflicted_by_mod_names)
                        } else {
                            String::new()
                        }),
                        #[watch]
                        set_visible: matches!(&self.kind, ModListItemKind::Mod(r) if r.overridden_by > 0),
                        add_css_class: "warning",
                    },

                    gtk::Image {
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
                    },

                    gtk::Button {
                        set_icon_name: "user-trash-symbolic",
                        set_tooltip_text: Some("Remove mod"),
                        set_valign: gtk::Align::Center,
                        add_css_class: "flat",
                        connect_clicked[sender, index] => move |_| {
                            sender.output(ModListItemOutput::Remove(index.clone())).unwrap();
                        }
                    },
                },
            },
        }
    }

    fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        Self {
            kind: init.kind,
            visible: true,
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
            root_ref.add_css_class("mod-separator-row");

            // Drag source so groups can be repositioned in the list
            let drag_source = gtk::DragSource::new();
            drag_source.set_actions(gtk::gdk::DragAction::MOVE);
            let idx = index.clone();
            drag_source.connect_prepare(move |_src, _x, _y| {
                let current = idx.current_index();
                Some(gtk::gdk::ContentProvider::for_value(
                    &format!("group:{current}").to_value(),
                ))
            });
            root_ref.add_controller(drag_source);

            // Rename button with popover for group separators
            let group_name = self.group_name().to_string();
            let entry = gtk::Entry::builder()
                .text(&group_name)
                .hexpand(true)
                .build();
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
                .tooltip_text("Rename group")
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
            drag_source.connect_prepare(move |_src, _x, _y| {
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

            if let Some(outer) = root_ref.child().and_downcast::<gtk::Box>()
                && let Some(mod_row_box) = outer.last_child().and_downcast::<gtk::Box>()
            {
                mod_row_box.append(&rename_btn);
                mod_row_box.append(&props_btn);
            }

            // Right-click gesture → open Properties dialog
            let right_click = gtk::GestureClick::new();
            right_click.set_button(3);
            let idx3 = index.clone();
            let sender3 = sender.clone();
            right_click.connect_pressed(move |gesture, _, _, _| {
                gesture.set_state(gtk::EventSequenceState::Claimed);
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
