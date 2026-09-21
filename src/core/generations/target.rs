use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::core::game;
use crate::models::game::{Game, GameEngine};

pub(super) fn relative(value: &str) -> Result<&Path> {
    ensure!(
        !value.is_empty()
            && !value.contains('\\')
            && !value.contains('\0')
            && value
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
        "Invalid relative generation path: {value}"
    );
    let path = Path::new(value);
    ensure!(
        path.components()
            .all(|part| matches!(part, Component::Normal(_))),
        "Invalid generation path"
    );
    Ok(path)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) enum Target {
    Bethesda { root: bool, path: String },
    Aurora { root: bool, path: String },
    Eclipse { documents: bool, path: String },
    Redengine { root: bool, path: String },
    MassEffect { path: String },
    MeleLauncher { path: String },
    PluginControl { slot: usize },
    CustomIni { slot: usize },
}

impl Target {
    pub(super) fn validate(&self, engine: &GameEngine) -> Result<()> {
        let path = match (self, engine) {
            (Self::Bethesda { path, .. }, GameEngine::Bethesda)
            | (Self::Aurora { path, .. }, GameEngine::Aurora)
            | (Self::Eclipse { path, .. }, GameEngine::Eclipse)
            | (Self::Redengine { path, .. }, GameEngine::REDEngine)
            | (Self::MassEffect { path }, GameEngine::MassEffect)
            | (Self::MeleLauncher { path }, GameEngine::MassEffect) => path,
            (Self::PluginControl { .. } | Self::CustomIni { .. }, GameEngine::Bethesda) => {
                return Ok(());
            }
            _ => bail!("Historical target belongs to a different engine"),
        };
        relative(path)?;
        ensure!(
            !path.starts_with("~docs~"),
            "Documents routing must use the Eclipse anchor"
        );
        Ok(())
    }

    pub(super) fn file(engine: &GameEngine, recorded: &str) -> Result<Self> {
        let recorded = recorded.trim_end_matches('/');
        let (root, path) = recorded
            .strip_prefix("../")
            .map_or((false, recorded), |path| (true, path));
        if let Some(path) = recorded.strip_prefix("~docs~/") {
            ensure!(
                *engine == GameEngine::Eclipse,
                "Documents routing belongs to Eclipse"
            );
            relative(path)?;
            return Ok(Self::Eclipse {
                documents: true,
                path: path.to_owned(),
            });
        }
        relative(path)?;
        ensure!(!path.starts_with("~docs~"), "Invalid Documents routing");
        let path = path.to_owned();
        Ok(match engine {
            GameEngine::Bethesda => Self::Bethesda { root, path },
            GameEngine::Aurora => Self::Aurora { root, path },
            GameEngine::REDEngine => Self::Redengine { root, path },
            GameEngine::Eclipse => {
                ensure!(!root, "Eclipse paths must use its data or Documents anchor");
                Self::Eclipse {
                    documents: false,
                    path,
                }
            }
            GameEngine::MassEffect => {
                ensure!(!root, "MELE paths are relative to its game root");
                Self::MassEffect { path }
            }
        })
    }

    pub(super) fn recorded(&self) -> Result<String> {
        Ok(match self {
            Self::Bethesda { root, path }
            | Self::Aurora { root, path }
            | Self::Redengine { root, path } => {
                format!("{}{path}", if *root { "../" } else { "" })
            }
            Self::Eclipse { documents, path } => {
                format!("{}{path}", if *documents { "~docs~/" } else { "" })
            }
            Self::MassEffect { path } => path.clone(),
            _ => bail!("Managed configuration is not a mod-file target"),
        })
    }

    pub(super) fn resolve(&self, game: &Game) -> Result<PathBuf> {
        self.resolve_with(game, None)
    }

    pub(super) fn resolve_with(
        &self,
        game: &Game,
        created: Option<&std::collections::BTreeSet<PathBuf>>,
    ) -> Result<PathBuf> {
        let destination = self.recorded_path(game)?;
        if matches!(self, Self::PluginControl { .. } | Self::CustomIni { .. }) {
            return Ok(destination);
        }
        let root = if matches!(self, Self::MeleLauncher { .. }) {
            game.path
                .parent()
                .context("Shared launcher root is unavailable")?
        } else if destination.starts_with(&game.path) {
            game.path.as_path()
        } else {
            game.wine_prefix
                .as_deref()
                .filter(|prefix| destination.starts_with(prefix))
                .context("Deployment target is outside its authorized roots")?
        };
        resolve_casing_with(root, destination.strip_prefix(root)?, created)
    }

    pub(super) fn recorded_path(&self, game: &Game) -> Result<PathBuf> {
        self.validate(&game.engine)?;
        let data = game::deploy_dir(game);
        let (base, path) = match self {
            Self::Bethesda { root, path } if game.engine == GameEngine::Bethesda => (if *root { game.path.clone() } else { data }, path),
            Self::Aurora { root, path } if game.engine == GameEngine::Aurora => (if *root { game.path.clone() } else { data }, path),
            Self::Redengine { root, path } if game.engine == GameEngine::REDEngine => (if *root { game.path.clone() } else { data }, path),
            Self::Eclipse { documents, path } if game.engine == GameEngine::Eclipse => {
                let base = if *documents { data.parent().and_then(Path::parent).ok_or_else(|| anyhow::anyhow!("Eclipse Documents location is unavailable"))?.to_owned() } else { data };
                (base, path)
            },
            Self::MassEffect { path } if game.engine == GameEngine::MassEffect => (game.path.clone(), path),
            Self::MeleLauncher { path } if game.engine == GameEngine::MassEffect => (game.path.parent().ok_or_else(|| anyhow::anyhow!("Shared launcher root is unavailable"))?.join("Launcher"),path),
            Self::PluginControl { slot } if game.engine == GameEngine::Bethesda => return game::plugins_txt_paths(game).get(*slot).cloned().ok_or_else(|| anyhow::anyhow!("Plugin configuration location is unavailable; restore Wine prefix access")),
            Self::CustomIni { slot } if game.engine == GameEngine::Bethesda => return game::custom_ini_paths(game).get(*slot).cloned().ok_or_else(|| anyhow::anyhow!("Managed INI location is unavailable; restore Wine prefix access")),
            _ => bail!("Historical target belongs to a different engine"),
        };
        Ok(base.join(relative(path)?))
    }
}

#[cfg(test)]
fn resolve_casing(root: &Path, relative: &Path) -> Result<PathBuf> {
    resolve_casing_with(root, relative, None)
}

fn resolve_casing_with(
    root: &Path,
    relative: &Path,
    created: Option<&std::collections::BTreeSet<PathBuf>>,
) -> Result<PathBuf> {
    let mut resolved = root.to_owned();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            bail!("Target escapes its authorized root");
        };
        match std::fs::symlink_metadata(&resolved) {
            Ok(metadata) => ensure!(
                metadata.is_dir(),
                "Deployment parent is redirected or is not a directory: {}",
                resolved.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Cannot inspect deployment parent"),
        }
        let entries = match std::fs::read_dir(&resolved) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                resolved.push(name);
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Cannot inspect deployment directory '{}'",
                        resolved.display()
                    )
                });
            }
        };
        let expected = name.to_string_lossy().to_lowercase();
        let mut matched = Vec::new();
        for entry in entries {
            let entry = entry?;
            if entry.file_name().to_string_lossy().to_lowercase() == expected {
                matched.push(entry.path());
            }
        }
        matched.sort();
        if matched.len() > 1 {
            let exact = resolved.join(name);
            ensure!(
                created.is_some_and(|created| matched.contains(&exact)
                    && matched.iter().all(|path| created.contains(path))),
                "Ambiguous filename casing: {}; conflicting paths were preserved",
                matched
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            resolved = exact;
        } else if let Some(path) = matched.pop() {
            resolved = path;
        } else {
            resolved.push(name);
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn preserves_existing_casing_and_rejects_ambiguous_names() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::create_dir_all(temp.path().join("data/Textures"))?;
        std::fs::write(temp.path().join("data/Textures/Example.dds"), b"texture")?;
        assert_eq!(
            resolve_casing(temp.path(), Path::new("Data/textures/example.dds"))?,
            temp.path().join("data/Textures/Example.dds")
        );
        std::fs::write(temp.path().join("data/Textures/example.dds"), b"other")?;
        assert!(resolve_casing(temp.path(), Path::new("Data/textures/Example.dds")).is_err());
        Ok(())
    }
}
