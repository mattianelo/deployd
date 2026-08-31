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
    #[serde(default, deserialize_with = "de_null_string")]
    pub name: String,
    #[serde(default, deserialize_with = "de_null_string")]
    pub author: String,
    #[serde(default, deserialize_with = "de_null_string")]
    pub version: String,
    /// Can be null for mods without a summary.
    #[serde(default)]
    pub summary: Option<String>,
    /// Can be null/absent for mods without a long description.
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub picture_url: Option<String>,
    /// Can be null for some mods (e.g. very old or moderation-locked mods).
    #[serde(
        rename = "endorsement_count",
        default,
        deserialize_with = "de_opt_number_as_i64"
    )]
    pub endorsements: Option<i64>,
    #[serde(default, deserialize_with = "de_null_string")]
    pub domain_name: String,
    #[serde(default, deserialize_with = "de_null_number_as_i64")]
    pub updated_timestamp: i64,
    #[serde(default, deserialize_with = "de_null_string")]
    pub status: String,
}

/// Wrapper for the `/mods/{id}/files.json` response.
#[derive(Debug, Clone, Deserialize)]
pub struct NexusFilesResponse {
    pub files: Vec<NexusFileEntry>,
    #[serde(default)]
    pub file_updates: Vec<NexusFileUpdate>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NexusFileUpdate {
    pub old_file_id: i64,
    pub new_file_id: i64,
}

#[allow(dead_code)] // see module-level comment above
#[derive(Debug, Clone, Deserialize)]
pub struct NexusFileEntry {
    pub file_id: i64,
    /// Nexus occasionally returns null for this on old/archived entries.
    #[serde(default, deserialize_with = "de_null_string")]
    pub name: String,
    /// Null for old/archived file entries.
    #[serde(default)]
    pub version: Option<String>,
    /// Never read; skipped to avoid float/int type mismatch on older mod entries.
    #[serde(skip_deserializing, default)]
    pub size_kb: Option<u64>,
    /// Nexus occasionally returns null for this on old/archived entries.
    #[serde(default, deserialize_with = "de_null_string")]
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

impl NexusFileEntry {
    pub(crate) fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.file_name
        } else {
            &self.name
        }
    }
}

fn de_null_string<'de, D>(de: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(de)?.unwrap_or_default())
}

fn de_null_number_as_i64<'de, D>(de: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    de_opt_number_as_i64(de).map(Option::unwrap_or_default)
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
            i64::try_from(v)
                .map(Some)
                .map_err(|_| E::custom("timestamp out of i64 range"))
        }
        fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
            if v >= i64::MIN as f64 && v <= i64::MAX as f64 {
                Ok(Some(v as i64))
            } else {
                Err(E::custom("timestamp out of i64 range"))
            }
        }
        fn visit_some<D2: Deserializer<'de>>(self, de: D2) -> Result<Self::Value, D2::Error> {
            de.deserialize_any(Self)
        }
    }
    de.deserialize_option(V)
}

#[cfg(test)]
mod tests {
    use super::{NexusFileEntry, NexusModInfo};

    #[test]
    fn parses_archived_mod_info_with_nullable_display_fields() {
        let info: NexusModInfo = serde_json::from_value(serde_json::json!({
            "mod_id": 42,
            "name": "Archived Mod",
            "author": null,
            "version": null,
            "summary": null,
            "description": null,
            "picture_url": null,
            "endorsement_count": null,
            "domain_name": "skyrimspecialedition",
            "updated_timestamp": null,
            "status": null
        }))
        .unwrap();

        assert_eq!(info.name, "Archived Mod");
        assert!(info.author.is_empty());
        assert!(info.version.is_empty());
        assert_eq!(info.updated_timestamp, 0);
    }

    #[test]
    fn archived_file_falls_back_to_literal_filename() {
        let entry: NexusFileEntry = serde_json::from_value(serde_json::json!({
            "file_id": 7,
            "name": null,
            "version": null,
            "file_name": "Archived-Mod-7-1700000000.7z",
            "category_name": "OLD_VERSION",
            "is_primary": false,
            "uploaded_timestamp": 1700000000
        }))
        .unwrap();

        assert_eq!(entry.display_name(), "Archived-Mod-7-1700000000.7z");
    }
}
