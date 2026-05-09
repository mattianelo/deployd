use std::path::PathBuf;

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::game;
use crate::core::tool_launcher;
use crate::models::game::GameEngine;
use crate::ui::tool_manager::{ToolManager, ToolManagerOutput};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};

const PROTON_SETUP_BODY: &str = "Proton GE needs to be installed before tools can run. \
This is a one-time setup handled by UMU Launcher.\n\n\
The tool will launch automatically when the setup starts.";

impl App {
    pub(crate) fn handle_launch_tool(&mut self, tool_id: String, sender: &ComponentSender<Self>) {
        self.handle_launch_tool_inner(tool_id, false, sender);
    }

    fn handle_launch_tool_inner(
        &mut self,
        tool_id: String,
        allow_umu_setup: bool,
        sender: &ComponentSender<Self>,
    ) {
        if self.needs_deploy {
            self.push_notification("Deploy your mods before launching tools");
            return;
        }

        let Some(tool) = self.tools.iter().find(|t| t.id == tool_id).cloned() else {
            self.push_notification("Tool not found");
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        if let game::SnapWineStatus::Missing(missing) = game::snap_wine_status() {
            sender.input(AppMsg::ConfirmSnapWineSetup(tool_id, missing));
            return;
        }

        if !game::is_snap() && !game::proton_runtime_available() && !allow_umu_setup {
            sender.input(AppMsg::ConfirmProtonSetup(tool_id));
            return;
        }

        let wine_config = match game::detect_wine_config(&game) {
            Some(c) => c,
            None => {
                if game::is_snap() {
                    self.push_notification("Snap Wine runtime not available. Connect the Wine interface and try again.");
                } else {
                    self.push_notification("UMU Launcher not found in this AppImage.");
                }
                return;
            }
        };

        // For Eclipse games under the Snap wine-runtime, wine-mono is not bundled.
        // Show a one-time blocking dialog so the user knows to accept the Mono install prompt.
        if game.engine == GameEngine::Eclipse && game::snap_wine_available() {
            let sentinel = wine_config.prefix.join(".deployd_mono_prompt_v1");
            if !sentinel.exists() {
                sender.input(AppMsg::ConfirmMonoPrompt(
                    tool_id,
                    wine_config.prefix.clone(),
                ));
                return;
            }
        }

        self.do_launch_tool(tool, game, wine_config, allow_umu_setup, sender);
    }

    /// Show the first-run Proton GE download confirmation dialog.
    pub(crate) fn handle_confirm_proton_setup(
        &mut self,
        tool_id: String,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading("First-run Setup Required")
            .body(PROTON_SETUP_BODY)
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("setup", "Set Up & Launch");
        dialog.set_default_response(Some("setup"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("setup", adw::ResponseAppearance::Suggested);

        let s = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "setup" {
                let _ = s.send(AppMsg::ProtonSetupConfirmed(tool_id.clone()));
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_confirm_snap_wine_setup(
        &mut self,
        tool_id: String,
        missing: game::MissingSnapWineContent,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading("Connect Snap Wine Interface")
            .body(game::missing_snap_wine_message(&missing))
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("connect", "Connect & Launch");
        dialog.set_default_response(Some("connect"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);

        let s = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "connect" {
                let _ = s.send(AppMsg::SnapWineSetupConfirmed(
                    tool_id.clone(),
                    missing.clone(),
                ));
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_snap_wine_setup_confirmed(
        &mut self,
        tool_id: String,
        missing: game::MissingSnapWineContent,
        sender: &ComponentSender<Self>,
    ) {
        self.proton_setup = true;
        self.status_msg = Some("Connecting Snap Wine interface…".to_string());

        sender.oneshot_command(async move {
            let result = connect_snap_wine_interfaces(missing).await;
            AppCmdMsg::SnapWineConnected { result, tool_id }
        });
    }

    pub(crate) fn handle_snap_wine_connected(
        &mut self,
        result: Result<(), String>,
        tool_id: String,
        sender: &ComponentSender<Self>,
    ) {
        self.proton_setup = false;
        self.status_msg = None;

        match result {
            Ok(()) => self.handle_launch_tool(tool_id, sender),
            Err(e) => {
                self.push_notification(&format!("Could not connect Snap Wine interface: {e}"))
            }
        }
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
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading("Mono Required for This Tool")
            .body(
                "Wine will ask to install Mono — a .NET runtime required by tools like \
                 CharGenMorph Compiler.\n\n\
                 Accept the installation when prompted. This message will not appear again.",
            )
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("launch", "Launch");
        dialog.set_default_response(Some("launch"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("launch", adw::ResponseAppearance::Suggested);

        let s = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "launch" {
                let _ = std::fs::write(prefix.join(".deployd_mono_prompt_v1"), b"");
                let _ = s.send(AppMsg::LaunchTool(tool_id.clone()));
            }
        });
        dialog.present(Some(root));
    }

    /// User confirmed the AppImage Proton GE setup — launch UMU so it can perform setup.
    pub(crate) fn handle_proton_setup_confirmed(
        &mut self,
        tool_id: String,
        sender: &ComponentSender<Self>,
    ) {
        self.proton_setup = true;
        self.status_msg = Some("Setting up Proton GE and launching tool…".to_string());
        self.handle_launch_tool_inner(tool_id, true, sender);
    }

    pub(crate) fn handle_proton_setup_ready(&mut self) {
        if self.proton_setup {
            self.proton_setup = false;
            self.status_msg = None;
            self.show_toast("Proton GE setup complete");
        }
    }

    pub(crate) fn handle_tool_exited(
        &mut self,
        tool_name: String,
        _error: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        self.proton_setup = false;
        self.status_msg = None;

        self.push_notification(&format!("{tool_name} closed — scanning for changes…"));
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
        root: &adw::ApplicationWindow,
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
        monitor_proton_setup: bool,
        sender: &ComponentSender<Self>,
    ) {
        let tool_name = tool.name.clone();
        let exit_sender = sender.input_sender().clone();
        let exit_tool_name = tool_name.clone();
        let setup_sender = sender.input_sender().clone();
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
                if monitor_proton_setup {
                    monitor_deployd_proton_runtime(setup_sender);
                }
                Ok(tool_name)
            })();
            AppCmdMsg::ToolLaunched(result)
        });
    }
}

async fn connect_snap_wine_interfaces(missing: game::MissingSnapWineContent) -> Result<(), String> {
    let plugs = game::missing_snap_wine_plugs(&missing);
    if plugs.is_empty() {
        return Ok(());
    }

    for plug in plugs {
        let status = tokio::process::Command::new("pkexec")
            .arg("snap")
            .arg("connect")
            .arg(plug)
            .status()
            .await
            .map_err(|e| format!("failed to start authorization prompt for {plug}: {e}"))?;

        if !status.success() {
            return Err(format!(
                "authorization failed for {plug} ({status}). You can connect it manually with: snap connect {plug}"
            ));
        }
    }

    Ok(())
}

fn monitor_deployd_proton_runtime(sender: relm4::Sender<AppMsg>) {
    std::thread::spawn(move || {
        for _ in 0..1800 {
            if game::proton_runtime_available() {
                let _ = sender.send(AppMsg::ProtonSetupReady);
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::PROTON_SETUP_BODY;

    #[test]
    fn proton_setup_message_does_not_claim_a_download_size() {
        assert!(!PROTON_SETUP_BODY.contains("MB"));
        assert!(!PROTON_SETUP_BODY.contains("GB"));
    }
}
