use anyhow::{Context, Result};

use super::Tracker;

pub(crate) struct DownloadNexusMetadata {
    pub(crate) mod_name: String,
    pub(crate) nexus_mod_id: i64,
    pub(crate) nexus_file_id: i64,
    pub(crate) domain: String,
    pub(crate) metadata_fetched: bool,
    pub(crate) nexus_file_name: Option<String>,
    pub(crate) nexus_is_primary: bool,
    pub(crate) version: Option<String>,
    pub(crate) author: Option<String>,
}

impl Tracker {
    pub(crate) async fn update_download_nexus_metadata(
        &self,
        download_id: &str,
        metadata: &DownloadNexusMetadata,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE download_entries
             SET mod_name = ?, nexus_mod_id = ?, nexus_file_id = ?, nexus_domain = ?,
                 game_domain = ?, metadata_fetched = ?, nexus_file_name = ?,
                 nexus_is_primary = ?, version = ?, author = ?
             WHERE id = ?",
        )
        .bind(&metadata.mod_name)
        .bind(metadata.nexus_mod_id)
        .bind(metadata.nexus_file_id)
        .bind(&metadata.domain)
        .bind(&metadata.domain)
        .bind(metadata.metadata_fetched)
        .bind(metadata.nexus_file_name.as_deref())
        .bind(metadata.nexus_is_primary)
        .bind(metadata.version.as_deref())
        .bind(metadata.author.as_deref())
        .bind(download_id)
        .execute(&self.pool)
        .await
        .context("Failed to persist fetched Nexus metadata")?;
        Ok(())
    }

    /// Save a download entry to the database.
    pub async fn save_download_entry(
        &self,
        entry: &crate::models::download::DownloadEntry,
    ) -> Result<()> {
        let (nexus_mod_id, nexus_file_id, nexus_domain) = match &entry.nexus_ids {
            Some(n) => (Some(n.mod_id), Some(n.file_id), Some(n.domain.as_str())),
            None => (None, None, None),
        };
        sqlx::query(
            "INSERT INTO download_entries (id, mod_name, archive_path, nexus_mod_id, nexus_file_id, nexus_domain, game_domain, metadata_fetched, nexus_file_name, nexus_is_primary, status, archive_hash, archive_md5, version, author, hidden)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                mod_name = excluded.mod_name,
                archive_path = excluded.archive_path,
                nexus_mod_id = excluded.nexus_mod_id,
                nexus_file_id = excluded.nexus_file_id,
                nexus_domain = excluded.nexus_domain,
                game_domain = excluded.game_domain,
                metadata_fetched = excluded.metadata_fetched,
                nexus_file_name = excluded.nexus_file_name,
                nexus_is_primary = excluded.nexus_is_primary,
                status = excluded.status,
                archive_hash = excluded.archive_hash,
                archive_md5 = excluded.archive_md5,
                version = excluded.version,
                author = excluded.author,
                hidden = excluded.hidden",
        )
        .bind(&entry.id)
        .bind(&entry.mod_name)
        .bind(entry.archive_path.as_ref().map(|p| p.to_string_lossy().to_string()))
        .bind(nexus_mod_id)
        .bind(nexus_file_id)
        .bind(nexus_domain)
        .bind(entry.game_domain.as_deref())
        .bind(entry.metadata_fetched)
        .bind(entry.nexus_file_name.as_deref())
        .bind(entry.nexus_is_primary)
        .bind(entry.status.as_db_str())
        .bind(entry.archive_hash.as_deref())
        .bind(entry.archive_md5.as_deref())
        .bind(entry.version.as_deref())
        .bind(entry.author.as_deref())
        .bind(entry.hidden)
        .execute(&self.pool)
        .await
        .context("Failed to save download entry")?;
        Ok(())
    }

    /// Delete download entries by ID from the database.
    pub async fn delete_download_entries(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let sql = format!("DELETE FROM download_entries WHERE id IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for id in ids {
            q = q.bind(id);
        }
        q.execute(&self.pool)
            .await
            .context("Failed to delete download entries")?;
        Ok(())
    }

    /// Load all persisted download entries.
    pub async fn load_download_entries(
        &self,
    ) -> Result<Vec<crate::models::download::DownloadEntry>> {
        use crate::models::download::{DownloadEntry, DownloadStatus, NexusIds};
        #[allow(clippy::type_complexity)] // Flat SQLx row tuple — a struct would need manual FromRow impl with no real gain.
        let rows: Vec<(String, String, Option<String>, Option<i64>, Option<i64>, Option<String>, Option<String>, bool, Option<String>, bool, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, bool)> =
            sqlx::query_as(
                "SELECT id, mod_name, archive_path, nexus_mod_id, nexus_file_id, nexus_domain, game_domain, metadata_fetched, nexus_file_name, nexus_is_primary, status, archive_hash, archive_md5, version, author, COALESCE(hidden, 0)
                 FROM download_entries"
            )
            .fetch_all(&self.pool)
            .await
            .context("Failed to load download entries")?;

        let entries = rows
            .into_iter()
            .filter_map(
                |(
                    id,
                    mod_name,
                    archive_path,
                    nexus_mod_id,
                    nexus_file_id,
                    nexus_domain,
                    game_domain,
                    metadata_fetched,
                    nexus_file_name,
                    nexus_is_primary,
                    status_str,
                    archive_hash,
                    archive_md5,
                    version,
                    author,
                    hidden,
                )| {
                    let mut path = archive_path.map(std::path::PathBuf::from);
                    let is_installed = status_str.as_deref().unwrap_or("downloaded") == "installed";
                    let keep_metadata_cache = is_installed
                        || metadata_fetched
                        || nexus_file_name.is_some()
                        || nexus_is_primary
                        || archive_hash.is_some()
                        || archive_md5.is_some()
                        || version.as_ref().is_some_and(|v| !v.is_empty())
                        || author.as_ref().is_some_and(|a| !a.is_empty());
                    let path_missing = path.as_ref().is_some_and(|p| !p.exists());
                    let hidden = hidden || (path_missing && keep_metadata_cache);
                    if path_missing {
                        if keep_metadata_cache {
                            path = None;
                        } else {
                            return None;
                        }
                    }
                    let nexus_ids = nexus_mod_id.zip(nexus_file_id).zip(nexus_domain).map(
                        |((mod_id, file_id), domain)| NexusIds {
                            mod_id,
                            file_id,
                            domain,
                        },
                    );
                    let status =
                        DownloadStatus::from_db_str(status_str.as_deref().unwrap_or("downloaded"));
                    let status_msg = status.default_status_msg().to_string();
                    Some(DownloadEntry {
                        id,
                        mod_name,
                        status,
                        progress: 1.0,
                        status_msg,
                        error_msg: None,
                        nexus_ids,
                        archive_path: path,
                        metadata_fetched,
                        game_domain,
                        nexus_file_name,
                        nexus_is_primary,
                        archive_hash,
                        archive_md5,
                        version: version.filter(|v| !v.is_empty()),
                        author: author.filter(|a| !a.is_empty()),
                        hidden,
                    })
                },
            )
            .collect();

        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::{DownloadNexusMetadata, Tracker};
    use crate::models::download::{DownloadEntry, DownloadStatus};

    #[tokio::test]
    async fn fetched_metadata_survives_reload_without_install() -> anyhow::Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let mut entry = DownloadEntry::new("download".to_string(), "Archive".to_string(), None);
        entry.status = DownloadStatus::Downloaded;
        tracker.save_download_entry(&entry).await?;

        tracker
            .update_download_nexus_metadata(
                &entry.id,
                &DownloadNexusMetadata {
                    mod_name: "Resolved Mod".to_string(),
                    nexus_mod_id: 108_480,
                    nexus_file_id: 123_456,
                    domain: "fallout4".to_string(),
                    metadata_fetched: true,
                    nexus_file_name: Some("Main File".to_string()),
                    nexus_is_primary: true,
                    version: Some("1.3.0".to_string()),
                    author: Some("Author".to_string()),
                },
            )
            .await?;

        let loaded = tracker.load_download_entries().await?;
        let loaded = loaded.first().expect("persisted download entry");
        assert_eq!(loaded.mod_name, "Resolved Mod");
        assert_eq!(loaded.version.as_deref(), Some("1.3.0"));
        assert_eq!(loaded.nexus_file_name.as_deref(), Some("Main File"));
        assert_eq!(loaded.author.as_deref(), Some("Author"));
        assert!(loaded.metadata_fetched);
        assert_eq!(
            loaded
                .nexus_ids
                .as_ref()
                .map(|ids| (ids.mod_id, ids.file_id, ids.domain.as_str())),
            Some((108_480, 123_456, "fallout4"))
        );
        Ok(())
    }
}
