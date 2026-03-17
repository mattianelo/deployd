use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum GameEngine {
    #[default]
    Bethesda,
    REDEngine,
    Eclipse,
}

#[derive(Debug, Clone)]
pub struct Game {
    pub id: String,
    pub title: String,
    pub path: PathBuf,
    pub data_subdir: String,
    pub engine: GameEngine,
    /// User-specified Wine prefix override. `None` means auto-detect via Heroic config / relative paths.
    pub wine_prefix: Option<PathBuf>,
}

impl Game {
    pub fn data_dir(&self) -> PathBuf {
        self.path.join(&self.data_subdir)
    }
}
