use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::models::plugin::Plugin;

use super::Tracker;

impl Tracker {
    /// Insert plugin records in a single transaction.
    pub async fn insert_plugins(&self, plugins: &[Plugin]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for p in plugins {
            sqlx::query(
                "INSERT INTO plugins (id, mod_id, filename, load_order, enabled)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&p.id)
            .bind(&p.mod_id)
            .bind(&p.filename)
            .bind(p.load_order)
            .bind(p.enabled)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await.context("Failed to commit plugins")?;
        Ok(())
    }

    /// Delete plugin records that no longer have a corresponding file in mod_files.
    pub async fn cleanup_orphaned_plugins(&self, game_id: &str) -> Result<()> {
        sqlx::query(
            "DELETE FROM plugins
             WHERE mod_id IN (SELECT id FROM mods WHERE game_id = ?)
               AND NOT EXISTS (
                 SELECT 1 FROM mod_files
                 WHERE mod_files.mod_id = plugins.mod_id
                   AND LOWER(mod_files.game_rel_lowercase) = LOWER(plugins.filename)
               )",
        )
        .bind(game_id)
        .execute(&self.pool)
        .await
        .context("Failed to cleanup orphaned plugins")?;
        Ok(())
    }

    /// List all plugins for a game, ordered by load_order ascending.
    ///
    /// When two mods provide the same plugin filename, only the record from the
    /// highest-priority mod is returned. Plugins belonging to a disabled mod are
    /// returned with `enabled = false`.
    pub async fn list_plugins(&self, game_id: &str) -> Result<Vec<Plugin>> {
        let rows = sqlx::query_as::<_, (String, String, String, i32, bool)>(
            "WITH ranked AS (
               SELECT p.id, p.mod_id, p.filename, p.load_order,
                      CASE WHEN m.enabled = 1 THEN p.enabled ELSE 0 END AS enabled,
                      ROW_NUMBER() OVER (
                        PARTITION BY LOWER(p.filename)
                        ORDER BY m.priority DESC
                      ) AS rn
               FROM plugins p
               JOIN mods m ON p.mod_id = m.id
               WHERE m.game_id = ?
             )
             SELECT id, mod_id, filename, load_order, enabled
             FROM ranked
             WHERE rn = 1
             ORDER BY load_order ASC",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list plugins")?;

        Ok(rows
            .into_iter()
            .map(|(id, mod_id, filename, load_order, enabled)| Plugin {
                id,
                mod_id,
                filename,
                load_order,
                enabled,
            })
            .collect())
    }

    /// Get the next plugin load_order value for a game.
    pub async fn next_load_order(&self, game_id: &str) -> Result<i32> {
        let row: (i32,) = sqlx::query_as(
            "SELECT COALESCE(MAX(p.load_order), -1) + 1
             FROM plugins p
             JOIN mods m ON p.mod_id = m.id
             WHERE m.game_id = ?",
        )
        .bind(game_id)
        .fetch_one(&self.pool)
        .await
        .context("Failed to query next load_order")?;

        Ok(row.0)
    }

    /// Batch-update plugin load_order values. Each tuple is (plugin_id, new_load_order).
    pub async fn update_plugin_order(&self, updates: &[(String, i32)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (plugin_id, load_order) in updates {
            sqlx::query("UPDATE plugins SET load_order = ? WHERE id = ?")
                .bind(load_order)
                .bind(plugin_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit plugin order updates")?;
        Ok(())
    }

    /// Toggle a plugin's enabled state.
    pub async fn toggle_plugin(&self, plugin_id: &str, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE plugins SET enabled = ? WHERE id = ?")
            .bind(enabled)
            .bind(plugin_id)
            .execute(&self.pool)
            .await
            .context("Failed to toggle plugin")?;
        Ok(())
    }

    /// Sync plugin enabled state from a parsed Plugins.txt.
    ///
    /// Only updates the `enabled` flag for plugins already in the DB.
    pub async fn sync_plugins_from_txt(
        &self,
        game_id: &str,
        txt_entries: &[(String, bool)],
    ) -> Result<()> {
        let plugins = self.list_plugins(game_id).await?;
        if plugins.is_empty() || txt_entries.is_empty() {
            return Ok(());
        }

        let txt_enabled: std::collections::HashMap<String, bool> = txt_entries
            .iter()
            .map(|(filename, enabled)| (filename.to_lowercase(), *enabled))
            .collect();

        let mut tx = self.pool.begin().await?;
        for p in &plugins {
            if let Some(&enabled) = txt_enabled.get(&p.filename.to_lowercase()) {
                sqlx::query(
                    "UPDATE plugins SET enabled = ? WHERE id = ?
                     AND mod_id IN (SELECT id FROM mods WHERE enabled = 1)",
                )
                .bind(enabled)
                .bind(&p.id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit()
            .await
            .context("Failed to commit plugin enabled sync from Plugins.txt")?;

        Ok(())
    }

    /// Delete all plugins belonging to a mod.
    pub async fn delete_plugins_for_mod(&self, mod_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM plugins WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete plugins for mod")?;
        Ok(())
    }

    /// Return (plugin_id, filename, load_order, enabled) for every plugin belonging to a mod.
    pub async fn get_plugins_for_mod(
        &self,
        mod_id: &str,
    ) -> Result<Vec<(String, String, i32, bool)>> {
        let rows: Vec<(String, String, i32, bool)> = sqlx::query_as(
            "SELECT id, filename, load_order, enabled FROM plugins WHERE mod_id = ?",
        )
        .bind(mod_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to get plugins for mod")?;
        Ok(rows)
    }

    /// Batch-update load_order and enabled state for a list of plugins.
    /// Each tuple is (plugin_id, new_load_order, new_enabled).
    pub async fn update_plugin_states(&self, updates: &[(String, i32, bool)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (plugin_id, load_order, enabled) in updates {
            sqlx::query("UPDATE plugins SET load_order = ?, enabled = ? WHERE id = ?")
                .bind(load_order)
                .bind(enabled)
                .bind(plugin_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit plugin state updates")?;
        Ok(())
    }

    /// Set `enabled` for every plugin belonging to a game's mods in one statement.
    pub async fn set_all_plugins_enabled(&self, game_id: &str, enabled: bool) -> Result<()> {
        sqlx::query(
            "UPDATE plugins SET enabled = ?
             WHERE mod_id IN (SELECT id FROM mods WHERE game_id = ?)",
        )
        .bind(enabled)
        .bind(game_id)
        .execute(&self.pool)
        .await
        .context("Failed to set all plugins enabled")?;
        Ok(())
    }

    /// Store the master-file requirements for a plugin.
    pub async fn insert_plugin_masters(&self, plugin_id: &str, masters: &[String]) -> Result<()> {
        if masters.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for master in masters {
            sqlx::query("INSERT OR IGNORE INTO plugin_masters (plugin_id, master) VALUES (?, ?)")
                .bind(plugin_id)
                .bind(master)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit plugin masters")?;
        Ok(())
    }

    /// Return a map of `plugin_id → [master, ...]` for all plugins of a game.
    pub async fn list_all_plugin_masters(
        &self,
        game_id: &str,
    ) -> Result<HashMap<String, Vec<String>>> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT pm.plugin_id, pm.master
             FROM plugin_masters pm
             JOIN plugins p ON pm.plugin_id = p.id
             JOIN mods m ON p.mod_id = m.id
             WHERE m.game_id = ?",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list plugin masters")?;

        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for (plugin_id, master) in rows {
            map.entry(plugin_id).or_default().push(master);
        }
        Ok(map)
    }
}
