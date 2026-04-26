use anyhow::{Context, Result};

use super::Tracker;

impl Tracker {
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
            "INSERT INTO download_entries (id, mod_name, archive_path, nexus_mod_id, nexus_file_id, nexus_domain, game_domain, metadata_fetched, nexus_file_name, nexus_is_primary, status, archive_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
                archive_hash = excluded.archive_hash",
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
        let rows: Vec<(String, String, Option<String>, Option<i64>, Option<i64>, Option<String>, Option<String>, bool, Option<String>, bool, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT id, mod_name, archive_path, nexus_mod_id, nexus_file_id, nexus_domain, game_domain, metadata_fetched, nexus_file_name, nexus_is_primary, status, archive_hash
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
                )| {
                    let path = archive_path.map(std::path::PathBuf::from);
                    let is_installed = status_str.as_deref().unwrap_or("downloaded") == "installed";
                    // Always load Installed entries (the archive may have been deleted
                    // or the downloads folder may have changed — the mod is still installed).
                    // Filter out other entries whose archive no longer exists.
                    if !is_installed
                        && let Some(ref p) = path
                        && !p.exists()
                    {
                        return None;
                    }
                    let nexus_ids = nexus_mod_id
                        .zip(nexus_file_id)
                        .zip(nexus_domain)
                        .map(|((mod_id, file_id), domain)| NexusIds { mod_id, file_id, domain });
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
                    })
                },
            )
            .collect();

        Ok(entries)
    }
}
