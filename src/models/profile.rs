#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SaveMode {
    /// All profiles share the same game save directory (default behaviour).
    #[default]
    Global,
    /// This profile owns its saves; Deployd backs them up and restores them on switch.
    ProfileSpecific,
}

impl SaveMode {
    pub fn from_db(s: &str) -> Self {
        match s {
            "profile" => SaveMode::ProfileSpecific,
            _ => SaveMode::Global,
        }
    }

    pub fn to_db(&self) -> &'static str {
        match self {
            SaveMode::Global => "global",
            SaveMode::ProfileSpecific => "profile",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub id: String,
    pub game_id: String,
    pub name: String,
    pub is_active: bool,
    /// Whether this profile manages its own save files.
    pub save_mode: SaveMode,
    /// Modification time of the profile's save snapshot directory; `None` if no snapshot exists yet.
    /// Populated by the app layer after loading profiles — not stored in the DB.
    pub save_synced_at: Option<std::time::SystemTime>,
}
