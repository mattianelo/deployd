use anyhow::{Context, Result, ensure};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::core::game::mass_effect::journal::{Journal, State};
use crate::core::game::mass_effect::recipe::Recipe;

use super::Tracker;

pub(super) async fn create_tables(pool: &SqlitePool) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mele_journals (
        game_id TEXT PRIMARY KEY, id TEXT NOT NULL, committed INTEGER NOT NULL DEFAULT 0,
        document TEXT NOT NULL, CHECK(committed IN (0, 1)))",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mele_deployments (
        game_id TEXT PRIMARY KEY, document TEXT NOT NULL)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mele_recipes (
        game_id TEXT NOT NULL, profile_id TEXT NOT NULL, document TEXT NOT NULL,
        PRIMARY KEY(game_id, profile_id),
        FOREIGN KEY(profile_id) REFERENCES profiles(id) ON DELETE CASCADE)",
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

impl Tracker {
    pub(crate) async fn mele_recipe(
        &self,
        game_id: &str,
        profile_id: &str,
    ) -> Result<Option<Recipe>> {
        let document: Option<String> = sqlx::query_scalar(
            "SELECT document FROM mele_recipes WHERE game_id = ? AND profile_id = ?",
        )
        .bind(game_id)
        .bind(profile_id)
        .fetch_optional(&self.pool)
        .await?;
        document
            .map(|value| {
                let recipe: Recipe = decode(&value)?;
                recipe.validate()?;
                ensure!(
                    recipe.target.game_id() == game_id,
                    "Stored MELE recipe targets another game"
                );
                Ok(recipe)
            })
            .transpose()
    }

    pub(super) async fn mele_durable_transaction(&self) -> Result<Transaction<'_, Sqlite>> {
        let mut tx = self.pool.begin().await?;
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&mut *tx)
            .await?;
        ensure!(
            synchronous >= 2,
            "MELE deployment requires fully durable database writes; restart Deployd with its default database settings"
        );
        Ok(tx)
    }

    pub(crate) async fn ensure_no_mele_journal(&self, game_id: &str) -> Result<()> {
        let pending: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mele_journals WHERE game_id = ? OR json_extract(document, '$.family') IS NOT NULL)")
                .bind(game_id)
                .fetch_one(&self.pool)
                .await?;
        ensure!(
            !pending,
            "MELE deployment recovery must finish before this operation can continue"
        );
        Ok(())
    }

    pub(crate) async fn mele_deployment(&self, game_id: &str) -> Result<Option<State>> {
        let document: Option<String> =
            sqlx::query_scalar("SELECT document FROM mele_deployments WHERE game_id = ?")
                .bind(game_id)
                .fetch_optional(&self.pool)
                .await?;
        document.map(|value| decode(&value)).transpose()
    }

    pub(crate) async fn mele_journal(&self, game_id: &str) -> Result<Option<(Journal, bool)>> {
        let row: Option<(String, String, bool)> =
            sqlx::query_as("SELECT id, document, committed FROM mele_journals WHERE game_id = ?")
                .bind(game_id)
                .fetch_optional(&self.pool)
                .await?;
        row.map(|(id, document, committed)| {
            let journal: Journal = decode(&document)?;
            ensure!(
                journal.id == id && journal.game_id == game_id,
                "MELE deployment journal identity is damaged"
            );
            Ok((journal, committed))
        })
        .transpose()
    }

    pub(crate) async fn begin_mele_journal(&self, journal: &Journal) -> Result<()> {
        let mut tx = self.mele_durable_transaction().await?;
        let pending_family: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mele_journals WHERE json_extract(document, '$.family') IS NOT NULL)")
            .fetch_one(&mut *tx).await?;
        ensure!(
            !pending_family,
            "Shared MELE launcher recovery must finish before deploying"
        );
        if let Some(change) = &journal.family {
            change.validate(&journal.game_id, &journal.desired)?;
            super::mele_families::check(&mut tx, change, &journal.game_id).await?;
        }
        let current: Option<String> =
            sqlx::query_scalar("SELECT document FROM mele_deployments WHERE game_id = ?")
                .bind(&journal.game_id)
                .fetch_optional(&mut *tx)
                .await?;
        let current: Option<State> = current.map(|value| decode(&value)).transpose()?;
        ensure!(
            current == journal.previous,
            "MELE deployment changed while staging; rebuild the installation plan"
        );
        let baseline: String =
            sqlx::query_scalar("SELECT inventory_sha256 FROM mele_baselines WHERE game_id = ?")
                .bind(&journal.game_id)
                .fetch_one(&mut *tx)
                .await?;
        ensure!(
            baseline == journal.baseline,
            "MELE restoration baseline changed while staging"
        );
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM profiles WHERE id = ? AND game_id = ? AND is_active = 1)",
        )
        .bind(&journal.desired.profile)
        .bind(&journal.game_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(
            active || journal.family_only,
            "Select the planned MELE profile before deploying"
        );
        sqlx::query("INSERT INTO mele_journals (game_id, id, document) VALUES (?, ?, ?)")
            .bind(&journal.game_id)
            .bind(&journal.id)
            .bind(encode(journal)?)
            .execute(&mut *tx)
            .await
            .context("Cannot start MELE deployment while recovery is pending")?;
        tx.commit()
            .await
            .context("Failed to commit MELE deployment journal")
    }

    pub(crate) async fn commit_mele_journal(&self, journal: &Journal) -> Result<()> {
        let mut tx = self.mele_durable_transaction().await?;
        if let Some(change) = &journal.family {
            change.validate(&journal.game_id, &journal.desired)?;
            super::mele_families::commit(&mut tx, change, &journal.game_id).await?;
        }
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM profiles WHERE id = ? AND game_id = ? AND is_active = 1)",
        )
        .bind(&journal.desired.profile)
        .bind(&journal.game_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(
            active || journal.family_only,
            "MELE profile changed during deployment; restoring previous files"
        );
        let result = sqlx::query("UPDATE mele_journals SET committed = 1 WHERE game_id = ? AND id = ? AND committed = 0 AND document = ?")
            .bind(&journal.game_id).bind(&journal.id).bind(encode(journal)?)
            .execute(&mut *tx).await?;
        ensure!(
            result.rows_affected() == 1,
            "MELE deployment journal changed before commit"
        );
        if journal.family_only {
            return tx
                .commit()
                .await
                .context("Failed to commit shared launcher mods");
        }
        sqlx::query("INSERT INTO mele_deployments (game_id, document) VALUES (?, ?) ON CONFLICT(game_id) DO UPDATE SET document = excluded.document")
            .bind(&journal.game_id).bind(encode(&journal.desired)?)
            .execute(&mut *tx).await?;
        if let Some(recipe) = &journal.desired.recipe {
            recipe.validate()?;
            ensure!(
                recipe.target.game_id() == journal.game_id,
                "MELE recipe targets another game"
            );
            sqlx::query("INSERT INTO mele_recipes (game_id, profile_id, document) VALUES (?, ?, ?) ON CONFLICT(game_id, profile_id) DO UPDATE SET document = excluded.document")
                .bind(&journal.game_id).bind(&journal.desired.profile).bind(encode(recipe)?).execute(&mut *tx).await?;
        } else {
            sqlx::query("DELETE FROM mele_recipes WHERE game_id = ? AND profile_id = ?")
                .bind(&journal.game_id)
                .bind(&journal.desired.profile)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
            .bind(format!("last_deployed_profile_{}", journal.game_id)).bind(&journal.desired.profile)
            .execute(&mut *tx).await?;
        tx.commit()
            .await
            .context("Failed to commit MELE deployment state")
    }

    pub(crate) async fn clear_mele_journal(&self, game_id: &str, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM mele_journals WHERE game_id = ? AND id = ?")
            .bind(game_id)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn encode(value: &impl serde::Serialize) -> Result<String> {
    let value = serde_json::to_string(value)?;
    ensure!(
        value.len() <= 32 * 1024 * 1024,
        "MELE deployment journal exceeds its size limit"
    );
    Ok(value)
}

fn decode<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    ensure!(
        value.len() <= 32 * 1024 * 1024,
        "MELE deployment journal exceeds its size limit"
    );
    serde_json::from_str(value).context("Invalid MELE deployment journal or state")
}
