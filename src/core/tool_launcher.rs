use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};

use crate::core::game::{self, WineConfig};
use crate::dlog;
use crate::models::game::Game;
use crate::models::tool::Tool;

mod appimage;
mod launch_plan;
mod runtime;
mod snap;

pub use runtime::{ToolLaunchHooks, ToolProcessHandle};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolSetupStage {
    CreatingPrefix,
    ConfiguringPrefix,
    CheckingMono,
    DownloadingMono,
    VerifyingMono,
    InstallingMono,
    LaunchingTool,
}

impl ToolSetupStage {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::CreatingPrefix => "Creating the Snap Wine prefix...",
            Self::ConfiguringPrefix => "Connecting game settings and configuring Wine...",
            Self::CheckingMono => "Checking Wine Mono...",
            Self::DownloadingMono => {
                "Downloading Wine Mono 10.4.1; please wait for setup to finish..."
            }
            Self::VerifyingMono => "Verifying the Wine Mono download...",
            Self::InstallingMono => "Installing Wine Mono...",
            Self::LaunchingTool => "Starting the external tool...",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolPrepareError {
    Mono(String),
    Fatal(String),
    Cancelled,
}

impl fmt::Display for ToolPrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Mono(error) | Self::Fatal(error) => formatter.write_str(error),
            Self::Cancelled => formatter.write_str("Tool launch cancelled"),
        }
    }
}

impl std::error::Error for ToolPrepareError {}

pub(crate) async fn prepare_tool_runtime(
    game: &Game,
    wine_config: &WineConfig,
    cancel: Arc<AtomicBool>,
    skip_mono: bool,
    on_progress: Arc<dyn Fn(ToolSetupStage) + Send + Sync>,
) -> std::result::Result<(), ToolPrepareError> {
    match &wine_config.launcher {
        game::WineLauncher::Umu(_) => {
            on_progress(ToolSetupStage::LaunchingTool);
            Ok(())
        }
        game::WineLauncher::SnapWine { .. } => {
            snap::prepare_runtime(game, wine_config, cancel, skip_mono, on_progress).await
        }
    }
}

pub(crate) fn initial_setup_required(wine_config: &WineConfig) -> bool {
    match &wine_config.launcher {
        game::WineLauncher::Umu(_) => false,
        game::WineLauncher::SnapWine { .. } => snap::initial_setup_required(wine_config),
    }
}

/// Suppress Gecko installer popup and the Wine menu builder for all tool launches.
/// mscoree (Mono/.NET) is intentionally not suppressed because Snap prefixes install Wine Mono
/// before the first tool launch.
pub(in crate::core::tool_launcher) const WINE_SILENT_DLL_OVERRIDES: &str =
    "mshtml=d;winemenubuilder.exe=d";

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
    let plan = launch_plan::build(tool, game, wine_config, cache_root, cancel)?;
    runtime::supervise(plan, hooks)
}

/// If the prefix path ends with `pfx` (Proton layout), return its parent as
/// the `STEAM_COMPAT_DATA_PATH` directory.  Otherwise return the path as-is.
pub(in crate::core::tool_launcher) fn strip_pfx_suffix(prefix: &Path) -> PathBuf {
    if prefix.ends_with("pfx") {
        prefix.parent().unwrap_or(prefix).to_path_buf()
    } else {
        prefix.to_path_buf()
    }
}

/// Determine the working directory to use when launching a tool.
///
/// Priority:
/// 1. `tool.working_dir` if explicitly set by the user.
/// 2. The directory that contains the tool executable.
/// 3. The game root as final fallback.
pub(in crate::core::tool_launcher) fn effective_cwd(tool: &Tool, game: &Game) -> PathBuf {
    if !tool.working_dir.is_empty() {
        return PathBuf::from(&tool.working_dir);
    }
    PathBuf::from(&tool.exe_path)
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| game.path.clone())
}

pub(in crate::core::tool_launcher) fn effective_tool_exe_path(tool: &Tool) -> PathBuf {
    let exe_path = PathBuf::from(&tool.exe_path);
    if !is_legacy_bodyslide_exe(&exe_path) {
        return exe_path;
    }

    let x64_path = exe_path.with_file_name("BodySlide x64.exe");
    if x64_path.is_file() {
        runtime::diagnostic_log(&format!(
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

/// Pre-configure BodySlide's Config.xml with the correct `GameDataPath` and `TargetGame`.
pub(in crate::core::tool_launcher) fn ensure_bodyslide_config(
    tool: &Tool,
    game: &Game,
    wine_config: &WineConfig,
) {
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
    use std::process::Command;

    use tempfile::tempdir;

    use super::*;
    use crate::models::game::{Game, GameEngine};
    use anyhow::anyhow;

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
            launch_plan::drive_entry_exists(&link),
            "broken dosdevices symlinks still occupy their drive letter"
        );

        Ok(())
    }

    #[test]
    fn cancellable_output_checks_cancel_before_spawn() -> anyhow::Result<()> {
        let cancel = AtomicBool::new(true);
        let mut cmd = Command::new("deployd-test-command-that-should-not-spawn");

        let Err(err) = runtime::run_output_cancellable(&mut cmd, Some(&cancel)) else {
            return Err(anyhow!("cancelled setup command unexpectedly succeeded"));
        };

        assert!(
            err.to_string().contains("cancelled"),
            "expected cancellation error, got: {err}"
        );

        Ok(())
    }

    // @variants: appimage
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

        let cmd = appimage::build_command_with_folders(
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

    // @variants: appimage
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

        let cmd = appimage::build_command_with_folders(
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

    // @variants: appimage
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

        let cmd = appimage::build_command_with_folders(
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
