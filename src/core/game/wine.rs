use std::path::{Path, PathBuf};

use crate::models::game::Game;
use crate::utils::paths;

const SNAP_WINE_RUNTIME_PLUG: &str = "deployd:wine-runtime";
const SNAP_WINE_RUNTIME_PROVIDER: &str = "wine-platform-runtime-core22:wine-runtime-c22";
const SNAP_WINE_PLATFORM_PLUG: &str = "deployd:wine-stable";
const SNAP_WINE_PLATFORM_PROVIDER: &str = "wine-platform:wine-base-stable";
const SNAP_WINE_RUNTIME_AUTO_CONNECTED: bool = true;
const SNAP_WINE_PLATFORM_AUTO_CONNECTED: bool = false;

pub(crate) fn find_wine_user_dir(game: &Game) -> Option<PathBuf> {
    let prefix = game.wine_prefix.clone()?;
    let users_dir = prefix.join("drive_c/users");

    for user_dir in &["steamuser", "Public"] {
        let candidate = users_dir.join(user_dir);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    if let Ok(entries) = std::fs::read_dir(&users_dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                return Some(entry.path());
            }
        }
    }

    // Fall back to "steamuser" even if it doesn't exist yet (Wine creates it on first run).
    Some(users_dir.join("steamuser"))
}

/// Which Wine-compatible launcher to use for running Windows tools.
#[derive(Debug, Clone)]
pub enum WineLauncher {
    /// UMU Launcher bundled with the AppImage.
    Umu(PathBuf),
    /// Wine from the snap content interface — binary and library paths come from the mounted
    /// wine-platform and wine-runtime snaps. Sommelier is NOT used as the runner because it
    /// unconditionally overrides WINEPREFIX; we resolve wine directly instead.
    SnapWine {
        wine_bin: PathBuf,
        /// `$SNAP/wine-platform/wine-{release}` directory.
        wine_platform: PathBuf,
        /// `$SNAP/wine-runtime` directory.
        wine_runtime: PathBuf,
    },
}

#[derive(Debug, Clone)]
pub struct WineConfig {
    pub prefix: PathBuf,
    pub launcher: WineLauncher,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSnapWineContent {
    pub wine_runtime: bool,
    pub wine_platform: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapWineStatus {
    NotSnap,
    Ready {
        wine_bin: PathBuf,
        wine_platform: PathBuf,
        wine_runtime: PathBuf,
    },
    Missing(MissingSnapWineContent),
}

/// Resolve the Wine/Proton configuration for a game.
///
/// Priority:
/// 1. Snap content interface (snap only): wine binary from `$SNAP/wine-platform/wine-*/bin/wine`.
///    Sommelier is NOT used as the runner — it unconditionally overrides WINEPREFIX, which
///    conflicts with deployd's per-game prefix management.
/// 2. AppImage UMU: bundled `umu-run`, with Proton GE managed under Deployd's data directory.
///
/// Returns `None` if no suitable launcher is found or if no wine prefix is configured.
pub fn detect_wine_config(game: &Game) -> Option<WineConfig> {
    let prefix = game.wine_prefix.clone()?;

    match snap_wine_status() {
        SnapWineStatus::Ready {
            wine_bin,
            wine_platform,
            wine_runtime,
        } => {
            return Some(WineConfig {
                prefix,
                launcher: WineLauncher::SnapWine {
                    wine_bin,
                    wine_platform,
                    wine_runtime,
                },
            });
        }
        SnapWineStatus::Missing(_) => return None,
        SnapWineStatus::NotSnap => {}
    }

    resolve_umu_binary().map(|umu| WineConfig {
        prefix,
        launcher: WineLauncher::Umu(umu),
    })
}

pub fn is_snap() -> bool {
    std::env::var_os("SNAP").is_some()
}

pub fn snap_wine_status() -> SnapWineStatus {
    let Some(snap) = std::env::var_os("SNAP").map(PathBuf::from) else {
        return SnapWineStatus::NotSnap;
    };

    snap_wine_status_in(&snap)
}

pub fn missing_snap_wine_message(missing: &MissingSnapWineContent) -> String {
    let commands = missing_snap_wine_commands(missing);
    if commands.is_empty() {
        return "Deployd needs the Snap Wine content interface to run external tools.\n\n\
                The Wine runtime content plug normally connects automatically. Wait a moment, \
                then try launching the tool again."
            .to_string();
    }

    format!(
        "Deployd needs the Snap Wine content interface to run external tools.\n\n\
         Run this command on your system, then restart Deployd so snapd can refresh the app's \
         content mounts:\n\n{commands}"
    )
}

pub fn missing_snap_wine_commands(missing: &MissingSnapWineContent) -> String {
    missing_snap_wine_connections(missing)
        .iter()
        .map(|connection| format!("snap connect {} {}", connection.plug, connection.provider))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SnapWineConnection {
    plug: &'static str,
    provider: &'static str,
}

fn missing_snap_wine_connections(missing: &MissingSnapWineContent) -> Vec<SnapWineConnection> {
    let mut connections = Vec::new();
    if missing.wine_runtime && !SNAP_WINE_RUNTIME_AUTO_CONNECTED {
        connections.push(SnapWineConnection {
            plug: SNAP_WINE_RUNTIME_PLUG,
            provider: SNAP_WINE_RUNTIME_PROVIDER,
        });
    }
    if missing.wine_platform && !SNAP_WINE_PLATFORM_AUTO_CONNECTED {
        connections.push(SnapWineConnection {
            plug: SNAP_WINE_PLATFORM_PLUG,
            provider: SNAP_WINE_PLATFORM_PROVIDER,
        });
    }
    connections
}

pub fn proton_runtime_available() -> bool {
    find_deployd_proton_runtime().is_some()
}

pub fn find_deployd_proton_runtime() -> Option<PathBuf> {
    let dir = deployd_compatibilitytools_dir().ok()?;
    find_proton_runtime_in(&dir)
}

pub fn deployd_compatibilitytools_dir() -> anyhow::Result<PathBuf> {
    Ok(paths::deployd_data_dir()?.join("Steam/compatibilitytools.d"))
}

pub fn umu_folders_path() -> anyhow::Result<PathBuf> {
    paths::deployd_data_dir()
}

fn resolve_umu_binary() -> Option<PathBuf> {
    if let Some(appdir) = std::env::var_os("APPDIR").map(PathBuf::from) {
        let candidate = appdir.join("usr/bin/umu-run");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join("umu-run");
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

/// Find the wine binary from the snap content interface mounts.
/// Returns `(wine_bin, wine_platform_dir, wine_runtime_dir)`.
fn find_snap_wine() -> Option<(PathBuf, PathBuf, PathBuf)> {
    match snap_wine_status() {
        SnapWineStatus::Ready {
            wine_bin,
            wine_platform,
            wine_runtime,
        } => Some((wine_bin, wine_platform, wine_runtime)),
        SnapWineStatus::NotSnap | SnapWineStatus::Missing(_) => None,
    }
}

fn find_snap_wine_platform(wine_platform_root: &Path) -> Option<(PathBuf, PathBuf)> {
    for entry in std::fs::read_dir(wine_platform_root).ok()?.flatten() {
        let platform_dir = entry.path();
        let wine_bin = platform_dir.join("bin/wine");
        if wine_bin.is_file() {
            return Some((wine_bin, platform_dir));
        }
    }
    None
}

fn snap_wine_status_in(snap: &Path) -> SnapWineStatus {
    let wine_runtime = snap.join("wine-runtime");
    let wine_platform_root = snap.join("wine-platform");
    let wine_runtime_present = wine_runtime.is_dir();
    let wine_platform = find_snap_wine_platform(&wine_platform_root);

    match (wine_runtime_present, wine_platform) {
        (true, Some((wine_bin, wine_platform))) => SnapWineStatus::Ready {
            wine_bin,
            wine_platform,
            wine_runtime,
        },
        (runtime_present, platform) => SnapWineStatus::Missing(MissingSnapWineContent {
            wine_runtime: !runtime_present,
            wine_platform: platform.is_none(),
        }),
    }
}

/// Returns `true` when running in a snap with Wine provided via the content interface.
/// In this case Wine is provided via the content interface — no Proton GE download needed.
pub fn snap_wine_available() -> bool {
    find_snap_wine().is_some()
}

fn find_proton_runtime_in(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.join("proton").exists()
            || path.join("files/bin-wow64/wine").exists()
            || path.join("files/bin/wine64").exists()
        {
            return Some(path);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn snap_wine_status_reports_missing_content() -> anyhow::Result<()> {
        let snap = tempdir()?;

        assert_eq!(
            snap_wine_status_in(snap.path()),
            SnapWineStatus::Missing(MissingSnapWineContent {
                wine_runtime: true,
                wine_platform: true,
            })
        );

        Ok(())
    }

    #[test]
    fn snap_wine_status_resolves_content_mounted_wine() -> anyhow::Result<()> {
        let snap = tempdir()?;
        std::fs::create_dir_all(snap.path().join("wine-runtime"))?;
        std::fs::create_dir_all(snap.path().join("wine-platform/wine-10-stable/bin"))?;
        std::fs::write(
            snap.path().join("wine-platform/wine-10-stable/bin/wine"),
            b"",
        )?;

        assert_eq!(
            snap_wine_status_in(snap.path()),
            SnapWineStatus::Ready {
                wine_bin: snap.path().join("wine-platform/wine-10-stable/bin/wine"),
                wine_platform: snap.path().join("wine-platform/wine-10-stable"),
                wine_runtime: snap.path().join("wine-runtime"),
            }
        );

        Ok(())
    }

    #[test]
    fn missing_snap_wine_commands_include_provider_slots() {
        let commands = missing_snap_wine_commands(&MissingSnapWineContent {
            wine_runtime: true,
            wine_platform: true,
        });

        assert_eq!(
            commands, "snap connect deployd:wine-stable wine-platform:wine-base-stable",
            "content interfaces provided by another snap require an explicit provider slot"
        );
    }

    #[test]
    fn missing_snap_wine_commands_omit_auto_connected_runtime() {
        let commands = missing_snap_wine_commands(&MissingSnapWineContent {
            wine_runtime: true,
            wine_platform: false,
        });

        assert_eq!(
            commands, "",
            "the runtime content plug auto-connects and should not be shown as a manual setup step"
        );
    }

    #[test]
    fn missing_snap_wine_message_mentions_restart_after_manual_connection() {
        let message = missing_snap_wine_message(&MissingSnapWineContent {
            wine_runtime: false,
            wine_platform: true,
        });

        assert!(
            message.contains("restart Deployd"),
            "manual content connections require a new snap mount namespace"
        );
    }

    #[test]
    fn proton_runtime_search_uses_only_deployd_dir() -> anyhow::Result<()> {
        let deployd_runtime = tempdir()?;
        let shared_runtime = tempdir()?;
        std::fs::create_dir_all(shared_runtime.path().join("GE-Proton/files/bin-wow64"))?;
        std::fs::write(
            shared_runtime.path().join("GE-Proton/files/bin-wow64/wine"),
            b"",
        )?;

        assert!(
            find_proton_runtime_in(deployd_runtime.path()).is_none(),
            "shared Proton runtimes must not be discovered from Deployd's isolated runtime dir"
        );

        std::fs::create_dir_all(deployd_runtime.path().join("GE-Proton/files/bin-wow64"))?;
        std::fs::write(
            deployd_runtime
                .path()
                .join("GE-Proton/files/bin-wow64/wine"),
            b"",
        )?;

        assert_eq!(
            find_proton_runtime_in(deployd_runtime.path()),
            Some(deployd_runtime.path().join("GE-Proton"))
        );

        Ok(())
    }
}

/// Translate a Linux absolute path to its Wine drive-letter form via `<prefix>/dosdevices/`.
/// Longest-prefix match wins. Result always ends with a backslash.
pub fn linux_path_to_wine_path(linux_path: &Path, prefix: &Path) -> Option<String> {
    let dosdevices = prefix.join("dosdevices");
    let entries = std::fs::read_dir(&dosdevices).ok()?;

    let mut best: Option<(String, usize)> = None;

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if name.len() != 2 || !name.ends_with(':') {
            continue;
        }
        let letter = name[..1].to_ascii_uppercase();

        let Ok(link_target) = std::fs::read_link(entry.path()) else {
            continue;
        };
        let abs_target = if link_target.is_absolute() {
            link_target
        } else {
            dosdevices.join(link_target)
        };
        let Ok(canon_target) = std::fs::canonicalize(&abs_target) else {
            continue;
        };

        if let Ok(rel) = linux_path.strip_prefix(&canon_target) {
            let match_len = canon_target.as_os_str().len();
            if best.as_ref().is_none_or(|(_, len)| match_len > *len) {
                let rel_win = rel.to_string_lossy().replace('/', "\\");
                let wine_path = if rel_win.is_empty() {
                    format!("{letter}:\\")
                } else {
                    format!("{letter}:\\{rel_win}\\")
                };
                best = Some((wine_path, match_len));
            }
        }
    }

    best.map(|(path, _)| path)
}
