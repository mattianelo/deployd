use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::models::game::Game;
use crate::models::mod_entry::InstallTarget;

/// Auto-detect install target for a file based on its relative path.
///
/// Files at the archive root with executable/library extensions go to the game
/// root directory (Root). Everything else goes to the Data subdirectory (Data).
pub fn auto_detect_install_target(rel_path: &str) -> InstallTarget {
    let path = Path::new(rel_path);
    let is_root_level = path.parent().is_none_or(|p| p == Path::new(""));
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    if is_root_level && matches!(ext.as_str(), "exe" | "dll" | "asi") {
        InstallTarget::Root
    } else {
        InstallTarget::Data
    }
}

/// Apply REDEngine-specific path transformations to the extracted file list.
///
/// **Witcher 3 wrapping**: W3 mods must live under `Mods/{name}/` in the game
/// root. Archives are packaged many ways: already in `Mods/ModName/content/`,
/// or just `content/` / `scripts/` at the top level. If no file already starts
/// with a `Mods/` component, every file is prefixed with `Mods/{name}/` so
/// files never land in the game's own `content/` folder.
///
/// **Bare `.archive` routing**: for CP2077 (and any game whose `archive_mod_dir`
/// is set), a `.archive` file with no directory prefix is redirected into the
/// game's archive mod subdirectory (e.g. `archive/pc/mod/foo.archive`).
///
/// **Flat REDmod detection**: if `info.json` is present at the extracted root
/// (i.e. `dest_rel == "info.json"`), the package is a REDmod distributed without
/// its `mods/{name}/` wrapper. Every file is prefixed with `mods/{sanitized}/`
/// so the game's REDmod loader can find it.
///
/// Both transforms are no-ops for Bethesda games.
pub(crate) fn apply_redengine_path_fixups(
    game: &Game,
    mod_name: &str,
    stripped_wrapper: Option<&str>,
    file_list: Vec<(PathBuf, PathBuf)>,
) -> Vec<(PathBuf, PathBuf)> {
    // ── Witcher 3 ────────────────────────────────────────────────────────────
    // W3 mods go in Mods/{name}/ at the game root. Many mod archives ship with
    // content/ or scripts/ directly at the top, which would otherwise collide
    // with the base-game content/ folder. Wrap everything in Mods/{name}/ when
    // the archive doesn't already have a Mods/ top-level component.
    //
    // Exception: tool archives (Script Merger, debug tools, etc.) ship with
    // executables at the archive root and must be deployed to the game root
    // directly — the same heuristic Bethesda uses for SKSE/ENB.
    if is_witcher3(game) {
        let has_mods_root = file_list.iter().any(|(_, dest)| {
            dest.components()
                .next()
                .and_then(|c| c.as_os_str().to_str())
                .map(|s| s.eq_ignore_ascii_case("mods"))
                .unwrap_or(false)
        });
        if has_mods_root {
            // Archive already has the Mods/ModName/… structure — leave it alone.
            return file_list;
        }

        // Tool archives have .exe / .dll at the root level (e.g. Script Merger).
        // Deploy them directly to the game root without adding Mods/ wrapping.
        let is_tool_archive = file_list.iter().any(|(_, dest)| {
            let is_root_level = dest.parent().is_none_or(|p| p.as_os_str().is_empty());
            let ext = dest
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            is_root_level && matches!(ext.as_str(), "exe" | "dll")
        });
        if is_tool_archive {
            return file_list;
        }

        // Choose the Mods/ sub-folder name:
        // • If detect_wrapper stripped a wrapper dir (e.g. `modSkipMovies`), that IS
        //   the mod's canonical folder name — use it to preserve archive structure.
        // • Otherwise fall back to the user-provided mod name (archive shipped files
        //   like content/ directly at the root with no mod-name wrapper).
        let mod_folder = stripped_wrapper
            .map(sanitize_mod_name_preserve_case)
            .unwrap_or_else(|| sanitize_mod_name_preserve_case(mod_name));

        return file_list
            .into_iter()
            .map(|(src, dest)| {
                (
                    src,
                    PathBuf::from(format!("Mods/{mod_folder}/{}", dest.to_string_lossy())),
                )
            })
            .collect();
    }

    // ── CP2077 / generic REDEngine ───────────────────────────────────────────
    let archive_subdir = crate::core::game::archive_mod_dir(game);

    // Detect flat REDmod: info.json at the extracted root (no directory component).
    let is_flat_redmod = file_list
        .iter()
        .any(|(_, dest)| dest == Path::new("info.json"));

    if archive_subdir.is_none() && !is_flat_redmod {
        return file_list;
    }

    // Sanitize mod_name for use as a directory name: lowercase, spaces → underscores,
    // strip characters that are unsafe on common filesystems.
    let sanitized: String = mod_name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let redmod_prefix = format!("mods/{sanitized}");

    file_list
        .into_iter()
        .map(|(src, dest)| {
            let dest_str = dest.to_string_lossy();

            // Flat REDmod: prefix everything with mods/{name}/
            if is_flat_redmod {
                return (src, PathBuf::from(format!("{redmod_prefix}/{dest_str}")));
            }

            // Bare .archive routing (only when there is no directory component).
            if let Some(subdir) = archive_subdir {
                let has_dir = dest
                    .parent()
                    .map(|p| !p.as_os_str().is_empty())
                    .unwrap_or(false);
                let ext = dest
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if !has_dir && ext == "archive" {
                    return (src, PathBuf::from(format!("{subdir}/{dest_str}")));
                }
            }

            (src, dest)
        })
        .collect()
}

/// Returns `true` for all known Witcher 3 game IDs.
fn is_witcher3(game: &Game) -> bool {
    matches!(
        game.id.as_str(),
        "witcher-3" | "witcher3" | "witcher3-goty" | "witcher3-steam"
    )
}

/// Sanitize a mod name for use as a filesystem directory name while
/// preserving original casing (used for Witcher 3 mod folders).
fn sanitize_mod_name_preserve_case(mod_name: &str) -> String {
    mod_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Route file paths for Aurora-engine (Witcher 1) mods.
///
/// Routing rules:
/// 1. Files under `system/`, `launcher/`, or `register/` live at the game
///    root (siblings of `Data/`). They are prefixed with `../` so the deployer
///    resolves them relative to `game.path` instead of `game.data_dir()`.
/// 2. Files under `modules/` or `override/` live inside `Data/` and are passed
///    through unchanged.
/// 3. Bare files (no recognised top-level prefix) are placed inside `Override/`
///    while preserving any relative sub-path
///    (e.g. `sub/foo.nif` → `Override/sub/foo.nif`).
/// 4. Directory sentinels (trailing `/`) not under a known prefix are dropped.
///
/// `Override/` and any required subdirectories are created at deploy time by
/// the directory-creation pass in `deployer.rs`.
pub(crate) fn route_aurora_paths(
    file_list: Vec<(PathBuf, PathBuf)>,
    data_subdir: &str,
    file_targets: &HashMap<String, InstallTarget>,
) -> Vec<(PathBuf, PathBuf)> {
    let data_prefix = format!("{}/", data_subdir.to_lowercase());
    file_list
        .into_iter()
        .filter_map(|(src, dest)| {
            let lower = dest.to_string_lossy().to_lowercase();

            // If the user explicitly set Root for this file (keyed by the
            // data/-stripped original archive path), bypass Override routing.
            let orig_key = {
                let k = dest.to_string_lossy().replace('\\', "/");
                let lk = k.to_lowercase();
                if lk.starts_with(&data_prefix) {
                    k[data_prefix.len()..].to_string()
                } else {
                    k
                }
            };
            if file_targets.get(orig_key.as_str()) == Some(&InstallTarget::Root) {
                // Strip data/ then override/ prefixes — the user's intent is game-root.
                let s = dest.to_string_lossy();
                let stripped = if lower.starts_with(&data_prefix) {
                    &s[data_prefix.len()..]
                } else {
                    &s[..]
                };
                let stripped_lower = stripped.to_lowercase();
                let stripped = if stripped_lower.starts_with("override/") {
                    &stripped["override/".len()..]
                } else {
                    stripped
                };
                let stripped_lower = stripped.to_lowercase();
                // Files not already in a game-root sibling dir (system/, launcher/,
                // register/) are wrapped in system/ — that is the Aurora game-root
                // target for loose mod files.
                let final_dest = if stripped_lower.starts_with("system/")
                    || stripped_lower.starts_with("launcher/")
                    || stripped_lower.starts_with("register/")
                {
                    PathBuf::from("..").join(stripped)
                } else {
                    PathBuf::from("..").join("system").join(stripped)
                };
                return Some((src, final_dest));
            }

            // Strip a leading Data/ prefix so that archives structured as
            // Data/system/foo.key are treated identically to system/foo.key.
            let (lower, dest) = if lower.starts_with(&data_prefix) {
                (
                    lower[data_prefix.len()..].to_string(),
                    PathBuf::from(&dest.to_string_lossy()[data_prefix.len()..]),
                )
            } else {
                (lower, dest)
            };

            // system/, launcher/, and register/ are game-root directories
            // (siblings of Data/). Prefix with ../ so resolve_deploy_path routes
            // them to game.path instead of game.data_dir().
            if lower.starts_with("system/")
                || lower.starts_with("launcher/")
                || lower.starts_with("register/")
            {
                return Some((src, PathBuf::from("..").join(&dest)));
            }

            // modules/ lives inside Data/ — pass through unchanged.
            if lower.starts_with("modules/") {
                return Some((src, dest));
            }

            // Files already under Override/ keep their full path — subfolder
            // structure is intentional and must not be discarded.
            if lower.starts_with("override/") {
                return Some((src, dest));
            }

            // Drop directory sentinels not under a known prefix.
            if dest.to_string_lossy().ends_with('/') {
                return None;
            }

            // Bare files go into Override/ preserving any relative sub-path
            // (e.g. sub/foo.nif → Override/sub/foo.nif).
            Some((src, PathBuf::from("Override").join(&dest)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(data_subdir: &str, pairs: Vec<(&str, &str)>) -> Vec<(PathBuf, PathBuf)> {
        route_aurora_paths(
            pairs
                .into_iter()
                .map(|(s, d)| (PathBuf::from(s), PathBuf::from(d)))
                .collect(),
            data_subdir,
            &HashMap::default(),
        )
    }

    fn dests(pairs: Vec<(PathBuf, PathBuf)>) -> Vec<PathBuf> {
        pairs.into_iter().map(|(_, d)| d).collect()
    }

    #[test]
    fn override_subfolder_structure_is_preserved() {
        assert_eq!(
            dests(route(
                "Data",
                vec![("src", "Override/ModName/textures/foo.dds")]
            )),
            vec![PathBuf::from("Override/ModName/textures/foo.dds")]
        );
    }

    #[test]
    fn nested_override_subdir_is_preserved() {
        assert_eq!(
            dests(route(
                "Data",
                vec![("src", "Override/items/keys/it_key_019.uti")]
            )),
            vec![PathBuf::from("Override/items/keys/it_key_019.uti")]
        );
    }

    #[test]
    fn single_override_subdir_is_preserved() {
        assert_eq!(
            dests(route("Data", vec![("src", "Override/textures/foo.dds")])),
            vec![PathBuf::from("Override/textures/foo.dds")]
        );
    }

    #[test]
    fn bare_override_file_stays_flat() {
        assert_eq!(
            dests(route("Data", vec![("src", "Override/foo.dlg")])),
            vec![PathBuf::from("Override/foo.dlg")]
        );
    }

    #[test]
    fn bare_path_goes_into_override() {
        assert_eq!(
            dests(route("Data", vec![("src", "foo.dlg")])),
            vec![PathBuf::from("Override/foo.dlg")]
        );
    }

    #[test]
    fn bare_path_with_subdir_goes_into_override_preserving_structure() {
        assert_eq!(
            dests(route("Data", vec![("src", "sub/foo.nif")])),
            vec![PathBuf::from("Override/sub/foo.nif")]
        );
    }

    #[test]
    fn system_path_routed_to_game_root() {
        assert_eq!(
            dests(route("Data", vec![("src", "system/foo.key")])),
            vec![PathBuf::from("../system/foo.key")]
        );
    }

    #[test]
    fn system_scripts_path_routed_to_game_root() {
        assert_eq!(
            dests(route("Data", vec![("src", "System/Scripts/foo.ws")])),
            vec![PathBuf::from("../System/Scripts/foo.ws")]
        );
    }

    #[test]
    fn launcher_path_routed_to_game_root() {
        assert_eq!(
            dests(route("Data", vec![("src", "Launcher/witcher.exe")])),
            vec![PathBuf::from("../Launcher/witcher.exe")]
        );
    }

    #[test]
    fn register_path_routed_to_game_root() {
        assert_eq!(
            dests(route("Data", vec![("src", "Register/witcher.reg")])),
            vec![PathBuf::from("../Register/witcher.reg")]
        );
    }

    #[test]
    fn modules_path_stays_in_data() {
        assert_eq!(
            dests(route("Data", vec![("src", "modules/chapter1.mod")])),
            vec![PathBuf::from("modules/chapter1.mod")]
        );
    }

    #[test]
    fn directory_sentinel_under_override_is_kept() {
        let result = route("Data", vec![("src", "Override/ModName/")]);
        assert_eq!(
            result,
            vec![(PathBuf::from("src"), PathBuf::from("Override/ModName/"))],
            "directory sentinels under Override/ should be preserved for deployer"
        );
    }

    #[test]
    fn directory_sentinel_without_prefix_is_dropped() {
        let result = route("Data", vec![("src", "SomeFolder/")]);
        assert!(
            result.is_empty(),
            "bare directory sentinels should be dropped"
        );
    }

    #[test]
    fn data_prefixed_system_path_routed_to_game_root() {
        assert_eq!(
            dests(route("Data", vec![("src", "Data/system/foo.key")])),
            vec![PathBuf::from("../system/foo.key")]
        );
    }

    #[test]
    fn data_prefixed_system_scripts_path_routed_to_game_root() {
        assert_eq!(
            dests(route("Data", vec![("src", "Data/System/Scripts/foo.ws")])),
            vec![PathBuf::from("../System/Scripts/foo.ws")]
        );
    }

    #[test]
    fn data_prefixed_launcher_path_routed_to_game_root() {
        assert_eq!(
            dests(route("Data", vec![("src", "Data/Launcher/witcher.exe")])),
            vec![PathBuf::from("../Launcher/witcher.exe")]
        );
    }

    #[test]
    fn explicit_root_target_wraps_loose_file_in_system() {
        let targets = HashMap::from([("foo.key".to_string(), InstallTarget::Root)]);
        let result = route_aurora_paths(
            vec![(PathBuf::from("src"), PathBuf::from("foo.key"))],
            "Data",
            &targets,
        );
        assert_eq!(result[0].1, PathBuf::from("../system/foo.key"));
    }

    #[test]
    fn explicit_root_target_strips_override_prefix_before_system_wrap() {
        // Archive has Override/foo.key; dialog key is "Override/foo.key".
        let targets = HashMap::from([("Override/foo.key".to_string(), InstallTarget::Root)]);
        let result = route_aurora_paths(
            vec![(PathBuf::from("src"), PathBuf::from("Override/foo.key"))],
            "Data",
            &targets,
        );
        assert_eq!(result[0].1, PathBuf::from("../system/foo.key"));
    }

    #[test]
    fn explicit_root_target_preserves_system_prefix() {
        let targets = HashMap::from([("system/foo.key".to_string(), InstallTarget::Root)]);
        let result = route_aurora_paths(
            vec![(PathBuf::from("src"), PathBuf::from("Data/system/foo.key"))],
            "Data",
            &targets,
        );
        assert_eq!(result[0].1, PathBuf::from("../system/foo.key"));
    }
}

/// Strip a leading data-subdir prefix from the deployment-relative path.
///
/// We deploy into `game/Data/`, so a relative path of `data/textures/foo.dds`
/// would create `game/Data/data/textures/foo.dds`. This strips the redundant
/// prefix regardless of how it got there (archive layout, rules, FOMOD mapping).
pub(super) fn strip_data_subdir_prefix(rel: &Path, data_subdir: &str) -> PathBuf {
    let s = rel.to_string_lossy();
    let prefix = format!("{}/", data_subdir.to_lowercase());
    if s.starts_with(&prefix) {
        PathBuf::from(&s[prefix.len()..])
    } else {
        rel.to_path_buf()
    }
}

/// Strip a leading data-subdir prefix from an original-cased path string.
/// Case-insensitive prefix match, but preserves original casing of the remainder.
pub(crate) fn strip_data_subdir_prefix_str(rel: &str, data_subdir: &str) -> String {
    let prefix_lower = format!("{}/", data_subdir.to_lowercase());
    if rel.to_lowercase().starts_with(&prefix_lower) {
        rel[prefix_lower.len()..].to_string()
    } else {
        rel.to_string()
    }
}

