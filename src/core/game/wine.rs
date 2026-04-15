use std::path::{Path, PathBuf};

use super::known_games::KNOWN_GAMES;
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

#[derive(Debug, Clone)]
pub struct WineConfig {
    pub prefix: PathBuf,
    pub wine_bin: PathBuf,
    /// `None` when using plain Wine. Used to set `LD_LIBRARY_PATH` etc.
    pub proton_dir: Option<PathBuf>,
}

/// Resolve the Wine configuration for a game.
///
/// Requires both a user-configured Wine prefix (`game.wine_prefix`) and a
/// deployd-managed ProtonGE runtime to be active. Neither is auto-detected —
/// the user selects the prefix manually and installs the runtime via the
/// ProtonGE manager. Returns `None` if either is missing.
pub fn detect_wine_config(game: &Game) -> Option<WineConfig> {
    let _known = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id)?;

    let prefix = game.wine_prefix.clone()?;

    let proton_dir = crate::core::proton_manager::active_runtime_path()?;
    let wine_bin = proton_dir.join("files/bin/wine");
    if !wine_bin.exists() {
        eprintln!(
            "deployd: ProtonGE runtime at {} has no wine binary",
            proton_dir.display()
        );
        return None;
    }

    Some(WineConfig {
        prefix,
        wine_bin,
        proton_dir: Some(proton_dir),
    })
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
