use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

use crate::core::game::{WineConfig, WineLauncher};
use crate::dlog;
use crate::models::game::Game;

const WINE_SILENT_DLL_OVERRIDES: &str = "mshtml=d;winemenubuilder.exe=d";

/// Launch the given Windows executable (typically a script extender loader) via
/// Wine or Proton GE using the game's existing Wine prefix.
///
/// `steam_app_id` should be the Steam App ID of the game (e.g. 1716740 for
/// Starfield). It is set as `SteamAppId` and `SteamGameId` so that
/// `SteamAPI_Init()` inside the game executable can connect to the running
/// Steam daemon and pass DRM validation.
///
/// `STEAM_COMPAT_CLIENT_INSTALL_PATH` is also set when a Steam installation is
/// detectable, enabling the Steam overlay and achievement notifications.
/// Both are best-effort: the game will still be launched if they cannot be
/// determined, but Steam features may be unavailable.
///
/// `on_exit` is called from a background thread once the process exits. Receives
/// `Some(error_string)` on non-zero exit or wait failure, `None` on clean exit.
pub(crate) fn launch_game(
    exe: &Path,
    game: &Game,
    wine_config: &WineConfig,
    steam_app_id: Option<u32>,
    on_exit: Option<Box<dyn FnOnce(Option<String>) + Send + 'static>>,
) -> Result<u32> {
    if !exe.exists() {
        return Err(anyhow!(
            "Script extender loader not found: {}",
            exe.display()
        ));
    }

    let cmd = match &wine_config.launcher {
        WineLauncher::Wine(bin) => {
            let wine_bin = resolve_wine64(bin);
            dlog!(
                "deployd: launching game '{}' via {}",
                exe.display(),
                wine_bin.display()
            );
            build_wine_cmd(&wine_bin, exe, game, wine_config, steam_app_id)
        }
        WineLauncher::SnapWine {
            wine_bin,
            wine_platform,
            wine_runtime,
        } => {
            let wine_bin = resolve_wine64(wine_bin);
            dlog!(
                "deployd: launching game '{}' via snap-wine {}",
                exe.display(),
                wine_bin.display()
            );
            build_snap_wine_cmd(
                &wine_bin,
                wine_platform,
                wine_runtime,
                exe,
                game,
                wine_config,
                steam_app_id,
            )
        }
    };

    spawn(cmd, exe, on_exit)
}

fn build_wine_cmd(
    wine_bin: &Path,
    exe: &Path,
    game: &Game,
    wine_config: &WineConfig,
    steam_app_id: Option<u32>,
) -> Command {
    let compat_data = strip_pfx(wine_config);
    let mut cmd = Command::new(wine_bin);
    cmd.env_remove("LD_PRELOAD");
    cmd.env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("STEAM_COMPAT_DATA_PATH", &compat_data)
        .env("WINEDLLOVERRIDES", WINE_SILENT_DLL_OVERRIDES);

    if let Some(proton_dir) = &wine_config.proton_dir {
        let lib_dir = proton_dir.join("files/lib");
        cmd.env(
            "LD_LIBRARY_PATH",
            format!(
                "{}:{}",
                lib_dir.join("x86_64-linux-gnu").display(),
                lib_dir.join("i386-linux-gnu").display(),
            ),
        );
        cmd.env(
            "WINEDLLPATH",
            format!(
                "{}:{}",
                lib_dir.join("vkd3d").display(),
                lib_dir.join("wine").display(),
            ),
        );
    }

    if let Some(steam_root) = super::steam::find_steam_root() {
        cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_root);
    }

    if let Some(appid) = steam_app_id {
        let id_str = appid.to_string();
        cmd.env("SteamAppId", &id_str).env("SteamGameId", &id_str);
    }

    cmd.arg(exe).current_dir(&game.path);
    cmd
}

fn build_snap_wine_cmd(
    wine_bin: &Path,
    wine_platform: &Path,
    wine_runtime: &Path,
    exe: &Path,
    game: &Game,
    wine_config: &WineConfig,
    steam_app_id: Option<u32>,
) -> Command {
    let compat_data = strip_pfx(wine_config);
    let mut cmd = Command::new(wine_bin);
    cmd.env_remove("LD_PRELOAD");
    cmd.env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("STEAM_COMPAT_DATA_PATH", &compat_data)
        .env("WINEDLLOVERRIDES", WINE_SILENT_DLL_OVERRIDES)
        .env(
            "LD_LIBRARY_PATH",
            snap_ld_library_path(wine_platform, wine_runtime),
        );

    let dri_path = snap_dri_drivers_path(wine_runtime);
    cmd.env("LIBGL_DRIVERS_PATH", &dri_path)
        .env("LIBVA_DRIVERS_PATH", &dri_path);

    if let Some(ids) = snap_amdgpu_ids_path(wine_runtime) {
        cmd.env("LIBDRM_AMDGPU_IDS", ids);
    }

    if let Some(steam_root) = super::steam::find_steam_root() {
        cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_root);
    }

    if let Some(appid) = steam_app_id {
        let id_str = appid.to_string();
        cmd.env("SteamAppId", &id_str).env("SteamGameId", &id_str);
    }

    cmd.arg(exe).current_dir(&game.path);
    cmd
}

fn spawn(
    mut cmd: Command,
    exe: &Path,
    on_exit: Option<Box<dyn FnOnce(Option<String>) + Send + 'static>>,
) -> Result<u32> {
    cmd.stderr(Stdio::piped());

    let name = exe
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| exe.to_string_lossy().into_owned());

    let mut child = cmd
        .spawn()
        .with_context(|| format!("Could not start process for \"{name}\""))?;

    let pid = child.id();

    let stderr_thread = child.stderr.take().map(|s| {
        std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = String::new();
            let _ = std::io::BufReader::new(s).read_to_string(&mut buf);
            buf
        })
    });

    std::thread::spawn(move || {
        let wait_result = child.wait();
        let stderr = stderr_thread
            .and_then(|h| h.join().ok())
            .filter(|s| !s.is_empty());

        let error = match wait_result {
            Ok(status) if !status.success() => {
                if let Some(s) = &stderr {
                    eprintln!("deployd: {name} exited {status}. stderr:\n{s}");
                } else {
                    eprintln!("deployd: {name} exited {status} (no stderr).");
                }
                Some(stderr.unwrap_or_else(|| format!("process exited with {status}")))
            }
            Err(e) => {
                eprintln!("deployd: failed to wait on process: {e}");
                Some(e.to_string())
            }
            _ => None,
        };

        if let Some(cb) = on_exit {
            cb(error);
        }
    });

    Ok(pid)
}

/// Derive `STEAM_COMPAT_DATA_PATH` from the Wine prefix.
/// Proton stores the prefix at `<compat_data>/pfx/`; strip that suffix to get
/// the `compatdata/<appid>` directory that Proton expects.
fn strip_pfx(wine_config: &WineConfig) -> PathBuf {
    if wine_config.prefix.ends_with("pfx") {
        wine_config
            .prefix
            .parent()
            .unwrap_or(&wine_config.prefix)
            .to_path_buf()
    } else {
        wine_config.prefix.clone()
    }
}

fn resolve_wine64(bin: &Path) -> PathBuf {
    if bin.file_name().is_some_and(|n| n == "wine") {
        let wine64 = bin.with_file_name("wine64");
        if wine64.exists() {
            return wine64;
        }
    }
    bin.to_path_buf()
}

fn snap_ld_library_path(wine_platform: &Path, wine_runtime: &Path) -> String {
    format!(
        "{p}/lib:{p}/lib64:{r}/lib:{r}/$LIB:{r}/usr/lib:{r}/usr/$LIB:{r}/usr/$LIB/dri:{r}/usr/$LIB/pulseaudio:{r}/usr/$LIB/samba",
        p = wine_platform.display(),
        r = wine_runtime.display(),
    )
}

fn snap_dri_drivers_path(wine_runtime: &Path) -> String {
    format!(
        "{r}/usr/lib/x86_64-linux-gnu/dri:{r}/usr/lib/i386-linux-gnu/dri",
        r = wine_runtime.display(),
    )
}

fn snap_amdgpu_ids_path(wine_runtime: &Path) -> Option<String> {
    let path = wine_runtime.join("usr/share/libdrm/amdgpu.ids");
    path.exists().then(|| path.to_string_lossy().into_owned())
}
