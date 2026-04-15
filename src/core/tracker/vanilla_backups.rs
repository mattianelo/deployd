use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::Tracker;

impl Tracker {
    /// Record a vanilla file backup path for a game-relative path.
    ///
    /// Idempotent: if a record for `(game_id, game_rel_path)` already exists it is left
    /// unchanged, so re-deploying the same mod never overwrites the original backup.
    pub async fn save_vanilla_backup(
        &self,
        game_id: &str,
        game_rel_path: &str,
        backup_path: &Path,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR IGNORE INTO vanilla_backups (game_id, game_rel_path, backup_path)
             VALUES (?, ?, ?)",
        )
        .bind(game_id)
        .bind(game_rel_path)
        .bind(backup_path.to_string_lossy().as_ref())
        .execute(&self.pool)
        .await
        .context("Failed to save vanilla backup record")?;
        Ok(())
    }

    /// Return the backup path for a given game-relative path, if one was recorded.
    pub async fn get_vanilla_backup(
        &self,
        game_id: &str,
        game_rel_path: &str,
    ) -> Result<Option<PathBuf>> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT backup_path FROM vanilla_backups
             WHERE game_id = ? AND game_rel_path = ?",
        )
        .bind(game_id)
        .bind(game_rel_path)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to query vanilla backup")?;
        Ok(row.map(|(p,)| PathBuf::from(p)))
    }

    /// Return all backup records for a game as `(game_rel_path, backup_path)` pairs.
    pub async fn get_all_vanilla_backups(
        &self,
        game_id: &str,
    ) -> Result<Vec<(String, PathBuf)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT game_rel_path, backup_path FROM vanilla_backups WHERE game_id = ?",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query vanilla backups")?;
        Ok(rows.into_iter().map(|(r, p)| (r, PathBuf::from(p))).collect())
    }

    /// Remove a single backup record (call after successfully restoring one file).
    pub async fn delete_vanilla_backup(
        &self,
        game_id: &str,
        game_rel_path: &str,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM vanilla_backups WHERE game_id = ? AND game_rel_path = ?",
        )
        .bind(game_id)
        .bind(game_rel_path)
        .execute(&self.pool)
        .await
        .context("Failed to delete vanilla backup record")?;
        Ok(())
    }
}
