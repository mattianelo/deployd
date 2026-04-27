use serde::{Deserialize, Deserializer};

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
    /// Nexus sometimes returns this as a float (e.g. `1604483725.0`) on old entries,
    /// which serde rejects when targeting i64.  The custom deserializer accepts both.
    #[serde(default, deserialize_with = "de_opt_number_as_i64")]
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

/// Result of a Nexus `md5_search` API call.
/// Each element pairs the mod-level info with the specific file entry whose
/// archive has the queried MD5, giving a single-call resolution of both
/// `mod_id` and `file_id` with correct domain — no filename parsing needed.
#[derive(Debug, Clone, Deserialize)]
pub struct Md5SearchResult {
    /// Shadowing the keyword `mod` with a raw identifier.
    pub r#mod: NexusModInfo,
    pub file_details: NexusFileEntry,
}

/// Deserializes an optional JSON number (integer or float) into `Option<i64>`.
/// Needed because the Nexus API sometimes encodes timestamps as floats on older entries.
fn de_opt_number_as_i64<'de, D>(de: D) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    struct V;
    impl<'de> serde::de::Visitor<'de> for V {
        type Value = Option<i64>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "an integer, float, or null")
        }
        fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
            Ok(None)
        }
        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(Some(v))
        }
        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(Some(v as i64))
        }
        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
            Ok(Some(v as i64))
        }
        fn visit_some<D2: Deserializer<'de>>(self, de: D2) -> Result<Self::Value, D2::Error> {
            de.deserialize_any(Self)
        }
    }
    de.deserialize_option(V)
}
