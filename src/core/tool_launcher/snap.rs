use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use anyhow::Result;

use crate::core::game::{self, WineConfig};
use crate::dlog;
use crate::models::game::Game;
use crate::models::tool::Tool;

use super::runtime;

pub(super) fn build_command(
    wine_binary: &Path,
    wine_platform: &Path,
    wine_runtime: &Path,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
) -> Command {
    let compat_data = super::strip_pfx_suffix(&wine_config.prefix);
    let mut command = Command::new(wine_binary);
    // The GNOME extension's preload points outside the Wine content mount and can corrupt Wine.
    command
        .env_remove("LD_PRELOAD")
        .env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("STEAM_COMPAT_DATA_PATH", &compat_data)
        .env("LD_LIBRARY_PATH", library_path(wine_platform, wine_runtime));

    let drivers_path = dri_drivers_path(wine_runtime);
    command
        .env("LIBGL_DRIVERS_PATH", &drivers_path)
        .env("LIBVA_DRIVERS_PATH", &drivers_path)
        .env("WINEDLLOVERRIDES", super::WINE_SILENT_DLL_OVERRIDES);
    if let Some(ids_path) = amdgpu_ids_path(wine_runtime) {
        command.env("LIBDRM_AMDGPU_IDS", ids_path);
    }

    command.arg(super::effective_tool_exe_path(tool));
    for argument in tool.custom_args.split_whitespace() {
        command.arg(argument);
    }
    command.current_dir(super::effective_cwd(tool, game));
    command
}

pub(super) fn library_path(wine_platform: &Path, wine_runtime: &Path) -> String {
    format!(
        "{p}/lib:{p}/lib64:{r}/lib:{r}/$LIB:{r}/usr/lib:{r}/usr/$LIB:{r}/usr/$LIB/dri:{r}/usr/$LIB/pulseaudio:{r}/usr/$LIB/samba",
        p = wine_platform.display(),
        r = wine_runtime.display(),
    )
}

pub(super) fn ensure_bethesda_reg_key(
    game: &Game,
    wine_config: &WineConfig,
    launcher_binary: &Path,
    library_path: &str,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let Some((registry_key, wine_path)) = game::missing_bethesda_reg_key(game) else {
        return Ok(());
    };
    dlog!("deployd: adding registry key {registry_key} → {wine_path}");
    let mut command = Command::new(launcher_binary);
    command
        .env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("LD_LIBRARY_PATH", library_path)
        .args([
            "reg",
            "add",
            &registry_key,
            "/v",
            "Installed Path",
            "/t",
            "REG_SZ",
            "/d",
            &wine_path,
            "/f",
        ]);

    match runtime::run_output_cancellable(&mut command, cancel) {
        Ok(output) if output.status.success() => {
            dlog!("deployd: registry key added successfully");
        }
        Ok(output) => {
            eprintln!(
                "deployd: failed to add registry key ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(error) if runtime::is_cancelled(cancel) => return Err(error),
        Err(error) => eprintln!("deployd: failed to run wine reg add: {error}"),
    }
    Ok(())
}

pub(super) fn ensure_silent_setup(
    wine_config: &WineConfig,
    launcher_binary: &Path,
    library_path: &str,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let sentinel_v2 = wine_config.prefix.join(".deployd_wine_setup_v2");
    if sentinel_v2.exists() {
        return Ok(());
    }

    let sentinel_v1 = wine_config.prefix.join(".deployd_wine_setup_v1");
    if sentinel_v1.exists() {
        // V1 disabled mscoree, preventing Wine from offering Mono to .NET-based Eclipse tools.
        let mut command = setup_command(wine_config, launcher_binary, library_path);
        command.args([
            "reg",
            "delete",
            r"HKCU\Software\Wine\DllOverrides",
            "/v",
            "mscoree",
            "/f",
        ]);
        if let Err(error) = runtime::run_output_cancellable(&mut command, cancel)
            && runtime::is_cancelled(cancel)
        {
            return Err(error);
        }
        let _ = std::fs::remove_file(&sentinel_v1);
    }

    // Persist these overrides so Wine-internal child processes suppress Gecko and menu prompts.
    for (name, value) in [("mshtml", ""), ("winemenubuilder.exe", "")] {
        let mut command = setup_command(wine_config, launcher_binary, library_path);
        command.args([
            "reg",
            "add",
            r"HKCU\Software\Wine\DllOverrides",
            "/v",
            name,
            "/t",
            "REG_SZ",
            "/d",
            value,
            "/f",
        ]);
        if let Err(error) = runtime::run_output_cancellable(&mut command, cancel) {
            if runtime::is_cancelled(cancel) {
                return Err(error);
            }
            eprintln!("deployd: wine silent setup reg add ({name}): {error}");
            return Ok(());
        }
    }

    if let Err(error) = std::fs::write(&sentinel_v2, b"") {
        eprintln!("deployd: failed to write wine setup sentinel: {error}");
    }
    Ok(())
}

fn setup_command(wine_config: &WineConfig, launcher_binary: &Path, library_path: &str) -> Command {
    let mut command = Command::new(launcher_binary);
    command
        .env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("WINEDLLOVERRIDES", super::WINE_SILENT_DLL_OVERRIDES)
        .env("LD_LIBRARY_PATH", library_path)
        .env_remove("LD_PRELOAD");
    command
}

fn dri_drivers_path(wine_runtime: &Path) -> String {
    format!(
        "{r}/usr/lib/x86_64-linux-gnu/dri:{r}/usr/lib/i386-linux-gnu/dri",
        r = wine_runtime.display(),
    )
}

fn amdgpu_ids_path(wine_runtime: &Path) -> Option<String> {
    let path = wine_runtime.join("usr/share/libdrm/amdgpu.ids");
    path.exists().then(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use std::ffi::OsStr;
    use std::path::PathBuf;

    use super::*;
    use crate::core::game::WineLauncher;
    use crate::models::game::GameEngine;

    fn environment(command: &Command, key: &str) -> Option<String> {
        command.get_envs().find_map(|(name, value)| {
            (name == OsStr::new(key))
                .then(|| value.map(|value| value.to_string_lossy().into_owned()))
                .flatten()
        })
    }

    // @variants: snap
    #[test]
    fn snap_plan_uses_only_content_runtime_paths() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let wine_platform = temp.path().join("wine-platform");
        let wine_runtime = temp.path().join("wine-runtime");
        let wine_binary = wine_platform.join("wine-stable/bin/wine64");
        let prefix = temp.path().join("compatdata/pfx");
        let executable = temp.path().join("tools/xEdit.exe");
        std::fs::create_dir_all(executable.parent().unwrap_or(temp.path()))?;
        std::fs::write(&executable, b"")?;
        let game = Game {
            id: "skyrim-se".to_string(),
            title: "Skyrim Special Edition".to_string(),
            path: temp.path().join("game"),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: Some(prefix.clone()),
        };
        let tool = Tool {
            id: "xedit".to_string(),
            game_id: game.id.clone(),
            name: "xEdit".to_string(),
            exe_path: executable.to_string_lossy().into_owned(),
            icon_name: String::new(),
            custom_args: "-quickautoclean".to_string(),
            sort_order: 0,
            working_dir: String::new(),
        };
        let wine_config = WineConfig {
            prefix: prefix.clone(),
            launcher: WineLauncher::SnapWine {
                wine_bin: wine_binary.clone(),
                wine_platform: wine_platform.clone(),
                wine_runtime: wine_runtime.clone(),
            },
        };

        let command = build_command(
            &wine_binary,
            &wine_platform,
            &wine_runtime,
            &tool,
            &game,
            &wine_config,
        );

        assert_eq!(command.get_program(), wine_binary);
        assert_eq!(
            environment(&command, "WINEPREFIX"),
            Some(prefix.display().to_string())
        );
        assert!(
            environment(&command, "LD_LIBRARY_PATH")
                .is_some_and(|value| value.contains(&wine_runtime.display().to_string()))
        );
        assert_eq!(environment(&command, "PROTONPATH"), None);
        assert_eq!(environment(&command, "UMU_FOLDERS_PATH"), None);
        assert!(command.get_args().any(|argument| argument == executable));
        Ok(())
    }

    // @variants: snap
    #[test]
    fn snap_setup_uses_content_runtime_without_host_preload() {
        let prefix = PathBuf::from("/tmp/deployd-test-prefix");
        let wine_config = WineConfig {
            prefix: prefix.clone(),
            launcher: WineLauncher::SnapWine {
                wine_bin: PathBuf::from("/snap/wine-platform/current/bin/wine64"),
                wine_platform: PathBuf::from("/snap/wine-platform/current"),
                wine_runtime: PathBuf::from("/snap/wine-runtime/current"),
            },
        };
        let launcher = Path::new("/snap/wine-platform/current/bin/wine64");
        let content_libraries = "/snap/wine-runtime/current/usr/lib";

        let command = setup_command(&wine_config, launcher, content_libraries);

        assert_eq!(command.get_program(), launcher);
        assert_eq!(
            environment(&command, "WINEPREFIX"),
            Some(prefix.display().to_string())
        );
        assert_eq!(
            environment(&command, "LD_LIBRARY_PATH"),
            Some(content_libraries.to_string())
        );
        assert_eq!(
            environment(&command, "WINEDLLOVERRIDES"),
            Some(super::super::WINE_SILENT_DLL_OVERRIDES.to_string())
        );
        assert!(
            command
                .get_envs()
                .any(|(name, value)| { name == OsStr::new("LD_PRELOAD") && value.is_none() })
        );
    }
}
