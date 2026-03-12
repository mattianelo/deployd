use anyhow::{Context, Result};

use super::Tracker;

impl Tracker {
    /// Record the pre-mod state of a game's file tree as the vanilla baseline.
    ///
    /// Each entry is `(game_rel_lowercase, size_bytes, mtime_secs)`. Idempotent:
    /// skips recording if a snapshot already exists for this game.
    pub async fn ensure_vanilla_snapshot(
        &self,
        game_id: &str,
        entries: &[(String, u64, i64)],
    ) -> Result<()> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM vanilla_files WHERE game_id = ?")
                .bind(game_id)
                .fetch_one(&self.pool)
                .await
                .context("Failed to count vanilla_files")?;

        if count > 0 {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for (path, size, mtime) in entries {
            sqlx::query(
                "INSERT OR IGNORE INTO vanilla_files
                 (game_id, game_rel_lowercase, file_size, mtime_secs)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(game_id)
            .bind(path)
            .bind(*size as i64)
            .bind(mtime)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit vanilla snapshot")?;
        Ok(())
    }

    /// Return a map of `game_rel_lowercase → (size_bytes, mtime_secs)` for the
    /// vanilla snapshot.
    pub async fn get_vanilla_metadata(
        &self,
        game_id: &str,
    ) -> Result<std::collections::HashMap<String, (u64, i64)>> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT game_rel_lowercase, file_size, mtime_secs
             FROM vanilla_files WHERE game_id = ?",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query vanilla_files")?;

        Ok(rows
            .into_iter()
            .map(|(path, size, mtime)| (path, (size as u64, mtime)))
            .collect())
    }

    /// Delete the existing vanilla snapshot for a game and record a fresh one.
    pub async fn reset_vanilla_snapshot(
        &self,
        game_id: &str,
        entries: &[(String, u64, i64)],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM vanilla_files WHERE game_id = ?")
            .bind(game_id)
            .execute(&mut *tx)
            .await
            .context("Failed to delete old vanilla snapshot")?;
        for (path, size, mtime) in entries {
            sqlx::query(
                "INSERT OR IGNORE INTO vanilla_files
                 (game_id, game_rel_lowercase, file_size, mtime_secs)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(game_id)
            .bind(path)
            .bind(*size as i64)
            .bind(mtime)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit new vanilla snapshot")
    }

    /// Upsert vanilla baseline entries for specific files.
    ///
    /// Use when individual files are incorrectly flagged as external changes
    /// (e.g. after a partial reinstall). Overwrites existing rows for the same key.
    pub async fn update_vanilla_entries(
        &self,
        game_id: &str,
        entries: &[(String, u64, i64)],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (path, size, mtime) in entries {
            sqlx::query(
                "INSERT OR REPLACE INTO vanilla_files
                 (game_id, game_rel_lowercase, file_size, mtime_secs)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(game_id)
            .bind(path)
            .bind(*size as i64)
            .bind(mtime)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to update vanilla entries")
    }
}
