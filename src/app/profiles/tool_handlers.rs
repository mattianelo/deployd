use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game;
use crate::core::tool_launcher;
use crate::ui::tool_manager::{ToolManager, ToolManagerOutput};

use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::App;

impl App {
    pub(crate) fn handle_launch_tool(
        &mut self,
        tool_id: String,
        sender: &ComponentSender<Self>,
    ) {
        if self.needs_deploy {
            self.toaster
                .toast("Deploy your mods before launching tools");
            return;
        }

        let Some(tool) = self.tools.iter().find(|t| t.id == tool_id).cloned() else {
            self.toaster.toast("Tool not found");
            return;
        };
        let Some(game) = self.selected_game().cloned() else { return };

        let tool_name = tool.name.clone();
        let exit_sender = sender.input_sender().clone();
        let exit_tool_name = tool_name.clone();
        sender.oneshot_command(async move {
            let result: Result<String, String> = (|| {
                let wine_config = game::detect_wine_config(&game).ok_or_else(|| {
                    "Could not detect Wine configuration for this game".to_string()
                })?;
                tool_launcher::launch_tool(
                    &tool,
                    &game,
                    &wine_config,
                    Some(Box::new(move || {
                        let _ = exit_sender.send(AppMsg::ToolExited(exit_tool_name));
                    })),
                )
                .map_err(|e| e.to_string())?;
                Ok(tool_name)
            })();
            AppCmdMsg::ToolLaunched(result)
        });
    }

    pub(crate) fn handle_tool_exited(&mut self, tool_name: String, sender: &ComponentSender<Self>) {
        self.toaster
            .toast(&format!("{tool_name} closed — scanning for changes…"));
        sender.input(AppMsg::ScanExternalFiles);
        #[cfg(feature = "loot")]
        if self
            .selected_game()
            .map(|g| crate::core::loot_sort::game_has_loot_support(&g.id))
            .unwrap_or(false)
        {
            sender.input(AppMsg::SortWithLoot);
        }
    }

    pub(crate) fn handle_manage_tools_clicked(
        &mut self,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let Some(game) = self.selected_game() else { return };
        let game_id = game.id.clone();
        let game_path = game.path.clone();
        let game_engine = game.engine.clone();
        let wine_prefix = game::detect_wine_config(game).map(|wc| wc.prefix);
        let tools = self.tools.clone();

        self.tool_manager_dialog = Some(
            ToolManager::builder()
                .transient_for(root)
                .launch((game_id, tools, game_path, wine_prefix, game_engine))
                .forward(sender.input_sender(), |output| match output {
                    ToolManagerOutput::ToolAdded(tool) => AppMsg::ToolAdded(tool),
                    ToolManagerOutput::ToolRemoved(id) => AppMsg::ToolRemoved(id),
                    ToolManagerOutput::ToolWorkingDirChanged(id, dir) => {
                        AppMsg::ToolWorkingDirChanged(id, dir)
                    }
                    ToolManagerOutput::Closed => AppMsg::ToolManagerClosed,
                }),
        );
    }

    pub(crate) fn handle_tool_added(
        &mut self,
        tool: crate::models::tool::Tool,
        sender: &ComponentSender<Self>,
    ) {
        let tool_clone = tool.clone();
        self.tools.push(tool);
        self.rebuild_tool_buttons(sender);

        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::ToolSaved(
                    tracker
                        .insert_tool(&tool_clone)
                        .await
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn handle_tool_removed(
        &mut self,
        tool_id: String,
        sender: &ComponentSender<Self>,
    ) {
        self.tools.retain(|t| t.id != tool_id);
        self.rebuild_tool_buttons(sender);

        if let Some(tracker) = self.tracker.clone() {
            let id = tool_id.clone();
            sender.oneshot_command(async move {
                AppCmdMsg::ToolDeleted(
                    tracker
                        .delete_tool(&id)
                        .await
                        .map(|_| id)
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn handle_tool_working_dir_changed(
        &mut self,
        tool_id: String,
        new_dir: String,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(tool) = self.tools.iter_mut().find(|t| t.id == tool_id) {
            tool.working_dir = new_dir.clone();
        }

        if let Some(tracker) = self.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::ToolWorkingDirSaved(
                    tracker
                        .update_tool_working_dir(&tool_id, &new_dir)
                        .await
                        .map_err(|e| e.to_string()),
                )
            });
        }
    }

    pub(crate) fn handle_tool_manager_closed(&mut self) {
        if let Some(dialog) = self.tool_manager_dialog.take() {
            dialog.widget().destroy();
        }
    }
}
