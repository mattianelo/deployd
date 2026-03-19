use anyhow::{Context, Result};

use crate::models::order_snapshot::{OrderSnapshot, SnapshotKind};

use super::Tracker;

impl Tracker {
    /// Save the current mod or plugin order as a named snapshot for a game.
    /// If a snapshot with the same name and kind already exists it is replaced.
    pub async fn save_order_snapshot(
        &self,
        game_id: &str,
        name: &str,
        kind: SnapshotKind,
        entries: &[(String, i32)],
    ) -> Result<()> {
        let kind_str = kind.as_str();
        let now = chrono::Utc::now().to_rfc3339();

        let mut tx = self.pool.begin().await?;

        // Upsert the snapshot record; ON CONFLICT replaces timestamp.
        let id: (String,) = sqlx::query_as(
            "INSERT INTO order_snapshots (id, game_id, name, kind, created_at)
             VALUES (?, ?, ?, ?, ?)
             ON CONFLICT(game_id, name, kind)
             DO UPDATE SET created_at = excluded.created_at
             RETURNING id",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(game_id)
        .bind(name)
        .bind(kind_str)
        .bind(&now)
        .fetch_one(&mut *tx)
        .await
        .context("Failed to upsert order snapshot")?;

        let snapshot_id = id.0;

        sqlx::query("DELETE FROM order_snapshot_entries WHERE snapshot_id = ?")
            .bind(&snapshot_id)
            .execute(&mut *tx)
            .await?;

        for (entry_id, position) in entries {
            sqlx::query(
                "INSERT INTO order_snapshot_entries (snapshot_id, entry_id, position)
                 VALUES (?, ?, ?)",
            )
            .bind(&snapshot_id)
            .bind(entry_id)
            .bind(position)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit()
            .await
            .context("Failed to commit order snapshot")?;
        Ok(())
    }

    /// List all saved snapshots for a game and kind, ordered by creation time.
    pub async fn list_order_snapshots(
        &self,
        game_id: &str,
        kind: SnapshotKind,
    ) -> Result<Vec<OrderSnapshot>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, name, created_at FROM order_snapshots
             WHERE game_id = ? AND kind = ?
             ORDER BY created_at DESC",
        )
        .bind(game_id)
        .bind(kind.as_str())
        .fetch_all(&self.pool)
        .await
        .context("Failed to list order snapshots")?;

        Ok(rows
            .into_iter()
            .map(|(id, name, created_at)| OrderSnapshot {
                id,
                name,
                kind: kind.clone(),
                created_at,
            })
            .collect())
    }

    /// Restore mod priorities from a snapshot. Mods not in the snapshot keep their
    /// current priority. Mods in the snapshot that are no longer installed are ignored.
    pub async fn restore_mod_order_snapshot(
        &self,
        snapshot_id: &str,
        game_id: &str,
    ) -> Result<()> {
        let entries: Vec<(String, i32)> = sqlx::query_as(
            "SELECT entry_id, position FROM order_snapshot_entries
             WHERE snapshot_id = ? ORDER BY position",
        )
        .bind(snapshot_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to load snapshot entries")?;

        let mut tx = self.pool.begin().await?;
        for (mod_id, position) in &entries {
            sqlx::query(
                "UPDATE mods SET priority = ? WHERE id = ? AND game_id = ?",
            )
            .bind(position)
            .bind(mod_id)
            .bind(game_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to restore mod order snapshot")?;
        Ok(())
    }

    /// Restore plugin load_order from a snapshot.
    pub async fn restore_plugin_order_snapshot(
        &self,
        snapshot_id: &str,
        game_id: &str,
    ) -> Result<()> {
        let entries: Vec<(String, i32)> = sqlx::query_as(
            "SELECT entry_id, position FROM order_snapshot_entries
             WHERE snapshot_id = ? ORDER BY position",
        )
        .bind(snapshot_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to load snapshot entries")?;

        let mut tx = self.pool.begin().await?;
        for (plugin_id, position) in &entries {
            sqlx::query(
                "UPDATE plugins SET load_order = ? WHERE id = ?
                 AND mod_id IN (SELECT id FROM mods WHERE game_id = ?)",
            )
            .bind(position)
            .bind(plugin_id)
            .bind(game_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to restore plugin order snapshot")?;
        Ok(())
    }

    /// Delete a saved snapshot.
    pub async fn delete_order_snapshot(&self, snapshot_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM order_snapshots WHERE id = ?")
            .bind(snapshot_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete order snapshot")?;
        Ok(())
    }
}
