use std::path::{Path, PathBuf};

use crate::models::game::Game;
use super::known_games::{KnownGame, KNOWN_GAMES};
use super::detection::heroic_game_config_paths;

/// Find the WINE user directory for a game (e.g. `<prefix>/drive_c/users/steamuser`).
///
/// If `game.wine_prefix` is set, that prefix is used directly instead of auto-detection.
/// Prefers existing user dirs, falls back to "steamuser" if none exist yet.
pub(crate) fn find_wine_user_dir(known: &KnownGame, game: &Game) -> Option<PathBuf> {
    let prefix = game
        .wine_prefix
        .clone()
        .or_else(|| detect_wine_prefix(known.heroic_app_name, &game.path))?;
    let users_dir = prefix.join("drive_c/users");

    // Try known user directory names first (prefer existing ones)
    for user_dir in &["steamuser", "Public"] {
        let candidate = users_dir.join(user_dir);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // Try any existing user dir
    if let Ok(entries) = std::fs::read_dir(&users_dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                return Some(entry.path());
            }
        }
    }

    // Fallback: default to "steamuser" even if dir doesn't exist yet
    Some(users_dir.join("steamuser"))
}

/// Wine/Proton configuration for a game, read from Heroic Launcher config.
#[derive(Debug, Clone)]
pub struct WineConfig {
    pub prefix: PathBuf,
    pub wine_bin: PathBuf,
    /// Root directory of the Proton distribution (e.g. `.../GE-Proton10-29/`).
    /// `None` when using a plain Wine build. Used to set `LD_LIBRARY_PATH` etc.
    pub proton_dir: Option<PathBuf>,
    /// Whether the Wine/Proton binaries come from a Flatpak Heroic installation.
    /// When true, tools must be launched via `flatpak run --command=... com.heroicgameslauncher.hgl`
    /// because the binaries depend on the Flatpak runtime's libraries.
    pub heroic_flatpak: bool,
}

/// Detect the full Wine/Proton configuration for a game.
///
/// Reads config from Heroic (wine binary + prefix) and applies any user-specified
/// prefix override on top. Falls back to probing common relative paths and
/// searching for Proton near the prefix when Heroic config is unavailable.
pub fn detect_wine_config(game: &Game) -> Option<WineConfig> {
    let known = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id)?;

    for (config_path, is_flatpak) in heroic_game_config_paths(known.heroic_app_name) {
        if let Ok(content) = std::fs::read_to_string(&config_path)
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
        {
            let config = json.get(known.heroic_app_name).unwrap_or(&json);

            // User-specified prefix takes priority; fall back to Heroic's prefix (must exist).
            let prefix = game.wine_prefix.clone().or_else(|| {
                config
                    .get("winePrefix")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .filter(|p| p.exists())
            });

            let wine_version = config.get("wineVersion");

            let wine_type = wine_version
                .and_then(|v| v.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let raw_bin = wine_version
                .and_then(|v| v.get("bin"))
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .filter(|p| p.exists());

            let (heroic_bin, heroic_proton_dir) = match raw_bin {
                Some(bin) => {
                    let (resolved, pdir) = resolve_wine_binary(bin, wine_type);
                    (Some(resolved), pdir)
                }
                None => (None, None),
            };

            if let Some(prefix) = prefix {
                // Prefer Heroic's wine binary; fall back to Proton near the prefix, then system wine.
                let (wine_bin, proton_dir) = if let Some(bin) = heroic_bin {
                    (bin, heroic_proton_dir)
                } else if let Some((found_bin, found_pdir)) = find_wine_near_prefix(&prefix) {
                    (found_bin, Some(found_pdir))
                } else {
                    (which_wine()?, None)
                };
                return Some(WineConfig {
                    prefix,
                    wine_bin,
                    proton_dir,
                    heroic_flatpak: is_flatpak,
                });
            }
        }
    }

    // No Heroic config found. Use user-specified prefix with best available wine.
    if let Some(prefix) = game.wine_prefix.clone() {
        let (wine_bin, proton_dir) = if let Some((bin, pdir)) = find_wine_near_prefix(&prefix) {
            (bin, Some(pdir))
        } else {
            (which_wine()?, None)
        };
        return Some(WineConfig {
            prefix,
            wine_bin,
            proton_dir,
            heroic_flatpak: false,
        });
    }

    // Fallback: probe prefix from common locations, use Proton near prefix or system wine.
    let prefix = detect_wine_prefix(known.heroic_app_name, &game.path)?;
    let (wine_bin, proton_dir) = if let Some((bin, pdir)) = find_wine_near_prefix(&prefix) {
        (bin, Some(pdir))
    } else {
        (which_wine()?, None)
    };
    Some(WineConfig {
        prefix,
        wine_bin,
        proton_dir,
        heroic_flatpak: false,
    })
}

/// Resolve the actual Wine binary from a Heroic `wineVersion.bin` path.
///
/// Proton distributions ship a Python wrapper script (`proton`) that requires
/// Steam-specific env vars. The real Wine binary lives at `<proton_root>/files/bin/wine`.
/// When Heroic reports type "proton" (or the binary is named "proton"), resolve to
/// the real Wine binary. Falls back to the original path if the resolved path doesn't exist.
///
/// Returns `(wine_binary, proton_dir)` — `proton_dir` is `Some` when using Proton.
fn resolve_wine_binary(bin: PathBuf, wine_type: &str) -> (PathBuf, Option<PathBuf>) {
    let is_proton = wine_type == "proton" || bin.file_name().is_some_and(|name| name == "proton");

    if is_proton && let Some(proton_root) = bin.parent() {
        let real_wine = proton_root.join("files/bin/wine");
        if real_wine.exists() {
            return (real_wine, Some(proton_root.to_path_buf()));
        }
    }

    (bin, None)
}

/// Search for the best Proton/Wine-GE binary in a directory of distributions.
///
/// Looks for subdirectories containing `files/bin/wine`, preferring GE-Proton
/// builds over vanilla Proton, then anything else.
///
/// Returns `(wine_bin, proton_root)`.
fn best_proton_in_dir(dir: &Path) -> Option<(PathBuf, PathBuf)> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter(|e| e.path().join("files/bin/wine").exists())
        .map(|e| e.path())
        .collect();

    let chosen = entries
        .iter()
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("GE-Proton"))
        })
        .or_else(|| {
            entries.iter().find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().contains("Proton"))
            })
        })
        .or_else(|| entries.first())?;

    Some((chosen.join("files/bin/wine"), chosen.clone()))
}

/// Search for a Wine/Proton binary near a known Wine prefix path.
///
/// First checks whether the prefix lives inside a Steam `compatdata/` tree and
/// scans the adjacent `common/` directory for Proton distributions. Then falls
/// back to the canonical Wine-GE drop-in directories under `~/.local/share/Steam`
/// and `~/.steam/steam`.
///
/// Returns `(wine_bin, proton_dir)`, or `None` — callers should fall back to `which_wine()`.
fn find_wine_near_prefix(prefix: &Path) -> Option<(PathBuf, PathBuf)> {
    // Walk ancestors looking for a "compatdata" component.
    // A Steam prefix lives at <steamapps>/compatdata/<appid>/pfx,
    // so the parent of "compatdata" is the steamapps directory.
    let mut ancestor = prefix.to_path_buf();
    loop {
        if ancestor.file_name().is_some_and(|n| n == "compatdata") {
            let steamapps = ancestor.parent()?;
            if let Some(result) = best_proton_in_dir(&steamapps.join("common")) {
                return Some(result);
            }
            break;
        }
        if !ancestor.pop() {
            break;
        }
    }

    // Wine-GE / custom Proton drop-in directories.
    let home = dirs::home_dir()?;
    for ctd in &[
        home.join(".local/share/Steam/compatibilitytools.d"),
        home.join(".steam/steam/compatibilitytools.d"),
    ] {
        if let Some(result) = best_proton_in_dir(ctd) {
            return Some(result);
        }
    }

    None
}

/// Try to find the `wine` binary on PATH.
fn which_wine() -> Option<PathBuf> {
    let output = std::process::Command::new("which")
        .arg("wine")
        .output()
        .ok()?;
    if output.status.success() {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    None
}

/// Detect the WINE prefix for a game.
pub(super) fn detect_wine_prefix(heroic_app_name: &str, game_path: &PathBuf) -> Option<PathBuf> {
    // Parse Heroic game config
    // Heroic nests config under the appName key: { "<appName>": { "winePrefix": "..." }, "version": "v0" }
    for (config_path, _is_flatpak) in heroic_game_config_paths(heroic_app_name) {
        if let Ok(content) = std::fs::read_to_string(&config_path)
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&content)
        {
            // Try nested structure first (standard Heroic format), then flat as fallback
            let config = json.get(heroic_app_name).unwrap_or(&json);
            if let Some(prefix) = config.get("winePrefix").and_then(|v| v.as_str()) {
                let prefix_path = PathBuf::from(prefix);
                if prefix_path.exists() {
                    return Some(prefix_path);
                }
            }
        }
    }

    // Probe common Proton prefix locations.
    // The Steam layout puts the prefix at <steamapps>/compatdata/<appid>/pfx
    // relative to the game at <steamapps>/common/<Game>/, so try that first.
    let steam_compat = format!("../../compatdata/{heroic_app_name}/pfx");
    for relative in &[steam_compat.as_str(), "../pfx", "../../pfx", "../compatdata/pfx"] {
        let candidate = game_path.join(relative);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    None
}

/// Translate a Linux absolute path to its Wine drive-letter form by reading
/// `<prefix>/dosdevices/`.
///
/// Each entry is a symlink:
///   `c:` → `../drive_c`   (always present, relative)
///   `z:` → `/`             (always present, maps all of `/`)
///   `x:` / `s:` / …        (Heroic/Proton may add extra mappings)
///
/// The **longest-prefix** match wins so a specific library mount (`x:` → `/mnt/ssd`)
/// beats the catch-all `z:`.  Result always ends with a backslash.
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
