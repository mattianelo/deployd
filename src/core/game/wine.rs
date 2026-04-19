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
    // UMU: commented out — pressure-vessel/bwrap blocked on AppImage + Snap strict confinement.
    // Umu(PathBuf),
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
/// 1. Snap content interface (snap only): wine binary from `$SNAP/wine-platform/wine-*/bin/wine`.
///    Sommelier is NOT used as the runner — it unconditionally overrides WINEPREFIX, which
///    conflicts with deployd's per-game prefix management.
/// 2. Proton GE direct wine: `files/bin-wow64/wine` from a user-installed Proton GE runtime.
/// 3. Plain Wine: `wine64` / `wine` on `$PATH`.
///
/// Returns `None` if no suitable launcher is found or if no wine prefix is configured.
pub fn detect_wine_config(game: &Game) -> Option<WineConfig> {
    let prefix = game.wine_prefix.clone()?;
    if let Some((wine_bin, wine_platform, wine_runtime)) = find_snap_wine() {
        return Some(WineConfig {
            prefix,
            launcher: WineLauncher::SnapWine { wine_bin, wine_platform, wine_runtime },
            proton_dir: None,
        });
    }
    let proton_dir = find_proton_runtime();
    let launcher = resolve_launcher(proton_dir.as_deref())?;
    Some(WineConfig { prefix, launcher, proton_dir })
}

/// Find the wine binary from the snap content interface mounts.
/// Returns `(wine_bin, wine_platform_dir, wine_runtime_dir)`.
fn find_snap_wine() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let snap = PathBuf::from(std::env::var_os("SNAP")?);
    let wine_runtime = snap.join("wine-runtime");
    if !wine_runtime.is_dir() {
        return None;
    }
    let wine_platform_root = snap.join("wine-platform");
    for entry in std::fs::read_dir(&wine_platform_root).ok()?.flatten() {
        let platform_dir = entry.path();
        let wine_bin = platform_dir.join("bin/wine");
        if wine_bin.is_file() {
            return Some((wine_bin, platform_dir, wine_runtime));
        }
    }
    None
}

/// Returns `true` when running in a snap with Wine provided via the content interface.
/// In this case Wine is provided via the content interface — no Proton GE download needed.
pub fn snap_wine_available() -> bool {
    find_snap_wine().is_some()
}

/// Find the wine binary inside a Proton installation directory.
///
/// Prefers the WoW64 build (`files/bin-wow64/wine`) which handles both
/// 32-bit and 64-bit Windows executables without a separate `wine64`.
pub(crate) fn resolve_proton_wine_binary(proton_dir: &Path) -> Option<PathBuf> {
    [
        proton_dir.join("files/bin-wow64/wine"),
        proton_dir.join("files/bin/wine64"),
        proton_dir.join("files/bin/wine"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// Resolve the best available launcher.
///
/// Prefers direct Proton GE wine over system wine.
/// UMU is commented out — pressure-vessel/bwrap requires CLONE_NEWUSER which
/// is blocked on AppImage (AppArmor) and Snap (strict confinement).
fn resolve_launcher(proton_dir: Option<&Path>) -> Option<WineLauncher> {
    if let Some(dir) = proton_dir
        && let Some(wine_bin) = resolve_proton_wine_binary(dir)
    {
        return Some(WineLauncher::Wine(wine_bin));
    }
    if let Some(bin) = resolve_wine_binary() {
        return Some(WineLauncher::Wine(bin));
    }
    // UMU: commented out — pressure-vessel/bwrap blocked on AppImage + Snap.
    // if let Some(umu) = resolve_umu_binary() {
    //     return Some(WineLauncher::Umu(umu));
    // }
    None
}

/// Find the `umu-run` binary.
///
/// Checks bundle locations first (snap, then AppImage), then falls back to
/// `$PATH` for system-wide installations.
#[allow(dead_code)] // UMU commented out; kept for future re-enable
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

    let mut search_dirs = vec![
        xdg_data.join("Steam/compatibilitytools.d"),
        home.join(".steam/steam/compatibilitytools.d"),
    ];

    // In a snap, XDG_DATA_HOME points to the revision-specific directory and
    // is wiped on every update. SNAP_USER_COMMON persists across revisions, so
    // UMU is told to download Proton GE there (see build_umu_command).
    if let Some(snap_common) = std::env::var_os("SNAP_USER_COMMON") {
        search_dirs.push(PathBuf::from(snap_common).join("Steam/compatibilitytools.d"));
    }

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
