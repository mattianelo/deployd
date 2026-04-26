use serde::Deserialize;

// Structs below are Nexus Mods API response shapes. All fields are kept for
// complete deserialization even when only a subset is read by the application.
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct NexusUser {
    pub user_id: i64,
    pub name: String,
    pub is_premium: bool,
    pub is_supporter: bool,
    #[serde(default)]
    pub profile_url: Option<String>,
}

#[allow(dead_code)] // see module-level comment above
#[derive(Debug, Clone, Deserialize)]
pub struct NexusModInfo {
    pub mod_id: i64,
    pub name: String,
    pub author: String,
    pub version: String,
    /// Can be null for mods without a summary.
    #[serde(default)]
    pub summary: Option<String>,
    /// Can be null/absent for mods without a long description.
    #[serde(default)]
    pub description: Option<String>,
    pub picture_url: Option<String>,
    /// Can be null for some mods (e.g. very old or moderation-locked mods).
    #[serde(rename = "endorsement_count", default)]
    pub endorsements: Option<i64>,
    pub domain_name: String,
    pub updated_timestamp: i64,
    pub status: String,
}

/// Wrapper for the `/mods/{id}/files.json` response.
#[derive(Debug, Clone, Deserialize)]
pub struct NexusFilesResponse {
    pub files: Vec<NexusFileEntry>,
}

#[allow(dead_code)] // see module-level comment above
#[derive(Debug, Clone, Deserialize)]
pub struct NexusFileEntry {
    pub file_id: i64,
    /// Nexus occasionally returns null for this on old/archived entries.
    #[serde(default)]
    pub name: String,
    /// Null for old/archived file entries.
    #[serde(default)]
    pub version: Option<String>,
    /// Never read; skipped to avoid float/int type mismatch on older mod entries.
    #[serde(skip_deserializing, default)]
    pub size_kb: Option<u64>,
    /// Nexus occasionally returns null for this on old/archived entries.
    #[serde(default)]
    pub file_name: String,
    pub category_name: Option<String>,
    /// Absent for some file categories — defaults to false.
    #[serde(default)]
    pub is_primary: bool,
    /// Never read; skipped to avoid float/int type mismatch on older mod entries.
    #[serde(skip_deserializing, default)]
    pub uploaded_timestamp: Option<i64>,
}

#[allow(dead_code)] // see module-level comment above
#[derive(Debug, Clone, Deserialize)]
pub struct DownloadLink {
    #[serde(rename = "URI")]
    pub uri: String,
    pub name: String,
    pub short_name: String,
}
