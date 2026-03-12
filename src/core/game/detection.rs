use std::path::PathBuf;

use crate::dlog;
use crate::models::game::Game;
use super::known_games::{GameStore, KnownGame, InstalledFile, KNOWN_GAMES};

/// Paths where Heroic Launcher stores GOG installed-games metadata.
fn heroic_gog_installed_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(home) = dirs::home_dir() {
        // Flatpak installation
        paths.push(
            home.join(
                ".var/app/com.heroicgameslauncher.hgl/config/heroic/gog_store/installed.json",
            ),
        );
        paths.push(home.join(".config/heroic/gog_store/installed.json"));
    }

    paths
}

/// Paths where Heroic Launcher stores Steam installed-games metadata.
fn heroic_steam_installed_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(home) = dirs::home_dir() {
        // Flatpak installation
        paths.push(
            home.join(
                ".var/app/com.heroicgameslauncher.hgl/config/heroic/steam_store/installed.json",
            ),
        );
        // Native installation — absolute path for Flatpak sandbox compatibility.
        paths.push(home.join(".config/heroic/steam_store/installed.json"));
    }

    paths
}

/// Candidate paths for Heroic per-game config files, paired with whether it's a Flatpak install.
pub(super) fn heroic_game_config_paths(app_name: &str) -> Vec<(PathBuf, bool)> {
    let filename = format!("{app_name}.json");
    let mut paths = Vec::new();

    if let Some(home) = dirs::home_dir() {
        // Flatpak Heroic
        paths.push((
            home.join(".var/app/com.heroicgameslauncher.hgl/config/heroic/GamesConfig")
                .join(&filename),
            true,
        ));
        // Native Heroic — absolute path so this is found even when Deployd runs as
        // a Flatpak (where XDG_CONFIG_HOME points to the app-private sandbox dir,
        // not to ~/.config).
        paths.push((
            home.join(".config/heroic/GamesConfig").join(&filename),
            false,
        ));
    }

    paths
}

/// Detect installed games from a single Heroic store's `installed.json`.
fn detect_from_store(store: GameStore, candidates: &[PathBuf]) -> Vec<Game> {
    for path in candidates {
        if let Ok(content) = std::fs::read_to_string(path) {
            match serde_json::from_str::<InstalledFile>(&content) {
                Ok(installed_file) => {
                    let games: Vec<Game> = installed_file
                        .installed
                        .iter()
                        .filter(|entry| !entry.is_dlc)
                        .filter_map(|entry| {
                            let known = KNOWN_GAMES.iter().find(|k| {
                                k.store == store && k.heroic_app_name == entry.app_name
                            })?;

                            let install_path = PathBuf::from(&entry.install_path);
                            // Accept any absolute path — the game may live outside the
                            // current Flatpak sandbox.  Accessibility is verified lazily
                            // via the file-chooser portal before the first deploy.
                            if !install_path.is_absolute() {
                                dlog!(
                                    "deployd: {} has non-absolute install path, skipping: {}",
                                    known.title,
                                    entry.install_path
                                );
                                return None;
                            }

                            Some(Game {
                                id: known.deployd_id.to_string(),
                                title: known.title.to_string(),
                                path: install_path,
                                data_subdir: known.data_subdir.to_string(),
                                engine: known.engine.clone(),
                                wine_prefix: None,
                            })
                        })
                        .collect();

                    if !games.is_empty() {
                        return games;
                    }
                }
                Err(e) => {
                    eprintln!("deployd: failed to parse {}: {e}", path.display());
                }
            }
        }
    }

    Vec::new()
}

/// Candidate paths for Steam's `steamapps/libraryfolders.vdf`.
///
/// Covers both the Steam Flatpak sandbox
/// (`~/.var/app/com.valvesoftware.Steam/.local/share/Steam`) and a native
/// Steam install (`~/.local/share/Steam`).
fn steam_library_vdf_paths() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    vec![
        // Steam Flatpak — this is where the user's Steam Flatpak stores its data
        home.join(
            ".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/libraryfolders.vdf",
        ),
        // Native Steam — ~/.steam/steam is the canonical symlink created by the Steam
        // installer on all major distros (Debian/Ubuntu point it to debian-installation/,
        // Arch/Fedora to ~/.local/share/Steam, etc.)
        home.join(".steam/steam/steamapps/libraryfolders.vdf"),
        // Native Steam — XDG fallback for distros / manual installs that write directly here
        home.join(".local/share/Steam/steamapps/libraryfolders.vdf"),
    ]
}

/// Extract the value from a single VDF line of the form `"key"\t"value"`.
/// Returns `None` if the line does not match the expected key.
fn vdf_value(line: &str, key: &str) -> Option<String> {
    let line = line.trim();
    let rest = line.strip_prefix('"')?;
    let key_end = rest.find('"')?;
    if &rest[..key_end] != key {
        return None;
    }
    let after = rest[key_end + 1..].trim();
    let inner = after.strip_prefix('"')?;
    let val_end = inner.find('"')?;
    Some(inner[..val_end].to_owned())
}

/// Extract all Steam library root paths from a `libraryfolders.vdf` file.
fn parse_steam_library_paths(content: &str) -> Vec<PathBuf> {
    content
        .lines()
        .filter_map(|line| {
            let val = vdf_value(line, "path")?;
            let p = PathBuf::from(val);
            if p.is_absolute() { Some(p) } else { None }
        })
        .collect()
}

/// Detect Steam games by reading Steam's own library metadata directly, without
/// going through Heroic.  Covers:
///   • Steam Flatpak  (`~/.var/app/com.valvesoftware.Steam/.local/share/Steam`)
///   • Native Steam   (`~/.local/share/Steam`)
///
/// Games already present in `already_found` (matched by `deployd_id`) are
/// skipped to avoid duplicates with Heroic-detected entries.
fn detect_from_steam_direct(already_found: &[Game]) -> Vec<Game> {
    let steam_known: Vec<&KnownGame> = KNOWN_GAMES
        .iter()
        .filter(|k| k.store == GameStore::Steam)
        .collect();

    let mut games: Vec<Game> = Vec::new();

    for vdf_path in steam_library_vdf_paths() {
        let Ok(content) = std::fs::read_to_string(&vdf_path) else {
            continue;
        };

        // Always start with the steamapps directory that contains the VDF itself.
        // This is necessary for Steam Flatpak: the paths written inside the VDF
        // use the container-internal view (e.g. `~/.local/share/Steam`) which
        // does not exist on the host filesystem.  The VDF parent dir is always
        // accessible because we just read the file from there.
        let mut steamapps_dirs: Vec<PathBuf> = Vec::new();
        if let Some(vdf_steamapps) = vdf_path.parent()
            && vdf_steamapps.is_dir()
        {
            steamapps_dirs.push(vdf_steamapps.to_path_buf());
        }
        // Also check any external library paths listed in the VDF (e.g. a
        // secondary SSD).  These use host-absolute paths and are accessible.
        for lib_root in parse_steam_library_paths(&content) {
            let dir = lib_root.join("steamapps");
            if !steamapps_dirs.contains(&dir) {
                steamapps_dirs.push(dir);
            }
        }

        for steamapps in steamapps_dirs {
            if !steamapps.is_dir() {
                continue;
            }

            for known in &steam_known {
                // Skip if already found (Heroic or an earlier Steam library path).
                if already_found
                    .iter()
                    .chain(games.iter())
                    .any(|g| g.id == known.deployd_id)
                {
                    continue;
                }

                let acf_path = steamapps.join(format!("appmanifest_{}.acf", known.heroic_app_name));
                let Ok(acf) = std::fs::read_to_string(&acf_path) else {
                    continue;
                };

                let Some(installdir) = acf.lines().find_map(|l| vdf_value(l, "installdir")) else {
                    continue;
                };

                let game_path = steamapps.join("common").join(&installdir);
                if !game_path.is_dir() {
                    dlog!(
                        "deployd: Steam {} manifest found but path missing: {}",
                        known.title,
                        game_path.display()
                    );
                    continue;
                }

                dlog!(
                    "deployd: detected {} via Steam library at {}",
                    known.title,
                    game_path.display()
                );
                games.push(Game {
                    id: known.deployd_id.to_string(),
                    title: known.title.to_string(),
                    path: game_path,
                    data_subdir: known.data_subdir.to_string(),
                    engine: known.engine.clone(),
                    wine_prefix: None,
                });
            }
        }
    }

    games
}

/// Detect games installed via Heroic Launcher (GOG and Steam) or directly
/// through Steam (native or Flatpak).
///
/// Heroic is tried first; direct Steam detection is used as a fallback for
/// games Heroic does not manage.  Returns only games whose paths exist on disk.
pub fn detect_games() -> Vec<Game> {
    let mut games = detect_from_store(GameStore::Gog, &heroic_gog_installed_paths());
    games.extend(detect_from_store(
        GameStore::Steam,
        &heroic_steam_installed_paths(),
    ));
    // Fallback: scan Steam libraries directly (covers Steam Flatpak and native
    // Steam installs that are not managed through Heroic).
    let steam_direct = detect_from_steam_direct(&games);
    games.extend(steam_direct);
    games
}
