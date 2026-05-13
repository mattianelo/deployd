use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::{ffi::OsStr, fs::OpenOptions, io::Write};

use anyhow::{Context, Result, anyhow};

use crate::core::game::{self, WineConfig};
use crate::dlog;
use crate::models::game::Game;
use crate::models::tool::Tool;
use crate::utils::paths;

/// Suppress Gecko installer popup and the Wine menu builder for all tool launches.
/// mscoree (Mono/.NET) is intentionally NOT suppressed here — Eclipse (DAO) tools such as
/// CharGenMorph Compiler require .NET, and wine-mono is not bundled with the Snap wine-runtime.
/// The user is informed via a blocking dialog before launch so they can accept the install prompt.
const WINE_SILENT_DLL_OVERRIDES: &str = "mshtml=d;winemenubuilder.exe=d";
const TOOL_TERMINATE_GRACE: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone)]
pub struct ToolProcessHandle {
    pub pid: u32,
    process_group_id: Option<i32>,
    cancel: Arc<AtomicBool>,
}

pub struct ToolLaunchHooks {
    pub cancel: Arc<AtomicBool>,
    pub on_spawn: Option<Box<dyn FnOnce(ToolProcessHandle) + Send + 'static>>,
    pub on_exit: Option<Box<dyn FnOnce(Option<String>) + Send + 'static>>,
}

impl ToolProcessHandle {
    pub fn request_stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let handle = self.clone();
        std::thread::spawn(move || {
            handle.terminate_process_tree();
        });
    }

    fn terminate_process_tree(&self) {
        #[cfg(unix)]
        {
            if let Some(pgid) = self.process_group_id {
                signal_process_group(pgid, libc::SIGTERM);
                std::thread::sleep(TOOL_TERMINATE_GRACE);
                if process_group_exists(pgid) {
                    signal_process_group(pgid, libc::SIGKILL);
                }
                return;
            }
        }

        signal_process(self.pid, libc::SIGTERM);
        std::thread::sleep(TOOL_TERMINATE_GRACE);
        if process_exists(self.pid) {
            signal_process(self.pid, libc::SIGKILL);
        }
    }
}

/// Launch a Windows tool via the package-specific runtime.
///
/// `on_exit` is called from a background thread once the process exits.
pub fn launch_tool(
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
    cache_root: &std::path::Path,
    cancel: Option<&AtomicBool>,
    hooks: ToolLaunchHooks,
) -> Result<u32> {
    ensure_not_cancelled(cancel)?;

    let exe_path = PathBuf::from(&tool.exe_path);
    if !exe_path.exists() {
        return Err(anyhow!("Tool executable not found: {}", tool.exe_path));
    }

    game::ensure_ini_symlinks(game);
    ensure_not_cancelled(cancel)?;
    ensure_bodyslide_config(tool, game, wine_config);
    ensure_not_cancelled(cancel)?;
    ensure_named_mods_drive(wine_config, cache_root);
    ensure_not_cancelled(cancel)?;
    ensure_no_x_drive_conflict(wine_config);
    ensure_not_cancelled(cancel)?;

    let cmd = match &wine_config.launcher {
        game::WineLauncher::Umu(bin) => build_umu_command(bin, tool, game, wine_config)?,
        game::WineLauncher::SnapWine {
            wine_bin,
            wine_platform,
            wine_runtime,
        } => {
            let wine_bin = resolve_wine64(wine_bin);
            let ld_lib = snap_ld_library_path(wine_platform, wine_runtime);
            ensure_bethesda_reg_key(game, wine_config, &wine_bin, Some(&ld_lib), cancel)?;
            ensure_not_cancelled(cancel)?;
            ensure_wine_silent_setup(wine_config, &wine_bin, Some(&ld_lib), cancel)?;
            ensure_not_cancelled(cancel)?;
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
    ensure_not_cancelled(cancel)?;
    spawn_tool(cmd, &tool.name, hooks)
}

/// Shared process-spawn logic for tool launches.
///
/// `on_exit` receives `Some(error_string)` if the process exited with a non-zero
/// status or could not be waited on; `None` on clean exit.
fn spawn_tool(mut cmd: Command, tool_name: &str, hooks: ToolLaunchHooks) -> Result<u32> {
    log_tool_command(tool_name, &cmd);
    cmd.stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| anyhow!("Could not start process for \"{tool_name}\".\nError: {e}"))?;

    let pid = child.id();
    diagnostic_log(&format!(
        "deployd-tool-debug: spawned '{tool_name}' pid={pid}"
    ));
    let handle = ToolProcessHandle {
        pid,
        #[cfg(unix)]
        process_group_id: Some(pid as i32),
        #[cfg(not(unix))]
        process_group_id: None,
        cancel: hooks.cancel,
    };
    if let Some(cb) = hooks.on_spawn {
        cb(handle);
    }
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

        match &wait_result {
            Ok(status) => diagnostic_log(&format!(
                "deployd-tool-debug: '{name}' wait status={status}"
            )),
            Err(e) => diagnostic_log(&format!("deployd-tool-debug: '{name}' wait failed: {e}")),
        }
        if let Some(s) = &stderr {
            diagnostic_log(&format!(
                "deployd-tool-debug: '{name}' stderr tail:\n{}",
                tail_for_log(s)
            ));
        }

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

        if let Some(cb) = hooks.on_exit {
            cb(error);
        }
    });

    Ok(pid)
}

fn log_tool_command(tool_name: &str, cmd: &Command) {
    diagnostic_log(&format!(
        "deployd-tool-debug: launching '{tool_name}' program={}",
        cmd.get_program().to_string_lossy()
    ));
    let args = cmd
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    diagnostic_log(&format!("deployd-tool-debug: args={args:?}"));
    diagnostic_log(&format!(
        "deployd-tool-debug: cwd={}",
        cmd.get_current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<inherit>".to_string())
    ));

    for key in [
        "APPDIR",
        "APPIMAGE",
        "GDK_PIXBUF_MODULEDIR",
        "GDK_PIXBUF_MODULE_FILE",
        "GIO_MODULE_DIR",
        "GI_TYPELIB_PATH",
        "GSETTINGS_SCHEMA_DIR",
        "GTK_PATH",
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
        "PROTONPATH",
        "STEAM_COMPAT_DATA_PATH",
        "UMU_FOLDERS_PATH",
        "WINEDLLOVERRIDES",
        "WINEDEBUG",
        "WINEPREFIX",
    ] {
        diagnostic_log(&format!(
            "deployd-tool-debug: env {key}={}",
            command_env_value(cmd, key)
        ));
    }
}

fn command_env_value(cmd: &Command, key: &str) -> String {
    if let Some((_, value)) = cmd.get_envs().find(|(k, _)| *k == OsStr::new(key)) {
        return value
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<removed>".to_string());
    }

    std::env::var(key).unwrap_or_else(|_| "<unset>".to_string())
}

fn diagnostic_log(message: &str) {
    eprintln!("{message}");

    let Ok(data_dir) = paths::deployd_data_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&data_dir);
    let log_path = data_dir.join("tool-launch-debug.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "{message}");
    }
}

fn tail_for_log(text: &str) -> String {
    const MAX_CHARS: usize = 20_000;
    let len = text.chars().count();
    if len <= MAX_CHARS {
        return text.to_string();
    }

    let tail = text
        .chars()
        .skip(len.saturating_sub(MAX_CHARS))
        .collect::<String>();
    format!("<truncated to last {MAX_CHARS} chars>\n{tail}")
}

fn signal_process(pid: u32, signal: libc::c_int) {
    // SAFETY: libc::kill is called with a pid returned by std::process::Child::id
    // and a constant signal. Errors are intentionally ignored because the process
    // may already have exited by the time cancellation reaches this thread.
    unsafe {
        libc::kill(pid as libc::pid_t, signal);
    }
}

fn process_exists(pid: u32) -> bool {
    // SAFETY: signal 0 performs existence/permission probing without delivering a
    // signal. The pid comes from std::process::Child::id.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(unix)]
fn signal_process_group(pgid: i32, signal: libc::c_int) {
    // SAFETY: negative pid targets the process group created for this child via
    // CommandExt::process_group(0). It is Deployd-owned and isolated from unrelated
    // user Wine processes.
    unsafe {
        libc::kill(-pgid, signal);
    }
}

#[cfg(unix)]
fn process_group_exists(pgid: i32) -> bool {
    // SAFETY: signal 0 probes the Deployd-created process group without sending a
    // real signal.
    unsafe { libc::kill(-pgid, 0) == 0 }
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::SeqCst))
}

fn ensure_not_cancelled(cancel: Option<&AtomicBool>) -> Result<()> {
    if is_cancelled(cancel) {
        Err(anyhow!("Tool launch cancelled"))
    } else {
        Ok(())
    }
}

fn run_output_cancellable(cmd: &mut Command, cancel: Option<&AtomicBool>) -> Result<Output> {
    ensure_not_cancelled(cancel)?;
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().context("start Wine setup command")?;
    loop {
        if is_cancelled(cancel) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("Tool launch cancelled"));
        }

        if child.try_wait()?.is_some() {
            return child
                .wait_with_output()
                .context("collect Wine setup command output");
        }

        std::thread::sleep(Duration::from_millis(50));
    }
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
    // The GNOME Snap extension injects LD_PRELOAD with a gnome-platform path that
    // wine's own loader cannot resolve, causing memory corruption (stack smashing).
    cmd.env_remove("LD_PRELOAD");
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

    if let Some(ids_path) = snap_amdgpu_ids_path(wine_runtime) {
        cmd.env("LIBDRM_AMDGPU_IDS", ids_path);
    }

    cmd.arg(effective_tool_exe_path(tool));
    for arg in tool.custom_args.split_whitespace() {
        cmd.arg(arg);
    }
    cmd.current_dir(effective_cwd(tool, game));
    cmd
}

fn build_umu_command(
    umu_bin: &Path,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
) -> Result<Command> {
    let umu_folders = game::umu_folders_path().context("resolve UMU data directory")?;
    Ok(build_umu_command_with_folders(
        umu_bin,
        tool,
        game,
        wine_config,
        &umu_folders,
    ))
}

fn build_umu_command_with_folders(
    umu_bin: &Path,
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
    umu_folders: &Path,
) -> Command {
    let compat_data = strip_pfx_suffix(&wine_config.prefix);

    let mut cmd = Command::new(umu_bin);
    cmd.env_remove("PYTHONPATH")
        .env_remove("PYTHONHOME")
        .env_remove("LD_PRELOAD");
    cmd.env("GAMEID", "0")
        .env("WINEPREFIX", &wine_config.prefix)
        .env("WINEDEBUG", "-all")
        .env("STEAM_COMPAT_DATA_PATH", &compat_data)
        .env("PROTONPATH", "GE-Proton")
        .env("UMU_FOLDERS_PATH", umu_folders)
        .env("WINEDLLOVERRIDES", WINE_SILENT_DLL_OVERRIDES);

    cmd.arg(effective_tool_exe_path(tool));
    for arg in tool.custom_args.split_whitespace() {
        cmd.arg(arg);
    }

    cmd.current_dir(effective_cwd(tool, game));
    cmd
}

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

/// Path to `amdgpu.ids` inside the wine-runtime content mount, or `None` if absent.
///
/// Mesa's libdrm looks for this file at the hardcoded host path `/usr/share/libdrm/amdgpu.ids`,
/// which does not exist inside the Snap sandbox. Setting `LIBDRM_AMDGPU_IDS` redirects it.
/// Returns `None` on non-AMD systems so no env var is set unnecessarily.
fn snap_amdgpu_ids_path(wine_runtime: &Path) -> Option<String> {
    let path = wine_runtime.join("usr/share/libdrm/amdgpu.ids");
    path.exists().then(|| path.to_string_lossy().into_owned())
}

fn ensure_bethesda_reg_key(
    game: &Game,
    wine_config: &WineConfig,
    launcher_bin: &Path,
    ld_library_path: Option<&str>,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let Some((reg_key, wine_path)) = game::missing_bethesda_reg_key(game) else {
        return Ok(()); // Key already exists
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
    let result = run_output_cancellable(&mut cmd, cancel);

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
            if is_cancelled(cancel) {
                return Err(e);
            }
            eprintln!("deployd: failed to run wine reg add: {e}");
        }
    }
    Ok(())
}

/// Bake DLL overrides into the wine prefix registry so snap/wine updates don't re-show
/// dialogs even when the env var isn't inherited by wine-internal processes.
///
/// v1 → v2 migration: removes the stale mscoree registry override written by v1 so wine can
/// show the Mono install dialog again (required for .NET tools like CharGenMorph Compiler).
///
/// Runs once per prefix: after the first successful run a sentinel file is written so
/// subsequent tool launches skip this entirely.
fn ensure_wine_silent_setup(
    wine_config: &WineConfig,
    launcher_bin: &Path,
    ld_library_path: Option<&str>,
    cancel: Option<&AtomicBool>,
) -> Result<()> {
    let sentinel_v2 = wine_config.prefix.join(".deployd_wine_setup_v2");
    if sentinel_v2.exists() {
        return Ok(());
    }

    // Migrate from v1: delete the stale mscoree=disabled registry key so wine can offer to
    // install Mono again. Failure is non-fatal — the key may simply not exist yet.
    let sentinel_v1 = wine_config.prefix.join(".deployd_wine_setup_v1");
    if sentinel_v1.exists() {
        let mut cmd = Command::new(launcher_bin);
        cmd.env("WINEPREFIX", &wine_config.prefix)
            .env("WINEDEBUG", "-all")
            .env("WINEDLLOVERRIDES", WINE_SILENT_DLL_OVERRIDES)
            .env_remove("LD_PRELOAD");
        if let Some(ld) = ld_library_path {
            cmd.env("LD_LIBRARY_PATH", ld);
        }
        cmd.args([
            "reg",
            "delete",
            r"HKCU\Software\Wine\DllOverrides",
            "/v",
            "mscoree",
            "/f",
        ]);
        if let Err(e) = run_output_cancellable(&mut cmd, cancel)
            && is_cancelled(cancel)
        {
            return Err(e);
        }
        let _ = std::fs::remove_file(&sentinel_v1);
    }

    // mshtml=disabled suppresses Gecko/HTML renderer installer
    // winemenubuilder.exe=disabled suppresses the wine menu builder dialog
    let overrides = [("mshtml", ""), ("winemenubuilder.exe", "")];
    for (name, value) in overrides {
        let mut cmd = Command::new(launcher_bin);
        cmd.env("WINEPREFIX", &wine_config.prefix)
            .env("WINEDEBUG", "-all")
            .env("WINEDLLOVERRIDES", WINE_SILENT_DLL_OVERRIDES)
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
        let result = run_output_cancellable(&mut cmd, cancel);
        if let Err(e) = result {
            if is_cancelled(cancel) {
                return Err(e);
            }
            eprintln!("deployd: wine silent setup reg add ({name}): {e}");
            return Ok(());
        }
    }

    if let Err(e) = std::fs::write(&sentinel_v2, b"") {
        eprintln!("deployd: failed to write wine setup sentinel: {e}");
    }
    Ok(())
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

fn effective_tool_exe_path(tool: &Tool) -> PathBuf {
    let exe_path = PathBuf::from(&tool.exe_path);
    if !is_legacy_bodyslide_exe(&exe_path) {
        return exe_path;
    }

    let x64_path = exe_path.with_file_name("BodySlide x64.exe");
    if x64_path.is_file() {
        diagnostic_log(&format!(
            "deployd-tool-debug: using BodySlide x64 sibling instead of {}",
            exe_path.display()
        ));
        x64_path
    } else {
        exe_path
    }
}

fn is_legacy_bodyslide_exe(exe_path: &Path) -> bool {
    exe_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("BodySlide.exe"))
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
        .find(|&&c| !drive_entry_exists(&dosdevices.join(format!("{c}:"))))
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

fn drive_entry_exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
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
        let escaped_path = xml_escape(game_data_path);
        let escaped_game = xml_escape(target_game);
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <Config>\n\
             \t<GameDataPath>{escaped_path}</GameDataPath>\n\
             \t<TargetGame>{escaped_game}</TargetGame>\n\
             </Config>\n"
        )
    };

    std::fs::write(config_path, content.as_bytes()).context("write BodySlide Config.xml")?;
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Replace the text content of `<tag>…</tag>` in `xml`.
/// If the tag is absent, inserts a new element before `</Config>`.
fn patch_xml_value(xml: &str, tag: &str, value: &str) -> String {
    let escaped = xml_escape(value);
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let (Some(start), Some(end)) = (xml.find(&open), xml.find(&close)) {
        let before = &xml[..start + open.len()];
        let after = &xml[end..];
        format!("{before}{escaped}{after}")
    } else if let Some(pos) = xml.rfind("</Config>") {
        let (before, after) = xml.split_at(pos);
        format!("{before}\t<{tag}>{escaped}</{tag}>\n{after}")
    } else {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <Config>\n\
             \t<{tag}>{escaped}</{tag}>\n\
             </Config>\n"
        )
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::*;
    use crate::models::game::{Game, GameEngine};

    fn env_value(cmd: &Command, key: &str) -> Option<PathBuf> {
        cmd.get_envs()
            .find_map(|(k, v)| (k == OsStr::new(key)).then(|| v.map(PathBuf::from)))
            .flatten()
    }

    fn env_string(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs().find_map(|(k, v)| {
            (k == OsStr::new(key))
                .then(|| v.map(|value| value.to_string_lossy().into_owned()))
                .flatten()
        })
    }

    fn game_with_prefix(prefix: &Path) -> Game {
        Game {
            id: "skyrim-se".to_string(),
            title: "Skyrim Special Edition".to_string(),
            path: prefix.join("game"),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: Some(prefix.join("compatdata/pfx")),
        }
    }

    fn tool(exe_path: &Path) -> Tool {
        Tool {
            id: "xedit".to_string(),
            game_id: "skyrim-se".to_string(),
            name: "xEdit".to_string(),
            exe_path: exe_path.to_string_lossy().into_owned(),
            icon_name: "application-x-executable-symbolic".to_string(),
            custom_args: "-quickautoclean".to_string(),
            sort_order: 0,
            working_dir: String::new(),
        }
    }

    fn bodyslide_tool(exe_path: &Path) -> Tool {
        Tool {
            id: "bodyslide".to_string(),
            game_id: "skyrim-se".to_string(),
            name: "BodySlide".to_string(),
            exe_path: exe_path.to_string_lossy().into_owned(),
            icon_name: "avatar-default-symbolic".to_string(),
            custom_args: String::new(),
            sort_order: 0,
            working_dir: String::new(),
        }
    }

    #[test]
    fn drive_entry_exists_counts_broken_symlinks() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let link = temp.path().join("q:");
        #[cfg(unix)]
        std::os::unix::fs::symlink(temp.path().join("missing"), &link)?;

        assert!(
            drive_entry_exists(&link),
            "broken dosdevices symlinks still occupy their drive letter"
        );

        Ok(())
    }

    #[test]
    fn cancellable_output_checks_cancel_before_spawn() -> anyhow::Result<()> {
        let cancel = AtomicBool::new(true);
        let mut cmd = Command::new("deployd-test-command-that-should-not-spawn");

        let Err(err) = run_output_cancellable(&mut cmd, Some(&cancel)) else {
            return Err(anyhow!("cancelled setup command unexpectedly succeeded"));
        };

        assert!(
            err.to_string().contains("cancelled"),
            "expected cancellation error, got: {err}"
        );

        Ok(())
    }

    #[test]
    fn umu_command_uses_deployd_runtime_folder() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let game = game_with_prefix(temp.path());
        let tool = tool(&temp.path().join("tools/SSEEdit.exe"));
        let wine_config = WineConfig {
            prefix: game
                .wine_prefix
                .clone()
                .ok_or_else(|| anyhow!("expected wine prefix"))?,
            launcher: game::WineLauncher::Umu(temp.path().join("AppDir/usr/bin/umu-run")),
        };
        let umu_folders = temp.path().join(".local/share/deployd");

        let cmd = build_umu_command_with_folders(
            temp.path().join("AppDir/usr/bin/umu-run").as_path(),
            &tool,
            &game,
            &wine_config,
            &umu_folders,
        );

        assert_eq!(
            cmd.get_program(),
            temp.path().join("AppDir/usr/bin/umu-run")
        );
        assert_eq!(env_string(&cmd, "PROTONPATH").as_deref(), Some("GE-Proton"));
        assert_eq!(env_value(&cmd, "UMU_FOLDERS_PATH"), Some(umu_folders));
        assert_eq!(env_value(&cmd, "WINEPREFIX"), game.wine_prefix);
        assert_eq!(
            env_value(&cmd, "STEAM_COMPAT_DATA_PATH"),
            Some(temp.path().join("compatdata"))
        );

        Ok(())
    }

    #[test]
    fn umu_command_prefers_bodyslide_x64_sibling() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let game = game_with_prefix(temp.path());
        let tool_dir = temp.path().join("tools/BodySlide");
        std::fs::create_dir_all(&tool_dir)?;
        std::fs::write(tool_dir.join("BodySlide.exe"), b"")?;
        std::fs::write(tool_dir.join("BodySlide x64.exe"), b"")?;
        let tool = bodyslide_tool(&tool_dir.join("BodySlide.exe"));
        let wine_config = WineConfig {
            prefix: game
                .wine_prefix
                .clone()
                .ok_or_else(|| anyhow!("expected wine prefix"))?,
            launcher: game::WineLauncher::Umu(temp.path().join("AppDir/usr/bin/umu-run")),
        };

        let cmd = build_umu_command_with_folders(
            temp.path().join("AppDir/usr/bin/umu-run").as_path(),
            &tool,
            &game,
            &wine_config,
            &temp.path().join(".local/share/deployd"),
        );

        let x64_path = tool_dir.join("BodySlide x64.exe");
        assert!(
            cmd.get_args().any(|arg| arg == x64_path.as_os_str()),
            "saved BodySlide.exe tools should launch the x64 sibling when present"
        );

        Ok(())
    }

    #[test]
    fn umu_command_does_not_reference_shared_steam_runtime() -> anyhow::Result<()> {
        let temp = tempdir()?;
        let game = game_with_prefix(temp.path());
        let tool = tool(&temp.path().join("tools/SSEEdit.exe"));
        let wine_config = WineConfig {
            prefix: game
                .wine_prefix
                .clone()
                .ok_or_else(|| anyhow!("expected wine prefix"))?,
            launcher: game::WineLauncher::Umu(temp.path().join("AppDir/usr/bin/umu-run")),
        };
        let umu_folders = temp.path().join(".local/share/deployd");

        let cmd = build_umu_command_with_folders(
            temp.path().join("AppDir/usr/bin/umu-run").as_path(),
            &tool,
            &game,
            &wine_config,
            &umu_folders,
        );

        let env_dump = cmd
            .get_envs()
            .filter_map(|(_, v)| v)
            .map(|v| v.to_string_lossy())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            !env_dump.contains(".steam/steam/compatibilitytools.d"),
            "UMU launch must not point at shared Steam Proton runtimes"
        );

        Ok(())
    }
}
