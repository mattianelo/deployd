use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};

use crate::core::game::{self, WineConfig};
use crate::models::game::Game;
use crate::models::tool::Tool;

pub(super) fn build_command(
    umu_binary: &Path,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
) -> Result<Command> {
    let folders = game::umu_folders_path().context("resolve UMU data directory")?;
    Ok(build_command_with_folders(
        umu_binary,
        tool,
        game,
        wine_config,
        &folders,
    ))
}

pub(super) fn build_command_with_folders(
    umu_binary: &Path,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
    folders: &Path,
) -> Command {
    let compat_data = super::strip_pfx_suffix(&wine_config.prefix);
    let mut command = Command::new(umu_binary);
    command
        .env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME")
        .env_remove("LD_PRELOAD")
        .env("GAMEID", "0")
        .env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("STEAM_COMPAT_DATA_PATH", &compat_data)
        .env("PROTONPATH", "GE-Proton")
        .env("UMU_FOLDERS_PATH", folders)
        .env("WINEDLLOVERRIDES", super::WINE_SILENT_DLL_OVERRIDES)
        .arg(super::effective_tool_exe_path(tool));
    for argument in tool.custom_args.split_whitespace() {
        command.arg(argument);
    }
    command.current_dir(super::effective_cwd(tool, game));
    command
}
