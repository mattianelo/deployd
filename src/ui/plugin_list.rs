use std::cell::Cell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::{DynamicIndex, FactoryComponent, FactorySender};
use relm4::prelude::*;

use crate::models::plugin::{Plugin, PluginDirtyInfo};

/// Init data for a plugin row.
pub struct PluginRowInit {
    pub plugin: Plugin,
    /// Display name shown in the panel. May differ from `plugin.filename` in casing
    /// when the on-disk file has different casing than what was stored in the archive.
    /// All internal operations (plugins.txt, DB, ordering) still use `plugin.filename`.
    pub display_filename: String,
    pub mod_name: String,
    pub order_label: String,
    pub missing_masters: Vec<String>,
    /// Whether the parent mod is currently enabled. When false the row is
    /// rendered greyed-out and the checkbox becomes non-interactive.
    pub mod_enabled: bool,
    /// LOOT dirty-edit summary for this plugin's current on-disk CRC, if any.
    /// `None` means no dirty edits recorded (or LOOT feature not enabled).
    pub dirty_info: Option<PluginDirtyInfo>,
    /// Whether this is a vanilla/DLC plugin not managed by Deployd.
    /// Vanilla rows are shown as read-only (checkbox insensitive).
    pub is_vanilla: bool,
}

#[derive(Debug)]
pub struct PluginRow {
    pub plugin: Plugin,
    pub display_filename: String,
    pub mod_name: String,
    pub search_key: String,
    pub order_label: String,
    pub missing_masters: Vec<String>,
    pub visible: bool,
    /// Mirrors the parent mod's enabled state. Toggling the mod live-updates
    /// this field so the row greys out without changing `plugin.enabled`
    /// (which preserves the user's individual plugin toggle for later restore).
    pub mod_enabled: bool,
    pub dirty_info: Option<PluginDirtyInfo>,
    /// True for vanilla/DLC plugins not managed by Deployd (checkbox read-only).
    pub is_vanilla: bool,
    /// Short type label derived from file extension: "ESM", "ESL", or "ESP".
    pub plugin_type_label: &'static str,
    /// CSS modifier class for the type badge: "plugin-badge-esm" etc.
    pub plugin_type_css: &'static str,
    pub selection_mode: bool,
    pub selected: bool,
    /// Shared with the row's DragSource; set to true only in selection mode.
    pub drag_enabled: Rc<Cell<bool>>,
}

impl PluginRow {
    fn badge_css_classes(&self) -> &'static [&'static str] {
        match self.plugin_type_css {
            "plugin-badge-esm" => &["plugin-badge", "plugin-badge-esm"],
            "plugin-badge-esl" => &["plugin-badge", "plugin-badge-esl"],
            _ => &["plugin-badge", "plugin-badge-esp"],
        }
    }
}

#[derive(Debug)]
pub enum PluginRowOutput {
    SetSelected(DynamicIndex, bool),
}

#[relm4::factory(pub)]
impl FactoryComponent for PluginRow {
    type Init = PluginRowInit;
    type Input = ();
    type Output = PluginRowOutput;
    type CommandOutput = ();
    type ParentWidget = gtk::ListBox;

    view! {
        root = adw::ActionRow {
            set_selectable: false,
            #[watch]
            set_activatable: self.selection_mode,
            #[watch]
            set_visible: self.visible,
            #[watch]
            set_title: &gtk::glib::markup_escape_text(&self.display_filename),
            #[watch]
            set_subtitle: &gtk::glib::markup_escape_text(&self.mod_name),
            set_title_lines: 1,
            set_subtitle_lines: 1,
            #[watch]
            set_sensitive: self.selection_mode || (self.mod_enabled && self.plugin.enabled),

            add_prefix = &gtk::CheckButton {
                #[watch]
                set_visible: self.selection_mode,
                #[watch]
                set_active: self.selected,
                set_can_focus: false,
                set_valign: gtk::Align::Center,
                connect_toggled[sender, index] => move |btn| {
                    sender.output(PluginRowOutput::SetSelected(index.clone(), btn.is_active())).ok();
                },
            },

            add_prefix = &gtk::Label {
                #[watch]
                set_label: self.plugin_type_label,
                #[watch]
                set_css_classes: self.badge_css_classes(),
                set_valign: gtk::Align::Center,
            },

            // Dirty edits indicator — red icon shown when the LOOT masterlist flags
            // this plugin's on-disk CRC. Tooltip shows ITM/UDR/NAV counts + utility.
            add_suffix = &gtk::Image {
                set_icon_name: Some("emblem-important-symbolic"),
                #[watch]
                set_visible: self.dirty_info.is_some(),
                #[watch]
                set_tooltip_text: Some(&match &self.dirty_info {
                    Some(info) => info.tooltip(),
                    None => String::new(),
                }),
                add_css_class: "error",
                set_valign: gtk::Align::Center,
            },

            add_suffix = &gtk::Image {
                set_icon_name: Some("dialog-warning-symbolic"),
                #[watch]
                set_visible: !self.missing_masters.is_empty(),
                #[watch]
                set_tooltip_text: Some(&if self.missing_masters.is_empty() {
                    String::new()
                } else {
                    format!("Missing master(s): {}", self.missing_masters.join(", "))
                }),
                add_css_class: "warning",
                set_valign: gtk::Align::Center,
            },

            add_suffix = &gtk::Label {
                #[watch]
                set_label: &self.order_label,
                add_css_class: "dim-label",
                add_css_class: "caption",
                set_valign: gtk::Align::Center,
            },

        }
    }

    fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        let fname_lower = init.plugin.filename.to_lowercase();
        let (plugin_type_label, plugin_type_css) = if fname_lower.ends_with(".esm") {
            ("ESM", "plugin-badge-esm")
        } else if fname_lower.ends_with(".esl") {
            ("ESL", "plugin-badge-esl")
        } else {
            ("ESP", "plugin-badge-esp")
        };
        Self {
            plugin: init.plugin,
            display_filename: init.display_filename,
            search_key: format!("{} {}", fname_lower, init.mod_name.to_lowercase()),
            mod_name: init.mod_name,
            order_label: init.order_label,
            missing_masters: init.missing_masters,
            visible: true,
            mod_enabled: init.mod_enabled,
            dirty_info: init.dirty_info,
            is_vanilla: init.is_vanilla,
            plugin_type_label,
            plugin_type_css,
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

        // Vanilla/DLC plugins have a fixed implicit load order managed by the game engine.
        // Only attach DragSource for Deployd-managed rows.
        if !self.is_vanilla {
            let drag_source = gtk::DragSource::new();
            drag_source.set_actions(gtk::gdk::DragAction::MOVE);
            let idx = index.clone();
            let drag_enabled = self.drag_enabled.clone();
            drag_source.connect_prepare(move |_src, _x, _y| {
                if !drag_enabled.get() {
                    return None;
                }
                let current = idx.current_index();
                Some(gtk::gdk::ContentProvider::for_value(
                    &format!("plugin:{current}").to_value(),
                ))
            });
            root_ref.add_controller(drag_source);
        }

        widgets
    }
}
