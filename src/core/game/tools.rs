use std::path::{Path, PathBuf};

use crate::models::game::{Game, GameEngine};

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

pub(super) const TOOL_PRESETS: &[ToolPreset] = &[
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
