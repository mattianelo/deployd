use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::Tracker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VanillaBackupRecord {
    pub(crate) game_rel_path: String,
    pub(crate) backup_path: PathBuf,
}

fn normalized_deploy_path(path: &str) -> String {
    path.to_lowercase().replace('\\', "/")
}

impl Tracker {
    /// Record a vanilla backup under its original-cased restoration path.
    ///
    /// Existing records are matched by normalized deployment path so a later
    /// winner with different casing cannot detach the original backup.
    pub(crate) async fn save_vanilla_backup(
        &self,
        game_id: &str,
        canonical_path: &str,
        game_rel_path: &str,
        backup_path: &Path,
    ) -> Result<()> {
        let canonical_path = normalized_deploy_path(canonical_path);
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query_as::<_, (String,)>(
            "SELECT game_rel_path FROM vanilla_backups WHERE game_id = ?",
        )
        .bind(game_id)
        .fetch_all(&mut *tx)
        .await
        .context("Failed to find an existing vanilla backup record")?
        .into_iter()
        .find(|(stored_path,)| normalized_deploy_path(stored_path) == canonical_path);

        if let Some((stored_path,)) = existing {
            sqlx::query(
                "UPDATE vanilla_backups
                 SET backup_path = ?
                 WHERE game_id = ? AND game_rel_path = ?",
            )
            .bind(backup_path.to_string_lossy().as_ref())
            .bind(game_id)
            .bind(stored_path)
            .execute(&mut *tx)
            .await
            .context("Failed to update vanilla backup record")?;
        } else {
            sqlx::query(
                "INSERT INTO vanilla_backups (game_id, game_rel_path, backup_path)
                 VALUES (?, ?, ?)",
            )
            .bind(game_id)
            .bind(game_rel_path)
            .bind(backup_path.to_string_lossy().as_ref())
            .execute(&mut *tx)
            .await
            .context("Failed to save vanilla backup record")?;
        }

        tx.commit()
            .await
            .context("Failed to commit vanilla backup record")
    }

    pub(crate) async fn get_vanilla_backup(
        &self,
        game_id: &str,
        canonical_path: &str,
    ) -> Result<Option<VanillaBackupRecord>> {
        let canonical_path = normalized_deploy_path(canonical_path);
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT game_rel_path, backup_path FROM vanilla_backups WHERE game_id = ?",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query vanilla backup")?
        .into_iter()
        .find(|(stored_path, _)| normalized_deploy_path(stored_path) == canonical_path);
        Ok(row.map(|(game_rel_path, backup_path)| VanillaBackupRecord {
            game_rel_path,
            backup_path: PathBuf::from(backup_path),
        }))
    }

    /// Return all backup records for a game as `(game_rel_path, backup_path)` pairs.
    pub async fn get_all_vanilla_backups(&self, game_id: &str) -> Result<Vec<(String, PathBuf)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT game_rel_path, backup_path FROM vanilla_backups WHERE game_id = ?",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query vanilla backups")?;
        Ok(rows
            .into_iter()
            .map(|(r, p)| (r, PathBuf::from(p)))
            .collect())
    }

    pub(crate) async fn delete_vanilla_backup(
        &self,
        game_id: &str,
        game_rel_path: &str,
    ) -> Result<()> {
        sqlx::query("DELETE FROM vanilla_backups WHERE game_id = ? AND game_rel_path = ?")
            .bind(game_id)
            .bind(game_rel_path)
            .execute(&self.pool)
            .await
            .context("Failed to delete vanilla backup record")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::Tracker;

    #[tokio::test]
    async fn finds_legacy_backup_with_case_and_separator_changes() -> Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        sqlx::query(
            "INSERT INTO vanilla_backups (game_id, game_rel_path, backup_path)
             VALUES ('witcher-2', 'BÄSE\\Scripts.d2a', '/legacy/backup')",
        )
        .execute(&tracker.pool)
        .await?;

        let backup = tracker
            .get_vanilla_backup("witcher-2", "bäse/scripts.d2a")
            .await?
            .expect("legacy backup should match the canonical path");

        assert_eq!(backup.game_rel_path, "BÄSE\\Scripts.d2a");
        assert_eq!(backup.backup_path, std::path::Path::new("/legacy/backup"));
        Ok(())
    }
}
