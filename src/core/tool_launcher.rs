use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

use crate::core::game::{self, WineConfig};
use crate::dlog;
use crate::models::game::Game;
use crate::models::tool::Tool;
use crate::utils::paths;

/// Suppress Mono/Gecko installer popups and the Wine menu builder for all tool launches.
const WINE_SILENT_DLL_OVERRIDES: &str = "mscoree,mshtml=d;winemenubuilder.exe=d";

/// Launch a Windows tool via Wine/Proton.
///
/// Invokes the tool directly under the Proton GE wine binary (bypassing
/// pressure-vessel/bwrap) or falls back to system wine.
///
/// `on_exit` is called from a background thread once the process exits.
pub fn launch_tool(
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
    cache_root: &std::path::Path,
    on_exit: Option<Box<dyn FnOnce(Option<String>) + Send + 'static>>,
) -> Result<u32> {
    let exe_path = PathBuf::from(&tool.exe_path);
    if !exe_path.exists() {
        return Err(anyhow!("Tool executable not found: {}", tool.exe_path));
    }

    game::ensure_ini_symlinks(game);
    ensure_bodyslide_config(tool, game, wine_config);
    ensure_named_mods_drive(wine_config, cache_root);
    ensure_no_x_drive_conflict(wine_config);

    let cmd = match &wine_config.launcher {
        game::WineLauncher::Wine(bin) => {
            let wine_bin = resolve_wine64(bin);
            ensure_bethesda_reg_key(game, wine_config, &wine_bin, None);
            ensure_wine_silent_setup(wine_config, &wine_bin, None);
            dlog!(
                "deployd: launching tool '{}' | wine={}",
                tool.name,
                wine_bin.display()
            );
            build_wine_command(&wine_bin, tool, game, wine_config)
        }
        game::WineLauncher::SnapWine {
            wine_bin,
            wine_platform,
            wine_runtime,
        } => {
            let wine_bin = resolve_wine64(wine_bin);
            let ld_lib = snap_ld_library_path(wine_platform, wine_runtime);
            ensure_bethesda_reg_key(game, wine_config, &wine_bin, Some(&ld_lib));
            ensure_wine_silent_setup(wine_config, &wine_bin, Some(&ld_lib));
            dlog!(
                "deployd: launching tool '{}' | snap-wine={}",
                tool.name,
                wine_bin.display()
            );
            build_snap_wine_command(
                &wine_bin,
                wine_platform,
                wine_runtime,
                tool,
                game,
                wine_config,
            )
        }
    };
    spawn_tool(cmd, &tool.name, on_exit)

    // UMU: commented out — pressure-vessel/bwrap blocked on AppImage + Snap.
    // WineLauncher::Umu(bin) => {
    //     game::ensure_ini_symlinks(game);
    //     ensure_bodyslide_config(tool, game, wine_config);
    //     ensure_named_mods_drive(wine_config);
    //     ensure_no_x_drive_conflict(wine_config);
    //     let cmd = build_umu_command(bin, tool, game, wine_config);
    //     spawn_tool(cmd, &tool.name, on_exit)
    // }
}

/// Shared process-spawn logic for both Wine and UMU paths.
///
/// `on_exit` receives `Some(error_string)` if the process exited with a non-zero
/// status or could not be waited on; `None` on clean exit.
fn spawn_tool(
    mut cmd: Command,
    tool_name: &str,
    on_exit: Option<Box<dyn FnOnce(Option<String>) + Send + 'static>>,
) -> Result<u32> {
    cmd.stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("Could not start process for \"{tool_name}\".\nError: {e}"))?;

    let pid = child.id();
    let name = tool_name.to_owned();

    // Drain stderr on a dedicated thread so the subprocess never blocks on a
    // full pipe buffer while we wait for it to exit (classic pipe-deadlock).
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

/// Build a command that runs the tool via Wine from the snap content interface.
///
/// Does NOT call sommelier — sommelier unconditionally overrides WINEPREFIX, which conflicts
/// with deployd's per-game prefix management. Instead, we call the wine binary directly and
/// mirror sommelier's LD_LIBRARY_PATH setup from wine-platform and wine-runtime mounts.
/// `$LIB` in the path is expanded by glibc's dynamic linker to the per-arch lib directory.
fn build_snap_wine_command(
    wine_bin: &Path,
    wine_platform: &Path,
    wine_runtime: &Path,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
) -> Command {
    let compat_data = strip_pfx_suffix(&wine_config.prefix);
    let mut cmd = Command::new(wine_bin);
    cmd.env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("STEAM_COMPAT_DATA_PATH", &compat_data);

    cmd.env(
        "LD_LIBRARY_PATH",
        snap_ld_library_path(wine_platform, wine_runtime),
    );

    let dri_path = snap_dri_drivers_path(wine_runtime);
    cmd.env("LIBGL_DRIVERS_PATH", &dri_path)
        .env("LIBVA_DRIVERS_PATH", &dri_path)
        .env("WINEDLLOVERRIDES", WINE_SILENT_DLL_OVERRIDES);

    cmd.arg(&tool.exe_path);
    for arg in tool.custom_args.split_whitespace() {
        cmd.arg(arg);
    }
    cmd.current_dir(effective_cwd(tool, game));
    cmd
}

/// Build a command that runs the tool under plain Wine.
fn build_wine_command(
    wine_bin: &PathBuf,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
) -> Command {
    let compat_data = strip_pfx_suffix(&wine_config.prefix);

    let mut cmd = Command::new(wine_bin);
    cmd.env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("STEAM_COMPAT_DATA_PATH", &compat_data);

    cmd.env("WINEDLLOVERRIDES", WINE_SILENT_DLL_OVERRIDES);

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

    cmd.arg(&tool.exe_path);
    for arg in tool.custom_args.split_whitespace() {
        cmd.arg(arg);
    }

    cmd.current_dir(effective_cwd(tool, game));
    cmd
}

// UMU: commented out — pressure-vessel/bwrap blocked on AppImage + Snap strict confinement.
// fn build_umu_command(umu_bin: &Path, tool: &Tool, game: &Game, wine_config: &WineConfig) -> Command {
//     let proton_path = game::find_proton_runtime()
//         .map(|p| p.to_string_lossy().into_owned())
//         .unwrap_or_else(|| "GE-Proton".to_string());
//     let compat_data = strip_pfx_suffix(&wine_config.prefix);
//     let mut cmd = Command::new(umu_bin);
//     cmd.env_remove("PYTHONPATH").env_remove("PYTHONHOME");
//     cmd.env("GAMEID", "0")
//         .env("WINEPREFIX", &wine_config.prefix)
//         .env("STEAM_COMPAT_DATA_PATH", &compat_data)
//         .env("PROTONPATH", &proton_path);
//     if let Ok(snap_common) = std::env::var("SNAP_USER_COMMON") {
//         cmd.env("XDG_DATA_HOME", snap_common);
//         cmd.env("PRESSURE_VESSEL_UNSHARE_USER", "0");
//     }
//     cmd.arg(&tool.exe_path);
//     for arg in tool.custom_args.split_whitespace() { cmd.arg(arg); }
//     cmd.current_dir(effective_cwd(tool, game));
//     cmd
// }

/// If the prefix path ends with `pfx` (Proton layout), return its parent as
/// the `STEAM_COMPAT_DATA_PATH` directory.  Otherwise return the path as-is.
fn strip_pfx_suffix(prefix: &Path) -> PathBuf {
    if prefix.ends_with("pfx") {
        prefix.parent().unwrap_or(prefix).to_path_buf()
    } else {
        prefix.to_path_buf()
    }
}

/// Ensure the standard Bethesda Softworks registry key exists for modding tool discovery.
///
/// GOG installers only create `GOG.com\Games\...` keys, but tools like xEdit look for
/// `Bethesda Softworks\<Game>\Installed Path`. This runs `wine reg add` to create it if missing.
fn snap_ld_library_path(wine_platform: &Path, wine_runtime: &Path) -> String {
    format!(
        "{p}/lib:{p}/lib64:{r}/lib:{r}/$LIB:{r}/usr/lib:{r}/usr/$LIB:{r}/usr/$LIB/dri:{r}/usr/$LIB/pulseaudio:{r}/usr/$LIB/samba",
        p = wine_platform.display(),
        r = wine_runtime.display(),
    )
}

/// Mesa DRI driver search path for both Wine arches on amd64.
///
/// Mesa's DRI loader doesn't expand `$LIB` tokens — must use concrete arch dirs.
fn snap_dri_drivers_path(wine_runtime: &Path) -> String {
    format!(
        "{r}/usr/lib/x86_64-linux-gnu/dri:{r}/usr/lib/i386-linux-gnu/dri",
        r = wine_runtime.display(),
    )
}

fn ensure_bethesda_reg_key(
    game: &Game,
    wine_config: &WineConfig,
    launcher_bin: &Path,
    ld_library_path: Option<&str>,
) {
    let Some((reg_key, wine_path)) = game::missing_bethesda_reg_key(game) else {
        return; // Key already exists
    };

    dlog!("deployd: adding registry key {reg_key} → {wine_path}");

    let reg_args: Vec<String> = [
        "reg",
        "add",
        &reg_key,
        "/v",
        "Installed Path",
        "/t",
        "REG_SZ",
        "/d",
        &wine_path,
        "/f",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    let mut cmd = Command::new(launcher_bin);
    cmd.env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all");
    if let Some(ld) = ld_library_path {
        cmd.env("LD_LIBRARY_PATH", ld);
    }
    cmd.args(&reg_args);
    let result = cmd.output();

    match result {
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
        Err(e) => {
            eprintln!("deployd: failed to run wine reg add: {e}");
        }
    }
}

/// Bake DLL overrides into the wine prefix registry so snap/wine updates don't re-show
/// mono/winecfg dialogs even when the env var isn't inherited by wine-internal processes.
///
/// Runs once per prefix: after the first successful run a sentinel file is written so
/// subsequent tool launches skip this entirely.
fn ensure_wine_silent_setup(
    wine_config: &WineConfig,
    launcher_bin: &Path,
    ld_library_path: Option<&str>,
) {
    let sentinel = wine_config.prefix.join(".deployd_wine_setup_v1");
    if sentinel.exists() {
        return;
    }

    // mscoree=disabled suppresses Mono/.NET installer
    // mshtml=disabled suppresses Gecko/HTML renderer installer
    // winemenubuilder.exe=disabled suppresses the wine menu builder dialog
    let overrides = [
        ("mscoree", ""),
        ("mshtml", ""),
        ("winemenubuilder.exe", ""),
    ];
    for (name, value) in overrides {
        let mut cmd = Command::new(launcher_bin);
        cmd.env("WINEPREFIX", &wine_config.prefix)
            .env("WINEDEBUG", "-all")
            .env_remove("LD_PRELOAD");
        if let Some(ld) = ld_library_path {
            cmd.env("LD_LIBRARY_PATH", ld);
        }
        cmd.args([
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
        let result = cmd.output();
        if let Err(e) = result {
            eprintln!("deployd: wine silent setup reg add ({name}): {e}");
            return;
        }
    }

    if let Err(e) = std::fs::write(&sentinel, b"") {
        eprintln!("deployd: failed to write wine setup sentinel: {e}");
    }
}

/// Determine the working directory to use when launching a tool.
///
/// Priority:
/// 1. `tool.working_dir` if explicitly set by the user.
/// 2. The directory that contains the tool executable.
/// 3. The game root as final fallback.
fn effective_cwd(tool: &Tool, game: &Game) -> PathBuf {
    if !tool.working_dir.is_empty() {
        return PathBuf::from(&tool.working_dir);
    }
    PathBuf::from(&tool.exe_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| game.path.clone())
}

/// If `wine_bin` points to `wine` and a `wine64` sibling exists, prefer `wine64`.
/// Modern Proton Wine uses WoW64 so `wine64` handles both 32/64-bit Windows apps.
fn resolve_wine64(wine_bin: &Path) -> PathBuf {
    if wine_bin.file_name().is_some_and(|n| n == "wine") {
        let wine64 = wine_bin.with_file_name("wine64");
        if wine64.exists() {
            return wine64;
        }
    }
    wine_bin.to_path_buf()
}

/// Create (or update) `<prefix>/dosdevices/m:` → `named_mods/` so the deployd mod
/// library is accessible as `M:\` inside any Wine/Proton process.
fn ensure_named_mods_drive(wine_config: &WineConfig, cache_root: &std::path::Path) {
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
    if let Err(e) = std::os::unix::fs::symlink(&named_mods, &link) {
        eprintln!("[deployd] failed to create M: drive in dosdevices: {e}");
    } else {
        dlog!("deployd: mapped M: → {}", named_mods.display());
    }
}

/// Remap drive X: to an available letter if it exists in dosdevices.
///
/// Proton reserves X: for the Steam Linux Runtime at container setup time,
/// shadowing any dosdevices entry the game installer (e.g. Heroic) placed
/// there. If the game's Windows path was X:\, tools running under Proton will
/// get access-denied errors even though the Linux path is readable.
///
/// This is a no-op when X: is not present — the common case for prefixes that
/// were never mapped to X:.
fn ensure_no_x_drive_conflict(wine_config: &WineConfig) {
    let dosdevices = wine_config.prefix.join("dosdevices");
    let x_drive = dosdevices.join("x:");

    if !x_drive.is_symlink() {
        return;
    }

    let Ok(target) = std::fs::read_link(&x_drive) else {
        return;
    };

    // Letters Proton does not reserve. M: is already used by deployd for mods.
    const CANDIDATES: &[char] = &['s', 'g', 'h', 'i', 'j', 'k', 'l', 'n', 'o', 'p', 'q', 'r'];
    let Some(&new_letter) = CANDIDATES
        .iter()
        .find(|&&c| !dosdevices.join(format!("{c}:")).exists())
    else {
        eprintln!("deployd: no free drive letter to remap X: — leaving as-is");
        return;
    };

    let new_drive = dosdevices.join(format!("{new_letter}:"));

    #[cfg(unix)]
    {
        if let Err(e) = std::os::unix::fs::symlink(&target, &new_drive) {
            eprintln!("deployd: failed to create {new_letter}: drive: {e}");
            return;
        }
        if let Err(e) = std::fs::remove_file(&x_drive) {
            eprintln!("deployd: failed to remove x: drive: {e}");
            let _ = std::fs::remove_file(&new_drive);
        } else {
            dlog!("deployd: remapped X: → {new_letter}: (Proton reserves X: for runtime)");
        }
    }
}

/// Pre-configure BodySlide's Config.xml with the correct `GameDataPath` and `TargetGame`.
fn ensure_bodyslide_config(tool: &Tool, game: &Game, wine_config: &WineConfig) {
    let exe_name = Path::new(&tool.exe_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if !exe_name.to_ascii_lowercase().contains("bodyslide") {
        return;
    }

    let users_dir = wine_config.prefix.join("drive_c/users");
    let Some(user_dir) = find_prefix_user_dir(&users_dir) else {
        eprintln!(
            "deployd: BodySlide config: no Wine user dir found in {}",
            users_dir.display()
        );
        return;
    };

    let config_dir_local = user_dir.join("AppData/Local/BodySlide and Outfit Studio");
    let config_dir_roaming = user_dir.join("AppData/Roaming/BodySlide and Outfit Studio");
    let config_dir = if config_dir_roaming.exists() && !config_dir_local.exists() {
        config_dir_roaming
    } else {
        config_dir_local
    };

    let config_path = config_dir.join("Config.xml");

    let game_data_dir = game.path.join(&game.data_subdir);
    let data_path = game::linux_path_to_wine_path(&game_data_dir, &wine_config.prefix)
        .unwrap_or_else(|| {
            format!(
                "Z:{}\\{}\\",
                game.path.to_string_lossy().replace('/', "\\"),
                game.data_subdir,
            )
        });

    match write_bodyslide_config(&config_path, &data_path, &game.title) {
        Ok(()) => dlog!(
            "deployd: BodySlide Config.xml written — GameDataPath={}",
            data_path
        ),
        Err(e) => eprintln!("deployd: failed to write BodySlide Config.xml: {e}"),
    }
}

/// Find the first usable Wine user directory under `<prefix>/drive_c/users/`.
fn find_prefix_user_dir(users_dir: &Path) -> Option<PathBuf> {
    for name in &["steamuser", "Public"] {
        let p = users_dir.join(name);
        if p.is_dir() {
            return Some(p);
        }
    }
    std::fs::read_dir(users_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
}

/// Write (or update in-place) BodySlide's `Config.xml`.
fn write_bodyslide_config(
    config_path: &Path,
    game_data_path: &str,
    target_game: &str,
) -> Result<()> {
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).context("create BodySlide config dir")?;
    }

    let content = if let Ok(existing) = std::fs::read_to_string(config_path) {
        let updated = patch_xml_value(&existing, "GameDataPath", game_data_path);
        patch_xml_value(&updated, "TargetGame", target_game)
    } else {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <Config>\n\
             \t<GameDataPath>{game_data_path}</GameDataPath>\n\
             \t<TargetGame>{target_game}</TargetGame>\n\
             </Config>\n"
        )
    };

    std::fs::write(config_path, content.as_bytes()).context("write BodySlide Config.xml")?;
    Ok(())
}

/// Replace the text content of `<tag>…</tag>` in `xml`.
/// If the tag is absent, inserts a new element before `</Config>`.
fn patch_xml_value(xml: &str, tag: &str, value: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let (Some(start), Some(end)) = (xml.find(&open), xml.find(&close)) {
        let before = &xml[..start + open.len()];
        let after = &xml[end..];
        format!("{before}{value}{after}")
    } else if let Some(pos) = xml.rfind("</Config>") {
        let (before, after) = xml.split_at(pos);
        format!("{before}\t<{tag}>{value}</{tag}>\n{after}")
    } else {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <Config>\n\
             \t<{tag}>{value}</{tag}>\n\
             </Config>\n"
        )
    }
}
