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
pub(super) fn apply_redengine_path_fixups(
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
        "witcher3" | "witcher3-goty" | "witcher3-steam"
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
pub(super) fn strip_data_subdir_prefix_str(rel: &str, data_subdir: &str) -> String {
    let prefix_lower = format!("{}/", data_subdir.to_lowercase());
    if rel.to_lowercase().starts_with(&prefix_lower) {
        rel[prefix_lower.len()..].to_string()
    } else {
        rel.to_string()
    }
}

/// Route file paths for Eclipse (Dragon Age: Origins) mods.
///
/// If the archive contains any executable, the entire mod is treated as an
/// external tool and every file goes to `~docs~/<mod_name>/`. Otherwise,
/// DAZIP-expanded files keep their `AddIns/<uid>/` prefix and loose files go
/// to `packages/core/override/`.
pub(super) fn route_eclipse_paths(
    file_list: Vec<(PathBuf, PathBuf)>,
    mod_name: &str,
) -> Vec<(PathBuf, PathBuf)> {
    use crate::core::game::eclipse;
    if eclipse::is_tool_mod(&file_list) {
        eclipse::route_tool_paths(file_list, mod_name)
    } else {
        file_list
            .into_iter()
            .map(|(src, dest)| {
                let routed = eclipse::route_path(&dest.to_string_lossy());
                (src, PathBuf::from(routed))
            })
            .collect()
    }
}
