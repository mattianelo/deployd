use std::path::PathBuf;

use crate::dlog;
use crate::models::game::Game;
use super::known_games::KNOWN_GAMES;
use super::wine::{find_wine_user_dir, detect_wine_prefix, linux_path_to_wine_path};

/// Resolve all possible paths to Plugins.txt for a game.
///
/// Located at `<prefix>/drive_c/users/<user>/AppData/Local/<game>/Plugins.txt`.
/// Returns multiple paths for games where GOG and Steam use different AppData folders.
/// Parent directories are created by the caller.
pub fn plugins_txt_paths(game: &Game) -> Vec<PathBuf> {
    let Some(known) = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id) else {
        return Vec::new();
    };
    let Some(user_dir) = find_wine_user_dir(known, game) else {
        return Vec::new();
    };
    known
        .appdata_folders
        .iter()
        .map(|folder| {
            user_dir
                .join("AppData/Local")
                .join(folder)
                .join("Plugins.txt")
        })
        .collect()
}

/// Resolve all possible paths to the custom INI file for ArchiveInvalidation.
///
/// Located at `<prefix>/drive_c/users/<user>/Documents/My Games/<game>/<Custom>.ini`.
/// Returns multiple paths for games where GOG and Steam use different AppData folders.
/// Parent directories are created by the caller.
pub fn custom_ini_paths(game: &Game) -> Vec<PathBuf> {
    let Some(known) = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id) else {
        return Vec::new();
    };
    let Some(user_dir) = find_wine_user_dir(known, game) else {
        return Vec::new();
    };
    known
        .appdata_folders
        .iter()
        .map(|folder| {
            user_dir
                .join("Documents/My Games")
                .join(folder)
                .join(known.custom_ini_name)
        })
        .collect()
}

/// Ensure the standard Bethesda registry key exists so modding tools (xEdit, etc.)
/// can find the game's install path.
///
/// GOG installers register games under `GOG.com\Games\...` but modding tools look for
/// `Bethesda Softworks\<Game>`. This checks the prefix's `system.reg` for the key
/// and returns a `wine reg add` command to create it if missing.
///
/// Returns `Some((reg_key, wine_path))` if the key needs to be added, `None` if it already exists.
pub fn missing_bethesda_reg_key(game: &Game) -> Option<(String, String)> {
    let known = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id)?;

    let prefix = game
        .wine_prefix
        .clone()
        .or_else(|| detect_wine_prefix(known.heroic_app_name, &game.path))?;
    let system_reg = prefix.join("system.reg");
    let reg_content = std::fs::read_to_string(&system_reg).ok()?;

    // Wine stores keys as [Software\\Bethesda Softworks\\...] (lowercase escaped backslashes).
    let key_needle = known.bethesda_reg_key.replace('\\', "\\\\");
    if reg_content.contains(&key_needle) {
        return None; // Key already exists
    }

    // Resolve the correct Wine drive letter for the game path (may be X:, S:, etc.
    // in Heroic/Proton setups) and fall back to Z: if dosdevices is unreadable.
    let wine_path = linux_path_to_wine_path(&game.path, &prefix)
        .unwrap_or_else(|| format!("Z:{}\\", game.path.to_string_lossy().replace('/', "\\")));

    Some((format!("HKLM\\{}", known.bethesda_reg_key), wine_path))
}

/// Ensure the standard My Games folder has INI files that modding tools expect.
///
/// GOG editions store INIs in a variant folder (e.g. "Skyrim Special Edition GOG")
/// while tools look in the standard folder (e.g. "Skyrim Special Edition").
/// This symlinks any `.ini` files from the GOG folder into the standard folder
/// if they don't already exist there.
pub fn ensure_ini_symlinks(game: &Game) {
    let Some(known) = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id) else {
        return;
    };
    if known.appdata_folders.len() < 2 {
        return; // No GOG variant to symlink from
    }

    let Some(user_dir) = find_wine_user_dir(known, game) else {
        return;
    };
    let my_games = user_dir.join("Documents/My Games");

    let standard_dir = my_games.join(known.appdata_folders[0]);

    // Find the first GOG variant folder that exists and has INI files
    let source_dir = known.appdata_folders[1..]
        .iter()
        .map(|f| my_games.join(f))
        .find(|p| p.exists());

    let Some(source_dir) = source_dir else {
        return;
    };

    // Ensure the standard directory exists
    let _ = std::fs::create_dir_all(&standard_dir);

    // Symlink .ini files that exist in source but not in standard
    let Ok(entries) = std::fs::read_dir(&source_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ini"))
            && let Some(name) = path.file_name()
        {
            let target = standard_dir.join(name);
            if !target.exists() {
                if let Err(e) = std::os::unix::fs::symlink(&path, &target) {
                    eprintln!(
                        "deployd: failed to symlink {} → {}: {e}",
                        target.display(),
                        path.display()
                    );
                } else {
                    dlog!(
                        "deployd: symlinked {} → {}",
                        target.display(),
                        path.display()
                    );
                }
            }
        }
    }
}
