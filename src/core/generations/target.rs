use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail, ensure};
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

    pub(super) fn resolve(&self, game: &Game) -> Result<PathBuf> {
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
