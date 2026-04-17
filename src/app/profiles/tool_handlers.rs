use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game::{self, WineLauncher};
use crate::core::tool_launcher;
use crate::ui::tool_manager::{ToolManager, ToolManagerOutput};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

impl App {
    pub(crate) fn handle_launch_tool(&mut self, tool_id: String, sender: &ComponentSender<Self>) {
        if self.needs_deploy {
            self.toaster
                .toast("Deploy your mods before launching tools");
            return;
        }

        let Some(tool) = self.tools.iter().find(|t| t.id == tool_id).cloned() else {
            self.toaster.toast("Tool not found");
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        // Detect wine/UMU config synchronously so we can check for UMU-specific
        // first-run requirements before spawning the async command.
        let wine_config = match game::detect_wine_config(&game) {
            Some(c) => c,
            None => {
                self.toaster.toast(
                    "Wine (wine64) or UMU Launcher (umu-run) not found. \
                     Install one via your system package manager.",
                );
                return;
            }
        };

        // For UMU: if no Proton runtime is installed, show a one-time confirmation
        // dialog that warns the user about the ~300 MB first-run download before proceeding.
        if let WineLauncher::Umu(_) = &wine_config.launcher
            && !game::proton_runtime_available()
        {
            sender.input(AppMsg::ConfirmUmuSetup(tool_id));
            return;
        }

        self.do_launch_tool(tool, game, wine_config, sender);
    }

    /// Show the first-run Proton GE confirmation dialog for UMU.
    ///
    /// If the user confirms, `UmuSetupConfirmed` is sent and the tool is launched
    /// (UMU will download Proton GE automatically during the launch).
    pub(crate) fn handle_confirm_umu_setup(
        &mut self,
        tool_id: String,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = gtk::AlertDialog::builder()
            .message("First-run Setup Required")
            .detail(
                "UMU Launcher needs to download Proton GE (~300 MB) before \
                 tools can run. This is a one-time download.\n\n\
                 The tool will launch automatically when the download finishes.",
            )
            .buttons(["Cancel", "Download & Launch"])
            .cancel_button(0)
            .default_button(1)
            .modal(true)
            .build();

        let s = sender.input_sender().clone();
        dialog.choose(Some(root), None::<&gio::Cancellable>, move |result| {
            if result == Ok(1) {
                let _ = s.send(AppMsg::UmuSetupConfirmed(tool_id));
            }
        });
    }

    /// User confirmed the first-run Proton GE download.
    ///
    /// Sets the busy state with a descriptive status message, then launches the
    /// tool via UMU.  `ToolExited` will clear the busy state when done.
    pub(crate) fn handle_umu_setup_confirmed(
        &mut self,
        tool_id: String,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tool) = self.tools.iter().find(|t| t.id == tool_id).cloned() else {
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };
        let Some(wine_config) = game::detect_wine_config(&game) else {
            return;
        };

        self.proton_setup = true;
        self.status_msg = Some("Downloading Proton GE for first use…".to_string());

        self.do_launch_tool(tool, game, wine_config, sender);
    }

    pub(crate) fn handle_tool_exited(
        &mut self,
        tool_name: String,
        error: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        let was_proton_setup = self.proton_setup;

        // Clear any Proton first-run busy state.
        self.proton_setup = false;
        if self.status_msg.as_deref() == Some("Downloading Proton GE for first use…") {
            self.status_msg = None;
        }

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
        self.overflow_menu_btn.popdown();
        let Some(game) = self.selected_game() else {
            return;
        };
        let game_id = game.id.clone();
        let game_path = game.path.clone();
        let game_engine = game.engine.clone();
        let deploy_dir = game::tool_search_dir(game);
        let wine_prefix = game::detect_wine_config(game).map(|wc| wc.prefix);
        let tools = self.tools.clone();

        self.tool_manager_dialog = Some(
            ToolManager::builder()
                .transient_for(root)
                .launch((
                    game_id,
                    tools,
                    game_path,
                    wine_prefix,
                    game_engine,
                    deploy_dir,
                ))
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

    pub(crate) fn handle_tool_removed(&mut self, tool_id: String, sender: &ComponentSender<Self>) {
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

    /// Shared inner launch logic used by both the normal path and the UMU-confirmed path.
    fn do_launch_tool(
        &mut self,
        tool: crate::models::tool::Tool,
        game: crate::models::game::Game,
        wine_config: crate::core::game::WineConfig,
        sender: &ComponentSender<Self>,
    ) {
        let tool_name = tool.name.clone();
        let exit_sender = sender.input_sender().clone();
        let exit_tool_name = tool_name.clone();

        sender.oneshot_command(async move {
            let result: Result<String, String> = (move || {
                tool_launcher::launch_tool(
                    &tool,
                    &game,
                    &wine_config,
                    Some(Box::new(move |error| {
                        let _ = exit_sender.send(AppMsg::ToolExited(exit_tool_name, error));
                    })),
                )
                .map_err(|e| e.to_string())?;
                Ok(tool_name)
            })();
            AppCmdMsg::ToolLaunched(result)
        });
    }
}
