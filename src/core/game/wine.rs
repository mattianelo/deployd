use std::path::{Path, PathBuf};

use crate::models::game::Game;

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
    /// Plain `wine` or `wine64` binary — the tool is invoked directly.
    Wine(PathBuf),
    /// UMU Launcher (`umu-run`) — manages Proton runtimes automatically and
    /// downloads Proton GE on first use if no runtime is already present.
    Umu(PathBuf),
}

#[derive(Debug, Clone)]
pub struct WineConfig {
    pub prefix: PathBuf,
    pub launcher: WineLauncher,
    /// Explicit Proton installation directory when known, `None` otherwise.
    /// For UMU launches the runtime is resolved via the `PROTONPATH` env var
    /// rather than this field.
    pub proton_dir: Option<PathBuf>,
}

/// Resolve the Wine/Proton configuration for a game.
///
/// Priority:
/// 1. UMU Launcher: preferred when `umu-run` is available (bundled in the
///    snap at `$SNAP/usr/bin/umu-run`, in the AppImage at
///    `$APPDIR/usr/bin/umu-run`, or installed system-wide).
/// 2. Plain Wine: falls back to `wine64` / `wine` on `$PATH`.
///
/// Returns `None` if no suitable launcher is found or if no wine prefix is
/// configured for the game.
pub fn detect_wine_config(game: &Game) -> Option<WineConfig> {
    let prefix = game.wine_prefix.clone()?;
    let launcher = resolve_launcher()?;
    Some(WineConfig {
        prefix,
        launcher,
        proton_dir: None,
    })
}

/// Resolve the best available launcher, preferring UMU over plain Wine.
fn resolve_launcher() -> Option<WineLauncher> {
    if let Some(umu) = resolve_umu_binary() {
        return Some(WineLauncher::Umu(umu));
    }
    resolve_wine_binary().map(WineLauncher::Wine)
}

/// Find the `umu-run` binary.
///
/// Checks bundle locations first (snap, then AppImage), then falls back to
/// `$PATH` for system-wide installations.
pub(crate) fn resolve_umu_binary() -> Option<PathBuf> {
    if let Ok(snap) = std::env::var("SNAP") {
        let p = PathBuf::from(&snap).join("usr/bin/umu-run");
        if p.is_file() {
            return Some(p);
        }
    }

    if let Some(appdir) = std::env::var_os("APPDIR").map(PathBuf::from) {
        let p = appdir.join("usr/bin/umu-run");
        if p.is_file() {
            return Some(p);
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

/// Find the `wine64` (or `wine`) binary.
fn resolve_wine_binary() -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for name in ["wine64", "wine"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Returns `true` if a usable Proton or Proton GE runtime is already present
/// in a standard Steam compatibility-tools directory.
///
/// When this returns `false` and UMU is the active launcher, deployd shows a
/// first-run setup dialog before calling `umu-run`, which will download
/// Proton GE automatically on first use.
pub fn proton_runtime_available() -> bool {
    find_proton_runtime().is_some()
}

/// Search the standard Steam compatibility-tools directories for an installed
/// Proton or Proton GE runtime.
///
/// A directory is accepted as a valid Proton installation if it contains
/// either a `proton` launcher script or a `files/bin/wine64` binary.
pub fn find_proton_runtime() -> Option<PathBuf> {
    let home = dirs::home_dir()?;

    let xdg_data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share"));

    let search_dirs = [
        xdg_data.join("Steam/compatibilitytools.d"),
        home.join(".steam/steam/compatibilitytools.d"),
    ];

    for dir in &search_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Accept any directory that looks like a valid Proton installation.
            if path.join("proton").exists() || path.join("files/bin/wine64").exists() {
                return Some(path);
            }
        }
    }

    None
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
