use anyhow::{Context, Result, ensure};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::core::game::mass_effect::family::{Change, Family};

use super::Tracker;

pub(super) async fn create_tables(pool: &SqlitePool) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS mele_families (location_id INTEGER PRIMARY KEY REFERENCES folder_locations(id), document TEXT NOT NULL)")
        .execute(pool).await?;
    Ok(())
}

impl Tracker {
    pub(crate) async fn mele_family_for_root(
        &self,
        root: &std::path::Path,
    ) -> Result<Option<Family>> {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT f.document FROM mele_families f JOIN folder_locations l ON l.id=f.location_id WHERE l.root=?",
        )
        .bind(root.to_string_lossy().as_ref())
        .fetch_optional(&self.pool)
        .await?;
        value.map(|value| decode(&value)).transpose()
    }

    pub(crate) async fn mele_family_for_game(&self, game_id: &str) -> Result<Option<Family>> {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT f.document FROM mele_families f JOIN game_locations g ON g.location_id=f.location_id WHERE g.game_id=? AND g.role='game'",
        )
        .bind(game_id)
        .fetch_optional(&self.pool)
        .await?;
        value.map(|value| decode(&value)).transpose()
    }

    pub(crate) async fn mele_family(&self, location_id: i64) -> Result<Option<Family>> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT document FROM mele_families WHERE location_id=?")
                .bind(location_id)
                .fetch_optional(&self.pool)
                .await?;
        value.map(|value| decode(&value)).transpose()
    }

    #[cfg(test)]
    pub(crate) async fn record_mele_family(&self, location_id: i64, family: &Family) -> Result<()> {
        family.validate()?;
        let mut tx = self.mele_durable_transaction().await?;
        sqlx::query("INSERT INTO mele_families(location_id,document) VALUES (?,?) ON CONFLICT(location_id) DO NOTHING")
            .bind(location_id).bind(serde_json::to_string(family)?).execute(&mut *tx).await?;
        let stored: String =
            sqlx::query_scalar("SELECT document FROM mele_families WHERE location_id=?")
                .bind(location_id)
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            decode(&stored)? == *family,
            "Shared MELE launcher state changed; rebuild the deployment plan"
        );
        tx.commit()
            .await
            .context("Failed to preserve MELE launcher baseline")
    }
    pub(crate) async fn extend_mele_family(
        &self,
        location: i64,
        previous: &Family,
        desired: &Family,
    ) -> Result<()> {
        desired.validate()?;
        previous.validate_extension(desired)?;
        let mut tx = self.mele_durable_transaction().await?;
        let changed =
            sqlx::query("UPDATE mele_families SET document=? WHERE location_id=? AND document=?")
                .bind(serde_json::to_string(desired)?)
                .bind(location)
                .bind(serde_json::to_string(previous)?)
                .execute(&mut *tx)
                .await?;
        ensure!(
            changed.rows_affected() == 1,
            "Shared launcher state changed; reopen launcher mods"
        );
        tx.commit()
            .await
            .context("Failed to preserve launcher originals")
    }
}

pub(super) async fn insert(
    tx: &mut Transaction<'_, Sqlite>,
    game_id: &str,
    family: &Family,
) -> Result<()> {
    family.validate()?;
    let location_id: i64 = sqlx::query_scalar(
        "SELECT location_id FROM game_locations WHERE game_id=? AND role='game'",
    )
    .bind(game_id)
    .fetch_one(&mut **tx)
    .await?;
    let inserted = sqlx::query(
        "INSERT INTO mele_families(location_id,document) VALUES (?,?) ON CONFLICT(location_id) DO NOTHING",
    )
    .bind(location_id)
    .bind(serde_json::to_string(family)?)
    .execute(&mut **tx)
    .await?;
    if inserted.rows_affected() == 0 {
        let stored: String =
            sqlx::query_scalar("SELECT document FROM mele_families WHERE location_id=?")
                .bind(location_id)
                .fetch_one(&mut **tx)
                .await?;
        ensure!(
            decode(&stored)? == *family,
            "MELE launcher baseline already exists and cannot be replaced"
        );
    }
    Ok(())
}

pub(super) async fn check(
    tx: &mut Transaction<'_, Sqlite>,
    change: &Change,
    game_id: &str,
) -> Result<()> {
    let location: i64 = sqlx::query_scalar(
        "SELECT location_id FROM game_locations WHERE game_id=? AND role='game'",
    )
    .bind(game_id)
    .fetch_one(&mut **tx)
    .await?;
    ensure!(
        location == change.location_id,
        "MELE launcher location changed during deployment"
    );
    let stored: String =
        sqlx::query_scalar("SELECT document FROM mele_families WHERE location_id=?")
            .bind(location)
            .fetch_one(&mut **tx)
            .await?;
    ensure!(
        decode(&stored)? == change.previous,
        "Shared MELE launcher ownership changed during deployment"
    );
    Ok(())
}

pub(super) async fn commit(
    tx: &mut Transaction<'_, Sqlite>,
    change: &Change,
    game_id: &str,
) -> Result<()> {
    check(tx, change, game_id).await?;
    sqlx::query("UPDATE mele_families SET document=? WHERE location_id=?")
        .bind(serde_json::to_string(&change.desired)?)
        .bind(change.location_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn decode(document: &str) -> Result<Family> {
    ensure!(
        document.len() <= 16 * 1024 * 1024,
        "MELE family record exceeds its size limit"
    );
    let family: Family =
        serde_json::from_str(document).context("Invalid MELE launcher ownership")?;
    family.validate()?;
    Ok(family)
}
