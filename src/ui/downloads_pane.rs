use gtk::prelude::*;
use relm4::prelude::*;

use crate::models::download::{DownloadFilter, DownloadSort};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DownloadsPaneState {
    pub(crate) filter: DownloadFilter,
    pub(crate) sort: DownloadSort,
    pub(crate) show_hidden: bool,
    pub(crate) active_count: usize,
    pub(crate) completed_count: usize,
    pub(crate) is_empty: bool,
}

pub(crate) struct DownloadsPaneInit {
    pub(crate) state: DownloadsPaneState,
    pub(crate) scroll: gtk::ScrolledWindow,
    pub(crate) list: gtk::ListBox,
}

pub(crate) struct DownloadsPane {
    state: DownloadsPaneState,
}

#[derive(Debug)]
pub(crate) enum DownloadsPaneOutput {
    SetFilter(DownloadFilter),
    SetSort(u32),
    Scan,
    SetShowHidden(bool),
}

#[relm4::component(pub(crate))]
impl SimpleComponent for DownloadsPane {
    type Init = DownloadsPaneInit;
    type Input = DownloadsPaneState;
    type Output = DownloadsPaneOutput;

    view! {
        adw::ToolbarView {
            add_css_class: "plain-panel-bg",

            add_top_bar = &adw::HeaderBar {
                set_centering_policy: adw::CenteringPolicy::Loose,
                set_show_back_button: false,
                set_decoration_layout: Some(""),

                #[wrap(Some)]
                set_title_widget = &gtk::Box {
                    set_orientation: gtk::Orientation::Horizontal,
                    set_spacing: 4,
                    set_halign: gtk::Align::Center,

                    gtk::Button {
                        #[watch]
                        set_css_classes: filter_css(model.state.filter, DownloadFilter::All),
                        set_label: "All",
                        connect_clicked[sender] => move |_| {
                            sender.output(DownloadsPaneOutput::SetFilter(DownloadFilter::All)).ok();
                        },
                    },

                    gtk::Button {
                        #[watch]
                        set_css_classes: filter_css(model.state.filter, DownloadFilter::Active),
                        #[watch]
                        set_label: &format!("Active ({})", model.state.active_count),
                        connect_clicked[sender] => move |_| {
                            sender.output(DownloadsPaneOutput::SetFilter(DownloadFilter::Active)).ok();
                        },
                    },

                    gtk::Button {
                        #[watch]
                        set_css_classes: filter_css(model.state.filter, DownloadFilter::Completed),
                        #[watch]
                        set_label: &format!("Completed ({})", model.state.completed_count),
                        connect_clicked[sender] => move |_| {
                            sender.output(DownloadsPaneOutput::SetFilter(DownloadFilter::Completed)).ok();
                        },
                    },
                },

                pack_start = &gtk::Label {
                    set_label: "Downloads",
                    add_css_class: "heading",
                    set_valign: gtk::Align::Center,
                    set_margin_start: 4,
                },

                pack_end = &gtk::MenuButton {
                    set_icon_name: "view-sort-ascending-symbolic",
                    set_tooltip_text: Some("Sort downloads"),
                    add_css_class: "flat",
                    #[wrap(Some)]
                    set_popover = &gtk::Popover {
                        #[wrap(Some)]
                        set_child = &gtk::Box {
                            set_orientation: gtk::Orientation::Vertical,
                            set_spacing: 6,
                            set_margin_all: 8,

                            gtk::Button {
                                set_label: "Default order",
                                add_css_class: "flat",
                                #[watch]
                                set_sensitive: model.state.sort != DownloadSort::Default,
                                connect_clicked[sender] => move |button| {
                                    sender.output(DownloadsPaneOutput::SetSort(0)).ok();
                                    close_ancestor_popover(button);
                                },
                            },

                            gtk::Button {
                                set_label: "Name",
                                add_css_class: "flat",
                                #[watch]
                                set_sensitive: model.state.sort != DownloadSort::Name,
                                connect_clicked[sender] => move |button| {
                                    sender.output(DownloadsPaneOutput::SetSort(1)).ok();
                                    close_ancestor_popover(button);
                                },
                            },

                            gtk::Button {
                                set_label: "Status",
                                add_css_class: "flat",
                                #[watch]
                                set_sensitive: model.state.sort != DownloadSort::Status,
                                connect_clicked[sender] => move |button| {
                                    sender.output(DownloadsPaneOutput::SetSort(2)).ok();
                                    close_ancestor_popover(button);
                                },
                            },
                        },
                    },
                },

                pack_end = &gtk::Button {
                    set_icon_name: "folder-open-symbolic",
                    set_tooltip_text: Some("Scan downloads folder"),
                    add_css_class: "flat",
                    connect_clicked[sender] => move |_| {
                        sender.output(DownloadsPaneOutput::Scan).ok();
                    },
                },

                pack_end = &gtk::ToggleButton {
                    #[watch]
                    set_icon_name: if model.state.show_hidden {
                        "view-conceal-symbolic"
                    } else {
                        "view-reveal-symbolic"
                    },
                    #[watch]
                    set_tooltip_text: Some(if model.state.show_hidden {
                        "Hide hidden downloads"
                    } else {
                        "Show hidden downloads"
                    }),
                    add_css_class: "flat",
                    #[watch]
                    set_active: model.state.show_hidden,
                    connect_toggled[sender] => move |button| {
                        sender.output(DownloadsPaneOutput::SetShowHidden(button.is_active())).ok();
                    },
                },
            },

            #[wrap(Some)]
            set_content = &adw::Clamp {
                set_maximum_size: 700,

                gtk::Box {
                    set_orientation: gtk::Orientation::Vertical,
                    add_css_class: "plain-panel-bg",

                    #[local_ref]
                    scroll -> gtk::ScrolledWindow {
                        set_vexpand: true,
                        set_hscrollbar_policy: gtk::PolicyType::Automatic,

                        #[local_ref]
                        list -> gtk::ListBox {
                            set_selection_mode: gtk::SelectionMode::None,
                            add_css_class: "boxed-list",
                            set_margin_all: 8,
                        }
                    },

                    adw::StatusPage {
                        #[watch]
                        set_visible: model.state.is_empty,
                        set_icon_name: Some("folder-download-symbolic"),
                        set_title: "No Downloads",
                        set_description: Some("Click Scan or download from Nexus Mods"),
                    },
                },
            },
        }
    }

    fn init(
        init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let scroll = &init.scroll;
        let list = &init.list;
        let model = Self { state: init.state };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, state: Self::Input, _sender: ComponentSender<Self>) {
        self.state = state;
    }
}

fn filter_css(current: DownloadFilter, target: DownloadFilter) -> &'static [&'static str] {
    if current == target {
        &["pill", "filter-chip", "suggested-action"]
    } else {
        &["pill", "filter-chip"]
    }
}

fn close_ancestor_popover(button: &gtk::Button) {
    if let Some(popover) = button
        .ancestor(gtk::Popover::static_type())
        .and_downcast::<gtk::Popover>()
    {
        popover.popdown();
    }
}
