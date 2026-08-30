use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::core::game::eclipse::DOCS_PREFIX;
use crate::models::game::Game;
use crate::models::manifest::ModFile;

pub(super) fn remove_deployed_file(
    f: &ModFile,
    game: &Game,
    game_data: &PathBuf,
) -> Result<Vec<String>> {
    let deploy_path = resolve_deploy_path(&f.game_rel_original, &game.path, game_data)
        .with_context(|| format!("Invalid deployed path '{}'", f.game_rel_original))?;
    let docs = docs_base(game_data);
    let stop_at: &PathBuf = if f.game_rel_original.starts_with("../") {
        &game.path
    } else if f.game_rel_original.starts_with(DOCS_PREFIX) {
        &docs
    } else {
        game_data
    };

    let mut warnings = Vec::new();
    if f.game_rel_original.ends_with('/') {
        if let Err(error) = fs::remove_dir(&deploy_path)
            && error.kind() != std::io::ErrorKind::NotFound
            && error.kind() != std::io::ErrorKind::DirectoryNotEmpty
        {
            warnings.push(format!(
                "Could not remove empty deployed directory '{}': {error}",
                deploy_path.display()
            ));
        }
        if let Some(parent) = deploy_path.parent() {
            remove_empty_parents(parent, stop_at, &mut warnings);
        }
    } else {
        if let Err(error) = fs::remove_file(&deploy_path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(error).with_context(|| {
                format!("Failed to remove deployed file '{}'", deploy_path.display())
            });
        }
        if let Some(parent) = deploy_path.parent() {
            remove_empty_parents(parent, stop_at, &mut warnings);
        }
    }
    Ok(warnings)
}

/// Returns `true` if the relative portion of a deploy path contains traversal components.
/// Each branch of `resolve_deploy_path` has its own legitimate anchor (game_root, docs_base,
/// or game_data), so we validate the relative part after prefix-stripping in each branch
/// rather than checking the final absolute path, which would break Eclipse's cross-mount
/// Wine user-dir paths.
fn has_deploy_traversal(rel: &str) -> bool {
    use std::path::Component;
    Path::new(rel)
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::RootDir))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DeployAnchor {
    GameRoot,
    Docs,
    Data,
}

impl DeployAnchor {
    pub(super) fn with_prefix(self, rel: &str) -> String {
        match self {
            Self::GameRoot => format!("../{rel}"),
            Self::Docs => format!("{DOCS_PREFIX}{rel}"),
            Self::Data => rel.to_string(),
        }
    }
}

pub(super) fn split_deploy_target<'a>(
    game_rel: &'a str,
    game_root: &Path,
    game_data: &Path,
) -> anyhow::Result<(PathBuf, &'a str, DeployAnchor)> {
    let (base, rel, anchor) = if let Some(root_rel) = game_rel.strip_prefix("../") {
        (game_root.to_path_buf(), root_rel, DeployAnchor::GameRoot)
    } else if let Some(docs_rel) = game_rel.strip_prefix(DOCS_PREFIX) {
        (docs_base(game_data), docs_rel, DeployAnchor::Docs)
    } else {
        (game_data.to_path_buf(), game_rel, DeployAnchor::Data)
    };
    if has_deploy_traversal(rel) {
        anyhow::bail!("path traversal in deploy path: {game_rel}");
    }
    Ok((base, rel, anchor))
}

pub(super) fn resolve_deploy_path(
    game_rel: &str,
    game_root: &Path,
    game_data: &Path,
) -> anyhow::Result<PathBuf> {
    let (base, rel, _) = split_deploy_target(game_rel, game_root, game_data)?;
    Ok(base.join(rel))
}

fn docs_base(game_data: &Path) -> PathBuf {
    game_data
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| game_data.to_path_buf())
}

/// Build a lowercase→best-cased directory name map from winner paths.
/// Non-all-lowercase form wins to preserve readability for tools browsing the game folder.
pub(super) fn build_dir_canonical_map(winners: &[ModFile]) -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for f in winners {
        let rel = if let Some(r) = f.game_rel_original.strip_prefix("../") {
            r
        } else if let Some(r) = f.game_rel_original.strip_prefix(DOCS_PREFIX) {
            r
        } else {
            &f.game_rel_original
        };
        let path = Path::new(rel.trim_end_matches('/'));
        let dir_path = if f.game_rel_lowercase.ends_with('/') {
            path
        } else {
            path.parent().unwrap_or(Path::new(""))
        };
        for component in dir_path.components() {
            if let std::path::Component::Normal(c) = component {
                let c_str = c.to_string_lossy();
                let c_lower = c_str.to_lowercase();
                map.entry(c_lower)
                    .and_modify(|existing| {
                        if c_str.chars().any(|ch| ch.is_uppercase()) {
                            *existing = c_str.to_string();
                        }
                    })
                    .or_insert_with(|| c_str.to_string());
            }
        }
    }
    map
}

pub(super) fn create_dirs_case_insensitive(
    base: &Path,
    components: &[&str],
    canonical: &HashMap<String, String>,
    dir_cache: &mut HashMap<PathBuf, HashMap<String, PathBuf>>,
) -> Result<PathBuf> {
    let mut current = base.to_path_buf();

    for component in components {
        if component.is_empty() {
            continue;
        }
        let component_lower = component.to_lowercase();

        if !dir_cache.contains_key(&current) {
            let listing = match fs::read_dir(&current) {
                Ok(entries) => {
                    let mut listing = HashMap::new();
                    for entry in entries {
                        let entry = entry.with_context(|| {
                            format!("Failed to inspect an entry in '{}'", current.display())
                        })?;
                        if entry
                            .file_type()
                            .with_context(|| {
                                format!("Failed to inspect '{}'", entry.path().display())
                            })?
                            .is_dir()
                        {
                            listing.insert(
                                entry.file_name().to_string_lossy().to_lowercase(),
                                entry.path(),
                            );
                        }
                    }
                    listing
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Failed to inspect deploy directory '{}'", current.display())
                    });
                }
            };
            dir_cache.insert(current.clone(), listing);
        }

        let existing = dir_cache
            .get(&current)
            .and_then(|m| m.get(&component_lower))
            .cloned();
        current = if let Some(path) = existing {
            path
        } else {
            let name = canonical
                .get(&component_lower)
                .map(|s| s.as_str())
                .unwrap_or(component);
            let new_dir = current.join(name);
            fs::create_dir_all(&new_dir)?;
            dir_cache.remove(new_dir.parent().unwrap_or(&new_dir));
            new_dir
        };
    }

    Ok(current)
}

pub(super) fn ensure_dirs_case_insensitive(
    base: &Path,
    rel_path: &str,
    canonical: &HashMap<String, String>,
    dir_cache: &mut HashMap<PathBuf, HashMap<String, PathBuf>>,
) -> Result<PathBuf> {
    let components: Vec<&str> = rel_path.split('/').collect();
    let dir_components = &components[..components.len().saturating_sub(1)];
    let parent = create_dirs_case_insensitive(base, dir_components, canonical, dir_cache)?;

    if let Some(filename) = components.last() {
        // Scan the parent directory case-insensitively so that deploying
        // "Scripts/Mod.lua" correctly finds and reuses the existing
        // "Scripts/mod.lua" path rather than creating a second file alongside it.
        let fname_lower = filename.to_lowercase();
        let entries = fs::read_dir(&parent).with_context(|| {
            format!("Failed to inspect deploy directory '{}'", parent.display())
        })?;
        for entry in entries {
            let entry = entry
                .with_context(|| format!("Failed to inspect an entry in '{}'", parent.display()))?;
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false)
                && entry.file_name().to_string_lossy().to_lowercase() == fname_lower
            {
                return Ok(entry.path());
            }
        }
        Ok(parent.join(filename))
    } else {
        Ok(parent)
    }
}

fn remove_empty_parents(dir: &Path, stop_at: &PathBuf, warnings: &mut Vec<String>) {
    let mut current = dir.to_path_buf();
    while current != *stop_at {
        if fs::read_dir(&current)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false)
        {
            if let Err(error) = fs::remove_dir(&current)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                warnings.push(format!(
                    "Could not remove empty deploy directory '{}': {error}",
                    current.display()
                ));
                break;
            }
        } else {
            break;
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::Result;
    use tempfile::tempdir;

    use crate::models::game::{Game, GameEngine};
    use crate::models::manifest::ModFile;

    use super::{
        DOCS_PREFIX, DeployAnchor, remove_deployed_file, resolve_deploy_path, split_deploy_target,
    };

    #[test]
    fn resolve_deploy_path_rejects_traversal_in_every_anchor() {
        let game_root = Path::new("/games/Fallout");
        let game_data = Path::new("/games/Fallout/Data");

        for rel in [
            "../../outside.txt",
            "../Data/../outside.txt",
            "textures/../../outside.txt",
            "/absolute.txt",
        ] {
            assert!(resolve_deploy_path(rel, game_root, game_data).is_err());
        }

        assert!(
            resolve_deploy_path(
                &format!("{DOCS_PREFIX}BioWare/../Settings.xml"),
                game_root,
                game_data
            )
            .is_err()
        );
    }

    #[test]
    fn split_deploy_target_preserves_anchor_prefixes() -> anyhow::Result<()> {
        let game_root = Path::new("/games/DAO");
        let game_data = Path::new("/games/DAO/packages/core/override");

        let (base, rel, anchor) =
            split_deploy_target("../bin_ship/tool.exe", game_root, game_data)?;
        assert_eq!(base, game_root);
        assert_eq!(rel, "bin_ship/tool.exe");
        assert_eq!(anchor, DeployAnchor::GameRoot);
        assert_eq!(anchor.with_prefix(rel), "../bin_ship/tool.exe");

        let docs_path = format!("{DOCS_PREFIX}BioWare/Settings.xml");
        let (base, rel, anchor) = split_deploy_target(&docs_path, game_root, game_data)?;
        assert_eq!(base, Path::new("/games/DAO/packages"));
        assert_eq!(rel, "BioWare/Settings.xml");
        assert_eq!(anchor, DeployAnchor::Docs);
        assert_eq!(
            anchor.with_prefix(rel),
            format!("{DOCS_PREFIX}BioWare/Settings.xml")
        );

        Ok(())
    }

    #[test]
    fn reports_failed_deployed_file_removal() -> Result<()> {
        let temp = tempdir()?;
        let game_data = temp.path().join("Data");
        std::fs::create_dir_all(game_data.join("blocked.esp"))?;
        let game = Game {
            id: "game".to_string(),
            title: "Game".to_string(),
            path: temp.path().to_path_buf(),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: None,
        };
        let deployed = ModFile {
            mod_id: "mod".to_string(),
            game_rel_lowercase: "blocked.esp".to_string(),
            game_rel_original: "blocked.esp".to_string(),
            cache_path: String::new(),
        };

        let error = remove_deployed_file(&deployed, &game, &game_data)
            .expect_err("a required deployed-file removal must fail");

        assert!(error.to_string().contains("Failed to remove deployed file"));
        Ok(())
    }
}
