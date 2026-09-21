use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::detector::ExternalFile;

#[derive(Debug, Default, PartialEq, Eq)]
struct Selection {
    unmanaged: bool,
    managed: bool,
    backup: bool,
}

impl Selection {
    fn from_files<'a>(files: impl IntoIterator<Item = &'a ExternalFile>) -> Self {
        let mut selection = Self::default();
        for file in files {
            selection.unmanaged |= !file.is_managed_plugin;
            selection.managed |= file.is_managed_plugin;
            selection.backup |= file.is_managed_plugin && file.xedit_backup_path.is_some();
        }
        selection
    }
}

pub struct AbsorbDialog {
    files: Vec<ExternalFile>,
    file_checks: Vec<gtk::CheckButton>,
    /// Whether any of the listed files is a managed plugin.
    /// Used to conditionally show the "Adopt Changes" button.
    has_managed_plugins: bool,
    /// Whether any of the listed managed plugins has an xEdit backup path available.
    /// Used to conditionally show the "Restore from Backup" button.
    has_xedit_backup: bool,
    window: adw::Window,
    selection: Selection,
}

#[derive(Debug)]
pub enum AbsorbDialogMsg {
    SelectAll,
    SelectNone,
    SelectionChanged,
    Confirm,
    Discard,
    MarkAsVanilla,
    AdoptManaged,
    RestoreFromBackup,
    Cancel,
}

#[derive(Debug)]
pub enum AbsorbDialogOutput {
    Selected(Vec<(PathBuf, PathBuf)>),
    /// Absolute paths of files the user wants deleted from the game folder.
    Discarded(Vec<PathBuf>),
    /// Selected files should be registered in the vanilla baseline so they
    /// are no longer reported as external changes.
    MarkedAsVanilla(Vec<ExternalFile>),
    /// User chose to adopt externally-cleaned managed plugins: copy cleaned content
    /// into the deployd cache and re-hardlink so the mod stays managed.
    AdoptManagedChanges(Vec<ExternalFile>),
    /// User chose to restore managed plugins to their pre-clean state using xEdit backups.
    RestoreFromBackup(Vec<ExternalFile>),
    Cancelled,
}

#[relm4::component(pub)]
impl SimpleComponent for AbsorbDialog {
    type Init = Vec<ExternalFile>;
    type Input = AbsorbDialogMsg;
    type Output = AbsorbDialogOutput;

    view! {
        adw::Window {
            set_title: Some("External File Changes"),
            set_default_size: (680, 640),
            set_modal: true,

            adw::ToolbarView {
                add_top_bar = &adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &adw::WindowTitle {
                        set_title: "External File Changes",
                        #[watch]
                        set_subtitle: &format!("{} file(s) detected", model.files.len()),
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
                                set_title: "Files",
                                set_description: Some("Select which files to act on."),

                                add = &gtk::Box {
                                    set_orientation: gtk::Orientation::Horizontal,
                                    set_spacing: 4,

                                    gtk::Button {
                                        set_label: "All",
                                        add_css_class: "flat",
                                        connect_clicked => AbsorbDialogMsg::SelectAll,
                                    },

                                    gtk::Button {
                                        set_label: "None",
                                        add_css_class: "flat",
                                        connect_clicked => AbsorbDialogMsg::SelectNone,
                                    },
                                },

                                add = &gtk::ScrolledWindow {
                                    set_vexpand: true,
                                    set_max_content_height: 340,
                                    set_propagate_natural_height: true,
                                    set_hscrollbar_policy: gtk::PolicyType::Never,

                                    #[name = "files_list"]
                                    gtk::ListBox {
                                        set_selection_mode: gtk::SelectionMode::None,
                                        add_css_class: "boxed-list",
                                    },
                                },
                            },

                            adw::PreferencesGroup {
                                set_title: "Actions",

                                add = &adw::ActionRow {
                                    set_title: "Discard Selected",
                                    set_subtitle: "Delete selected non-managed files from the game folder",

                                    add_suffix = &gtk::Button {
                                        set_label: "Discard",
                                        #[watch]
                                        set_sensitive: model.selection.unmanaged,
                                        set_tooltip_text: Some("Delete selected unmanaged files; managed plugins cannot be discarded"),
                                        set_valign: gtk::Align::Center,
                                        add_css_class: "destructive-action",
                                        connect_clicked => AbsorbDialogMsg::Discard,
                                    },
                                },

                                add = &adw::ActionRow {
                                    set_title: "Mark as Vanilla",
                                    set_subtitle: "Remember selected non-managed files as part of the vanilla game",

                                    add_suffix = &gtk::Button {
                                        set_label: "Mark",
                                        #[watch]
                                        set_sensitive: model.selection.unmanaged,
                                        set_tooltip_text: Some("Mark as Vanilla"),
                                        set_valign: gtk::Align::Center,
                                        add_css_class: "flat",
                                        connect_clicked => AbsorbDialogMsg::MarkAsVanilla,
                                    },
                                },

                                add = &adw::ActionRow {
                                    set_title: "Restore from Backup",
                                    set_subtitle: "Restore selected managed plugins to their pre-clean state",
                                    #[watch]
                                    set_visible: model.has_xedit_backup,

                                    add_suffix = &gtk::Button {
                                        set_label: "Restore",
                                        #[watch]
                                        set_sensitive: model.selection.backup,
                                        set_tooltip_text: Some("Restore from Backup"),
                                        set_valign: gtk::Align::Center,
                                        add_css_class: "flat",
                                        connect_clicked => AbsorbDialogMsg::RestoreFromBackup,
                                    },
                                },

                                add = &adw::ActionRow {
                                    set_title: "Adopt Changes",
                                    set_subtitle: "Keep the selected managed plugin changes for future deployments",
                                    #[watch]
                                    set_visible: model.has_managed_plugins,

                                    add_suffix = &gtk::Button {
                                        set_label: "Adopt",
                                        #[watch]
                                        set_sensitive: model.selection.managed,
                                        set_tooltip_text: Some("Adopt Changes"),
                                        set_valign: gtk::Align::Center,
                                        add_css_class: "suggested-action",
                                        connect_clicked => AbsorbDialogMsg::AdoptManaged,
                                    },
                                },

                                add = &adw::ActionRow {
                                    set_title: "Create Mod",
                                    set_subtitle: "Absorb selected non-managed files into a new managed mod",
                                    #[watch]
                                    set_visible: !model.has_managed_plugins || model.files.iter().any(|f| !f.is_managed_plugin),

                                    add_suffix = &gtk::Button {
                                        set_label: "Create",
                                        #[watch]
                                        set_sensitive: model.selection.unmanaged,
                                        set_tooltip_text: Some("Create Mod"),
                                        set_valign: gtk::Align::Center,
                                        set_css_classes: if model.has_managed_plugins { &["flat"] } else { &["suggested-action"] },
                                        connect_clicked => AbsorbDialogMsg::Confirm,
                                    },
                                },
                            },
                        },
                    },
                },

                add_bottom_bar = &gtk::ActionBar {
                    pack_start = &gtk::Button {
                        set_label: "Cancel",
                        connect_clicked => AbsorbDialogMsg::Cancel,
                    },
                },
            },

            connect_close_request[sender] => move |window| {
                window.set_visible(false);
                sender.input(AbsorbDialogMsg::Cancel);
                glib::Propagation::Stop
            },
        }
    }

    fn init(
        files: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let has_managed_plugins = files.iter().any(|f| f.is_managed_plugin);
        let has_xedit_backup = files.iter().any(|f| f.xedit_backup_path.is_some());
        let mut model = AbsorbDialog {
            selection: Selection::from_files(&files),
            files,
            file_checks: Vec::new(),
            has_managed_plugins,
            has_xedit_backup,
            window: root.clone(),
        };

        let widgets = view_output!();

        for file in &model.files {
            let check = gtk::CheckButton::new();
            check.set_active(true);
            let selection_sender = sender.input_sender().clone();
            check.connect_toggled(move |_| {
                let _ = selection_sender.send(AbsorbDialogMsg::SelectionChanged);
            });
            check.set_valign(gtk::Align::Center);

            let row = adw::ActionRow::new();
            row.add_css_class("monospace");
            row.set_title_lines(1);
            if file.is_managed_plugin {
                row.set_title(&gtk::glib::markup_escape_text(&file.game_rel_original));
                row.set_tooltip_text(Some(&file.game_rel_original));
                if file.xedit_backup_path.is_some() {
                    // In-place save: both Data and cache already hold the cleaned content;
                    // a backup file is available to undo the clean if needed.
                    row.set_subtitle("Managed plugin — xEdit backup available");
                } else {
                    // Rename-save: hardlink broken, cache still holds the dirty original.
                    row.set_subtitle("Managed plugin — content modified");
                }
            } else {
                // Show the original on-disk casing so the user can distinguish vanilla
                // game files (e.g. "DLCRobot.esm") from Deployd-deployed files (lowercase).
                // Strip the "../" prefix used internally for game-root files; show a
                // subtitle instead so the location is unambiguous.
                let display = file
                    .game_rel_original
                    .strip_prefix("../")
                    .unwrap_or(&file.game_rel_original);
                row.set_title(&gtk::glib::markup_escape_text(display));
                row.set_tooltip_text(Some(display));
                if file.game_rel_original.starts_with("../") {
                    row.set_subtitle("Game root file");
                }
            }
            row.add_prefix(&check);
            row.set_activatable_widget(Some(&check));
            widgets.files_list.append(&row);
            model.file_checks.push(check);
        }

        gtk::glib::idle_add_local_once({
            let root = root.clone();
            move || root.present()
        });

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        self.selection = Selection::from_files(
            self.files
                .iter()
                .zip(&self.file_checks)
                .filter(|(_, check)| check.is_active())
                .map(|(file, _)| file),
        );
        match msg {
            AbsorbDialogMsg::SelectionChanged => {}
            AbsorbDialogMsg::SelectAll => {
                for check in &self.file_checks {
                    check.set_active(true);
                }
            }
            AbsorbDialogMsg::SelectNone => {
                for check in &self.file_checks {
                    check.set_active(false);
                }
            }
            AbsorbDialogMsg::Confirm => {
                if !self.selection.unmanaged {
                    return;
                }
                // Only absorb non-managed files into a new mod.
                let file_list: Vec<(PathBuf, PathBuf)> = self
                    .files
                    .iter()
                    .zip(self.file_checks.iter())
                    .filter(|(ef, check)| check.is_active() && !ef.is_managed_plugin)
                    .map(|(ef, _)| {
                        let dest = ef
                            .game_rel_original
                            .strip_prefix("../")
                            .unwrap_or(&ef.game_rel_original);
                        (ef.abs_path.clone(), PathBuf::from(dest))
                    })
                    .collect();
                self.window.set_visible(false);
                let _ = sender.output(AbsorbDialogOutput::Selected(file_list));
            }
            AbsorbDialogMsg::Discard => {
                if !self.selection.unmanaged {
                    return;
                }
                // Only discard non-managed files; managed plugins must use "Adopt Changes"
                // or "Restore from Backup" — deleting a managed file outright would break
                // the mod deployment.
                let paths: Vec<PathBuf> = self
                    .files
                    .iter()
                    .zip(self.file_checks.iter())
                    .filter(|(ef, check)| check.is_active() && !ef.is_managed_plugin)
                    .map(|(ef, _)| ef.abs_path.clone())
                    .collect();
                self.window.set_visible(false);
                let _ = sender.output(AbsorbDialogOutput::Discarded(paths));
            }
            AbsorbDialogMsg::MarkAsVanilla => {
                if !self.selection.unmanaged {
                    return;
                }
                // Only mark non-managed files as vanilla; managed plugins cannot be
                // treated as vanilla.
                let files: Vec<ExternalFile> = self
                    .files
                    .iter()
                    .zip(self.file_checks.iter())
                    .filter(|(ef, check)| check.is_active() && !ef.is_managed_plugin)
                    .map(|(ef, _)| ef.clone())
                    .collect();
                self.window.set_visible(false);
                let _ = sender.output(AbsorbDialogOutput::MarkedAsVanilla(files));
            }
            AbsorbDialogMsg::AdoptManaged => {
                if !self.selection.managed {
                    return;
                }
                // Adopt the selected managed plugins: the backend will copy the cleaned
                // on-disk content into the deployd cache and re-hardlink.
                let files: Vec<ExternalFile> = self
                    .files
                    .iter()
                    .zip(self.file_checks.iter())
                    .filter(|(ef, check)| check.is_active() && ef.is_managed_plugin)
                    .map(|(ef, _)| ef.clone())
                    .collect();
                self.window.set_visible(false);
                let _ = sender.output(AbsorbDialogOutput::AdoptManagedChanges(files));
            }
            AbsorbDialogMsg::RestoreFromBackup => {
                if !self.selection.backup {
                    return;
                }
                // Restore selected managed plugins from their xEdit backup.
                // Only applies to in-place saves that have a backup path recorded.
                let files: Vec<ExternalFile> = self
                    .files
                    .iter()
                    .zip(self.file_checks.iter())
                    .filter(|(ef, check)| check.is_active() && ef.xedit_backup_path.is_some())
                    .map(|(ef, _)| ef.clone())
                    .collect();
                self.window.set_visible(false);
                let _ = sender.output(AbsorbDialogOutput::RestoreFromBackup(files));
            }
            AbsorbDialogMsg::Cancel => {
                self.window.set_visible(false);
                let _ = sender.output(AbsorbDialogOutput::Cancelled);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(managed: bool, backup: bool) -> ExternalFile {
        ExternalFile {
            abs_path: PathBuf::from("Example.esp"),
            game_rel: "example.esp".into(),
            game_rel_original: "Example.esp".into(),
            is_managed_plugin: managed,
            xedit_backup_path: backup.then(|| PathBuf::from("backup/Example.esp")),
        }
    }

    // @variants: both
    #[test]
    fn managed_plugins_enable_adoption_without_enabling_discard() {
        let selection = Selection::from_files(&[file(true, false)]);
        assert!(selection.managed);
        assert!(!selection.unmanaged);
        assert!(!selection.backup);
    }

    #[test]
    fn clearing_selection_disables_all_file_actions() {
        assert_eq!(Selection::from_files(&[]), Selection::default());
    }

    #[test]
    fn mixed_selections_enable_only_the_applicable_actions() {
        assert_eq!(
            Selection::from_files(&[file(false, false)]),
            Selection {
                unmanaged: true,
                managed: false,
                backup: false
            }
        );
        assert_eq!(
            Selection::from_files(&[file(false, false), file(true, true)]),
            Selection {
                unmanaged: true,
                managed: true,
                backup: true
            }
        );
    }
}
