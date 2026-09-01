use anyhow::{Context, Result};
use sqlx::{Executor, Sqlite};

use super::Tracker;

impl Tracker {
    pub(crate) async fn persist_fetched_download_metadata(
        &self,
        entry: &crate::models::download::DownloadEntry,
        latest_version: Option<&str>,
        description: Option<&str>,
    ) -> Result<()> {
        let Some(nexus_ids) = entry.nexus_ids.as_ref() else {
            anyhow::bail!("Fetched Nexus metadata has no Nexus identity");
        };
        let mut tx = self.pool.begin().await?;
        let update = sqlx::query(
            "UPDATE download_entries
             SET mod_name = ?, archive_path = ?, nexus_mod_id = ?, nexus_file_id = ?,
                 nexus_domain = ?, game_domain = ?, metadata_fetched = ?, nexus_file_name = ?,
                 nexus_is_primary = ?, status = ?, archive_hash = ?, archive_md5 = ?,
                 version = ?, author = ?, hidden = ?
             WHERE id = ?",
        )
        .bind(&entry.mod_name)
        .bind(
            entry
                .archive_path
                .as_ref()
                .map(|path| path.to_string_lossy().to_string()),
        )
        .bind(nexus_ids.mod_id)
        .bind(nexus_ids.file_id)
        .bind(&nexus_ids.domain)
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
        .bind(&entry.id)
        .execute(&mut *tx)
        .await?;
        if update.rows_affected() != 1 {
            anyhow::bail!("Download metadata row no longer exists");
        }
        if nexus_ids.file_id > 0 {
            sqlx::query(
                "UPDATE mods
                 SET version = COALESCE(?, version), author = COALESCE(?, author),
                     latest_version = ?, nexus_description = COALESCE(?, nexus_description),
                     nexus_file_name = COALESCE(?, nexus_file_name),
                     nexus_is_primary = CASE WHEN ? THEN 1 ELSE nexus_is_primary END
                 WHERE nexus_mod_id = ? AND nexus_file_id = ? AND nexus_domain = ?",
            )
            .bind(entry.version.as_deref())
            .bind(entry.author.as_deref())
            .bind(latest_version)
            .bind(description)
            .bind(entry.nexus_file_name.as_deref())
            .bind(entry.nexus_is_primary)
            .bind(nexus_ids.mod_id)
            .bind(nexus_ids.file_id)
            .bind(&nexus_ids.domain)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit fetched Nexus metadata")?;
        Ok(())
    }

    /// Save a download entry to the database.
    pub async fn save_download_entry(
        &self,
        entry: &crate::models::download::DownloadEntry,
    ) -> Result<()> {
        save_download_entry_with(&self.pool, entry).await
    }

    pub(crate) async fn persist_download_scan(
        &self,
        removed_ids: &[String],
        entries: &[crate::models::download::DownloadEntry],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        if !removed_ids.is_empty() {
            let placeholders = removed_ids
                .iter()
                .map(|_| "?")
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!("DELETE FROM download_entries WHERE id IN ({placeholders})");
            let mut query = sqlx::query(&sql);
            for id in removed_ids {
                query = query.bind(id);
            }
            query.execute(&mut *tx).await?;
        }
        for entry in entries {
            save_download_entry_with(&mut *tx, entry).await?;
        }
        tx.commit()
            .await
            .context("Failed to commit downloads scan")?;
        Ok(())
    }

    pub(crate) async fn update_download_nexus_identity(
        &self,
        download_id: &str,
        nexus_ids: &crate::models::download::NexusIds,
    ) -> Result<()> {
        let update = sqlx::query(
            "UPDATE download_entries
             SET nexus_mod_id = ?, nexus_file_id = ?, nexus_domain = ?
             WHERE id = ?",
        )
        .bind(nexus_ids.mod_id)
        .bind(nexus_ids.file_id)
        .bind(&nexus_ids.domain)
        .bind(download_id)
        .execute(&self.pool)
        .await
        .context("Failed to persist download Nexus identity")?;
        if update.rows_affected() != 1 {
            anyhow::bail!("Download row no longer exists");
        }
        Ok(())
    }

    pub(crate) async fn update_download_archive_md5(
        &self,
        download_id: &str,
        archive_md5: &str,
    ) -> Result<()> {
        let update = sqlx::query("UPDATE download_entries SET archive_md5 = ? WHERE id = ?")
            .bind(archive_md5)
            .bind(download_id)
            .execute(&self.pool)
            .await
            .context("Failed to persist download archive MD5")?;
        if update.rows_affected() != 1 {
            anyhow::bail!("Download row no longer exists");
        }
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

async fn save_download_entry_with<'e, E>(
    executor: E,
    entry: &crate::models::download::DownloadEntry,
) -> Result<()>
where
    E: Executor<'e, Database = Sqlite>,
{
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
    .bind(
        entry
            .archive_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
    )
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
    .execute(executor)
    .await
    .context("Failed to save download entry")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Tracker;
    use crate::models::download::{DownloadEntry, DownloadStatus, NexusIds};
    use crate::models::mod_entry::{InstallTarget, ModEntry};

    #[tokio::test]
    async fn fetched_metadata_survives_reload_without_install() -> anyhow::Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let mut entry = DownloadEntry::new("download".to_string(), "Archive".to_string(), None);
        entry.status = DownloadStatus::Downloaded;
        tracker.save_download_entry(&entry).await?;

        entry.mod_name = "Resolved Mod".to_string();
        entry.nexus_ids = Some(NexusIds {
            mod_id: 108_480,
            file_id: 123_456,
            domain: "fallout4".to_string(),
        });
        entry.game_domain = Some("fallout4".to_string());
        entry.metadata_fetched = true;
        entry.nexus_file_name = Some("Main File".to_string());
        entry.nexus_is_primary = true;
        entry.version = Some("1.3.0".to_string());
        entry.author = Some("Author".to_string());
        tracker
            .persist_fetched_download_metadata(&entry, Some("1.4.0"), Some("Description"))
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

    #[tokio::test]
    async fn fetched_metadata_updates_matching_installed_mod() -> anyhow::Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let mut installed = ModEntry {
            id: "installed".to_string(),
            game_id: "fallout4".to_string(),
            name: "Installed Mod".to_string(),
            archive_hash: None,
            archive_path: None,
            installed_at: None,
            enabled: true,
            priority: 0,
            nexus_mod_id: Some(108_480),
            nexus_file_id: Some(123_456),
            nexus_domain: Some("fallout4".to_string()),
            version: Some("1.2.0".to_string()),
            author: None,
            nexus_description: None,
            latest_version: Some("stale".to_string()),
            nexus_file_name: None,
            nexus_is_primary: false,
            archive_md5: None,
            install_target: InstallTarget::Data,
            notes: None,
        };
        tracker.insert_mod(&installed).await?;
        let mut entry = DownloadEntry::new("download".to_string(), "Archive".to_string(), None);
        entry.status = DownloadStatus::Downloaded;
        tracker.save_download_entry(&entry).await?;
        entry.nexus_ids = Some(NexusIds {
            mod_id: 108_480,
            file_id: 123_456,
            domain: "fallout4".to_string(),
        });
        entry.metadata_fetched = true;
        entry.version = Some("1.3.0".to_string());
        entry.author = Some("Author".to_string());
        entry.nexus_file_name = Some("Main File".to_string());
        entry.nexus_is_primary = true;

        tracker
            .persist_fetched_download_metadata(&entry, None, Some("Description"))
            .await?;

        installed = tracker
            .list_mods("fallout4")
            .await?
            .into_iter()
            .next()
            .expect("installed mod");
        assert_eq!(installed.version.as_deref(), Some("1.3.0"));
        assert_eq!(installed.author.as_deref(), Some("Author"));
        assert_eq!(installed.latest_version, None);
        assert_eq!(installed.nexus_description.as_deref(), Some("Description"));
        assert_eq!(installed.nexus_file_name.as_deref(), Some("Main File"));
        assert!(installed.nexus_is_primary);
        Ok(())
    }

    #[tokio::test]
    async fn auxiliary_updates_preserve_fetched_metadata() -> anyhow::Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let mut entry = DownloadEntry::new(
            "download".to_string(),
            "Resolved Mod".to_string(),
            Some(NexusIds {
                mod_id: 42,
                file_id: 7,
                domain: "fallout4".to_string(),
            }),
        );
        entry.status = DownloadStatus::Downloaded;
        entry.metadata_fetched = true;
        entry.nexus_file_name = Some("Resolved File".to_string());
        entry.version = Some("1.3.0".to_string());
        tracker.save_download_entry(&entry).await?;

        tracker
            .update_download_archive_md5(&entry.id, "archive-md5")
            .await?;
        tracker
            .update_download_nexus_identity(
                &entry.id,
                &NexusIds {
                    mod_id: 42,
                    file_id: 7,
                    domain: "fallout4".to_string(),
                },
            )
            .await?;

        let loaded = tracker.load_download_entries().await?;
        let loaded = loaded.first().expect("persisted download");
        assert!(loaded.metadata_fetched);
        assert_eq!(loaded.nexus_file_name.as_deref(), Some("Resolved File"));
        assert_eq!(loaded.version.as_deref(), Some("1.3.0"));
        assert_eq!(loaded.archive_md5.as_deref(), Some("archive-md5"));
        Ok(())
    }
}
