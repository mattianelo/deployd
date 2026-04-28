use std::path::PathBuf;

use gtk::gio;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game;
use crate::core::tool_launcher;
use crate::models::game::GameEngine;
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

        // If Proton GE is not yet installed, prompt to download before launching.
        // Skip when sommelier is active — Wine comes from the content interface, not Proton GE.
        if !game::proton_runtime_available() && !game::snap_wine_available() {
            sender.input(AppMsg::ConfirmProtonSetup(tool_id));
            return;
        }

        let wine_config = match game::detect_wine_config(&game) {
            Some(c) => c,
            None => {
                self.toaster
                    .toast("Wine not found. Install wine via your system package manager.");
                return;
            }
        };

        // For Eclipse games under the Snap wine-runtime, wine-mono is not bundled.
        // Show a one-time blocking dialog so the user knows to accept the Mono install prompt.
        if game.engine == GameEngine::Eclipse && game::snap_wine_available() {
            let sentinel = wine_config.prefix.join(".deployd_mono_prompt_v1");
            if !sentinel.exists() {
                sender.input(AppMsg::ConfirmMonoPrompt(tool_id, wine_config.prefix.clone()));
                return;
            }
        }

        self.do_launch_tool(tool, game, wine_config, sender);
    }

    /// Show the first-run Proton GE download confirmation dialog.
    pub(crate) fn handle_confirm_proton_setup(
        &mut self,
        tool_id: String,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = gtk::AlertDialog::builder()
            .message("First-run Setup Required")
            .detail(
                "Proton GE (~600 MB) needs to be downloaded before tools can run. \
                 This is a one-time download.\n\n\
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
                let _ = s.send(AppMsg::ProtonSetupConfirmed(tool_id));
            }
        });
    }

    /// Show a one-time Mono install info dialog for Eclipse tools under the Snap wine-runtime.
    ///
    /// wine-mono is not bundled with the Snap's wine-platform content snap. Wine will offer to
    /// install it when a .NET tool (e.g. CharGenMorph Compiler) is first launched. This dialog
    /// tells the user to accept that prompt. After the user acknowledges, a sentinel file is
    /// written to the prefix so the dialog never appears again.
    pub(crate) fn handle_confirm_mono_prompt(
        &mut self,
        tool_id: String,
        prefix: PathBuf,
        root: &adw::Window,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = gtk::AlertDialog::builder()
            .message("Mono Required for This Tool")
            .detail(
                "Wine will ask to install Mono — a .NET runtime required by tools like \
                 CharGenMorph Compiler.\n\n\
                 Accept the installation when prompted. This message will not appear again.",
            )
            .buttons(["Cancel", "Launch"])
            .cancel_button(0)
            .default_button(1)
            .modal(true)
            .build();

        let s = sender.input_sender().clone();
        dialog.choose(Some(root), None::<&gio::Cancellable>, move |result| {
            if result == Ok(1) {
                let _ = std::fs::write(prefix.join(".deployd_mono_prompt_v1"), b"");
                let _ = s.send(AppMsg::LaunchTool(tool_id));
            }
        });
    }

    /// User confirmed the Proton GE download — start the async GitHub download.
    pub(crate) fn handle_proton_setup_confirmed(
        &mut self,
        tool_id: String,
        sender: &ComponentSender<Self>,
    ) {
        self.proton_setup = true;
        self.status_msg = Some("Downloading Proton GE…".to_string());

        sender.oneshot_command(async move {
            let result = crate::app::downloads::proton_setup::download_proton_ge()
                .await
                .map_err(|e| e.to_string());
            AppCmdMsg::ProtonDownloaded { result, tool_id }
        });
    }

    /// Proton GE download completed — re-enter the launch flow on success.
    pub(crate) fn handle_proton_downloaded(
        &mut self,
        result: Result<(), String>,
        tool_id: String,
        sender: &ComponentSender<Self>,
    ) {
        self.proton_setup = false;
        self.status_msg = None;
        match result {
            Ok(()) => self.handle_launch_tool(tool_id, sender),
            Err(e) => self
                .toaster
                .toast(&format!("Proton GE download failed: {e}")),
        }
    }

    pub(crate) fn handle_tool_exited(
        &mut self,
        tool_name: String,
        _error: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        self.proton_setup = false;
        if self.status_msg.as_deref() == Some("Downloading Proton GE…") {
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
        let cache_root = self
            .cache_root_for(&game.id)
            .unwrap_or_else(|_| crate::utils::paths::cache_root().unwrap_or_default());

        sender.oneshot_command(async move {
            let result: Result<String, String> = (move || {
                tool_launcher::launch_tool(
                    &tool,
                    &game,
                    &wine_config,
                    &cache_root,
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
