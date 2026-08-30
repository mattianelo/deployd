use gtk::prelude::*;
use relm4::prelude::*;

use super::super::App;
use super::super::messages::AppMsg;

impl App {
    /// Rebuild tool buttons in the headerbar for the current game's tools.
    /// Up to 3 tools are shown as individual buttons; any beyond that are
    /// collected into an overflow `gtk::MenuButton`.
    pub(crate) fn rebuild_tool_buttons(&self, sender: &ComponentSender<Self>) {
        const MAX_VISIBLE: usize = 3;

        // Remove all existing tool buttons
        while let Some(child) = self.ui.tool_buttons_box.first_child() {
            self.ui.tool_buttons_box.remove(&child);
        }

        let (visible, overflow) = if self.tools.entries.len() > MAX_VISIBLE {
            self.tools.entries.split_at(MAX_VISIBLE)
        } else {
            (&self.tools.entries[..], &[][..])
        };

        for tool in visible {
            let btn = gtk::Button::new();
            btn.set_icon_name(&tool.icon_name);
            btn.set_tooltip_text(Some(&tool.name));
            btn.add_css_class("flat");

            let tool_id = tool.id.clone();
            let input_sender = sender.input_sender().clone();
            btn.connect_clicked(move |_| {
                let _ = input_sender.send(AppMsg::Tools(
                    crate::app::messages::ToolsMsg::LaunchTool(tool_id.clone()),
                ));
            });

            self.ui.tool_buttons_box.append(&btn);
        }

        if !overflow.is_empty() {
            let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
            for tool in overflow {
                let btn = gtk::Button::new();
                btn.set_icon_name(&tool.icon_name);
                btn.set_tooltip_text(Some(&tool.name));
                btn.set_label(&tool.name);
                btn.add_css_class("flat");

                let tool_id = tool.id.clone();
                let input_sender = sender.input_sender().clone();
                btn.connect_clicked(move |b| {
                    let _ = input_sender.send(AppMsg::Tools(
                        crate::app::messages::ToolsMsg::LaunchTool(tool_id.clone()),
                    ));
                    if let Some(p) = b
                        .ancestor(gtk::Popover::static_type())
                        .and_downcast::<gtk::Popover>()
                    {
                        p.popdown();
                    }
                });

                popover_box.append(&btn);
            }

            let popover = gtk::Popover::new();
            popover.set_child(Some(&popover_box));

            let overflow_btn = gtk::MenuButton::new();
            overflow_btn.set_icon_name("view-more-symbolic");
            overflow_btn.set_tooltip_text(Some("More tools"));
            overflow_btn.add_css_class("flat");
            overflow_btn.set_popover(Some(&popover));

            self.ui.tool_buttons_box.append(&overflow_btn);
        }

        let has_tools = self.has_games() && !self.tools.entries.is_empty();
        self.ui.tool_buttons_box.set_visible(has_tools);

        if has_tools {
            // Append a vertical separator so it sits between the tool buttons
            // and the main action buttons (deploy, search, notifications, etc.).
            let sep = gtk::Separator::builder()
                .orientation(gtk::Orientation::Vertical)
                .margin_top(6)
                .margin_bottom(6)
                .build();
            self.ui.tool_buttons_box.append(&sep);
        }
    }
}
