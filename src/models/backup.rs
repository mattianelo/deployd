use serde::{Deserialize, Serialize};

/// Manifest embedded in every `.deployd-backup` archive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupManifest {
    /// Format version — always 1.
    pub version: u32,
    /// ISO 8601 creation timestamp.
    pub created_at: String,
    pub deployd_version: String,
    pub games: Vec<BackupGameEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupGameEntry {
    pub id: String,
    pub title: String,
    pub profile_count: usize,
    pub mod_count: usize,
}
