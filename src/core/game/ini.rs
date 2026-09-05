use std::path::PathBuf;

use super::known_games::KNOWN_GAMES;
use super::wine::{find_wine_user_dir, linux_path_to_wine_path};
use crate::dlog;
use crate::models::game::Game;

pub(crate) fn repair_registered_game_path(
    game: &Game,
    tool_prefix: &std::path::Path,
    changes: &[crate::utils::location::FolderChange],
) -> anyhow::Result<()> {
    if game.engine != crate::models::game::GameEngine::Bethesda {
        return Ok(());
    }
    let Some(known) = KNOWN_GAMES
        .iter()
        .find(|known| known.deployd_id == game.id && !known.bethesda_reg_key.is_empty())
    else {
        return Ok(());
    };
    let registry = tool_prefix.join("system.reg");
    let metadata = match registry.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_file() {
        anyhow::bail!("The Snap tool registry was replaced; it was preserved")
    }
    let original = std::fs::read_to_string(&registry)?;
    let mut updated = original.clone();
    for change in changes
        .iter()
        .filter(|change| change.role == crate::utils::location::FolderRole::Game)
    {
        updated = rebase_registered_path(
            &updated,
            known.bethesda_reg_key,
            &change.old_path,
            &change.new_path,
        );
    }
    if updated == original {
        return Ok(());
    }
    let temporary = tool_prefix.join(format!(".deployd-registry-repair-{}", uuid::Uuid::new_v4()));
    let result = (|| -> anyhow::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(updated.as_bytes())?;
        file.set_permissions(metadata.permissions())?;
        file.sync_all()?;
        if std::fs::read_to_string(&registry)? != original {
            anyhow::bail!("The Snap tool registry changed during repair; it was preserved")
        }
        std::fs::rename(&temporary, &registry)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

fn rebase_registered_path(
    content: &str,
    key: &str,
    old: &std::path::Path,
    new: &std::path::Path,
) -> String {
    let key = key.replace('\\', "\\\\");
    let entry = |path: &std::path::Path| {
        let wine_path = format!("Z:{}\\", path.to_string_lossy().replace('/', "\\"));
        format!(
            "\"Installed Path\"=\"{}\"",
            wine_path.replace('\\', "\\\\").replace('"', "\\\"")
        )
    };
    let old_entry = entry(old);
    let new_entry = entry(new);
    let mut in_key = false;
    content
        .split_inclusive('\n')
        .map(|line| {
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if let Some(section) = trimmed
                .strip_prefix('[')
                .and_then(|line| line.split_once(']').map(|(section, _)| section))
            {
                in_key = section.eq_ignore_ascii_case(&key);
            }
            if in_key && trimmed == old_entry {
                format!("{new_entry}{}", &line[trimmed.len()..])
            } else {
                line.to_string()
            }
        })
        .collect()
}

pub(crate) fn repair_ini_links(
    game: &Game,
    old_prefix: &std::path::Path,
    new_prefix: &std::path::Path,
) -> anyhow::Result<()> {
    if game.engine != crate::models::game::GameEngine::Bethesda {
        return Ok(());
    }
    let Some(known) = KNOWN_GAMES.iter().find(|known| known.deployd_id == game.id) else {
        return Ok(());
    };
    if known.appdata_folders.len() < 2 {
        return Ok(());
    }
    let Some(user) = find_wine_user_dir(game) else {
        return Ok(());
    };
    let standard = user
        .join("Documents/My Games")
        .join(known.appdata_folders[0]);
    if !standard.is_dir() {
        return Ok(());
    }
    crate::core::location_recovery::require_contained(new_prefix, &standard)?;
    for entry in std::fs::read_dir(&standard)? {
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ini"))
        {
            continue;
        }
        let Ok(old_target) = std::fs::read_link(&path) else {
            continue;
        };
        let Some(new_target) = crate::utils::location::rebase(&old_target, old_prefix, new_prefix)?
        else {
            continue;
        };
        let recognized = known.appdata_folders[1..].iter().any(|folder| {
            new_target
                == user
                    .join("Documents/My Games")
                    .join(folder)
                    .join(path.file_name().unwrap_or_default())
        });
        if recognized {
            crate::core::location_recovery::replace_owned_link(&path, &old_target, &new_target)?;
        }
    }
    Ok(())
}

/// Resolve all possible paths to Plugins.txt for a game.
///
/// Located at `<prefix>/drive_c/users/<user>/AppData/Local/<game>/Plugins.txt`.
/// Returns multiple paths for games where GOG and Steam use different AppData folders.
/// Parent directories are created by the caller.
pub fn plugins_txt_paths(game: &Game) -> Vec<PathBuf> {
    let Some(known) = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id) else {
        return Vec::new();
    };
    let Some(user_dir) = find_wine_user_dir(game) else {
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
    let Some(user_dir) = find_wine_user_dir(game) else {
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
pub fn missing_bethesda_reg_key(game: &Game, prefix: &std::path::Path) -> Option<(String, String)> {
    let known = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id)?;

    let system_reg = prefix.join("system.reg");
    let reg_content = std::fs::read_to_string(&system_reg).ok()?;

    // Wine stores keys as [Software\\Bethesda Softworks\\...] (lowercase escaped backslashes).
    let key_needle = known.bethesda_reg_key.replace('\\', "\\\\");
    if reg_content.contains(&key_needle) {
        return None; // Key already exists
    }

    // Resolve the correct Wine drive letter for the game path (may be X:, S:, etc.
    // in Heroic/Proton setups) and fall back to Z: if dosdevices is unreadable.
    let wine_path = linux_path_to_wine_path(&game.path, prefix)
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

    let Some(user_dir) = find_wine_user_dir(game) else {
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

#[cfg(test)]
mod recovery_tests {
    use super::*;
    use std::path::Path;

    // @variants: snap
    #[test]
    fn repairs_only_the_generated_bethesda_registry_value() {
        let original = concat!(
            "[Software\\\\Bethesda Softworks\\\\Skyrim Special Edition] 0\n",
            "\"Installed Path\"=\"Z:\\\\run\\\\user\\\\1000\\\\doc\\\\old\\\\Game\\\\\"\n",
            "[Software\\\\Other] 0\n",
            "\"Installed Path\"=\"Z:\\\\run\\\\user\\\\1000\\\\doc\\\\old\\\\Game\\\\\"\n"
        );
        let updated = rebase_registered_path(
            original,
            r"SOFTWARE\Bethesda Softworks\Skyrim Special Edition",
            Path::new("/run/user/1000/doc/old/Game"),
            Path::new("/run/user/1000/doc/new/Game"),
        );
        assert_eq!(updated.matches(r"doc\\new").count(), 1);
        assert_eq!(updated.matches(r"doc\\old").count(), 1);
        assert_eq!(
            rebase_registered_path(
                &updated,
                r"SOFTWARE\Bethesda Softworks\Skyrim Special Edition",
                Path::new("/run/user/1000/doc/old/Game"),
                Path::new("/run/user/1000/doc/new/Game")
            ),
            updated
        );
    }

    // @variants: snap
    #[test]
    fn preserves_custom_registry_paths_and_other_engines() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let registry = temp.path().join("system.reg");
        let original = "[Software\\\\Bethesda Softworks\\\\Skyrim Special Edition] 0\n\"Installed Path\"=\"G:\\\\\"\n";
        for engine in [
            crate::models::game::GameEngine::Bethesda,
            crate::models::game::GameEngine::Aurora,
            crate::models::game::GameEngine::Eclipse,
            crate::models::game::GameEngine::REDEngine,
        ] {
            std::fs::write(&registry, original)?;
            let game = Game {
                id: "skyrim-se".into(),
                title: "Fixture".into(),
                path: "/new/Game".into(),
                data_subdir: "Data".into(),
                engine,
                wine_prefix: None,
            };
            let change = crate::utils::location::FolderChange {
                game_id: game.id.clone(),
                role: crate::utils::location::FolderRole::Game,
                old_path: "/old/Game".into(),
                new_path: "/new/Game".into(),
            };
            repair_registered_game_path(&game, temp.path(), &[change])?;
            assert_eq!(std::fs::read_to_string(&registry)?, original);
        }
        Ok(())
    }
}
