use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GameEngine {
    #[default]
    #[serde(rename = "bethesda")]
    Bethesda,
    #[serde(rename = "redengine")]
    REDEngine,
    #[serde(rename = "eclipse")]
    Eclipse,
    #[serde(rename = "aurora")]
    Aurora,
    #[serde(rename = "mass_effect")]
    MassEffect,
}

impl GameEngine {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Bethesda => "bethesda",
            Self::REDEngine => "redengine",
            Self::Eclipse => "eclipse",
            Self::Aurora => "aurora",
            Self::MassEffect => "mass_effect",
        }
    }

    pub(crate) fn from_persisted(value: Option<&str>) -> Result<Self> {
        value
            .map(str::parse)
            .transpose()
            .map(|engine| engine.unwrap_or_default())
    }
}

impl FromStr for GameEngine {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "bethesda" => Ok(Self::Bethesda),
            "redengine" => Ok(Self::REDEngine),
            "eclipse" => Ok(Self::Eclipse),
            "aurora" => Ok(Self::Aurora),
            "mass_effect" => Ok(Self::MassEffect),
            _ => bail!(
                "Unsupported game engine '{value}'; update Deployd or correct the game configuration before deploying"
            ),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
    pub data_subdir: String,
    pub engine: GameEngine,
    /// User-specified Wine prefix. `None` means no prefix (native game or not yet configured).
    pub wine_prefix: Option<PathBuf>,
}

/// A game configuration confirmed by setup UI and ready for persistence.
#[derive(Debug, Clone)]
pub struct GameConfig {
    pub game: Game,
    /// `true` when the user added the game manually.
    pub custom: bool,
    pub(crate) locations: Vec<crate::utils::location::FolderSelection>,
}

impl Game {
    pub fn data_dir(&self) -> PathBuf {
        self.path.join(&self.data_subdir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_engine_identity() -> Result<()> {
        for engine in [
            GameEngine::Bethesda,
            GameEngine::REDEngine,
            GameEngine::Eclipse,
            GameEngine::Aurora,
            GameEngine::MassEffect,
        ] {
            assert_eq!(engine.as_str().parse::<GameEngine>()?, engine);
            assert_eq!(
                serde_json::to_string(&engine)?,
                format!("\"{}\"", engine.as_str())
            );
            assert_eq!(
                serde_json::from_str::<GameEngine>(&serde_json::to_string(&engine)?)?,
                engine
            );
        }
        Ok(())
    }

    #[test]
    fn defaults_only_missing_legacy_engine_values() -> Result<()> {
        assert_eq!(GameEngine::from_persisted(None)?, GameEngine::Bethesda);
        for unknown in ["", "unknown", "MassEffect", "mass-effect", "unreal"] {
            assert!(GameEngine::from_persisted(Some(unknown)).is_err());
        }
        Ok(())
    }
}
