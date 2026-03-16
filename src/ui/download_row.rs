use gtk::prelude::*;
use relm4::factory::{DynamicIndex, FactoryComponent, FactorySender};
use relm4::prelude::*;

use crate::models::download::{DownloadEntry, DownloadStatus};

#[derive(Debug)]
pub struct DownloadRow {
    pub entry: DownloadEntry,
    pub visible: bool,
}

#[derive(Debug)]
pub enum DownloadRowOutput {
    Install(DynamicIndex),
    Reinstall(DynamicIndex),
    FetchMetadata(DynamicIndex),
    ClearMetadata(DynamicIndex),
    Rename(DynamicIndex),
}

#[relm4::factory(pub)]
impl FactoryComponent for DownloadRow {
    type Init = DownloadEntry;
    type Input = ();
    type Output = DownloadRowOutput;
    type CommandOutput = ();
    type ParentWidget = gtk::ListBox;

    view! {
        root = gtk::ListBoxRow {
            set_selectable: false,
            #[watch]
            set_visible: self.visible,

            gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                set_spacing: 8,
                set_margin_start: 12,
                set_margin_end: 12,
                set_margin_top: 8,
                set_margin_bottom: 8,

                gtk::Image {
                    #[watch]
                    set_icon_name: Some(status_icon(&self.entry.status)),
                    #[watch]
                    set_css_classes: &status_css(&self.entry.status),
                },

                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    set_hexpand: true,
                    set_valign: gtk::Align::Center,
                    set_spacing: 2,

                    gtk::Label {
                        #[watch]
                        set_label: &self.display_name(),
                        set_halign: gtk::Align::Start,
                        set_ellipsize: gtk::pango::EllipsizeMode::End,
                        set_width_request: 1,
                    },

                    // File-specific label from Nexus (e.g. "Main File", "Textures 4K").
                    // Always shown when available so multiple files from the same mod page
                    // are visually distinct regardless of their primary/non-primary status.
                    gtk::Label {
                        #[watch]
                        set_label: self.entry.nexus_file_name.as_deref().unwrap_or(""),
                        #[watch]
                        set_visible: self.entry.metadata_fetched
                            && self.entry.nexus_file_name.is_some(),
                        set_halign: gtk::Align::Start,
                        set_ellipsize: gtk::pango::EllipsizeMode::End,
                        add_css_class: "dim-label",
                        add_css_class: "caption",
                    },

                    gtk::Label {
                        #[watch]
                        set_label: &self.entry.status_msg,
                        set_halign: gtk::Align::Start,
                        set_ellipsize: gtk::pango::EllipsizeMode::End,
                        add_css_class: "dim-label",
                        add_css_class: "caption",
                    },

                    gtk::Label {
                        #[watch]
                        set_label: self.entry.error_msg.as_deref().unwrap_or(""),
                        set_halign: gtk::Align::Start,
                        set_wrap: true,
                        set_max_width_chars: 30,
                        add_css_class: "caption",
                        add_css_class: "error",
                        #[watch]
                        set_visible: self.entry.error_msg.is_some(),
                    },

                    gtk::ProgressBar {
                        #[watch]
                        set_fraction: self.entry.progress,
                        #[watch]
                        set_visible: self.entry.status == DownloadStatus::Downloading
                            || self.entry.status == DownloadStatus::Extracting,
                    },
                },

                gtk::Button {
                    set_icon_name: "document-edit-symbolic",
                    set_tooltip_text: Some("Rename"),
                    set_valign: gtk::Align::Center,
                    add_css_class: "flat",
                    add_css_class: "circular",
                    #[watch]
                    set_visible: !self.entry.is_active(),
                    connect_clicked[sender, index] => move |_| {
                        sender.output(DownloadRowOutput::Rename(index.clone())).ok();
                    }
                },

                gtk::Button {
                    set_icon_name: "emblem-synchronizing-symbolic",
                    set_tooltip_text: Some("Fetch Nexus metadata"),
                    set_valign: gtk::Align::Center,
                    add_css_class: "flat",
                    add_css_class: "circular",
                    #[watch]
                    set_visible: self.entry.nexus_ids.is_some() && !self.entry.metadata_fetched,
                    connect_clicked[sender, index] => move |_| {
                        sender.output(DownloadRowOutput::FetchMetadata(index.clone())).unwrap();
                    }
                },

                gtk::Button {
                    #[watch]
                    set_icon_name: if self.entry.status == DownloadStatus::Failed {
                        "view-refresh-symbolic"
                    } else {
                        "package-x-generic-symbolic"
                    },
                    #[watch]
                    set_tooltip_text: Some(if self.entry.status == DownloadStatus::Failed {
                        "Retry install"
                    } else {
                        "Install"
                    }),
                    set_valign: gtk::Align::Center,
                    add_css_class: "flat",
                    add_css_class: "circular",
                    #[watch]
                    set_visible: self.entry.is_installable(),
                    connect_clicked[sender, index] => move |_| {
                        sender.output(DownloadRowOutput::Install(index.clone())).unwrap();
                    }
                },

                gtk::Button {
                    set_icon_name: "view-refresh-symbolic",
                    set_tooltip_text: Some("Reinstall (replace existing mod)"),
                    set_valign: gtk::Align::Center,
                    add_css_class: "flat",
                    add_css_class: "circular",
                    #[watch]
                    set_visible: self.entry.status == DownloadStatus::Installed
                        && self.entry.archive_path.is_some(),
                    connect_clicked[sender, index] => move |_| {
                        sender.output(DownloadRowOutput::Reinstall(index.clone())).ok();
                    }
                },

                gtk::Button {
                    set_icon_name: "edit-clear-symbolic",
                    set_tooltip_text: Some("Clear metadata (re-fetch later)"),
                    set_valign: gtk::Align::Center,
                    add_css_class: "flat",
                    add_css_class: "circular",
                    #[watch]
                    set_visible: self.entry.metadata_fetched,
                    connect_clicked[sender, index] => move |_| {
                        sender.output(DownloadRowOutput::ClearMetadata(index.clone())).unwrap();
                    }
                },
            },
        }
    }

    fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        Self {
            entry: init,
            visible: true,
        }
    }
}

impl DownloadRow {
    /// Primary display name: always the mod name once metadata is fetched.
    /// The per-file Nexus label is shown separately as a subtitle below.
    fn display_name(&self) -> String {
        self.entry.mod_name.clone()
    }
}

fn status_icon(status: &DownloadStatus) -> &'static str {
    match status {
        DownloadStatus::Downloading => "folder-download-symbolic",
        DownloadStatus::Downloaded => "document-save-symbolic",
        DownloadStatus::Extracting => "package-x-generic-symbolic",
        DownloadStatus::Installed => "object-select-symbolic",
        DownloadStatus::Failed => "dialog-error-symbolic",
    }
}

fn status_css(status: &DownloadStatus) -> Vec<&'static str> {
    match status {
        DownloadStatus::Downloading => vec!["accent"],
        DownloadStatus::Downloaded => vec!["success"],
        DownloadStatus::Extracting => vec!["accent"],
        DownloadStatus::Installed => vec!["success"],
        DownloadStatus::Failed => vec!["error"],
    }
}
