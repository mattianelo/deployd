use std::fmt;

/// Where the mod's files are deployed relative to the game directory.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum InstallTarget {
    /// Files go into the game's Data subdirectory (default for most mods).
    #[default]
    Data,
    /// Files go directly into the game root directory (script extenders, ENB, etc.).
    Root,
}

impl fmt::Display for InstallTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallTarget::Data => write!(f, "data"),
            InstallTarget::Root => write!(f, "root"),
        }
    }
}

impl From<Option<&str>> for InstallTarget {
    fn from(s: Option<&str>) -> Self {
        match s {
            Some("root") => InstallTarget::Root,
            _ => InstallTarget::Data,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ModEntry {
    pub id: String,
    pub game_id: String,
    pub name: String,
    pub archive_hash: Option<String>,
    pub archive_path: Option<String>,
    pub installed_at: Option<String>,
    pub enabled: bool,
    pub priority: i32,
    pub nexus_mod_id: Option<i64>,
    pub nexus_file_id: Option<i64>,
    pub nexus_domain: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub nexus_description: Option<String>,
    pub latest_version: Option<String>,
    pub install_target: InstallTarget,
    pub notes: Option<String>,
}
