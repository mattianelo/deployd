use anyhow::{Context, Result};
use sqlx::Row;

use crate::models::mod_entry::{InstallTarget, ModEntry};

use super::Tracker;

impl Tracker {
    /// Insert a new mod record.
    pub async fn insert_mod(&self, entry: &ModEntry) -> Result<()> {
        sqlx::query(
            "INSERT INTO mods (id, game_id, name, archive_hash, archive_path, installed_at,
                               enabled, priority, nexus_mod_id, nexus_file_id, nexus_domain,
                               install_target)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(&entry.game_id)
        .bind(&entry.name)
        .bind(&entry.archive_hash)
        .bind(&entry.archive_path)
        .bind(&entry.installed_at)
        .bind(entry.enabled)
        .bind(entry.priority)
        .bind(entry.nexus_mod_id)
        .bind(entry.nexus_file_id)
        .bind(&entry.nexus_domain)
        .bind(entry.install_target.to_string())
        .execute(&self.pool)
        .await
        .context("Failed to insert mod entry")?;
        Ok(())
    }

    /// Delete a mod entry.
    pub async fn delete_mod(&self, mod_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM mods WHERE id = ?")
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete mod")?;
        Ok(())
    }

    /// List all mods for a given game, ordered by priority ascending (lowest priority first).
    pub async fn list_mods(&self, game_id: &str) -> Result<Vec<ModEntry>> {
        let rows = sqlx::query(
            "SELECT id, game_id, name, archive_hash, archive_path, installed_at, enabled, priority,
                    nexus_mod_id, nexus_file_id, nexus_domain, version, author,
                    nexus_description, latest_version, install_target, notes
             FROM mods WHERE game_id = ? ORDER BY priority ASC",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list mods")?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let install_target: Option<String> = row.get("install_target");
                ModEntry {
                    id: row.get("id"),
                    game_id: row.get("game_id"),
                    name: row.get("name"),
                    archive_hash: row.get("archive_hash"),
                    archive_path: row.get("archive_path"),
                    installed_at: row.get("installed_at"),
                    enabled: row.get("enabled"),
                    priority: row.get("priority"),
                    nexus_mod_id: row.get("nexus_mod_id"),
                    nexus_file_id: row.get("nexus_file_id"),
                    nexus_domain: row.get("nexus_domain"),
                    version: row.get("version"),
                    author: row.get("author"),
                    nexus_description: row.get("nexus_description"),
                    latest_version: row.get("latest_version"),
                    install_target: InstallTarget::from(install_target.as_deref()),
                    notes: row.get("notes"),
                }
            })
            .collect())
    }

    /// Get the next priority value for a game (one higher than current max).
    pub async fn next_priority(&self, game_id: &str) -> Result<i32> {
        let row: (i32,) =
            sqlx::query_as("SELECT COALESCE(MAX(priority), -1) + 1 FROM mods WHERE game_id = ?")
                .bind(game_id)
                .fetch_one(&self.pool)
                .await
                .context("Failed to query next priority")?;

        Ok(row.0)
    }

    /// Batch-update priority values. Each tuple is (mod_id, new_priority).
    pub async fn update_priorities(&self, updates: &[(String, i32)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (mod_id, priority) in updates {
            sqlx::query("UPDATE mods SET priority = ? WHERE id = ?")
                .bind(priority)
                .bind(mod_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit priority updates")?;
        Ok(())
    }

    /// Toggle a mod's enabled state.
    pub async fn toggle_mod(&self, mod_id: &str, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE mods SET enabled = ? WHERE id = ?")
            .bind(enabled)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to toggle mod")?;
        Ok(())
    }

    /// Update a mod's display name.
    pub async fn update_mod_name(&self, mod_id: &str, new_name: &str) -> Result<()> {
        sqlx::query("UPDATE mods SET name = ? WHERE id = ?")
            .bind(new_name)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to update mod name")?;
        Ok(())
    }

    /// Set `enabled` for every mod belonging to a game in one statement.
    pub async fn set_all_mods_enabled(&self, game_id: &str, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE mods SET enabled = ? WHERE game_id = ?")
            .bind(enabled)
            .bind(game_id)
            .execute(&self.pool)
            .await
            .context("Failed to set all mods enabled")?;
        Ok(())
    }

    /// Update a mod's user notes. Stores NULL when the string is empty.
    pub async fn update_mod_notes(&self, mod_id: &str, notes: &str) -> Result<()> {
        let value: Option<&str> = if notes.is_empty() { None } else { Some(notes) };
        sqlx::query("UPDATE mods SET notes = ? WHERE id = ?")
            .bind(value)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to update mod notes")?;
        Ok(())
    }

    /// Update the Nexus coordinates attached to an installed mod.
    pub async fn update_mod_nexus_ids(
        &self,
        mod_id: &str,
        nexus_mod_id: Option<i64>,
        nexus_file_id: Option<i64>,
        nexus_domain: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE mods
             SET nexus_mod_id = ?, nexus_file_id = ?, nexus_domain = ?
             WHERE id = ?",
        )
        .bind(nexus_mod_id)
        .bind(nexus_file_id)
        .bind(nexus_domain)
        .bind(mod_id)
        .execute(&self.pool)
        .await
        .context("Failed to update mod Nexus IDs")?;
        Ok(())
    }

    pub async fn save_fomod_selections(&self, mod_id: &str, json: &str) -> Result<()> {
        sqlx::query("UPDATE mods SET fomod_selections = ? WHERE id = ?")
            .bind(json)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to save FOMOD selections")?;
        Ok(())
    }

    pub async fn get_fomod_selections(&self, mod_id: &str) -> Result<Option<String>> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT fomod_selections FROM mods WHERE id = ?")
                .bind(mod_id)
                .fetch_optional(&self.pool)
                .await
                .context("Failed to get FOMOD selections")?;
        Ok(row.and_then(|(v,)| v))
    }

    /// Get all mods that have Nexus IDs (for update checking).
    pub async fn mods_with_nexus_ids(&self, game_id: &str) -> Result<Vec<ModEntry>> {
        let all = self.list_mods(game_id).await?;
        Ok(all
            .into_iter()
            .filter(|m| m.nexus_mod_id.is_some())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_tracker() -> Result<Tracker> {
        let tracker = Tracker::open("sqlite::memory:").await?;
        sqlx::query(
            "INSERT INTO games (id, title, path, data_subdir)
             VALUES ('g', 'Test', '/tmp/g', 'Data')",
        )
        .execute(&tracker.pool)
        .await?;
        Ok(tracker)
    }

    fn mod_entry(id: &str) -> ModEntry {
        ModEntry {
            id: id.to_string(),
            game_id: "g".to_string(),
            name: "Test Mod".to_string(),
            archive_hash: None,
            archive_path: None,
            installed_at: None,
            enabled: true,
            priority: 0,
            nexus_mod_id: None,
            nexus_file_id: None,
            nexus_domain: None,
            version: None,
            author: None,
            nexus_description: None,
            latest_version: None,
            install_target: InstallTarget::Data,
            notes: None,
        }
    }

    #[tokio::test]
    async fn updates_nexus_ids_without_changing_mod_identity() -> Result<()> {
        let tracker = make_tracker().await?;
        tracker.insert_mod(&mod_entry("mod-a")).await?;

        tracker
            .update_mod_nexus_ids("mod-a", Some(101), None, Some("witcher"))
            .await?;

        let mods = tracker.list_mods("g").await?;
        assert_eq!(mods.len(), 1);
        let mod_entry = mods
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("expected one mod entry"))?;
        assert_eq!(mod_entry.id, "mod-a");
        assert_eq!(mod_entry.nexus_mod_id, Some(101));
        assert_eq!(mod_entry.nexus_file_id, None);
        assert_eq!(mod_entry.nexus_domain.as_deref(), Some("witcher"));
        Ok(())
    }
}
