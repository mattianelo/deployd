use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, anyhow};

use crate::core::game::{self, WineConfig};
use crate::dlog;
use crate::models::game::Game;
use crate::models::tool::Tool;
use crate::utils::paths;

const HEROIC_FLATPAK_ID: &str = "com.heroicgameslauncher.hgl";

/// Launch a Windows tool via Wine/Proton.
///
/// When Heroic is a Flatpak, the Wine/Proton binaries depend on the Flatpak runtime's
/// libraries (including 32-bit compat). We launch via `flatpak run --command=<wine>`
/// so the process runs inside Heroic's sandbox with all necessary libraries available.
///
/// For native Heroic installs, runs the Wine binary directly.
///
/// Before launching, ensures the standard Bethesda registry key exists so modding tools
/// can find the game (GOG installers don't create it).
///
/// Returns the child process ID on success.
///
/// `on_exit` is called from a background thread once the Wine process exits
/// (regardless of exit status). Use this to trigger a post-tool file scan.
pub fn launch_tool(
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
    on_exit: Option<Box<dyn FnOnce() + Send + 'static>>,
) -> Result<u32> {
    let exe_path = PathBuf::from(&tool.exe_path);
    if !exe_path.exists() {
        return Err(anyhow!("Tool executable not found: {}", tool.exe_path));
    }

    let wine_bin = resolve_wine64(&wine_config.wine_bin);

    // Ensure modding tools can find the game via standard registry keys
    // and INI files in the standard My Games folder.
    ensure_bethesda_reg_key(game, wine_config, &wine_bin);
    game::ensure_ini_symlinks(game);
    ensure_bodyslide_config(tool, game, wine_config);

    // Map M: → named_mods/ so tools like NPC Plugin Chooser 2 can access all mod folders.
    ensure_named_mods_drive(wine_config);

    let mut cmd = if wine_config.heroic_flatpak {
        build_flatpak_command(&wine_bin, tool, game, wine_config)
    } else {
        build_native_command(&wine_bin, tool, game, wine_config)
    };

    dlog!(
        "deployd: launching tool '{}' | heroic_flatpak={} | wine={}",
        tool.name,
        wine_config.heroic_flatpak,
        wine_bin.display()
    );

    // Capture stderr so we can report Wine errors on early failure.
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(|e| {
        anyhow!(
            "Could not start Wine ({}).\n\
             Binary: {}\n\
             Heroic Flatpak: {}\n\
             Error: {e}",
            tool.name,
            wine_bin.display(),
            wine_config.heroic_flatpak,
        )
    })?;

    let pid = child.id();
    let tool_name = tool.name.clone();

    // Background thread captures stderr on early crash, then fires the on_exit callback.
    std::thread::spawn(move || {
        match child.wait() {
            Ok(status) if !status.success() => {
                let stderr = child
                    .stderr
                    .take()
                    .and_then(|mut s| {
                        use std::io::Read;
                        let mut buf = String::new();
                        s.read_to_string(&mut buf).ok()?;
                        Some(buf)
                    })
                    .unwrap_or_default();

                if !stderr.is_empty() {
                    eprintln!("deployd: {tool_name} Wine exited {status}. stderr:\n{stderr}");
                } else {
                    eprintln!("deployd: {tool_name} Wine exited {status} (no stderr).");
                }
            }
            Err(e) => {
                eprintln!("deployd: failed to wait on Wine process: {e}");
            }
            _ => {}
        }
        if let Some(cb) = on_exit {
            cb();
        }
    });

    Ok(pid)
}


/// Ensure the standard Bethesda Softworks registry key exists for modding tool discovery.
///
/// GOG installers only create `GOG.com\Games\...` keys, but tools like xEdit look for
/// `Bethesda Softworks\<Game>\Installed Path`. This runs `wine reg add` to create it if missing.
fn ensure_bethesda_reg_key(game: &Game, wine_config: &WineConfig, wine_bin: &PathBuf) {
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

    let result = if wine_config.heroic_flatpak {
        let mut flatpak_args = vec![
            "run".to_string(),
            format!("--command={}", wine_bin.display()),
            format!("--env=WINEPREFIX={}", wine_config.prefix.display()),
            "--env=WINEDEBUG=-all".to_string(),
            HEROIC_FLATPAK_ID.to_string(),
        ];
        flatpak_args.extend(reg_args);

        Command::new("flatpak").args(&flatpak_args).output()
    } else {
        Command::new(wine_bin)
            .env("WINEPREFIX", &wine_config.prefix)
            .env("WINEDEBUG", "-all")
            .args(&reg_args)
            .output()
    };

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

/// Build a command that runs Wine inside Heroic's Flatpak sandbox.
///
/// Uses `flatpak run --command=<wine_bin> --env=KEY=VAL ... com.heroicgameslauncher.hgl <exe>`.
///
/// CWD is set to the game root so modding tools can find game files.
/// The tool's directory and the game directory are both exposed to Heroic's sandbox.
fn build_flatpak_command(
    wine_bin: &PathBuf,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
) -> Command {
    let mut flatpak_args = vec![
        "run".to_string(),
        format!("--command={}", wine_bin.display()),
        format!("--env=WINEPREFIX={}", wine_config.prefix.display()),
        "--env=WINEDEBUG=-all".to_string(),
    ];

    let compat_data = if wine_config.prefix.ends_with("pfx") {
        wine_config.prefix.parent().unwrap_or(&wine_config.prefix)
    } else {
        &wine_config.prefix
    };
    flatpak_args.push(format!(
        "--env=STEAM_COMPAT_DATA_PATH={}",
        compat_data.display()
    ));

    if let Some(proton_dir) = &wine_config.proton_dir {
        let lib_dir = proton_dir.join("files/lib");

        let ld_val = format!(
            "{}:{}",
            lib_dir.join("x86_64-linux-gnu").display(),
            lib_dir.join("i386-linux-gnu").display(),
        );
        flatpak_args.push(format!("--env=LD_LIBRARY_PATH={ld_val}"));

        let dll_val = format!(
            "{}:{}",
            lib_dir.join("vkd3d").display(),
            lib_dir.join("wine").display(),
        );
        flatpak_args.push(format!("--env=WINEDLLPATH={dll_val}"));
    }

    // Grant full home directory access inside Heroic's sandbox. Using --filesystem=home
    // keeps the filesystem layout identical to the host, so Wine's Z: drive (symlink to /)
    // resolves all paths correctly. Path-specific --filesystem= flags create additional
    // bind mounts that Wine may assign separate drive letters (X:), breaking tools like
    // BodySlide that resolve paths relative to their own exe location.
    flatpak_args.push("--filesystem=home".to_string());

    // CWD: use the tool's explicit working_dir when set, otherwise the exe's parent directory.
    // The exe parent is the right default for bat/script wrappers that use %CD% to find
    // siblings (e.g. Complex Sorter locating FO4Edit.exe next to itself).
    let cwd = effective_cwd(tool, game);
    flatpak_args.push(format!("--cwd={}", cwd.display()));

    flatpak_args.push(HEROIC_FLATPAK_ID.to_string());
    flatpak_args.push(tool.exe_path.clone());

    for arg in tool.custom_args.split_whitespace() {
        flatpak_args.push(arg.to_string());
    }

    let mut cmd = Command::new("flatpak");
    cmd.args(flatpak_args);
    cmd
}

/// Build a command that runs Wine directly (native Heroic install).
///
/// CWD is set to the game root so modding tools can find game files.
fn build_native_command(
    wine_bin: &PathBuf,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
) -> Command {
    let compat_data = if wine_config.prefix.ends_with("pfx") {
        wine_config
            .prefix
            .parent()
            .unwrap_or(&wine_config.prefix)
            .to_path_buf()
    } else {
        wine_config.prefix.clone()
    };

    let mut cmd = Command::new(wine_bin);
    cmd.env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("STEAM_COMPAT_DATA_PATH", &compat_data);

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

    // CWD: use the tool's explicit working_dir when set, otherwise the exe's parent directory.
    let cwd = effective_cwd(tool, game);
    cmd.current_dir(&cwd);
    cmd
}

/// Determine the working directory to use when launching a tool.
///
/// Priority:
/// 1. `tool.working_dir` if explicitly set by the user.
/// 2. The directory that contains the tool executable (covers bat/script wrappers
///    that use `%CD%` to locate sibling files such as FO4Edit.exe).
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
fn resolve_wine64(wine_bin: &PathBuf) -> PathBuf {
    if wine_bin.file_name().is_some_and(|n| n == "wine") {
        let wine64 = wine_bin.with_file_name("wine64");
        if wine64.exists() {
            return wine64;
        }
    }
    wine_bin.clone()
}

/// Pre-configure BodySlide's Config.xml with the correct `GameDataPath` and `TargetGame`.
///
/// BodySlide stores its settings at `%LOCALAPPDATA%\BodySlide and Outfit Studio\Config.xml`
/// inside the Wine prefix. Without a correct `GameDataPath`, BodySlide cannot find slider
/// sets or outfit groups — resulting in an empty outfit list even when CBBE is deployed.
///
/// MO2 performs the same operation on Windows before launching BodySlide. We replicate that
/// here to handle both first-run and stale-config cases.
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

    // BodySlide uses %LOCALAPPDATA% (AppData/Local) in recent versions.
    // Fall back to AppData/Roaming if that's where a previous run stored the config.
    let config_dir_local = user_dir.join("AppData/Local/BodySlide and Outfit Studio");
    let config_dir_roaming = user_dir.join("AppData/Roaming/BodySlide and Outfit Studio");
    let config_dir = if config_dir_roaming.exists() && !config_dir_local.exists() {
        config_dir_roaming
    } else {
        config_dir_local
    };

    let config_path = config_dir.join("Config.xml");

    // Resolve the correct Wine drive letter for the game Data directory by inspecting
    // <prefix>/dosdevices/. Heroic/Proton may map game library paths to X:, S:, etc.
    // rather than Z:, so we must not hardcode Z:.
    let game_data_dir = game.path.join(&game.data_subdir);
    let data_path = game::linux_path_to_wine_path(&game_data_dir, &wine_config.prefix)
        .unwrap_or_else(|| {
            // Fallback: Z: always maps to / in Wine.
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

/// Create (or update) `<prefix>/dosdevices/m:` → `named_mods/` so the deployd mod
/// library is accessible as `M:\` inside any Wine/Proton process.
///
/// `M:` is used as the deployd-specific drive letter. If it is already mapped to the
/// correct target, this is a no-op. Errors are logged but do not abort the tool launch.
fn ensure_named_mods_drive(wine_config: &WineConfig) {
    let named_mods = match paths::named_mods_dir() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[deployd] named_mods_dir: {e}");
            return;
        }
    };

    if !named_mods.exists() {
        return; // No mods installed yet — nothing to map.
    }

    let dosdevices = wine_config.prefix.join("dosdevices");
    if !dosdevices.is_dir() {
        return;
    }

    let link = dosdevices.join("m:");
    if link.exists() || link.is_symlink() {
        if link.is_symlink() {
            if let Ok(target) = std::fs::read_link(&link) {
                if target == named_mods {
                    return; // Already correct.
                }
            }
            let _ = std::fs::remove_file(&link);
        }
    }

    #[cfg(unix)]
    if let Err(e) = std::os::unix::fs::symlink(&named_mods, &link) {
        eprintln!("[deployd] failed to create M: drive in dosdevices: {e}");
    } else {
        dlog!("deployd: mapped M: → {}", named_mods.display());
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
///
/// If the file already exists, the `GameDataPath` and `TargetGame` elements are updated
/// while all other settings are preserved. If the file is absent or malformed it is
/// replaced with a minimal valid config.
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
        let after = &xml[end..]; // starts at `</tag>`
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
