use serde::Deserialize;

use crate::models::game::GameEngine;

#[derive(PartialEq, Eq, Clone, Copy)]
pub(super) enum GameStore {
    Gog,
    Steam,
}

pub(super) struct KnownGame {
    /// Which store this entry represents. Used to route detection to the right installed.json.
    pub(super) store: GameStore,
    /// Heroic Launcher internal identifier for the game:
    ///   GOG entries  → GOG numeric app name (e.g. "1711230643")
    ///   Steam entries → Steam numeric app ID (e.g. "489830")
    /// This is also the key used in Heroic's per-game GamesConfig/<heroic_app_name>.json.
    pub(super) heroic_app_name: &'static str,
    pub(super) deployd_id: &'static str,
    pub(super) title: &'static str,
    pub(super) data_subdir: &'static str,
    /// All AppData/Local subfolder variants for Plugins.txt.
    /// GOG editions often use a separate folder (e.g. "Skyrim Special Edition GOG");
    /// Steam editions use the standard folder only.
    /// Empty for non-Bethesda engines (no Plugins.txt management needed).
    pub(super) appdata_folders: &'static [&'static str],
    /// Game-specific Custom.ini filename (e.g. "Fallout4Custom.ini").
    /// Empty for non-Bethesda engines.
    pub(super) custom_ini_name: &'static str,
    /// Windows registry key (under HKLM\SOFTWARE) where modding tools look for the game.
    /// Empty for non-Bethesda engines.
    pub(super) bethesda_reg_key: &'static str,
    /// Nexus Mods game domain name (e.g. "skyrimspecialedition").
    pub(super) nexus_domain: &'static str,
    /// Game engine family, used to gate engine-specific behaviour.
    pub(super) engine: GameEngine,
    /// Path to the save directory **relative to the Wine user directory**
    /// (e.g. `"Saved Games/CD Projekt Red/Cyberpunk 2077"`).
    /// `None` for Bethesda games (saves are not managed by Deployd).
    pub(super) save_game_subpath: Option<&'static str>,
    /// Marks games whose support is still being validated. Shown as
    /// "(Experimental)" in the game-type dropdown to set user expectations.
    pub(super) experimental: bool,
}

pub(super) const KNOWN_GAMES: &[KnownGame] = &[
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
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
        experimental: false,
    },
    // ── Dragon Age: Origins (experimental) ───────────────────────────────────
    // data_subdir is relative to the Wine user dir (not the game root), because
    // deploy_dir() for Eclipse games resolves <wine_user>/<data_subdir>.
    // .dazip mods install to AddIns/<UID>/ and register in Settings/Addins.xml.
    // Loose override files go to packages/core/override/ via eclipse path routing.
    KnownGame {
        store: GameStore::Gog,
        heroic_app_name: "1949616134",
        deployd_id: "dragonage",
        title: "Dragon Age: Origins - Ultimate Edition",
        data_subdir: "Documents/BioWare/Dragon Age",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "dragonage",
        engine: GameEngine::Eclipse,
        save_game_subpath: Some("Documents/BioWare/Dragon Age/Characters"),
        experimental: true,
    },
    KnownGame {
        store: GameStore::Steam,
        heroic_app_name: "47810",
        deployd_id: "dragonage-steam",
        title: "Dragon Age: Origins",
        data_subdir: "Documents/BioWare/Dragon Age",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "dragonage",
        engine: GameEngine::Eclipse,
        save_game_subpath: Some("Documents/BioWare/Dragon Age/Characters"),
        experimental: true,
    },
];

#[derive(Deserialize)]
pub(super) struct InstalledFile {
    pub(super) installed: Vec<InstalledEntry>,
}

#[derive(Deserialize)]
pub(super) struct InstalledEntry {
    #[serde(rename = "appName")]
    pub(super) app_name: String,
    pub(super) install_path: String,
    #[serde(default)]
    pub(super) is_dlc: bool,
}
