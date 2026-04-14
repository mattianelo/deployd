use std::path::{Path, PathBuf};

use crate::models::game::Game;
use super::known_games::{KnownGame, KNOWN_GAMES};

pub(crate) fn find_wine_user_dir(known: &KnownGame, game: &Game) -> Option<PathBuf> {
    let prefix = game
        .wine_prefix
        .clone()
        .or_else(|| detect_wine_prefix(known.heroic_app_name, &game.path))?;
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

    // Fall back to "steamuser" even if it doesn't exist yet.
    Some(users_dir.join("steamuser"))
}

#[derive(Debug, Clone)]
pub struct WineConfig {
    pub prefix: PathBuf,
    pub wine_bin: PathBuf,
    /// `None` when using plain Wine. Used to set `LD_LIBRARY_PATH` etc.
    pub proton_dir: Option<PathBuf>,
}

pub fn detect_wine_config(game: &Game) -> Option<WineConfig> {
    let known = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id)?;

    if let Some(prefix) = game.wine_prefix.clone() {
        let (wine_bin, proton_dir) = if let Some((bin, pdir)) = find_wine_near_prefix(&prefix) {
            (bin, Some(pdir))
        } else {
            (which_wine()?, None)
        };
        return Some(WineConfig { prefix, wine_bin, proton_dir });
    }

    let prefix = detect_wine_prefix(known.heroic_app_name, &game.path)?;
    let (wine_bin, proton_dir) = if let Some((bin, pdir)) = find_wine_near_prefix(&prefix) {
        (bin, Some(pdir))
    } else {
        (which_wine()?, None)
    };
    Some(WineConfig { prefix, wine_bin, proton_dir })
}

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

fn find_wine_near_prefix(prefix: &Path) -> Option<(PathBuf, PathBuf)> {
    // A Steam prefix lives at <steamapps>/compatdata/<appid>/pfx.
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

pub(super) fn detect_wine_prefix(heroic_app_name: &str, game_path: &PathBuf) -> Option<PathBuf> {
    // Steam: <steamapps>/compatdata/<appid>/pfx relative to the game at <steamapps>/common/<Game>/.
    let steam_compat = format!("../../compatdata/{heroic_app_name}/pfx");
    for relative in &[steam_compat.as_str(), "../pfx", "../../pfx", "../compatdata/pfx"] {
        let candidate = game_path.join(relative);
        if candidate.exists() {
            return Some(candidate);
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
