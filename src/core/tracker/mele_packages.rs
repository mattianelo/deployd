use anyhow::{Context, Result, ensure};
use sqlx::SqlitePool;

use crate::core::game::mass_effect::library::Record;
use crate::models::{manifest::ModFile, mod_entry::ModEntry};

use super::Tracker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LauncherComponentLink {
    pub(crate) legacy_id: String,
    pub(crate) game_id: String,
    pub(crate) mod_id: String,
    pub(crate) adopted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LauncherReconciliationCandidate {
    pub(crate) game_id: String,
    pub(crate) mod_id: String,
    pub(crate) archive_sha256: String,
    pub(crate) archive_path: Option<String>,
}

pub(crate) struct LauncherComponentAssociation<'a> {
    pub(crate) location_id: i64,
    pub(crate) legacy_id: &'a str,
    pub(crate) source_sha256: &'a str,
    pub(crate) archive_sha256: Option<&'a str>,
    pub(crate) game_id: &'a str,
    pub(crate) mod_id: &'a str,
}

pub(super) async fn create_tables(pool: &SqlitePool) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS mele_packages (mod_id TEXT PRIMARY KEY REFERENCES mods(id) ON DELETE CASCADE, document TEXT NOT NULL)").execute(pool).await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mele_launcher_component_links (
            location_id INTEGER NOT NULL REFERENCES folder_locations(id) ON DELETE CASCADE,
            legacy_id TEXT NOT NULL,
            game_id TEXT NOT NULL,
            mod_id TEXT NOT NULL,
            source_sha256 TEXT NOT NULL,
            archive_sha256 TEXT,
            adopted INTEGER NOT NULL DEFAULT 0 CHECK(adopted IN (0, 1)),
            PRIMARY KEY(location_id, legacy_id, game_id, mod_id)
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mele_launcher_reconciliation_candidates (
            location_id INTEGER NOT NULL REFERENCES folder_locations(id) ON DELETE CASCADE,
            game_id TEXT NOT NULL,
            mod_id TEXT NOT NULL,
            archive_sha256 TEXT NOT NULL,
            archive_path TEXT,
            PRIMARY KEY(location_id, game_id, mod_id)
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

impl Tracker {
    pub(crate) async fn bind_mele_launcher_component(
        &self,
        association: &LauncherComponentAssociation<'_>,
        expected: &Record,
        desired: &Record,
    ) -> Result<()> {
        expected.validate()?;
        desired.validate()?;
        ensure!(
            association.location_id > 0
                && expected.package.id == association.mod_id
                && desired.package.id == association.mod_id
                && expected.target.game_id() == association.game_id
                && desired.target.game_id() == association.game_id
                && desired.launcher_source_sha256() == Some(association.source_sha256),
            "Invalid MELE launcher component association"
        );
        let mut tx = self.mele_durable_transaction().await?;
        let bound: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM game_locations WHERE location_id=? AND game_id=? AND role='game')",
        )
        .bind(association.location_id)
        .bind(association.game_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(
            bound,
            "MELE launcher component belongs to another installation"
        );
        let stored: String =
            sqlx::query_scalar("SELECT document FROM mele_packages WHERE mod_id=?")
                .bind(association.mod_id)
                .fetch_one(&mut *tx)
                .await?;
        let current: Record =
            serde_json::from_str(&stored).context("Invalid MELE library record")?;
        current.validate()?;
        ensure!(
            current == *expected || current == *desired,
            "The MELE package changed during launcher component reconciliation"
        );
        if current != *desired {
            let update =
                sqlx::query("UPDATE mele_packages SET document=? WHERE mod_id=? AND document=?")
                    .bind(serde_json::to_string(desired)?)
                    .bind(association.mod_id)
                    .bind(stored)
                    .execute(&mut *tx)
                    .await?;
            ensure!(
                update.rows_affected() == 1,
                "The MELE package changed during launcher component reconciliation"
            );
        }
        sqlx::query(
            "INSERT INTO mele_launcher_component_links(location_id,legacy_id,game_id,mod_id,source_sha256,archive_sha256)
             VALUES (?,?,?,?,?,?) ON CONFLICT(location_id,legacy_id,game_id,mod_id) DO UPDATE SET
             archive_sha256=COALESCE(mele_launcher_component_links.archive_sha256,excluded.archive_sha256)",
        )
        .bind(association.location_id)
        .bind(association.legacy_id)
        .bind(association.game_id)
        .bind(association.mod_id)
        .bind(association.source_sha256)
        .bind(association.archive_sha256)
        .execute(&mut *tx)
        .await?;
        let link: (String, Option<String>) = sqlx::query_as(
            "SELECT source_sha256,archive_sha256 FROM mele_launcher_component_links
             WHERE location_id=? AND legacy_id=? AND game_id=? AND mod_id=?",
        )
        .bind(association.location_id)
        .bind(association.legacy_id)
        .bind(association.game_id)
        .bind(association.mod_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(
            link.0 == association.source_sha256
                && (link.1.as_deref() == association.archive_sha256
                    || link.1.is_none()
                    || association.archive_sha256.is_none()),
            "MELE launcher component association changed"
        );
        tx.commit()
            .await
            .context("Could not preserve MELE launcher component ownership")
    }

    pub(crate) async fn mele_launcher_component_links(
        &self,
        location_id: i64,
    ) -> Result<Vec<LauncherComponentLink>> {
        let rows: Vec<(String, String, String, bool)> = sqlx::query_as(
            "SELECT legacy_id,game_id,mod_id,adopted
             FROM mele_launcher_component_links WHERE location_id=?
             ORDER BY legacy_id,game_id,mod_id",
        )
        .bind(location_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(legacy_id, game_id, mod_id, adopted)| LauncherComponentLink {
                    legacy_id,
                    game_id,
                    mod_id,
                    adopted,
                },
            )
            .collect())
    }

    pub(crate) async fn mele_launcher_reconciliation_candidates(
        &self,
        location_id: i64,
    ) -> Result<Vec<LauncherReconciliationCandidate>> {
        let rows: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT game_id,mod_id,archive_sha256,archive_path
             FROM mele_launcher_reconciliation_candidates WHERE location_id=?
             ORDER BY game_id,mod_id",
        )
        .bind(location_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(game_id, mod_id, archive_sha256, archive_path)| {
                LauncherReconciliationCandidate {
                    game_id,
                    mod_id,
                    archive_sha256,
                    archive_path,
                }
            })
            .collect())
    }

    pub(crate) async fn clear_mele_launcher_reconciliation_candidates(
        &self,
        location_id: i64,
    ) -> Result<()> {
        let mut tx = self.mele_durable_transaction().await?;
        sqlx::query("DELETE FROM mele_launcher_reconciliation_candidates WHERE location_id=?")
            .bind(location_id)
            .execute(&mut *tx)
            .await?;
        tx.commit()
            .await
            .context("Could not clear completed MELE launcher reconciliation")
    }

    pub(crate) async fn bind_removed_mele_launcher_component(
        &self,
        location_id: i64,
        candidate: &LauncherReconciliationCandidate,
        legacy_id: &str,
        source_sha256: &str,
    ) -> Result<()> {
        ensure!(
            ["mass-effect-le1", "mass-effect-le2", "mass-effect-le3"]
                .contains(&candidate.game_id.as_str()),
            "Removed launcher ownership names an unsupported MELE game"
        );
        let mut tx = self.mele_durable_transaction().await?;
        let stored: (String, Option<String>) = sqlx::query_as(
            "SELECT archive_sha256,archive_path
             FROM mele_launcher_reconciliation_candidates
             WHERE location_id=? AND game_id=? AND mod_id=?",
        )
        .bind(location_id)
        .bind(&candidate.game_id)
        .bind(&candidate.mod_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(
            stored.0 == candidate.archive_sha256 && stored.1 == candidate.archive_path,
            "The removed MELE parent changed during launcher component reconciliation"
        );
        sqlx::query(
            "INSERT INTO mele_launcher_component_links(location_id,legacy_id,game_id,mod_id,source_sha256,archive_sha256)
             VALUES (?,?,?,?,?,?) ON CONFLICT(location_id,legacy_id,game_id,mod_id) DO UPDATE SET
             archive_sha256=COALESCE(mele_launcher_component_links.archive_sha256,excluded.archive_sha256)",
        )
        .bind(location_id)
        .bind(legacy_id)
        .bind(&candidate.game_id)
        .bind(&candidate.mod_id)
        .bind(source_sha256)
        .bind(&candidate.archive_sha256)
        .execute(&mut *tx)
        .await?;
        let link: (String, Option<String>) = sqlx::query_as(
            "SELECT source_sha256,archive_sha256 FROM mele_launcher_component_links
             WHERE location_id=? AND legacy_id=? AND game_id=? AND mod_id=?",
        )
        .bind(location_id)
        .bind(legacy_id)
        .bind(&candidate.game_id)
        .bind(&candidate.mod_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(
            link.0 == source_sha256 && link.1.as_deref() == Some(candidate.archive_sha256.as_str()),
            "Removed MELE launcher component association changed"
        );
        let deleted = sqlx::query(
            "DELETE FROM mele_launcher_reconciliation_candidates
             WHERE location_id=? AND game_id=? AND mod_id=? AND archive_sha256=?",
        )
        .bind(location_id)
        .bind(&candidate.game_id)
        .bind(&candidate.mod_id)
        .bind(&candidate.archive_sha256)
        .execute(&mut *tx)
        .await?;
        ensure!(
            deleted.rows_affected() == 1,
            "The removed MELE parent changed during launcher component reconciliation"
        );
        tx.commit()
            .await
            .context("Could not preserve removed MELE launcher ownership")
    }

    pub(crate) async fn mele_package(&self, mod_id: &str) -> Result<Option<Record>> {
        let value: Option<String> =
            sqlx::query_scalar("SELECT document FROM mele_packages WHERE mod_id = ?")
                .bind(mod_id)
                .fetch_optional(&self.pool)
                .await?;
        value
            .map(|value| {
                let record: Record =
                    serde_json::from_str(&value).context("Invalid MELE library record")?;
                record.validate()?;
                ensure!(record.package.id == mod_id, "MELE library identity changed");
                Ok(record)
            })
            .transpose()
    }

    pub(crate) async fn remove_mele_packages(
        &self,
        location_id: i64,
        game_id: &str,
        ids: &[String],
        retain_reconciliation: bool,
    ) -> Result<()> {
        let mut tx = self.mele_durable_transaction().await?;
        let bound: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM game_locations WHERE location_id=? AND game_id=? AND role='game')",
        )
        .bind(location_id)
        .bind(game_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(bound, "MELE packages belong to another installation");
        for id in ids {
            let belongs: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mods JOIN mele_packages ON mods.id = mele_packages.mod_id WHERE mods.id = ? AND mods.game_id = ?)")
                .bind(id).bind(game_id).fetch_one(&mut *tx).await?;
            ensure!(
                belongs,
                "The MELE mod library changed; reload before removing mods"
            );
            if retain_reconciliation {
                let row: (String, Option<String>, Option<String>) = sqlx::query_as(
                    "SELECT mele_packages.document,mods.archive_hash,mods.archive_path
                     FROM mele_packages JOIN mods ON mods.id=mele_packages.mod_id
                     WHERE mods.id=? AND mods.game_id=?",
                )
                .bind(id)
                .bind(game_id)
                .fetch_one(&mut *tx)
                .await?;
                let record: Record =
                    serde_json::from_str(&row.0).context("Invalid MELE library record")?;
                record.validate()?;
                if record.launcher().is_none()
                    && let Some(archive_sha256) = row.1
                {
                    sqlx::query(
                        "INSERT INTO mele_launcher_reconciliation_candidates(location_id,game_id,mod_id,archive_sha256,archive_path)
                         VALUES (?,?,?,?,?) ON CONFLICT(location_id,game_id,mod_id) DO UPDATE SET
                         archive_sha256=excluded.archive_sha256,archive_path=excluded.archive_path",
                    )
                    .bind(location_id)
                    .bind(game_id)
                    .bind(id)
                    .bind(archive_sha256)
                    .bind(row.2)
                    .execute(&mut *tx)
                    .await?;
                }
            }
            sqlx::query("DELETE FROM mod_files WHERE mod_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM profile_mods WHERE mod_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM mods WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Could not remove MELE library entries")
    }

    pub(crate) async fn register_mele_package(
        &self,
        entry: &ModEntry,
        record: &Record,
        files: &[ModFile],
        replacing: bool,
    ) -> Result<()> {
        record.validate()?;
        ensure!(
            record.package.id == entry.id && record.target.game_id() == entry.game_id,
            "MELE library targets another game"
        );
        let mut tx = self.mele_durable_transaction().await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mods WHERE id = ?)")
            .bind(&entry.id)
            .fetch_one(&mut *tx)
            .await?;
        ensure!(
            exists == replacing,
            "The mod library changed; inspect the archive again"
        );
        if replacing {
            let game: String = sqlx::query_scalar("SELECT game_id FROM mods WHERE id = ?")
                .bind(&entry.id)
                .fetch_one(&mut *tx)
                .await?;
            ensure!(
                game == entry.game_id,
                "Cannot replace a mod from another game"
            );
            let known: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mele_packages WHERE mod_id = ?)")
                    .bind(&entry.id)
                    .fetch_one(&mut *tx)
                    .await?;
            ensure!(known, "Cannot replace an unrecognized MELE library entry");
        }
        sqlx::query("INSERT INTO mods (id, game_id, name, archive_hash, archive_path, installed_at, enabled, priority, nexus_mod_id, nexus_file_id, nexus_domain, version, author, install_target) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'data') ON CONFLICT(id) DO UPDATE SET name=excluded.name, archive_hash=excluded.archive_hash, archive_path=excluded.archive_path, installed_at=excluded.installed_at, nexus_mod_id=excluded.nexus_mod_id, nexus_file_id=excluded.nexus_file_id, nexus_domain=excluded.nexus_domain, version=excluded.version, author=excluded.author")
            .bind(&entry.id).bind(&entry.game_id).bind(&entry.name).bind(&entry.archive_hash).bind(&entry.archive_path).bind(&entry.installed_at).bind(entry.enabled).bind(entry.priority).bind(entry.nexus_mod_id).bind(entry.nexus_file_id).bind(&entry.nexus_domain).bind(&entry.version).bind(&entry.author).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO mele_packages (mod_id, document) VALUES (?, ?) ON CONFLICT(mod_id) DO UPDATE SET document=excluded.document")
            .bind(&entry.id).bind(serde_json::to_string(record)?).execute(&mut *tx).await?;
        sqlx::query("DELETE FROM mod_files WHERE mod_id = ?")
            .bind(&entry.id)
            .execute(&mut *tx)
            .await?;
        for file in files {
            ensure!(
                file.mod_id == entry.id,
                "MELE file record belongs to another mod"
            );
            sqlx::query("INSERT INTO mod_files (mod_id, game_rel_lowercase, game_rel_original, cache_path) VALUES (?, ?, ?, ?)")
                .bind(&file.mod_id).bind(&file.game_rel_lowercase).bind(&file.game_rel_original).bind(&file.cache_path).execute(&mut *tx).await?;
        }
        tx.commit()
            .await
            .context("Could not save the MELE mod and its package recipe")
    }
}
