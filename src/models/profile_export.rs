use serde::{Deserialize, Serialize};

/// Portable snapshot of a profile's mod and plugin state.
/// Uses human-readable names/filenames so it can be shared across installs.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProfileExport {
    /// Format version — always 1.
    pub version: u32,
    pub game_id: String,
    pub profile_name: String,
    pub mods: Vec<ProfileModExport>,
    pub plugins: Vec<ProfilePluginExport>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProfileModExport {
    pub name: String,
    pub enabled: bool,
    pub priority: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProfilePluginExport {
    pub filename: String,
    pub enabled: bool,
    pub load_order: i32,
}
