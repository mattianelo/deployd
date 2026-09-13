use anyhow::{Context, Result, ensure};
use sqlx::SqlitePool;

use crate::core::game::mass_effect::library::Record;
use crate::models::{manifest::ModFile, mod_entry::ModEntry};

use super::Tracker;

pub(super) async fn create_tables(pool: &SqlitePool) -> Result<()> {
    sqlx::query("CREATE TABLE IF NOT EXISTS mele_packages (mod_id TEXT PRIMARY KEY REFERENCES mods(id) ON DELETE CASCADE, document TEXT NOT NULL)").execute(pool).await?;
    Ok(())
}

impl Tracker {
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

    pub(crate) async fn remove_mele_packages(&self, game_id: &str, ids: &[String]) -> Result<()> {
        let mut tx = self.mele_durable_transaction().await?;
        for id in ids {
            let belongs: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mods JOIN mele_packages ON mods.id = mele_packages.mod_id WHERE mods.id = ? AND mods.game_id = ?)")
                .bind(id).bind(game_id).fetch_one(&mut *tx).await?;
            ensure!(
                belongs,
                "The MELE mod library changed; reload before removing mods"
            );
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
