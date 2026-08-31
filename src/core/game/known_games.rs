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
        save_game_subpath: Some("Documents/Witcher 2/gamesaves"),
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
    },
];

#[cfg(test)]
mod tests {
    use super::KNOWN_GAMES;

    // Regression: user-reported Witcher 2 save backup path.
    // @variants: both
    #[test]
    fn witcher_games_use_their_distinct_save_directories() {
        let witcher_2_path = KNOWN_GAMES
            .iter()
            .find(|game| game.deployd_id == "witcher-2")
            .and_then(|game| game.save_game_subpath);
        let witcher_1_path = KNOWN_GAMES
            .iter()
            .find(|game| game.deployd_id == "witcher-1")
            .and_then(|game| game.save_game_subpath);

        assert_eq!(witcher_2_path, Some("Documents/Witcher 2/gamesaves"));
        assert_eq!(witcher_1_path, Some("Documents/The Witcher/saves"));
    }

    // @variants: both
    #[test]
    fn engine_save_anchors_do_not_overlap() {
        let path = |game_id| {
            KNOWN_GAMES
                .iter()
                .find(|game| game.deployd_id == game_id)
                .and_then(|game| game.save_game_subpath)
        };

        assert_eq!(
            path("skyrim-se"),
            Some("Documents/My Games/Skyrim Special Edition/Saves")
        );
        assert_eq!(
            path("cyberpunk-2077"),
            Some("Saved Games/CD Projekt Red/Cyberpunk 2077")
        );
        assert_eq!(path("witcher-1"), Some("Documents/The Witcher/saves"));
        assert_eq!(
            path("dragon-age"),
            Some("Documents/BioWare/Dragon Age/Characters")
        );
    }
}
