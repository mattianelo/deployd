use relm4::prelude::*;

use crate::core::game;

use super::App;
use super::messages::{AppCmdMsg, AppMsg};

impl App {
    pub(crate) fn handle_launch_game_clicked(&mut self, sender: &ComponentSender<Self>) {
        let Some(game) = self.selected_game().cloned() else {
            return;
        };

        let Some(loader_path) = game::script_extender_loader_path(&game) else {
            return;
        };

        let wine_config = match game::detect_wine_config(&game) {
            Some(c) => c,
            None => {
                self.push_notification("Wine not configured for this game");
                return;
            }
        };

        let steam_app_id = game::game_steam_app_id(&game);
        let exit_sender = sender.input_sender().clone();
        sender.oneshot_command(async move {
            let result: Result<(), String> = game::launch_game(
                &loader_path,
                &game,
                &wine_config,
                steam_app_id,
                Some(Box::new(move |error| {
                    let _ = exit_sender.send(AppMsg::GameExited(error));
                })),
            )
            .map(|_| ())
            .map_err(|e| e.to_string());
            AppCmdMsg::GameLaunched(result)
        });
    }

    pub(crate) fn handle_cmd_game_launched(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.show_toast("Game launched");
            }
            Err(e) => {
                crate::dlog!("deployd: game launch error: {e}");
                self.push_notification(&format!("Launch failed: {e}"));
            }
        }
    }

    pub(crate) fn handle_game_exited(&mut self, error: Option<String>) {
        if let Some(msg) = error {
            self.push_notification(&format!("Game exited with error: {msg}"));
        }
    }
}
