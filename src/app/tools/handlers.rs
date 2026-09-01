use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use adw::prelude::*;
use gtk::prelude::*;
use relm4::prelude::*;

use crate::core::{game, tool_launcher};
use crate::ui::tool_manager::{ToolManager, ToolManagerOutput};

use super::super::App;
use super::super::messages::{AppCmdMsg, AppMsg};
use super::super::types::{PostLootAction, ToolLaunchSession, ToolSessionState, WorkKind};

const PROTON_SETUP_BODY: &str = "Proton GE needs to be installed before tools can run. \
This is a one-time setup handled by UMU Launcher.\n\n\
The tool will launch automatically when the setup starts.";

#[derive(Debug, Default, PartialEq, Eq)]
struct PostToolExitActions {
    scan_external_files: bool,
    sort_with_loot: bool,
    deploy: Option<PostToolDeploy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolExitKind {
    Completed,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PostToolDeploy {
    AfterLoot,
    Now,
}

#[derive(Debug, Clone, Copy)]
struct ToolLaunchOptions {
    monitor_proton_setup: bool,
    skip_mono: bool,
}

fn post_tool_exit_actions(
    exit_kind: ToolExitKind,
    _selected_game_id: Option<&str>,
) -> PostToolExitActions {
    if exit_kind != ToolExitKind::Completed {
        return PostToolExitActions::default();
    }

    #[cfg(feature = "loot")]
    let sort_with_loot =
        _selected_game_id.is_some_and(crate::core::loot_sort::game_has_loot_support);
    #[cfg(not(feature = "loot"))]
    let sort_with_loot = false;

    PostToolExitActions {
        scan_external_files: true,
        sort_with_loot,
        deploy: Some(if sort_with_loot {
            PostToolDeploy::AfterLoot
        } else {
            PostToolDeploy::Now
        }),
    }
}

impl App {
    pub(crate) fn handle_launch_tool(
        &mut self,
        tool_id: String,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if self.is_busy() {
            self.show_toast("Wait for the current task to finish before launching tools");
            return;
        }
        self.handle_launch_tool_inner(tool_id, false, false, root, sender);
    }

    fn handle_launch_tool_inner(
        &mut self,
        tool_id: String,
        allow_umu_setup: bool,
        skip_mono: bool,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        if self.shell.needs_deploy {
            self.show_toast("Deploy your mods before launching tools");
            return;
        }

        let Some(tool) = self.tools.entries.iter().find(|t| t.id == tool_id).cloned() else {
            self.show_toast("Tool not found");
            return;
        };
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        if let game::SnapWineStatus::Missing(missing) = game::snap_wine_status() {
            sender.input(AppMsg::Tools(
                crate::app::messages::ToolsMsg::ConfirmSnapWineSetup(tool_id, missing),
            ));
            return;
        }

        if !game::is_snap() && !game::proton_runtime_available() && !allow_umu_setup {
            sender.input(AppMsg::Tools(
                crate::app::messages::ToolsMsg::ConfirmProtonSetup(tool_id),
            ));
            return;
        }

        let wine_config = match game::detect_wine_config(&game) {
            Ok(Some(config)) => config,
            Ok(None) => {
                if game::is_snap() {
                    self.push_notification("Snap Wine runtime not available. Connect the Wine interface and try again.");
                } else {
                    self.push_notification("UMU Launcher not found in this AppImage.");
                }
                return;
            }
            Err(error) => {
                self.push_notification(&format!("Cannot prepare the Wine runtime: {error}"));
                return;
            }
        };

        self.do_launch_tool(
            tool,
            game,
            wine_config,
            ToolLaunchOptions {
                monitor_proton_setup: allow_umu_setup,
                skip_mono,
            },
            root,
            sender,
        );
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
                let _ = s.send(AppMsg::Tools(
                    crate::app::messages::ToolsMsg::ProtonSetupConfirmed(tool_id.clone()),
                ));
            }
        });
        dialog.present(Some(root));
    }

    pub(crate) fn handle_confirm_snap_wine_setup(
        &mut self,
        _tool_id: String,
        missing: game::MissingSnapWineContent,
        root: &adw::ApplicationWindow,
        _sender: &ComponentSender<Self>,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading("Connect Snap Wine Interface")
            .body(game::missing_snap_wine_message(&missing))
            .build();
        dialog.add_response("close", "Close");
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("close");

        let command = game::missing_snap_wine_commands(&missing);
        if !command.is_empty() {
            let command_entry = gtk::Entry::builder()
                .text(&command)
                .editable(false)
                .hexpand(true)
                .build();
            command_entry.add_css_class("monospace");

            let copy_btn = gtk::Button::builder()
                .icon_name("edit-copy-symbolic")
                .tooltip_text("Copy command")
                .build();
            let command_for_clipboard = command.clone();
            copy_btn.connect_clicked(move |_| {
                if let Some(display) = gtk::gdk::Display::default() {
                    display.clipboard().set_text(&command_for_clipboard);
                }
            });

            let command_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            command_box.append(&command_entry);
            command_box.append(&copy_btn);
            dialog.set_extra_child(Some(&command_box));
        }

        dialog.present(Some(root));
    }

    /// User confirmed the AppImage Proton GE setup — launch UMU so it can perform setup.
    pub(crate) fn handle_proton_setup_confirmed(
        &mut self,
        tool_id: String,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.tools.proton_setup = true;
        self.begin_work(
            WorkKind::SettingUpRuntime,
            "Setting up Proton GE and launching tool...",
        );
        self.handle_launch_tool_inner(tool_id, true, false, root, sender);
    }

    pub(crate) fn handle_proton_setup_ready(&mut self) {
        if self.tools.proton_setup {
            self.tools.proton_setup = false;
            self.finish_work(WorkKind::SettingUpRuntime);
            self.show_toast("Proton GE setup complete");
        }
    }

    pub(crate) fn handle_tool_exited(
        &mut self,
        tool_name: String,
        error: Option<String>,
        sender: &ComponentSender<Self>,
    ) {
        let was_cancelling = self
            .tools
            .launch_session
            .as_ref()
            .is_some_and(|session| session.state == ToolSessionState::Cancelling);
        let exit_kind = if was_cancelling {
            ToolExitKind::Cancelled
        } else if error.is_some() {
            ToolExitKind::Failed
        } else {
            ToolExitKind::Completed
        };
        let actions =
            post_tool_exit_actions(exit_kind, self.selected_game().map(|game| game.id.as_str()));
        if let Some(session) = self.tools.launch_session.as_ref() {
            crate::dlog!(
                "deployd: tool session ended tool_id={} variant={} elapsed_ms={}",
                session.tool_id,
                session.package_variant,
                session.started_at.elapsed().as_millis()
            );
        }
        self.close_tool_launch_dialog();
        self.tools.launch_session = None;
        self.tools.proton_setup = false;
        self.finish_work(WorkKind::LaunchingTool);
        self.finish_work(WorkKind::SettingUpRuntime);

        if was_cancelling {
            self.show_toast(&format!("{tool_name} stopped"));
            return;
        }

        if let Some(error) = error {
            crate::dlog!("deployd: {tool_name} launch failed: {error}");
            self.push_notification(&format!("{tool_name} failed: {error}"));
            return;
        }

        self.show_toast(&format!("{tool_name} closed — scanning for changes…"));
        if actions.scan_external_files {
            sender.input(AppMsg::Mods(
                crate::app::messages::ModsMsg::ScanExternalFiles,
            ));
        }
        if actions.sort_with_loot {
            if actions.deploy == Some(PostToolDeploy::AfterLoot) {
                self.plugins.pending_post_loot_action = PostLootAction::Deploy;
            }
            sender.input(AppMsg::Plugins(
                crate::app::messages::PluginsMsg::SortWithLoot,
            ));
        } else if actions.deploy == Some(PostToolDeploy::Now) {
            sender.input(AppMsg::Shell(
                crate::app::messages::ShellMsg::DeployConfirmed,
            ));
        }
    }

    pub(crate) fn handle_manage_tools_clicked(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.ui.overflow_menu_btn.popdown();
        let Some(game) = self.selected_game() else {
            return;
        };
        let game_id = game.id.clone();
        let game_path = game.path.clone();
        let game_engine = game.engine.clone();
        let deploy_dir = game::tool_search_dir(game);
        let wine_prefix = game.wine_prefix.clone();
        let tools = self.tools.entries.clone();

        self.ui.tool_manager_dialog = Some(
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
                    ToolManagerOutput::ToolAdded(tool) => {
                        AppMsg::Tools(crate::app::messages::ToolsMsg::ToolAdded(tool))
                    }
                    ToolManagerOutput::ToolRemoved(id) => {
                        AppMsg::Tools(crate::app::messages::ToolsMsg::ToolRemoved(id))
                    }
                    ToolManagerOutput::ToolWorkingDirChanged(id, dir) => AppMsg::Tools(
                        crate::app::messages::ToolsMsg::ToolWorkingDirChanged(id, dir),
                    ),
                    ToolManagerOutput::Closed => {
                        AppMsg::Tools(crate::app::messages::ToolsMsg::ToolManagerClosed)
                    }
                }),
        );
    }

    pub(crate) fn handle_tool_added(
        &mut self,
        tool: crate::models::tool::Tool,
        sender: &ComponentSender<Self>,
    ) {
        let tool_clone = tool.clone();
        self.tools.entries.push(tool);
        self.rebuild_tool_buttons(sender);

        if let Some(tracker) = self.session.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::Tools(crate::app::messages::ToolsCmdMsg::Saved(
                    tracker
                        .insert_tool(&tool_clone)
                        .await
                        .map_err(|e| e.to_string()),
                ))
            });
        }
    }

    pub(crate) fn handle_tool_removed(&mut self, tool_id: String, sender: &ComponentSender<Self>) {
        self.tools.entries.retain(|t| t.id != tool_id);
        self.rebuild_tool_buttons(sender);

        if let Some(tracker) = self.session.tracker.clone() {
            let id = tool_id.clone();
            sender.oneshot_command(async move {
                AppCmdMsg::Tools(crate::app::messages::ToolsCmdMsg::Deleted(
                    tracker
                        .delete_tool(&id)
                        .await
                        .map(|_| id)
                        .map_err(|e| e.to_string()),
                ))
            });
        }
    }

    pub(crate) fn handle_tool_working_dir_changed(
        &mut self,
        tool_id: String,
        new_dir: String,
        sender: &ComponentSender<Self>,
    ) {
        if let Some(tool) = self.tools.entries.iter_mut().find(|t| t.id == tool_id) {
            tool.working_dir = new_dir.clone();
        }

        if let Some(tracker) = self.session.tracker.clone() {
            sender.oneshot_command(async move {
                AppCmdMsg::Tools(crate::app::messages::ToolsCmdMsg::WorkingDirSaved(
                    tracker
                        .update_tool_working_dir(&tool_id, &new_dir)
                        .await
                        .map_err(|e| e.to_string()),
                ))
            });
        }
    }

    pub(crate) fn handle_tool_manager_closed(&mut self) {
        if let Some(dialog) = self.ui.tool_manager_dialog.take() {
            dialog.widget().destroy();
        }
    }

    pub(crate) fn handle_cancel_tool_launch(&mut self) {
        if let Some(session) = self.tools.launch_session.as_mut()
            && let Some(process) = session.process.clone()
        {
            session.state = ToolSessionState::Cancelling;
            session.process = Some(process.clone());
            let tool_name = session.tool_name.clone();
            self.update_work(
                WorkKind::LaunchingTool,
                format!("Cancelling {tool_name}..."),
                None,
            );
            process.request_stop();
            return;
        }

        let had_launch = if let Some(cancel) = self.tools.launch_cancel.take() {
            cancel.store(true, Ordering::SeqCst);
            true
        } else {
            false
        };
        self.close_tool_launch_dialog();
        self.tools.pending_mono_tool = None;
        self.tools.proton_setup = false;
        self.finish_work(WorkKind::LaunchingTool);
        self.finish_work(WorkKind::SettingUpRuntime);
        if had_launch {
            self.show_toast("Tool launch cancelled");
        }
    }

    pub(crate) fn handle_tool_session_started(
        &mut self,
        handle: crate::core::tool_launcher::ToolProcessHandle,
    ) {
        if let Some(session) = self.tools.launch_session.as_mut() {
            session.process = Some(handle);
            session.state = ToolSessionState::Running;
            let message = format!("{} is running", session.tool_name);
            self.update_work(WorkKind::LaunchingTool, message, None);
        }
    }

    pub(crate) fn close_tool_launch_dialog(&mut self) {
        if let Some(dialog) = self.ui.tool_launch_dialog.take() {
            dialog.close();
        }
        self.ui.tool_launch_log = None;
        self.tools.launch_cancel = None;
    }

    pub(crate) fn handle_tool_setup_progress(
        &mut self,
        stage: crate::core::tool_launcher::ToolSetupStage,
    ) {
        if self.tools.launch_session.is_none() {
            return;
        }
        let message = stage.message();
        self.update_work(WorkKind::LaunchingTool, message, None);
        if let Some(buffer) = self.ui.tool_launch_log.as_ref() {
            let mut end = buffer.end_iter();
            buffer.insert(&mut end, &format!("{message}\n"));
        }
    }

    pub(crate) fn handle_retry_mono_setup(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tool_id) = self.tools.pending_mono_tool.take() else {
            return;
        };
        self.close_tool_launch_dialog();
        self.handle_launch_tool_inner(tool_id, false, false, root, sender);
    }

    pub(crate) fn handle_launch_without_mono(
        &mut self,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let Some(tool_id) = self.tools.pending_mono_tool.take() else {
            return;
        };
        self.close_tool_launch_dialog();
        self.handle_launch_tool_inner(tool_id, false, true, root, sender);
    }

    pub(crate) fn show_mono_setup_failure(
        &mut self,
        error: &str,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        self.close_tool_launch_dialog();
        let dialog = adw::AlertDialog::builder()
            .heading("Wine Mono Setup Failed")
            .body(
                "Retry the verified setup, continue with a native Windows tool, or cancel. \
                 Deployd will retry Mono automatically on a later launch.",
            )
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("without-mono", "Launch Without Mono");
        dialog.add_response("retry", "Retry");
        dialog.set_default_response(Some("retry"));
        dialog.set_close_response("cancel");
        dialog.set_response_appearance("retry", adw::ResponseAppearance::Suggested);

        let buffer = gtk::TextBuffer::new(None);
        buffer.set_text(&format!("Wine Mono setup failed:\n{error}\n"));
        let console = gtk::TextView::builder()
            .buffer(&buffer)
            .editable(false)
            .cursor_visible(false)
            .monospace(true)
            .wrap_mode(gtk::WrapMode::WordChar)
            .height_request(150)
            .build();
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&console)
            .build();
        dialog.set_extra_child(Some(&scroll));

        let input_sender = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            let message = match response {
                "retry" => Some(crate::app::messages::ToolsMsg::RetryMonoSetup),
                "without-mono" => Some(crate::app::messages::ToolsMsg::LaunchWithoutMono),
                "cancel" => Some(crate::app::messages::ToolsMsg::CancelToolLaunch),
                _ => None,
            };
            if let Some(message) = message {
                let _ = input_sender.send(AppMsg::Tools(message));
            }
        });
        dialog.present(Some(root));
        self.ui.tool_launch_log = Some(buffer);
        self.ui.tool_launch_dialog = Some(dialog);
    }

    /// Shared inner launch logic used by both the normal path and the UMU-confirmed path.
    fn do_launch_tool(
        &mut self,
        tool: crate::models::tool::Tool,
        game: crate::models::game::Game,
        wine_config: crate::core::game::WineConfig,
        options: ToolLaunchOptions,
        root: &adw::ApplicationWindow,
        sender: &ComponentSender<Self>,
    ) {
        let tool_name = tool.name.clone();
        let exit_sender = sender.input_sender().clone();
        let exit_tool_name = tool_name.clone();
        let setup_sender = sender.input_sender().clone();
        let progress_sender = sender.input_sender().clone();
        let cache_root = match self.cache_root_for(&game.id) {
            Ok(path) => path,
            Err(error) => {
                self.push_notification(&format!("Cannot resolve the mod cache: {error}"));
                return;
            }
        };

        self.begin_work(
            WorkKind::LaunchingTool,
            format!("Launching {}...", tool_name),
        );
        let cancel = Arc::new(AtomicBool::new(false));
        self.tools.launch_cancel = Some(cancel.clone());
        self.tools.launch_session = Some(ToolLaunchSession {
            tool_id: tool.id.clone(),
            tool_name: tool_name.clone(),
            package_variant: if game::is_snap() { "snap" } else { "appimage" },
            started_at: std::time::Instant::now(),
            state: ToolSessionState::Preparing,
            process: None,
        });
        let show_setup_console = tool_launcher::initial_setup_required(&wine_config);
        self.show_tool_launch_dialog(root, &tool_name, show_setup_console, sender);
        sender.oneshot_command(async move {
            let launch_cancel = cancel.clone();
            let session_cancel = cancel.clone();
            let original_tool_name = tool_name.clone();
            let spawn_sender = setup_sender.clone();
            let timing_start = std::time::Instant::now();
            let timing_game_id = game.id.clone();
            let progress = Arc::new(move |stage| {
                let _ = progress_sender.send(AppMsg::Tools(
                    crate::app::messages::ToolsMsg::ToolSetupProgress(stage),
                ));
            });
            let result: Result<String, tool_launcher::ToolPrepareError> = async {
                tool_launcher::prepare_tool_runtime(
                    &game,
                    &wine_config,
                    launch_cancel.clone(),
                    options.skip_mono,
                    progress,
                )
                .await?;
                if launch_cancel.load(Ordering::SeqCst) {
                    return Err(tool_launcher::ToolPrepareError::Cancelled);
                }
                tool_launcher::launch_tool(
                    &tool,
                    &game,
                    &wine_config,
                    &cache_root,
                    Some(launch_cancel.as_ref()),
                    tool_launcher::ToolLaunchHooks {
                        cancel: session_cancel,
                        on_spawn: Some(Box::new(move |handle| {
                            let _ = spawn_sender.send(AppMsg::Tools(
                                crate::app::messages::ToolsMsg::ToolSessionStarted(handle),
                            ));
                        })),
                        on_exit: Some(Box::new(move |error| {
                            let _ = exit_sender.send(AppMsg::Tools(
                                crate::app::messages::ToolsMsg::ToolExited(exit_tool_name, error),
                            ));
                        })),
                    },
                )
                .map_err(|error| tool_launcher::ToolPrepareError::Fatal(error.to_string()))?;
                if options.monitor_proton_setup {
                    monitor_deployd_proton_runtime(setup_sender);
                }
                Ok(tool_name)
            }
            .await;
            crate::app::timing::log_phase(
                "tools.launch_prepare",
                &timing_game_id,
                timing_start,
                Some(1),
            );
            match result {
                Ok(name) if cancel.load(Ordering::SeqCst) => {
                    AppCmdMsg::Tools(crate::app::messages::ToolsCmdMsg::LaunchCancelled(name))
                }
                Err(tool_launcher::ToolPrepareError::Cancelled) => AppCmdMsg::Tools(
                    crate::app::messages::ToolsCmdMsg::LaunchCancelled(original_tool_name),
                ),
                other => AppCmdMsg::Tools(crate::app::messages::ToolsCmdMsg::Launched(other)),
            }
        });
    }

    fn show_tool_launch_dialog(
        &mut self,
        root: &adw::ApplicationWindow,
        tool_name: &str,
        show_setup_console: bool,
        sender: &ComponentSender<Self>,
    ) {
        let dialog = adw::AlertDialog::builder()
            .heading(format!("Launching {tool_name}"))
            .body("Preparing the Windows runtime and tool environment.")
            .build();
        dialog.add_response("cancel", "Cancel");
        dialog.set_close_response("cancel");

        let content = gtk::Box::builder()
            .orientation(if show_setup_console {
                gtk::Orientation::Vertical
            } else {
                gtk::Orientation::Horizontal
            })
            .spacing(12)
            .margin_top(6)
            .margin_bottom(6)
            .margin_start(6)
            .margin_end(6)
            .build();
        let spinner = gtk::Spinner::builder().spinning(true).build();
        let label = gtk::Label::builder()
            .label("Starting Wine/UMU...")
            .xalign(0.0)
            .hexpand(true)
            .build();
        label.add_css_class("dim-label");
        if show_setup_console {
            let status = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            status.append(&spinner);
            status.append(&label);
            content.append(&status);

            let buffer = gtk::TextBuffer::new(None);
            buffer.set_text("Preparing the tool environment...\n");
            let console = gtk::TextView::builder()
                .buffer(&buffer)
                .editable(false)
                .cursor_visible(false)
                .monospace(true)
                .wrap_mode(gtk::WrapMode::WordChar)
                .height_request(150)
                .build();
            let scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Automatic)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .child(&console)
                .build();
            content.append(&scroll);
            self.ui.tool_launch_log = Some(buffer);
        } else {
            content.append(&spinner);
            content.append(&label);
            self.ui.tool_launch_log = None;
        }
        dialog.set_extra_child(Some(&content));

        let input_sender = sender.input_sender().clone();
        dialog.connect_response(None, move |_, response| {
            if response == "cancel" {
                let _ = input_sender.send(AppMsg::Tools(
                    crate::app::messages::ToolsMsg::CancelToolLaunch,
                ));
            }
        });
        dialog.present(Some(root));
        self.ui.tool_launch_dialog = Some(dialog);
    }
}

fn monitor_deployd_proton_runtime(sender: relm4::Sender<AppMsg>) {
    std::thread::spawn(move || {
        for _ in 0..1800 {
            if game::proton_runtime_available() {
                let _ = sender.send(AppMsg::Tools(
                    crate::app::messages::ToolsMsg::ProtonSetupReady,
                ));
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{
        PROTON_SETUP_BODY, PostToolDeploy, PostToolExitActions, ToolExitKind,
        post_tool_exit_actions,
    };

    #[test]
    fn proton_setup_message_does_not_claim_a_download_size() {
        assert!(!PROTON_SETUP_BODY.contains("MB"));
        assert!(!PROTON_SETUP_BODY.contains("GB"));
    }

    // @variants: both
    #[test]
    fn normal_tool_exit_scans_and_sorts_every_loot_game() {
        for game_id in ["skyrim-se", "fallout-4", "fallout-nv", "starfield"] {
            assert_eq!(
                post_tool_exit_actions(ToolExitKind::Completed, Some(game_id)),
                PostToolExitActions {
                    scan_external_files: true,
                    sort_with_loot: cfg!(feature = "loot"),
                    deploy: Some(if cfg!(feature = "loot") {
                        PostToolDeploy::AfterLoot
                    } else {
                        PostToolDeploy::Now
                    }),
                },
                "unexpected post-tool actions for {game_id}",
            );
        }
    }

    // @variants: both
    #[test]
    fn cancelled_tool_exit_does_not_scan_or_sort() {
        assert_eq!(
            post_tool_exit_actions(ToolExitKind::Cancelled, Some("skyrim-se")),
            PostToolExitActions::default(),
        );
    }

    // @variants: both
    #[test]
    fn failed_tool_exit_does_not_scan_sort_or_deploy() {
        assert_eq!(
            post_tool_exit_actions(ToolExitKind::Failed, Some("skyrim-se")),
            PostToolExitActions::default(),
        );
    }

    // @variants: both
    #[test]
    fn unsupported_game_scans_without_loot_sort() {
        assert_eq!(
            post_tool_exit_actions(ToolExitKind::Completed, Some("witcher-3")),
            PostToolExitActions {
                scan_external_files: true,
                sort_with_loot: false,
                deploy: Some(PostToolDeploy::Now),
            },
        );
    }
}
