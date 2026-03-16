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
}

#[derive(Debug)]
pub enum PluginRowOutput {
    ToggleEnabled(DynamicIndex, bool),
}

#[relm4::factory(pub)]
impl FactoryComponent for PluginRow {
    type Init = PluginRowInit;
    type Input = ();
    type Output = PluginRowOutput;
    type CommandOutput = ();
    type ParentWidget = gtk::ListBox;

    view! {
        root = gtk::ListBoxRow {
            set_selectable: false,
            #[watch]
            set_visible: self.visible,

            gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                set_spacing: 6,
                set_margin_start: 8,
                set_margin_end: 8,
                set_margin_top: 4,
                set_margin_bottom: 4,
                // Greys out all child widgets (checkbutton + labels) when the
                // parent mod is disabled. GTK applies visual dimming automatically.
                #[watch]
                set_sensitive: self.mod_enabled,

                gtk::CheckButton {
                    // Show as unchecked when the mod is disabled so the user
                    // immediately sees the effective (inactive) state.
                    #[watch]
                    set_active: self.plugin.enabled && self.mod_enabled,
                    // Vanilla/DLC plugins are managed by the game engine, not Deployd.
                    #[watch]
                    set_sensitive: !self.is_vanilla,
                    connect_toggled[sender, index] => move |btn| {
                        sender.output(PluginRowOutput::ToggleEnabled(index.clone(), btn.is_active())).unwrap();
                    }
                },

                gtk::Label {
                    #[watch]
                    set_label: &self.display_filename,
                    set_hexpand: true,
                    set_halign: gtk::Align::Start,
                    set_ellipsize: gtk::pango::EllipsizeMode::End,
                },

                // Dirty edits indicator — red icon shown when the LOOT masterlist flags
                // this plugin's on-disk CRC. Tooltip shows ITM/UDR/NAV counts + utility.
                gtk::Image {
                    set_icon_name: Some("emblem-important-symbolic"),
                    #[watch]
                    set_visible: self.dirty_info.is_some(),
                    #[watch]
                    set_tooltip_text: Some(&match &self.dirty_info {
                        Some(info) => info.tooltip(),
                        None => String::new(),
                    }),
                    add_css_class: "error",
                },

                gtk::Image {
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
                },

                gtk::Label {
                    #[watch]
                    set_label: &self.order_label,
                    add_css_class: "dim-label",
                    add_css_class: "caption",
                },

                gtk::Label {
                    #[watch]
                    set_label: &self.mod_name,
                    add_css_class: "dim-label",
                    set_ellipsize: gtk::pango::EllipsizeMode::End,
                    set_max_width_chars: 20,
                },
            },
        }
    }

    fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        Self {
            plugin: init.plugin,
            display_filename: init.display_filename,
            mod_name: init.mod_name,
            order_label: init.order_label,
            missing_masters: init.missing_masters,
            visible: true,
            mod_enabled: init.mod_enabled,
            dirty_info: init.dirty_info,
            is_vanilla: init.is_vanilla,
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
            drag_source.connect_prepare(move |_src, _x, _y| {
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
