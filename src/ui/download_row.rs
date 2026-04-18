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
    Pause(DynamicIndex),
    Resume(DynamicIndex),
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

                // Pause — shown while actively downloading
                gtk::Button {
                    set_icon_name: "media-playback-pause-symbolic",
                    set_tooltip_text: Some("Pause download"),
                    set_valign: gtk::Align::Center,
                    add_css_class: "flat",
                    add_css_class: "circular",
                    #[watch]
                    set_visible: self.entry.status == DownloadStatus::Downloading,
                    connect_clicked[sender, index] => move |_| {
                        sender.output(DownloadRowOutput::Pause(index.clone())).ok();
                    },
                },

                // Resume — shown when paused
                gtk::Button {
                    set_icon_name: "media-playback-start-symbolic",
                    set_tooltip_text: Some("Resume download"),
                    set_valign: gtk::Align::Center,
                    add_css_class: "flat",
                    add_css_class: "circular",
                    #[watch]
                    set_visible: self.entry.status == DownloadStatus::Paused,
                    connect_clicked[sender, index] => move |_| {
                        sender.output(DownloadRowOutput::Resume(index.clone())).ok();
                    },
                },

                // Install — labeled pill shown when ready or failed (not just icon)
                gtk::Button {
                    #[watch]
                    set_label: if self.entry.status == DownloadStatus::Failed {
                        "Retry"
                    } else {
                        "Install"
                    },
                    #[watch]
                    set_tooltip_text: Some(if self.entry.status == DownloadStatus::Failed {
                        "Retry install"
                    } else {
                        "Install mod"
                    }),
                    set_valign: gtk::Align::Center,
                    add_css_class: "suggested-action",
                    add_css_class: "pill",
                    add_css_class: "install-action-btn",
                    #[watch]
                    set_visible: self.entry.is_installable(),
                    connect_clicked[sender, index] => move |_| {
                        sender.output(DownloadRowOutput::Install(index.clone())).unwrap();
                    },
                },

                // Reinstall — icon only, shown when installed and archive present
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
                    },
                },

                // Rename — icon only, shown when not active
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
                    },
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

    fn init_widgets(
        &mut self,
        index: &DynamicIndex,
        root: Self::Root,
        _returned_widget: &<Self::ParentWidget as relm4::factory::FactoryView>::ReturnedWidget,
        sender: FactorySender<Self>,
    ) -> Self::Widgets {
        // Right-click context menu for metadata actions
        let popover = gtk::Popover::new();
        let menu_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .margin_top(4)
            .margin_bottom(4)
            .margin_start(4)
            .margin_end(4)
            .build();

        let fetch_btn = gtk::Button::builder()
            .label("Fetch Nexus metadata")
            .css_classes(["flat"])
            .halign(gtk::Align::Fill)
            .build();
        let clear_btn = gtk::Button::builder()
            .label("Clear metadata")
            .css_classes(["flat"])
            .halign(gtk::Align::Fill)
            .build();

        let fetch_idx = index.clone();
        let fetch_sender = sender.clone();
        fetch_btn.connect_clicked(move |_| {
            fetch_sender
                .output(DownloadRowOutput::FetchMetadata(fetch_idx.clone()))
                .ok();
        });

        let clear_idx = index.clone();
        let clear_sender = sender.clone();
        clear_btn.connect_clicked(move |_| {
            clear_sender
                .output(DownloadRowOutput::ClearMetadata(clear_idx.clone()))
                .ok();
        });

        menu_box.append(&fetch_btn);
        menu_box.append(&clear_btn);
        popover.set_child(Some(&menu_box));
        popover.set_parent(&root);

        let gesture = gtk::GestureClick::new();
        gesture.set_button(3); // right mouse button
        let pop = popover.clone();
        gesture.connect_released(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            pop.popup();
        });
        root.add_controller(gesture);

        let widgets = view_output!();
        widgets
    }
}

impl DownloadRow {
    fn display_name(&self) -> String {
        self.entry.mod_name.clone()
    }
}

fn status_icon(status: &DownloadStatus) -> &'static str {
    match status {
        DownloadStatus::Downloading => "folder-download-symbolic",
        DownloadStatus::Paused => "media-playback-pause-symbolic",
        DownloadStatus::Downloaded => "document-save-symbolic",
        DownloadStatus::Extracting => "package-x-generic-symbolic",
        DownloadStatus::Installed => "object-select-symbolic",
        DownloadStatus::Failed => "dialog-error-symbolic",
    }
}

fn status_css(status: &DownloadStatus) -> Vec<&'static str> {
    match status {
        DownloadStatus::Downloading => vec!["accent"],
        DownloadStatus::Paused => vec!["dim-label"],
        DownloadStatus::Downloaded => vec!["success"],
        DownloadStatus::Extracting => vec!["accent"],
        DownloadStatus::Installed => vec!["success"],
        DownloadStatus::Failed => vec!["error"],
    }
}
