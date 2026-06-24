use adw::prelude::*;
use gtk::prelude::*;
use relm4::factory::{DynamicIndex, FactoryComponent, FactorySender};
use relm4::prelude::*;

use crate::models::download::{DownloadEntry, DownloadStatus};

#[derive(Debug)]
pub struct DownloadRow {
    pub entry: DownloadEntry,
    pub visible: bool,
    // Stored so update_view can reactively relabel it without needing it in view!.
    hide_btn: gtk::Button,
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
    Delete(DynamicIndex),
    HideDownload(DynamicIndex),
}

#[relm4::factory(pub)]
impl FactoryComponent for DownloadRow {
    type Init = DownloadEntry;
    type Input = ();
    type Output = DownloadRowOutput;
    type CommandOutput = ();
    type ParentWidget = gtk::ListBox;

    view! {
        root = adw::ActionRow {
            set_selectable: false,
            #[watch]
            set_visible: self.visible,
            #[watch]
            set_title: &gtk::glib::markup_escape_text(&self.display_name()),
            #[watch]
            set_subtitle: &gtk::glib::markup_escape_text(&self.download_subtitle()),
            set_title_lines: 1,
            set_subtitle_lines: 2,
            set_activatable: false,

            add_prefix = &gtk::Image {
                #[watch]
                set_icon_name: Some(status_icon(&self.entry.status)),
                #[watch]
                set_css_classes: &status_css(&self.entry.status),
                set_valign: gtk::Align::Center,
            },

            add_suffix = &gtk::ProgressBar {
                #[watch]
                set_fraction: self.entry.progress,
                #[watch]
                set_visible: self.entry.status == DownloadStatus::Downloading
                    || self.entry.status == DownloadStatus::Extracting,
                set_valign: gtk::Align::Center,
                set_width_request: 96,
            },

                // Pause — shown while actively downloading
                add_suffix = &gtk::Button {
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
                add_suffix = &gtk::Button {
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
                add_suffix = &gtk::Button {
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
                add_suffix = &gtk::Button {
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
                add_suffix = &gtk::Button {
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
        }
    }

    fn init_model(init: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        let hide_btn = gtk::Button::builder()
            .label(if init.hidden { "Unhide" } else { "Hide" })
            .css_classes(["flat"])
            .halign(gtk::Align::Fill)
            .build();
        Self {
            entry: init,
            visible: true,
            hide_btn,
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

        let reinstall_btn = gtk::Button::builder()
            .label("Reinstall")
            .css_classes(["flat"])
            .halign(gtk::Align::Fill)
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
        let hide_btn = self.hide_btn.clone();
        let delete_btn = gtk::Button::builder()
            .label("Move to Trash")
            .css_classes(["flat", "download-menu-delete"])
            .halign(gtk::Align::Fill)
            .build();

        let reinstall_idx = index.clone();
        let reinstall_sender = sender.clone();
        let reinstall_pop = popover.clone();
        reinstall_btn.connect_clicked(move |_| {
            reinstall_pop.popdown();
            reinstall_sender
                .output(DownloadRowOutput::Reinstall(reinstall_idx.clone()))
                .ok();
        });

        let fetch_idx = index.clone();
        let fetch_sender = sender.clone();
        let fetch_pop = popover.clone();
        fetch_btn.connect_clicked(move |_| {
            fetch_pop.popdown();
            fetch_sender
                .output(DownloadRowOutput::FetchMetadata(fetch_idx.clone()))
                .ok();
        });

        let clear_idx = index.clone();
        let clear_sender = sender.clone();
        let clear_pop = popover.clone();
        clear_btn.connect_clicked(move |_| {
            clear_pop.popdown();
            clear_sender
                .output(DownloadRowOutput::ClearMetadata(clear_idx.clone()))
                .ok();
        });

        let hide_idx = index.clone();
        let hide_sender = sender.clone();
        let hide_pop = popover.clone();
        hide_btn.connect_clicked(move |_| {
            hide_pop.popdown();
            hide_sender
                .output(DownloadRowOutput::HideDownload(hide_idx.clone()))
                .ok();
        });

        let delete_idx = index.clone();
        let delete_sender = sender.clone();
        let delete_pop = popover.clone();
        delete_btn.connect_clicked(move |_| {
            delete_pop.popdown();
            delete_sender
                .output(DownloadRowOutput::Delete(delete_idx.clone()))
                .ok();
        });

        menu_box.append(&reinstall_btn);
        menu_box.append(&fetch_btn);
        menu_box.append(&clear_btn);
        menu_box.append(&hide_btn);
        menu_box.append(&delete_btn);
        popover.set_child(Some(&menu_box));
        popover.set_parent(&root);

        // Unparent the popover when the row leaves the widget tree; otherwise GTK
        // warns about finalizing a GtkListBoxRow that still has children.
        let pop = popover.clone();
        root.connect_unrealize(move |_| {
            pop.unparent();
        });

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

    fn secondary_label(&self) -> String {
        match (&self.entry.nexus_file_name, &self.entry.version) {
            (Some(fname), Some(ver)) if !fname.contains(ver.as_str()) => {
                format!("{fname}  •  v{ver}")
            }
            (Some(fname), _) => fname.clone(),
            (None, Some(ver)) => format!("v{ver}"),
            (None, None) => String::new(),
        }
    }

    fn download_subtitle(&self) -> String {
        let mut parts = Vec::new();
        if self.entry.metadata_fetched
            && (self.entry.nexus_file_name.is_some() || self.entry.version.is_some())
        {
            parts.push(self.secondary_label());
        }
        if !self.entry.status_msg.is_empty() {
            parts.push(self.entry.status_msg.clone());
        }
        if let Some(error) = &self.entry.error_msg {
            parts.push(error.clone());
        }
        parts.join("\n")
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
