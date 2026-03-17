use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use adw::prelude::*;
use gtk::gdk;
use gtk::prelude::*;
use relm4::prelude::*;
use walkdir::WalkDir;

use crate::utils::fomod_resolver::{
    FomodGroupType, FomodSelections, FomodUiConfig, FomodUiGroup, FomodUiPlugin,
};

pub struct FomodDialogInit {
    pub config: FomodUiConfig,
    pub extracted_root: PathBuf,
    /// Lowercased filenames of all active plugins in the current game's modlist.
    pub active_plugin_files: HashSet<String>,
}

/// Build default selections for a FOMOD config without user input.
pub fn default_fomod_selections(config: &FomodUiConfig, active_files: &HashSet<String>) -> FomodSelections {
    FomodSelections {
        selections: compute_default_selections(config, active_files),
        flags: std::collections::HashMap::new(),
    }
}

pub struct FomodDialog {
    config: FomodUiConfig,
    extracted_root: PathBuf,
    current_step: usize,
    /// selections[step_idx][group_idx] = set of selected plugin indices
    selections: Vec<Vec<HashSet<usize>>>,
    /// Container for dynamic step content
    content_box: gtk::Box,
    /// Right-side panel that wraps the preview picture; hidden when no images
    image_panel: gtk::Box,
    /// Preview image for the currently selected plugin
    preview_picture: gtk::Picture,
}

#[derive(Debug)]
pub enum FomodDialogMsg {
    NextStep,
    PrevStep,
    TogglePlugin(usize, usize, bool),
    /// Radio selection: select exactly this plugin in the group (deselect others)
    SelectRadio(usize, usize),
    Confirm,
    Cancel,
}

#[derive(Debug)]
pub enum FomodDialogOutput {
    Confirmed(FomodSelections),
    Cancelled,
}

impl FomodDialog {
    /// Accumulate condition flags from all selected plugins in steps up to (not including) `up_to`.
    fn accumulated_flags(&self, up_to: usize) -> HashMap<String, String> {
        let mut flags = HashMap::new();
        for (step_idx, step) in self.config.steps.iter().enumerate() {
            if step_idx >= up_to {
                break;
            }
            if let Some(step_sel) = self.selections.get(step_idx) {
                for (group_idx, group) in step.groups.iter().enumerate() {
                    if let Some(group_sel) = step_sel.get(group_idx) {
                        for &plugin_idx in group_sel {
                            if let Some(plugin) = group.plugins.get(plugin_idx) {
                                for (name, value) in &plugin.condition_flags {
                                    flags.insert(name.clone(), value.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        flags
    }

    /// Accumulate all flags from all steps.
    fn all_flags(&self) -> HashMap<String, String> {
        self.accumulated_flags(self.config.steps.len())
    }

    /// Check if a step should be shown: has user-input groups AND visibility conditions are met.
    fn step_is_visible(&self, idx: usize) -> bool {
        let Some(step) = self.config.steps.get(idx) else {
            return false;
        };
        // Must have groups that need user input
        if !step.groups.iter().any(group_needs_input) {
            return false;
        }
        // Check visibility conditions (evaluated against flags from prior steps)
        if let Some(ref visible) = step.visible {
            let flags = self.accumulated_flags(idx);
            if !visible.evaluate(&flags) {
                return false;
            }
        }
        true
    }

    /// Count visible steps (for the "Step X of N" display).
    fn visible_step_count(&self) -> usize {
        (0..self.config.steps.len())
            .filter(|&i| self.step_is_visible(i))
            .count()
    }

    /// Get the 1-based position of the current step among visible steps.
    fn current_step_position(&self) -> usize {
        (0..=self.current_step)
            .filter(|&i| self.step_is_visible(i))
            .count()
    }

    fn is_last_step(&self) -> bool {
        ((self.current_step + 1)..self.config.steps.len()).all(|i| !self.step_is_visible(i))
    }

    fn is_first_step(&self) -> bool {
        (0..self.current_step).all(|i| !self.step_is_visible(i))
    }

    fn current_step_title(&self) -> String {
        self.config
            .steps
            .get(self.current_step)
            .map(|s| {
                if s.name.is_empty() {
                    format!("Step {}", self.current_step_position())
                } else {
                    s.name.clone()
                }
            })
            .unwrap_or_else(|| "FOMOD Installer".to_string())
    }

    fn step_subtitle(&self) -> String {
        format!(
            "Step {} of {}",
            self.current_step_position(),
            self.visible_step_count()
        )
    }

    fn current_step_valid(&self) -> bool {
        let Some(step) = self.config.steps.get(self.current_step) else {
            return true;
        };
        let Some(step_sel) = self.selections.get(self.current_step) else {
            return false;
        };
        for (group_idx, group) in step.groups.iter().enumerate() {
            if group.plugins.is_empty() {
                continue;
            }
            let selected = step_sel.get(group_idx).map(|s| s.len()).unwrap_or(0);
            let valid = match group.group_type {
                FomodGroupType::SelectAll => true,
                FomodGroupType::SelectExactlyOne => selected == 1,
                FomodGroupType::SelectAtLeastOne => selected >= 1,
                FomodGroupType::SelectAtMostOne => selected <= 1,
                FomodGroupType::SelectAny => true,
            };
            if !valid {
                return false;
            }
        }
        true
    }

    fn rebuild_content(&self, sender: &ComponentSender<Self>) {
        while let Some(child) = self.content_box.first_child() {
            self.content_box.remove(&child);
        }

        let Some(step) = self.config.steps.get(self.current_step) else {
            return;
        };
        let Some(step_sel) = self.selections.get(self.current_step) else {
            return;
        };

        for (group_idx, group) in step.groups.iter().enumerate() {
            if !group_needs_input(group) {
                continue;
            }
            let selected = step_sel.get(group_idx).cloned().unwrap_or_default();
            let group_widget = build_group_widget(group, group_idx, &selected, sender);
            self.content_box.append(&group_widget);
        }

        self.update_preview();
    }

    /// Update the image preview based on the first selected plugin with an image in the current step.
    fn update_preview(&self) {
        let Some(step) = self.config.steps.get(self.current_step) else {
            self.preview_picture.set_paintable(gdk::Paintable::NONE);
            self.image_panel.set_visible(false);
            return;
        };
        let Some(step_sel) = self.selections.get(self.current_step) else {
            self.preview_picture.set_paintable(gdk::Paintable::NONE);
            self.image_panel.set_visible(false);
            return;
        };

        for (group_idx, group) in step.groups.iter().enumerate() {
            let Some(group_sel) = step_sel.get(group_idx) else {
                continue;
            };
            let mut indices: Vec<usize> = group_sel.iter().copied().collect();
            indices.sort_unstable();
            for plugin_idx in indices {
                if let Some(plugin) = group.plugins.get(plugin_idx) {
                    if let Some(ref img_path) = plugin.image_path {
                        if let Some(abs_path) =
                            resolve_image_path(&self.extracted_root, img_path)
                        {
                            self.preview_picture.set_filename(Some(abs_path));
                            self.image_panel.set_visible(true);
                            return;
                        }
                    }
                }
            }
        }

        self.preview_picture.set_paintable(gdk::Paintable::NONE);
        self.image_panel.set_visible(false);
    }
}

/// Resolve a FOMOD image path (may use Windows backslashes, may have case differences)
/// to an absolute filesystem path within the extracted archive root.
fn resolve_image_path(extracted_root: &Path, image_path: &str) -> Option<PathBuf> {
    let normalized = image_path.replace('\\', "/").to_lowercase();

    // Fast path: direct case-sensitive join
    let candidate = extracted_root.join(&normalized);
    if candidate.exists() {
        return Some(candidate);
    }

    // Case-insensitive scan
    for entry in WalkDir::new(extracted_root).max_depth(6) {
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_file() {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(extracted_root) {
            let rel_lower = rel.to_string_lossy().to_lowercase().replace('\\', "/");
            if rel_lower == normalized {
                return Some(entry.path().to_path_buf());
            }
        }
    }

    None
}

fn compute_default_selections(config: &FomodUiConfig, files: &HashSet<String>) -> Vec<Vec<HashSet<usize>>> {
    config
        .steps
        .iter()
        .map(|step| {
            step.groups
                .iter()
                .map(|g| default_selection_for_group(g, files))
                .collect()
        })
        .collect()
}

fn default_selection_for_group(group: &FomodUiGroup, files: &HashSet<String>) -> HashSet<usize> {
    let plugins = &group.plugins;
    match group.group_type {
        FomodGroupType::SelectAll => (0..plugins.len()).collect(),
        FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtLeastOne => {
            let rec = find_recommended_indices(plugins, files);
            if rec.is_empty() {
                if plugins.is_empty() {
                    HashSet::new()
                } else {
                    HashSet::from([0])
                }
            } else if group.group_type == FomodGroupType::SelectExactlyOne {
                HashSet::from([rec[0]])
            } else {
                rec.into_iter().collect()
            }
        }
        FomodGroupType::SelectAtMostOne | FomodGroupType::SelectAny => {
            let rec = find_recommended_indices(plugins, files);
            if group.group_type == FomodGroupType::SelectAtMostOne && rec.len() > 1 {
                HashSet::from([rec[0]])
            } else {
                rec.into_iter().collect()
            }
        }
    }
}

fn effective_type_hint<'a>(plugin: &'a FomodUiPlugin, files: &HashSet<String>) -> &'a str {
    for (deps, type_name) in &plugin.dep_type_patterns {
        if deps.evaluate_with_files(&std::collections::HashMap::new(), files) {
            return type_name.as_str();
        }
    }
    if !plugin.dep_type_default.is_empty() {
        return plugin.dep_type_default.as_str();
    }
    plugin.type_hint.as_str()
}

fn find_recommended_indices(plugins: &[FomodUiPlugin], files: &HashSet<String>) -> Vec<usize> {
    plugins
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            let hint = effective_type_hint(p, files);
            hint == "Recommended" || hint == "Required"
        })
        .map(|(i, _)| i)
        .collect()
}

/// A group needs user input only when there is a real choice to make.
fn group_needs_input(group: &FomodUiGroup) -> bool {
    if group.plugins.is_empty() {
        return false;
    }
    match group.group_type {
        FomodGroupType::SelectAll => false,
        FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtLeastOne => {
            group.plugins.len() > 1
        }
        FomodGroupType::SelectAtMostOne | FomodGroupType::SelectAny => true,
    }
}

fn group_type_hint(group_type: &FomodGroupType) -> &'static str {
    match group_type {
        FomodGroupType::SelectAll => "All options will be installed",
        FomodGroupType::SelectExactlyOne => "Select exactly one option",
        FomodGroupType::SelectAtLeastOne => "Select at least one option",
        FomodGroupType::SelectAtMostOne => "Select at most one option",
        FomodGroupType::SelectAny => "Select any options you want",
    }
}

fn build_group_widget(
    group: &FomodUiGroup,
    group_idx: usize,
    selected: &HashSet<usize>,
    sender: &ComponentSender<FomodDialog>,
) -> gtk::Box {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
    container.set_margin_bottom(16);

    // Group name
    let name_label = gtk::Label::new(Some(&group.name));
    name_label.add_css_class("heading");
    name_label.set_halign(gtk::Align::Start);
    name_label.set_margin_start(4);
    container.append(&name_label);

    // Selection hint
    let hint_label = gtk::Label::new(Some(group_type_hint(&group.group_type)));
    hint_label.add_css_class("dim-label");
    hint_label.set_halign(gtk::Align::Start);
    hint_label.set_margin_start(4);
    hint_label.set_margin_bottom(4);
    container.append(&hint_label);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);
    list_box.add_css_class("boxed-list");

    // For radio button grouping (SelectExactlyOne)
    let radio_group: Option<gtk::CheckButton> =
        if group.group_type == FomodGroupType::SelectExactlyOne {
            Some(gtk::CheckButton::new())
        } else {
            None
        };

    for (plugin_idx, plugin) in group.plugins.iter().enumerate() {
        let row = adw::ActionRow::new();
        row.set_title(&plugin.name);
        // Only show description subtitle when it adds information beyond the title.
        // Many FOMODs have descriptions that merely restate the plugin name.
        if !plugin.description.is_empty()
            && plugin.description.to_lowercase() != plugin.name.to_lowercase()
        {
            row.set_subtitle(&plugin.description);
            row.set_subtitle_lines(2);
        }

        // Type hint badge
        if !plugin.type_hint.is_empty() {
            let badge = gtk::Label::new(Some(&plugin.type_hint));
            badge.add_css_class("dim-label");
            badge.add_css_class("caption");
            badge.set_valign(gtk::Align::Center);
            row.add_suffix(&badge);
        }

        let check = gtk::CheckButton::new();
        check.set_active(selected.contains(&plugin_idx));
        check.set_valign(gtk::Align::Center);

        match group.group_type {
            FomodGroupType::SelectAll => {
                check.set_active(true);
                check.set_sensitive(false);
            }
            FomodGroupType::SelectExactlyOne => {
                if let Some(ref group_btn) = radio_group {
                    check.set_group(Some(group_btn));
                }
                let s = sender.clone();
                check.connect_toggled(move |btn| {
                    if btn.is_active() {
                        s.input(FomodDialogMsg::SelectRadio(group_idx, plugin_idx));
                    }
                });
            }
            _ => {
                let s = sender.clone();
                check.connect_toggled(move |btn| {
                    s.input(FomodDialogMsg::TogglePlugin(
                        group_idx,
                        plugin_idx,
                        btn.is_active(),
                    ));
                });
            }
        }

        row.add_prefix(&check);
        row.set_activatable_widget(Some(&check));
        list_box.append(&row);
    }

    container.append(&list_box);
    container
}

#[relm4::component(pub)]
impl SimpleComponent for FomodDialog {
    type Init = FomodDialogInit;
    type Input = FomodDialogMsg;
    type Output = FomodDialogOutput;

    view! {
        adw::Window {
            set_title: Some("FOMOD Installer"),
            set_default_size: (780, 480),
            set_modal: true,

            gtk::Box {
                set_orientation: gtk::Orientation::Vertical,

                adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        #[watch]
                        set_title: &model.current_step_title(),
                        #[watch]
                        set_subtitle: &model.step_subtitle(),
                    },
                },

                // Main content: options on the left, image preview on the right
                gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_vexpand: true,

                    gtk::ScrolledWindow {
                        set_hexpand: true,
                        set_vexpand: true,
                        set_hscrollbar_policy: gtk::PolicyType::Never,
                        set_margin_start: 16,
                        set_margin_end: 16,
                        set_margin_top: 8,
                        set_margin_bottom: 8,

                        #[local_ref]
                        content_box -> gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 8,
                        },
                    },

                    #[local_ref]
                    image_panel -> gtk::Box {
                        set_orientation: gtk::Orientation::Vertical,
                        set_size_request: (260, -1),
                        set_visible: false,

                        gtk::Separator {
                            set_orientation: gtk::Orientation::Vertical,
                        },

                        #[local_ref]
                        preview_picture -> gtk::Picture {
                            set_can_shrink: true,
                            set_content_fit: gtk::ContentFit::Contain,
                            set_vexpand: true,
                            set_margin_all: 8,
                        },
                    },
                },

                gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_spacing: 8,
                    set_margin_all: 16,
                    set_halign: gtk::Align::End,

                    gtk::Button {
                        set_label: "Cancel",
                        connect_clicked => FomodDialogMsg::Cancel,
                    },

                    gtk::Box {
                        set_hexpand: true,
                    },

                    gtk::Button {
                        set_label: "Back",
                        #[watch]
                        set_sensitive: !model.is_first_step(),
                        connect_clicked => FomodDialogMsg::PrevStep,
                    },

                    gtk::Button {
                        set_label: "Next",
                        add_css_class: "suggested-action",
                        #[watch]
                        set_visible: !model.is_last_step(),
                        #[watch]
                        set_sensitive: model.current_step_valid(),
                        connect_clicked => FomodDialogMsg::NextStep,
                    },

                    gtk::Button {
                        set_label: "Install",
                        add_css_class: "suggested-action",
                        #[watch]
                        set_visible: model.is_last_step(),
                        #[watch]
                        set_sensitive: model.current_step_valid(),
                        connect_clicked => FomodDialogMsg::Confirm,
                    },
                },
            },

            connect_close_request[sender] => move |_| {
                sender.input(FomodDialogMsg::Cancel);
                glib::Propagation::Stop
            },
        },
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let FomodDialogInit {
            config,
            extracted_root,
            active_plugin_files,
        } = init;

        let selections = compute_default_selections(&config, &active_plugin_files);
        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let image_panel = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let preview_picture = gtk::Picture::new();

        let mut model = FomodDialog {
            config,
            extracted_root,
            current_step: 0,
            selections,
            content_box: content_box.clone(),
            image_panel: image_panel.clone(),
            preview_picture: preview_picture.clone(),
        };

        // Find first visible step
        let first_step = (0..model.config.steps.len())
            .find(|&i| model.step_is_visible(i))
            .unwrap_or(0);
        model.current_step = first_step;

        model.rebuild_content(&sender);

        let widgets = view_output!();

        // Update preview after view_output!() so the view macro's initial
        // set_visible: false on image_panel doesn't clobber our state.
        model.update_preview();

        root.present();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            FomodDialogMsg::NextStep => {
                if let Some(next) = ((self.current_step + 1)..self.config.steps.len())
                    .find(|&i| self.step_is_visible(i))
                {
                    self.current_step = next;
                    self.rebuild_content(&sender);
                }
            }
            FomodDialogMsg::PrevStep => {
                if let Some(prev) = (0..self.current_step)
                    .rev()
                    .find(|&i| self.step_is_visible(i))
                {
                    self.current_step = prev;
                    self.rebuild_content(&sender);
                }
            }
            FomodDialogMsg::TogglePlugin(group_idx, plugin_idx, active) => {
                if let Some(step_sel) = self.selections.get_mut(self.current_step)
                    && let Some(group_sel) = step_sel.get_mut(group_idx)
                {
                    if active {
                        // For SelectAtMostOne, clear others first
                        if let Some(step) = self.config.steps.get(self.current_step)
                            && let Some(group) = step.groups.get(group_idx)
                            && group.group_type == FomodGroupType::SelectAtMostOne
                        {
                            group_sel.clear();
                        }
                        group_sel.insert(plugin_idx);
                    } else {
                        group_sel.remove(&plugin_idx);
                    }
                }
                self.update_preview();
            }
            FomodDialogMsg::SelectRadio(group_idx, plugin_idx) => {
                if let Some(step_sel) = self.selections.get_mut(self.current_step)
                    && let Some(group_sel) = step_sel.get_mut(group_idx)
                {
                    group_sel.clear();
                    group_sel.insert(plugin_idx);
                }
                self.update_preview();
            }
            FomodDialogMsg::Confirm => {
                let flags = self.all_flags();
                let selections = FomodSelections {
                    selections: self.selections.clone(),
                    flags,
                };
                sender.output(FomodDialogOutput::Confirmed(selections)).ok();
            }
            FomodDialogMsg::Cancel => {
                sender.output(FomodDialogOutput::Cancelled).ok();
            }
        }
    }
}
