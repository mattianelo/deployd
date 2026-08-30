use adw::prelude::*;
use relm4::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BottomStatusState {
    pub(crate) initializing: bool,
    pub(crate) mod_status: String,
    pub(crate) plugin_status: String,
    pub(crate) conflict_status: String,
    pub(crate) has_conflicts: bool,
    pub(crate) rate_limit_status: String,
    pub(crate) rate_limit_visible: bool,
    pub(crate) rate_limit_warning: bool,
    pub(crate) needs_deploy: bool,
    pub(crate) has_games: bool,
}

pub(crate) struct BottomStatus {
    state: BottomStatusState,
}

#[relm4::component(pub(crate))]
impl SimpleComponent for BottomStatus {
    type Init = BottomStatusState;
    type Input = BottomStatusState;
    type Output = ();

    view! {
        gtk::Box {
            set_orientation: gtk::Orientation::Horizontal,
            set_margin_start: 10,
            set_margin_end: 10,
            set_margin_top: 3,
            set_margin_bottom: 3,
            set_spacing: 8,
            #[watch]
            set_visible: !model.state.initializing,

            gtk::Label {
                #[watch]
                set_label: &model.state.mod_status,
                add_css_class: "caption",
                add_css_class: "dim-label",
            },

            gtk::Label {
                set_label: "\u{00b7}",
                add_css_class: "caption",
                add_css_class: "dim-label",
            },

            gtk::Label {
                #[watch]
                set_label: &model.state.plugin_status,
                add_css_class: "caption",
                add_css_class: "dim-label",
            },

            gtk::Label {
                set_label: "\u{00b7}",
                #[watch]
                set_visible: model.state.has_conflicts,
                add_css_class: "caption",
                add_css_class: "dim-label",
            },

            gtk::Label {
                #[watch]
                set_label: &model.state.conflict_status,
                #[watch]
                set_visible: model.state.has_conflicts,
                add_css_class: "caption",
                add_css_class: "warning",
            },

            gtk::Box { set_hexpand: true },

            gtk::Label {
                #[watch]
                set_label: &model.state.rate_limit_status,
                #[watch]
                set_visible: model.state.rate_limit_visible,
                add_css_class: "caption",
                #[watch]
                set_css_classes: if model.state.rate_limit_warning {
                    &["caption", "warning"]
                } else {
                    &["caption", "dim-label"]
                },
            },

            gtk::Label {
                #[watch]
                set_label: if model.state.needs_deploy {
                    "\u{25cf} Unsaved changes"
                } else {
                    "\u{2713} Synced"
                },
                #[watch]
                set_css_classes: if model.state.needs_deploy {
                    &["caption", "warning"]
                } else {
                    &["caption", "dim-label"]
                },
                #[watch]
                set_visible: model.state.has_games,
            },
        }
    }

    fn init(
        state: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = Self { state };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, state: Self::Input, _sender: ComponentSender<Self>) {
        self.state = state;
    }
}
