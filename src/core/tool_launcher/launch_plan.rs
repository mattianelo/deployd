use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use anyhow::{Result, anyhow};

use crate::core::game::{self, WineConfig};
use crate::dlog;
use crate::models::game::Game;
use crate::models::tool::Tool;
use crate::utils::paths;

use super::{appimage, runtime, snap};

pub(super) struct LaunchPlan {
    pub command: Command,
    pub tool_name: String,
}

pub(super) fn build(
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
    cache_root: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<LaunchPlan> {
    runtime::ensure_not_cancelled(cancel)?;

    let exe_path = Path::new(&tool.exe_path);
    if !exe_path.exists() {
        return Err(anyhow!("Tool executable not found: {}", tool.exe_path));
    }

    game::ensure_ini_symlinks(game);
    runtime::ensure_not_cancelled(cancel)?;
    super::ensure_bodyslide_config(tool, game, wine_config);
    runtime::ensure_not_cancelled(cancel)?;
    ensure_named_mods_drive(wine_config, cache_root);
    runtime::ensure_not_cancelled(cancel)?;
    ensure_no_x_drive_conflict(wine_config);
    runtime::ensure_not_cancelled(cancel)?;

    let command = match &wine_config.launcher {
        game::WineLauncher::Umu(binary) => {
            appimage::build_command(binary, tool, game, wine_config)?
        }
        game::WineLauncher::SnapWine {
            wine_bin,
            wine_platform,
            wine_runtime,
        } => {
            let wine_bin = super::resolve_wine64(wine_bin);
            let library_path = snap::library_path(wine_platform, wine_runtime);
            snap::ensure_bethesda_reg_key(game, wine_config, &wine_bin, &library_path, cancel)?;
            runtime::ensure_not_cancelled(cancel)?;
            snap::ensure_silent_setup(wine_config, &wine_bin, &library_path, cancel)?;
            runtime::ensure_not_cancelled(cancel)?;
            dlog!(
                "deployd: launching tool '{}' | snap-wine={}",
                tool.name,
                wine_bin.display()
            );
            snap::build_command(
                &wine_bin,
                wine_platform,
                wine_runtime,
                tool,
                game,
                wine_config,
            )
        }
    };
    runtime::ensure_not_cancelled(cancel)?;

    Ok(LaunchPlan {
        command,
        tool_name: tool.name.clone(),
    })
}

// Windows tools need a stable drive path to Deployd's named-mod library.
fn ensure_named_mods_drive(wine_config: &WineConfig, cache_root: &Path) {
    let named_mods = paths::named_mods_dir_in(cache_root);
    if !named_mods.exists() {
        return;
    }

    let dosdevices = wine_config.prefix.join("dosdevices");
    if !dosdevices.is_dir() {
        return;
    }

    let link = dosdevices.join("m:");
    if link.is_symlink() {
        if let Ok(target) = std::fs::read_link(&link)
            && target == named_mods
        {
            return;
        }
        let _ = std::fs::remove_file(&link);
    }

    #[cfg(unix)]
    if let Err(error) = std::os::unix::fs::symlink(&named_mods, &link) {
        eprintln!("[deployd] failed to create M: drive in dosdevices: {error}");
    } else {
        dlog!("deployd: mapped M: → {}", named_mods.display());
    }
}

// Proton reserves `X:`, so preserve an existing game mapping under a free drive letter.
fn ensure_no_x_drive_conflict(wine_config: &WineConfig) {
    let dosdevices = wine_config.prefix.join("dosdevices");
    let x_drive = dosdevices.join("x:");
    if !x_drive.is_symlink() {
        return;
    }
    let Ok(target) = std::fs::read_link(&x_drive) else {
        return;
    };

    const CANDIDATES: &[char] = &['s', 'g', 'h', 'i', 'j', 'k', 'l', 'n', 'o', 'p', 'q', 'r'];
    let Some(&new_letter) = CANDIDATES
        .iter()
        .find(|&&letter| !drive_entry_exists(&dosdevices.join(format!("{letter}:"))))
    else {
        eprintln!("deployd: no free drive letter to remap X: — leaving as-is");
        return;
    };
    let new_drive = dosdevices.join(format!("{new_letter}:"));

    #[cfg(unix)]
    {
        if let Err(error) = std::os::unix::fs::symlink(&target, &new_drive) {
            eprintln!("deployd: failed to create {new_letter}: drive: {error}");
            return;
        }
        if let Err(error) = std::fs::remove_file(&x_drive) {
            eprintln!("deployd: failed to remove x: drive: {error}");
            let _ = std::fs::remove_file(&new_drive);
        } else {
            dlog!("deployd: remapped X: → {new_letter}: (Proton reserves X: for runtime)");
        }
    }
}

pub(super) fn drive_entry_exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::models::game::GameEngine;

    #[test]
    fn missing_executable_returns_actionable_error_before_side_effects() {
        let game = Game {
            id: "test".to_string(),
            title: "Test".to_string(),
            path: PathBuf::from("game"),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: Some(PathBuf::from("prefix")),
        };
        let tool = Tool {
            id: "missing".to_string(),
            game_id: game.id.clone(),
            name: "Missing Tool".to_string(),
            exe_path: "definitely-missing-tool.exe".to_string(),
            icon_name: String::new(),
            custom_args: String::new(),
            sort_order: 0,
            working_dir: String::new(),
        };
        let wine_config = WineConfig {
            prefix: PathBuf::from("prefix"),
            launcher: game::WineLauncher::Umu(PathBuf::from("umu-run")),
        };

        let error = build(&tool, &game, &wine_config, Path::new("cache"), None)
            .err()
            .expect("missing executable must fail planning");

        assert!(error.to_string().contains("Tool executable not found"));
        assert!(error.to_string().contains("definitely-missing-tool.exe"));
    }
}
