use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use sqlx::Row;

use crate::models::mod_entry::{InstallTarget, ModEntry};

use super::Tracker;

pub(crate) struct ModPropertiesUpdate<'a> {
    pub(crate) mod_id: &'a str,
    pub(crate) name: &'a str,
    pub(crate) notes: Option<&'a str>,
    pub(crate) version: Option<&'a str>,
    pub(crate) install_target: &'a InstallTarget,
    pub(crate) file_targets: &'a HashMap<String, InstallTarget>,
    pub(crate) nexus_identity: Option<ModNexusIdentityUpdate<'a>>,
}

pub(crate) struct ModNexusIdentityUpdate<'a> {
    pub(crate) mod_id: Option<i64>,
    pub(crate) file_id: Option<i64>,
    pub(crate) domain: Option<&'a str>,
}

impl Tracker {
    pub(crate) async fn update_mod_properties(
        &self,
        update: ModPropertiesUpdate<'_>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let result = if let Some(identity) = update.nexus_identity {
            sqlx::query(
                "UPDATE mods
                 SET name = ?, notes = ?, version = ?, install_target = ?,
                     nexus_mod_id = ?, nexus_file_id = ?, nexus_domain = ?
                 WHERE id = ?",
            )
            .bind(update.name)
            .bind(update.notes)
            .bind(update.version)
            .bind(update.install_target.to_string())
            .bind(identity.mod_id)
            .bind(identity.file_id)
            .bind(identity.domain)
            .bind(update.mod_id)
            .execute(&mut *tx)
            .await
        } else {
            sqlx::query(
                "UPDATE mods
                 SET name = ?, notes = ?, version = ?, install_target = ?
                 WHERE id = ?",
            )
            .bind(update.name)
            .bind(update.notes)
            .bind(update.version)
            .bind(update.install_target.to_string())
            .bind(update.mod_id)
            .execute(&mut *tx)
            .await
        };
        let result = result.context("Failed to update mod properties")?;
        if result.rows_affected() != 1 {
            bail!("Mod '{}' no longer exists", update.mod_id);
        }

        for (current_lowercase, target) in update.file_targets {
            match target {
                InstallTarget::Root if !current_lowercase.starts_with("../") => {
                    sqlx::query(
                        "UPDATE mod_files
                         SET game_rel_lowercase = '../' || game_rel_lowercase,
                             game_rel_original = '../' || game_rel_original
                         WHERE mod_id = ? AND game_rel_lowercase = ?",
                    )
                    .bind(update.mod_id)
                    .bind(current_lowercase)
                    .execute(&mut *tx)
                    .await
                    .context("Failed to set file target to root")?;
                }
                InstallTarget::Data if current_lowercase.starts_with("../") => {
                    sqlx::query(
                        "UPDATE mod_files
                         SET game_rel_lowercase = SUBSTR(game_rel_lowercase, 4),
                             game_rel_original = SUBSTR(game_rel_original, 4)
                         WHERE mod_id = ? AND game_rel_lowercase = ?",
                    )
                    .bind(update.mod_id)
                    .bind(current_lowercase)
                    .execute(&mut *tx)
                    .await
                    .context("Failed to set file target to data")?;
                }
                _ => {}
            }
        }

        tx.commit()
            .await
            .context("Failed to commit mod properties")?;
        Ok(())
    }

    /// Insert a new mod record.
    pub async fn insert_mod(&self, entry: &ModEntry) -> Result<()> {
        sqlx::query(
            "INSERT INTO mods (id, game_id, name, archive_hash, archive_path, installed_at,
                               enabled, priority, nexus_mod_id, nexus_file_id, nexus_domain,
                               install_target, nexus_file_name, nexus_is_primary, archive_md5)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
        .bind(entry.nexus_file_name.as_deref())
        .bind(entry.nexus_is_primary)
        .bind(entry.archive_md5.as_deref())
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
                    nexus_description, latest_version, nexus_file_name, nexus_is_primary,
                    archive_md5, install_target, notes
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
                    nexus_file_name: row.get("nexus_file_name"),
                    nexus_is_primary: row.get("nexus_is_primary"),
                    archive_md5: row.get("archive_md5"),
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
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn make_tracker() -> Result<Tracker> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
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
            nexus_file_name: None,
            nexus_is_primary: false,
            archive_md5: None,
            install_target: InstallTarget::Data,
            notes: None,
        }
    }

    // @variants: both
    #[tokio::test]
    async fn saves_all_mod_properties_in_one_transaction() -> Result<()> {
        let tracker = make_tracker().await?;
        tracker.insert_mod(&mod_entry("mod-a")).await?;
        tracker
            .record_files(&[crate::models::manifest::ModFile {
                mod_id: "mod-a".to_string(),
                game_rel_lowercase: "bin/tool.dll".to_string(),
                game_rel_original: "bin/tool.dll".to_string(),
                cache_path: "/cache/tool.dll".to_string(),
            }])
            .await?;
        let file_targets = HashMap::from([("bin/tool.dll".to_string(), InstallTarget::Root)]);

        tracker
            .update_mod_properties(ModPropertiesUpdate {
                mod_id: "mod-a",
                name: "Renamed Mod",
                notes: Some("Remember this"),
                version: Some("2 beta"),
                install_target: &InstallTarget::Root,
                file_targets: &file_targets,
                nexus_identity: Some(ModNexusIdentityUpdate {
                    mod_id: Some(42),
                    file_id: None,
                    domain: Some("skyrimspecialedition"),
                }),
            })
            .await?;

        let entry = tracker
            .list_mods("g")
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("expected saved mod"))?;
        assert_eq!(entry.name, "Renamed Mod");
        assert_eq!(entry.notes.as_deref(), Some("Remember this"));
        assert_eq!(entry.version.as_deref(), Some("2 beta"));
        assert_eq!(entry.install_target, InstallTarget::Root);
        assert_eq!(entry.nexus_mod_id, Some(42));
        assert_eq!(entry.nexus_file_id, None);
        assert_eq!(entry.nexus_domain.as_deref(), Some("skyrimspecialedition"));
        assert_eq!(
            tracker.get_mod_files("mod-a").await?[0].game_rel_lowercase,
            "../bin/tool.dll"
        );

        let file_targets = HashMap::from([("../bin/tool.dll".to_string(), InstallTarget::Root)]);
        tracker
            .update_mod_properties(ModPropertiesUpdate {
                mod_id: "mod-a",
                name: "Renamed Mod",
                notes: Some("Remember this"),
                version: None,
                install_target: &InstallTarget::Root,
                file_targets: &file_targets,
                nexus_identity: Some(ModNexusIdentityUpdate {
                    mod_id: Some(42),
                    file_id: None,
                    domain: Some("skyrimspecialedition"),
                }),
            })
            .await?;
        assert_eq!(tracker.list_mods("g").await?[0].version, None);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn rolls_back_mod_fields_when_file_target_update_fails() -> Result<()> {
        let tracker = make_tracker().await?;
        tracker.insert_mod(&mod_entry("mod-a")).await?;
        tracker
            .record_files(&[
                crate::models::manifest::ModFile {
                    mod_id: "mod-a".to_string(),
                    game_rel_lowercase: "same.txt".to_string(),
                    game_rel_original: "same.txt".to_string(),
                    cache_path: "/cache/data.txt".to_string(),
                },
                crate::models::manifest::ModFile {
                    mod_id: "mod-a".to_string(),
                    game_rel_lowercase: "../same.txt".to_string(),
                    game_rel_original: "../same.txt".to_string(),
                    cache_path: "/cache/root.txt".to_string(),
                },
            ])
            .await?;
        let file_targets = HashMap::from([("same.txt".to_string(), InstallTarget::Root)]);

        let result = tracker
            .update_mod_properties(ModPropertiesUpdate {
                mod_id: "mod-a",
                name: "Must Roll Back",
                notes: None,
                version: None,
                install_target: &InstallTarget::Data,
                file_targets: &file_targets,
                nexus_identity: None,
            })
            .await;

        assert!(result.is_err());
        let entry = tracker
            .list_mods("g")
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("expected original mod"))?;
        assert_eq!(entry.name, "Test Mod");
        Ok(())
    }
}
