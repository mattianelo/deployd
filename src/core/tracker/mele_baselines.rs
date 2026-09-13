use anyhow::{Context, Result, ensure};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::core::game::mass_effect::baseline::{Baseline, BaselineFile};

use super::Tracker;

pub(super) async fn create_tables(pool: &SqlitePool) -> Result<()> {
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mele_baselines (
            game_id TEXT PRIMARY KEY,
            format_version INTEGER NOT NULL CHECK(format_version = 1),
            trust TEXT NOT NULL CHECK(trust = 'assumed_at_setup'),
            inventory_sha256 TEXT NOT NULL,
            captured_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mele_baseline_files (
            game_id TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            size INTEGER NOT NULL CHECK(size >= 0),
            modified INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            PRIMARY KEY(game_id, relative_path),
            FOREIGN KEY(game_id) REFERENCES mele_baselines(game_id) ON DELETE CASCADE
        )",
    )
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mele_originals (
            game_id TEXT NOT NULL,
            baseline_sha256 TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            PRIMARY KEY(game_id, relative_path),
            FOREIGN KEY(game_id, relative_path) REFERENCES mele_baseline_files(game_id, relative_path)
        )",
    )
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

impl Tracker {
    pub(crate) async fn record_mele_originals(
        &self,
        baseline: &Baseline,
        files: &[BaselineFile],
    ) -> Result<()> {
        baseline.validate()?;
        let mut transaction = self.pool.begin().await?;
        let identity: String =
            sqlx::query_scalar("SELECT inventory_sha256 FROM mele_baselines WHERE game_id = ?")
                .bind(&baseline.game_id)
                .fetch_one(&mut *transaction)
                .await?;
        ensure!(
            identity == baseline.sha256,
            "MELE baseline changed before originals could be recorded"
        );
        for file in files {
            ensure!(
                baseline
                    .files
                    .binary_search_by(|entry| entry.relative.cmp(&file.relative))
                    .ok()
                    .is_some_and(|index| baseline.files[index] == *file),
                "Original file does not belong to the MELE baseline"
            );
            sqlx::query("INSERT INTO mele_originals (game_id, baseline_sha256, relative_path) VALUES (?, ?, ?) ON CONFLICT(game_id, relative_path) DO NOTHING")
                .bind(&baseline.game_id).bind(&baseline.sha256).bind(&file.relative)
                .execute(&mut *transaction).await?;
            let stored: String = sqlx::query_scalar("SELECT baseline_sha256 FROM mele_originals WHERE game_id = ? AND relative_path = ?")
                .bind(&baseline.game_id).bind(&file.relative).fetch_one(&mut *transaction).await?;
            ensure!(
                stored == baseline.sha256,
                "MELE original belongs to a different baseline"
            );
        }
        transaction
            .commit()
            .await
            .context("Failed to commit MELE original-file ownership")
    }

    pub(crate) async fn mele_originals(&self, game_id: &str) -> Result<Vec<(String, String)>> {
        sqlx::query_as("SELECT relative_path, baseline_sha256 FROM mele_originals WHERE game_id = ? ORDER BY relative_path")
            .bind(game_id).fetch_all(&self.pool).await.context("Failed to load MELE original-file ownership")
    }

    pub(crate) async fn load_mele_baseline(&self, game_id: &str) -> Result<Option<Baseline>> {
        let header: Option<(i64, String, String)> = sqlx::query_as(
            "SELECT format_version, trust, inventory_sha256 FROM mele_baselines WHERE game_id = ?",
        )
        .bind(game_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((version, trust, sha256)) = header else {
            return Ok(None);
        };
        ensure!(
            version == 1 && trust == "assumed_at_setup",
            "Unsupported MELE restoration baseline format"
        );
        let rows: Vec<(String, i64, i64, String)> = sqlx::query_as(
            "SELECT relative_path, size, modified, sha256 FROM mele_baseline_files
             WHERE game_id = ? ORDER BY relative_path",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await?;
        let files = rows
            .into_iter()
            .map(|(relative, size, modified, sha256)| {
                Ok(BaselineFile {
                    relative,
                    size: u64::try_from(size).context("Invalid MELE baseline file size")?,
                    modified,
                    sha256,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let baseline = Baseline {
            game_id: game_id.to_string(),
            files,
            sha256,
        };
        baseline.validate()?;
        Ok(Some(baseline))
    }

    pub(crate) async fn save_mele_baseline(&self, baseline: &Baseline) -> Result<()> {
        let mut transaction = self.pool.begin().await?;
        insert(&mut transaction, baseline).await?;
        transaction
            .commit()
            .await
            .context("Failed to commit the MELE restoration baseline")
    }
}

pub(super) async fn insert(
    transaction: &mut Transaction<'_, Sqlite>,
    baseline: &Baseline,
) -> Result<()> {
    baseline.validate()?;
    let inserted = sqlx::query(
        "INSERT INTO mele_baselines (game_id, format_version, trust, inventory_sha256)
         VALUES (?, 1, 'assumed_at_setup', ?) ON CONFLICT(game_id) DO NOTHING",
    )
    .bind(&baseline.game_id)
    .bind(&baseline.sha256)
    .execute(&mut **transaction)
    .await?;
    if inserted.rows_affected() == 0 {
        let existing: String =
            sqlx::query_scalar("SELECT inventory_sha256 FROM mele_baselines WHERE game_id = ?")
                .bind(&baseline.game_id)
                .fetch_one(&mut **transaction)
                .await?;
        ensure!(
            existing == baseline.sha256,
            "MELE restoration baseline already exists; it cannot be replaced by the current installation"
        );
        return Ok(());
    }
    for file in &baseline.files {
        sqlx::query(
            "INSERT INTO mele_baseline_files (game_id, relative_path, size, modified, sha256)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&baseline.game_id)
        .bind(&file.relative)
        .bind(file.size as i64)
        .bind(file.modified)
        .bind(&file.sha256)
        .execute(&mut **transaction)
        .await?;
        let detection_path = match file.relative.strip_prefix("BioGame/") {
            Some(relative) => relative.to_lowercase(),
            None => format!("../{}", file.relative.to_lowercase()),
        };
        sqlx::query(
            "INSERT OR IGNORE INTO vanilla_files (game_id, game_rel_lowercase, file_size, mtime_secs)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&baseline.game_id).bind(detection_path).bind(file.size as i64).bind(file.modified)
        .execute(&mut **transaction).await?;
    }
    Ok(())
}
