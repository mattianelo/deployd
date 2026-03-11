use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::dlog;
use crate::models::game::{Game, GameEngine};

#[derive(PartialEq, Eq, Clone, Copy)]
enum GameStore {
    Gog,
    Steam,
}

struct KnownGame {
    /// Which store this entry represents. Used to route detection to the right installed.json.
    store: GameStore,
    /// Heroic Launcher internal identifier for the game:
    ///   GOG entries  → GOG numeric app name (e.g. "1711230643")
    ///   Steam entries → Steam numeric app ID (e.g. "489830")
    /// This is also the key used in Heroic's per-game GamesConfig/<heroic_app_name>.json.
    heroic_app_name: &'static str,
    deployd_id: &'static str,
    title: &'static str,
    data_subdir: &'static str,
    /// All AppData/Local subfolder variants for Plugins.txt.
    /// GOG editions often use a separate folder (e.g. "Skyrim Special Edition GOG");
    /// Steam editions use the standard folder only.
    /// Empty for non-Bethesda engines (no Plugins.txt management needed).
    appdata_folders: &'static [&'static str],
    /// Game-specific Custom.ini filename (e.g. "Fallout4Custom.ini").
    /// Empty for non-Bethesda engines.
    custom_ini_name: &'static str,
    /// Windows registry key (under HKLM\SOFTWARE) where modding tools look for the game.
    /// Empty for non-Bethesda engines.
    bethesda_reg_key: &'static str,
    /// Nexus Mods game domain name (e.g. "skyrimspecialedition").
    nexus_domain: &'static str,
    /// Game engine family, used to gate engine-specific behaviour.
    engine: GameEngine,
    /// Path to the save directory **relative to the Wine user directory**
    /// (e.g. `"Saved Games/CD Projekt Red/Cyberpunk 2077"`).
    /// `None` for Bethesda games (saves are not managed by Deployd).
    save_game_subpath: Option<&'static str>,
}

const KNOWN_GAMES: &[KnownGame] = &[
    // ── GOG editions ──────────────────────────────────────────────────────────
    KnownGame {
        store: GameStore::Gog,
        heroic_app_name: "1711230643",
        deployd_id: "skyrimse",
        title: "Skyrim Special Edition",
        data_subdir: "Data",
        appdata_folders: &["Skyrim Special Edition", "Skyrim Special Edition GOG"],
        custom_ini_name: "SkyrimCustom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\Skyrim Special Edition",
        nexus_domain: "skyrimspecialedition",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/Skyrim Special Edition/Saves"),
    },
    KnownGame {
        store: GameStore::Gog,
        heroic_app_name: "1998527297",
        deployd_id: "fallout4",
        title: "Fallout 4",
        data_subdir: "Data",
        appdata_folders: &["Fallout4"],
        custom_ini_name: "Fallout4Custom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\Fallout4",
        nexus_domain: "fallout4",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/Fallout4/Saves"),
    },
    KnownGame {
        store: GameStore::Gog,
        heroic_app_name: "1454587428",
        deployd_id: "falloutnv",
        title: "Fallout: New Vegas",
        data_subdir: "Data",
        appdata_folders: &["Fallout New Vegas"],
        custom_ini_name: "FalloutCustom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\FalloutNV",
        nexus_domain: "newvegas",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/FalloutNV/Saves"),
    },
    // ── Steam editions ────────────────────────────────────────────────────────
    // heroic_app_name holds the Steam App ID, which Heroic also uses as the
    // GamesConfig key. GOG and Steam installs get distinct deployd_ids so they
    // are tracked separately in the database.
    KnownGame {
        store: GameStore::Steam,
        heroic_app_name: "489830",
        deployd_id: "skyrimse-steam",
        title: "Skyrim Special Edition",
        data_subdir: "Data",
        appdata_folders: &["Skyrim Special Edition"],
        custom_ini_name: "SkyrimCustom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\Skyrim Special Edition",
        nexus_domain: "skyrimspecialedition",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/Skyrim Special Edition/Saves"),
    },
    KnownGame {
        store: GameStore::Steam,
        heroic_app_name: "377160",
        deployd_id: "fallout4-steam",
        title: "Fallout 4",
        data_subdir: "Data",
        appdata_folders: &["Fallout4"],
        custom_ini_name: "Fallout4Custom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\Fallout4",
        nexus_domain: "fallout4",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/Fallout4/Saves"),
    },
    KnownGame {
        store: GameStore::Steam,
        heroic_app_name: "22380",
        deployd_id: "falloutnv-steam",
        title: "Fallout: New Vegas",
        data_subdir: "Data",
        appdata_folders: &["Fallout New Vegas"],
        custom_ini_name: "FalloutCustom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\FalloutNV",
        nexus_domain: "newvegas",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/FalloutNV/Saves"),
    },
    KnownGame {
        store: GameStore::Steam,
        heroic_app_name: "1716740",
        deployd_id: "starfield",
        title: "Starfield",
        data_subdir: "Data",
        appdata_folders: &["Starfield"],
        custom_ini_name: "StarfieldCustom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\Starfield",
        nexus_domain: "starfield",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/Starfield/Saves"),
    },
    // ── REDEngine games ───────────────────────────────────────────────────────
    // data_subdir = "." → game.data_dir() resolves to the game root itself.
    // All mod files (archive/pc/mod/, r6/scripts/, Mods/, etc.) are deployed
    // relative to the game installation root, which is correct for REDEngine.
    // appdata_folders / custom_ini_name / bethesda_reg_key are intentionally
    // empty; Bethesda-specific helpers already guard on these being non-empty.
    KnownGame {
        store: GameStore::Gog,
        heroic_app_name: "1207664663",
        deployd_id: "witcher3",
        title: "The Witcher 3: Wild Hunt",
        data_subdir: ".",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "witcher3",
        engine: GameEngine::REDEngine,
        save_game_subpath: Some("Documents/The Witcher 3/gamesaves"),
    },
    KnownGame {
        store: GameStore::Gog,
        heroic_app_name: "1495134320",
        deployd_id: "witcher3-goty",
        title: "The Witcher 3: Wild Hunt - Game of the Year Edition",
        data_subdir: ".",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "witcher3",
        engine: GameEngine::REDEngine,
        save_game_subpath: Some("Documents/The Witcher 3/gamesaves"),
    },
    KnownGame {
        store: GameStore::Steam,
        heroic_app_name: "292030",
        deployd_id: "witcher3-steam",
        title: "The Witcher 3: Wild Hunt",
        data_subdir: ".",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "witcher3",
        engine: GameEngine::REDEngine,
        save_game_subpath: Some("Documents/The Witcher 3/gamesaves"),
    },
    KnownGame {
        store: GameStore::Gog,
        heroic_app_name: "1423049311",
        deployd_id: "cyberpunk2077",
        title: "Cyberpunk 2077",
        data_subdir: ".",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "cyberpunk2077",
        engine: GameEngine::REDEngine,
        save_game_subpath: Some("Saved Games/CD Projekt Red/Cyberpunk 2077"),
    },
    KnownGame {
        store: GameStore::Steam,
        heroic_app_name: "1091500",
        deployd_id: "cyberpunk2077-steam",
        title: "Cyberpunk 2077",
        data_subdir: ".",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "cyberpunk2077",
        engine: GameEngine::REDEngine,
        save_game_subpath: Some("Saved Games/CD Projekt Red/Cyberpunk 2077"),
    },
];

#[derive(Deserialize)]
struct InstalledFile {
    installed: Vec<InstalledEntry>,
}

#[derive(Deserialize)]
struct InstalledEntry {
    #[serde(rename = "appName")]
    app_name: String,
    install_path: String,
    #[serde(default)]
    is_dlc: bool,
}

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
        // Native installation — use absolute path so this works from inside the
        // Deployd Flatpak sandbox where XDG_CONFIG_HOME is redirected to
        // ~/.var/app/io.github.mattianelo.Deployd/config.
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
fn heroic_game_config_paths(app_name: &str) -> Vec<(PathBuf, bool)> {
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

/// Detect the game's save directory inside the Wine prefix.
///
/// Returns `None` for Bethesda games (saves are not managed by Deployd) or when
/// the Wine prefix cannot be located.
pub fn detect_save_dir(game: &Game) -> Option<PathBuf> {
    let known = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id)?;
    let subpath = known.save_game_subpath?;
    let user_dir = find_wine_user_dir(known, game)?;
    Some(user_dir.join(subpath))
}

/// Returns `true` if this game has a save directory configured in `KNOWN_GAMES`.
/// Unlike [`detect_save_dir`], this performs no filesystem I/O and is safe to call
/// in UI helpers.
pub fn has_save_management(game: &Game) -> bool {
    KNOWN_GAMES
        .iter()
        .find(|k| k.deployd_id == game.id)
        .is_some_and(|k| k.save_game_subpath.is_some())
}

/// Find the WINE user directory for a game (e.g. `<prefix>/drive_c/users/steamuser`).
///
/// If `game.wine_prefix` is set, that prefix is used directly instead of auto-detection.
/// Prefers existing user dirs, falls back to "steamuser" if none exist yet.
fn find_wine_user_dir(known: &KnownGame, game: &Game) -> Option<PathBuf> {
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
fn detect_wine_prefix(heroic_app_name: &str, game_path: &PathBuf) -> Option<PathBuf> {
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

/// A well-known modding tool with conventional install path and defaults.
pub struct ToolPreset {
    /// Human-readable display name.
    pub name: &'static str,
    /// Relative paths from game root to the exe (forward-slash separated), checked in order
    /// before falling back to the recursive `known_exe_names` search.
    pub rel_exe_paths: &'static [&'static str],
    /// Alternative executable names to search for when auto-detecting.
    pub known_exe_names: &'static [&'static str],
    /// GTK icon name for the headerbar button.
    pub icon_name: &'static str,
    /// Default command-line arguments.
    pub default_args: &'static str,
    /// The game engine family this tool is applicable to.
    pub engine: GameEngine,
}

const TOOL_PRESETS: &[ToolPreset] = &[
    // ── Bethesda tools ───────────────────────────────────────────────────────
    ToolPreset {
        name: "xEdit",
        rel_exe_paths: &[],
        known_exe_names: &[
            "SSEEdit.exe",
            "FO4Edit.exe",
            "TES5Edit.exe",
            "FNVEdit.exe",
            "FO3Edit.exe",
            "xEdit.exe",
            "SFEdit.exe",
        ],
        icon_name: "document-edit-symbolic",
        default_args: "",
        engine: GameEngine::Bethesda,
    },
    ToolPreset {
        name: "BodySlide",
        rel_exe_paths: &["Data/CalienteTools/BodySlide/BodySlide x64.exe"],
        known_exe_names: &["BodySlide x64.exe", "BodySlide.exe"],
        icon_name: "avatar-default-symbolic",
        default_args: "",
        engine: GameEngine::Bethesda,
    },
    ToolPreset {
        name: "Synthesis",
        rel_exe_paths: &[],
        known_exe_names: &["Synthesis.exe"],
        icon_name: "emblem-synchronizing-symbolic",
        default_args: "",
        engine: GameEngine::Bethesda,
    },
    ToolPreset {
        name: "Pandora",
        rel_exe_paths: &[
            // Standard install: engine subfolder inside Data
            "Data/Pandora_Engine/Pandora Behaviour Engine+.exe",
            // Alternate: engine subfolder at game root
            "Pandora_Engine/Pandora Behaviour Engine+.exe",
            // Minimal install: exe placed directly in the game root
            "Pandora Behaviour Engine+.exe",
        ],
        known_exe_names: &["Pandora Behaviour Engine+.exe"],
        icon_name: "media-playlist-shuffle-symbolic",
        default_args: "",
        engine: GameEngine::Bethesda,
    },
    // ── REDEngine tools ──────────────────────────────────────────────────────
    ToolPreset {
        // Script Merger merges conflicting script mods for The Witcher 3.
        // Users install it as a standalone archive; Deployd can deploy it into
        // the game folder and detect it automatically from there.
        name: "Script Merger",
        rel_exe_paths: &[
            // Common layout when extracted at game root
            "Script Merger/WitcherScriptMerger.exe",
            // Some users place the exe directly at game root
            "WitcherScriptMerger.exe",
        ],
        known_exe_names: &["WitcherScriptMerger.exe"],
        icon_name: "emblem-synchronizing-symbolic",
        default_args: "",
        engine: GameEngine::REDEngine,
    },
];

/// Look up the Nexus Mods domain name for a game (e.g. "skyrimspecialedition").
/// Returns the game-relative subdirectory where bare `.archive` files should be
/// deployed. Only set for games that have a dedicated archive mod directory
/// (currently Cyberpunk 2077 → `archive/pc/mod`). Returns `None` for Bethesda
/// games and The Witcher 3 where the mod structure must be provided by the packager.
pub fn archive_mod_dir(game: &Game) -> Option<&'static str> {
    match game.id.as_str() {
        "cyberpunk2077" | "cyberpunk2077-steam" => Some("archive/pc/mod"),
        _ => None,
    }
}

pub fn nexus_domain(game: &Game) -> Option<&'static str> {
    KNOWN_GAMES
        .iter()
        .find(|k| k.deployd_id == game.id)
        .map(|k| k.nexus_domain)
}

/// Find the deployd game ID that matches a Nexus domain name.
/// Returns the first match (GOG before Steam for backward compatibility).
pub fn game_id_for_nexus_domain(domain: &str) -> Option<&'static str> {
    KNOWN_GAMES
        .iter()
        .find(|k| k.nexus_domain == domain)
        .map(|k| k.deployd_id)
}

/// Return all known Nexus domain names (for scanning per-game download subfolders).
pub fn all_nexus_domains() -> Vec<&'static str> {
    let mut seen = std::collections::HashSet::new();
    KNOWN_GAMES
        .iter()
        .filter_map(|k| {
            if seen.insert(k.nexus_domain) {
                Some(k.nexus_domain)
            } else {
                None
            }
        })
        .collect()
}

/// A lightweight descriptor of a supported game type, used to populate the
/// "Add Custom Game" dropdown in the game setup dialog.
pub struct KnownGameOption {
    pub deployd_id: &'static str,
    pub title: &'static str,
    pub store: &'static str,
    pub data_subdir: &'static str,
    pub engine: &'static GameEngine,
}

/// Return all supported game types for the "Add Custom Game" dropdown.
pub fn known_game_options() -> Vec<KnownGameOption> {
    KNOWN_GAMES
        .iter()
        .map(|k| KnownGameOption {
            deployd_id: k.deployd_id,
            title: k.title,
            store: match k.store {
                GameStore::Gog => "GOG",
                GameStore::Steam => "Steam",
            },
            data_subdir: k.data_subdir,
            engine: &k.engine,
        })
        .collect()
}

/// Return tool presets applicable to the given game engine.
pub fn tool_presets_for(engine: &GameEngine) -> Vec<&'static ToolPreset> {
    TOOL_PRESETS
        .iter()
        .filter(|p| p.engine == *engine)
        .collect()
}

/// Try to auto-detect a tool's executable path.
///
/// Search order:
/// 1. Known relative path from game root (fastest)
/// 2. Recursive search of game directory for known exe names (max depth 5)
/// 3. Search Wine prefix directories (Program Files, AppData) if available
pub fn detect_tool_path(
    preset: &ToolPreset,
    game_path: &Path,
    wine_prefix: Option<&Path>,
) -> Option<PathBuf> {
    // Try each known relative path (fast O(1) lookups before the recursive walk)
    for rel in preset.rel_exe_paths {
        let candidate = game_path.join(rel);
        if candidate.exists() {
            return Some(candidate);
        }
    }

    // Search game directory recursively
    if let Some(found) = search_dir_for_exes(game_path, preset.known_exe_names, 5) {
        return Some(found);
    }

    // Search Wine prefix
    if let Some(prefix) = wine_prefix {
        let drive_c = prefix.join("drive_c");
        let search_roots = [
            drive_c.join("Program Files"),
            drive_c.join("Program Files (x86)"),
        ];
        for root in &search_roots {
            if root.is_dir()
                && let Some(found) = search_dir_for_exes(root, preset.known_exe_names, 3)
            {
                return Some(found);
            }
        }

        // Search AppData/Local for each user
        let users_dir = drive_c.join("users");
        if users_dir.is_dir()
            && let Ok(entries) = std::fs::read_dir(&users_dir)
        {
            for entry in entries.flatten() {
                let local = entry.path().join("AppData/Local");
                if local.is_dir()
                    && let Some(found) = search_dir_for_exes(&local, preset.known_exe_names, 3)
                {
                    return Some(found);
                }
            }
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
pub(crate) fn linux_path_to_wine_path(linux_path: &Path, prefix: &Path) -> Option<String> {
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

/// Walk a directory up to `max_depth` looking for any file matching `exe_names` (case-insensitive).
fn search_dir_for_exes(root: &Path, exe_names: &[&str], max_depth: usize) -> Option<PathBuf> {
    use walkdir::WalkDir;

    for entry in WalkDir::new(root)
        .max_depth(max_depth)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file()
            && let Some(name) = entry.file_name().to_str()
        {
            for exe in exe_names {
                if name.eq_ignore_ascii_case(exe) {
                    return Some(entry.into_path());
                }
            }
        }
    }
    None
}
