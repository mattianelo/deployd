use crate::models::game::GameEngine;

pub(super) struct KnownGame {
    pub(super) deployd_id: &'static str,
    pub(super) title: &'static str,
    pub(super) data_subdir: &'static str,
    /// AppData/Local subfolder variants for Plugins.txt. Empty for non-Bethesda engines.
    pub(super) appdata_folders: &'static [&'static str],
    /// Empty for non-Bethesda engines.
    pub(super) custom_ini_name: &'static str,
    /// HKLM\SOFTWARE key where modding tools look for the game. Empty for non-Bethesda engines.
    pub(super) bethesda_reg_key: &'static str,
    pub(super) nexus_domain: &'static str,
    pub(super) engine: GameEngine,
    /// Save directory relative to the Wine user directory. `None` disables save management.
    pub(super) save_game_subpath: Option<&'static str>,
    /// Root-level loader executable for SKSE/F4SE/NVSE/SFSE.
    /// `None` for games without a script extender.
    pub(super) script_extender_loader: Option<&'static str>,
    /// Steam App ID, used to set `SteamAppId`/`SteamGameId` env vars so that
    /// `SteamAPI_Init()` can connect to the running Steam daemon.
    /// `None` for non-Steam or non-Bethesda games.
    pub(super) steam_app_id: Option<u32>,
}

pub(super) const KNOWN_GAMES: &[KnownGame] = &[
    // ── Bethesda games ────────────────────────────────────────────────────────
    KnownGame {
        deployd_id: "skyrim-se",
        title: "Skyrim Special Edition",
        data_subdir: "Data",
        // Includes both the standard and GOG-specific AppData folder names.
        appdata_folders: &["Skyrim Special Edition", "Skyrim Special Edition GOG"],
        custom_ini_name: "SkyrimCustom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\Skyrim Special Edition",
        nexus_domain: "skyrimspecialedition",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/Skyrim Special Edition/Saves"),
        script_extender_loader: Some("skse64_loader.exe"),
        steam_app_id: Some(489830),
    },
    KnownGame {
        deployd_id: "fallout-4",
        title: "Fallout 4",
        data_subdir: "Data",
        appdata_folders: &["Fallout4"],
        custom_ini_name: "Fallout4Custom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\Fallout4",
        nexus_domain: "fallout4",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/Fallout4/Saves"),
        script_extender_loader: Some("f4se_loader.exe"),
        steam_app_id: Some(377160),
    },
    KnownGame {
        deployd_id: "fallout-nv",
        title: "Fallout: New Vegas",
        data_subdir: "Data",
        appdata_folders: &["Fallout New Vegas"],
        custom_ini_name: "FalloutCustom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\FalloutNV",
        nexus_domain: "newvegas",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/FalloutNV/Saves"),
        script_extender_loader: Some("nvse_loader.exe"),
        steam_app_id: Some(22380),
    },
    KnownGame {
        deployd_id: "starfield",
        title: "Starfield",
        data_subdir: "Data",
        appdata_folders: &["Starfield"],
        custom_ini_name: "StarfieldCustom.ini",
        bethesda_reg_key: "SOFTWARE\\Bethesda Softworks\\Starfield",
        nexus_domain: "starfield",
        engine: GameEngine::Bethesda,
        save_game_subpath: Some("Documents/My Games/Starfield/Saves"),
        script_extender_loader: Some("sfse_loader.exe"),
        steam_app_id: Some(1716740),
    },
    // ── REDEngine games ───────────────────────────────────────────────────────
    KnownGame {
        deployd_id: "witcher-3",
        title: "The Witcher 3: Wild Hunt",
        data_subdir: ".",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "witcher3",
        engine: GameEngine::REDEngine,
        save_game_subpath: Some("Documents/The Witcher 3/gamesaves"),
        script_extender_loader: None,
        steam_app_id: None,
    },
    KnownGame {
        deployd_id: "cyberpunk-2077",
        title: "Cyberpunk 2077",
        data_subdir: ".",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "cyberpunk2077",
        engine: GameEngine::REDEngine,
        save_game_subpath: Some("Saved Games/CD Projekt Red/Cyberpunk 2077"),
        script_extender_loader: None,
        steam_app_id: None,
    },
    KnownGame {
        deployd_id: "witcher-2",
        title: "The Witcher 2: Assassins of Kings",
        data_subdir: "CookedPC",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "witcher2",
        engine: GameEngine::REDEngine,
        save_game_subpath: Some("Documents/The Witcher 2/gamesaves"),
        script_extender_loader: None,
        steam_app_id: None,
    },
    // ── The Witcher (Aurora engine) ───────────────────────────────────────────
    KnownGame {
        deployd_id: "witcher-1",
        title: "The Witcher",
        data_subdir: "Data",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "witcher",
        engine: GameEngine::Aurora,
        save_game_subpath: Some("Documents/The Witcher/saves"),
        script_extender_loader: None,
        steam_app_id: None,
    },
    // ── Dragon Age: Origins ───────────────────────────────────────────────────
    // data_subdir is relative to the Wine user dir; deploy_dir() resolves <wine_user>/<data_subdir>.
    KnownGame {
        deployd_id: "dragon-age",
        title: "Dragon Age: Origins",
        data_subdir: "Documents/BioWare/Dragon Age",
        appdata_folders: &[],
        custom_ini_name: "",
        bethesda_reg_key: "",
        nexus_domain: "dragonage",
        engine: GameEngine::Eclipse,
        save_game_subpath: Some("Documents/BioWare/Dragon Age/Characters"),
        script_extender_loader: None,
        steam_app_id: None,
    },
];
