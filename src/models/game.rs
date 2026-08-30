use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GameEngine {
    #[default]
    Bethesda,
    REDEngine,
    Eclipse,
    Aurora,
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
}

impl Game {
    pub fn data_dir(&self) -> PathBuf {
        self.path.join(&self.data_subdir)
    }
}
