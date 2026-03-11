use std::collections::{HashMap, HashSet};

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::utils::fomod_resolver::{
    FomodGroupType, FomodSelections, FomodUiConfig, FomodUiGroup, FomodUiPlugin,
};

/// Build default selections for a FOMOD config without user input.
pub fn default_fomod_selections(config: &FomodUiConfig) -> FomodSelections {
    FomodSelections {
        selections: compute_default_selections(config),
        flags: std::collections::HashMap::new(),
    }
}

pub struct FomodDialog {
    config: FomodUiConfig,
    current_step: usize,
    /// selections[step_idx][group_idx] = set of selected plugin indices
    selections: Vec<Vec<HashSet<usize>>>,
    /// Container for dynamic step content
    content_box: gtk::Box,
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
    }
}

fn compute_default_selections(config: &FomodUiConfig) -> Vec<Vec<HashSet<usize>>> {
    config
        .steps
        .iter()
        .map(|step| {
            step.groups
                .iter()
                .map(default_selection_for_group)
                .collect()
        })
        .collect()
}

fn default_selection_for_group(group: &FomodUiGroup) -> HashSet<usize> {
    let plugins = &group.plugins;
    match group.group_type {
        FomodGroupType::SelectAll => (0..plugins.len()).collect(),
        FomodGroupType::SelectExactlyOne | FomodGroupType::SelectAtLeastOne => {
            let rec = find_recommended_indices(plugins);
            if rec.is_empty() {
                // Fall back to first plugin
                if plugins.is_empty() {
                    HashSet::new()
                } else {
                    HashSet::from([0])
                }
            } else {
                // For SelectExactlyOne, only take the first recommended
                if group.group_type == FomodGroupType::SelectExactlyOne {
                    HashSet::from([rec[0]])
                } else {
                    rec.into_iter().collect()
                }
            }
        }
        FomodGroupType::SelectAtMostOne | FomodGroupType::SelectAny => {
            let rec = find_recommended_indices(plugins);
            if group.group_type == FomodGroupType::SelectAtMostOne && rec.len() > 1 {
                HashSet::from([rec[0]])
            } else {
                rec.into_iter().collect()
            }
        }
    }
}

fn find_recommended_indices(plugins: &[FomodUiPlugin]) -> Vec<usize> {
    plugins
        .iter()
        .enumerate()
        .filter(|(_, p)| p.type_hint == "Recommended" || p.type_hint == "Required")
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
    type Init = FomodUiConfig;
    type Input = FomodDialogMsg;
    type Output = FomodDialogOutput;

    view! {
        adw::Window {
            set_title: Some("FOMOD Installer"),
            set_default_size: (520, 420),
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

                gtk::ScrolledWindow {
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
        config: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let selections = compute_default_selections(&config);
        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 8);

        let mut model = FomodDialog {
            config,
            current_step: 0,
            selections,
            content_box: content_box.clone(),
        };

        // Find first visible step
        let first_step = (0..model.config.steps.len())
            .find(|&i| model.step_is_visible(i))
            .unwrap_or(0);
        model.current_step = first_step;

        // Build initial step content before view_output!() moves model
        model.rebuild_content(&sender);

        let widgets = view_output!();

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
            }
            FomodDialogMsg::SelectRadio(group_idx, plugin_idx) => {
                if let Some(step_sel) = self.selections.get_mut(self.current_step)
                    && let Some(group_sel) = step_sel.get_mut(group_idx)
                {
                    group_sel.clear();
                    group_sel.insert(plugin_idx);
                }
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
