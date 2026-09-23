use std::collections::HashSet;

use anyhow::{Context, Result, ensure};

use crate::models::{manifest::ModFile, mod_entry::ModEntry};

use super::Tracker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EclipseComponent {
    pub(crate) kind: String,
    pub(crate) source_key: String,
}

pub(crate) struct EclipseInstall {
    pub(crate) entry: ModEntry,
    pub(crate) component: EclipseComponent,
    pub(crate) files: Vec<ModFile>,
}

impl Tracker {
    pub(crate) async fn set_eclipse_order(
        &self,
        game: &str,
        profile: &str,
        ids: &[String],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM profiles WHERE id=? AND game_id=? AND is_active=1)",
        )
        .bind(profile)
        .bind(game)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(active, "Selected DAO profile changed; reload its mod order");
        let stored: HashSet<String> = sqlx::query_scalar("SELECT id FROM mods WHERE game_id=?")
            .bind(game)
            .fetch_all(&mut *tx)
            .await?
            .into_iter()
            .collect();
        ensure!(
            stored.len() == ids.len() && stored == ids.iter().cloned().collect(),
            "DAO library changed; reload before reordering"
        );
        for (priority, id) in ids.iter().enumerate() {
            let priority = i32::try_from(priority)?;
            sqlx::query("UPDATE mods SET priority=? WHERE id=? AND game_id=?")
                .bind(priority)
                .bind(id)
                .bind(game)
                .execute(&mut *tx)
                .await?;
            sqlx::query("INSERT INTO profile_mods(profile_id,mod_id,enabled,priority) SELECT ?,id,enabled,priority FROM mods WHERE id=? ON CONFLICT(profile_id,mod_id) DO UPDATE SET priority=excluded.priority")
                .bind(profile).bind(id).execute(&mut *tx).await?;
        }
        tx.commit().await.context("Could not save DAO mod order")
    }

    pub(crate) async fn eclipse_component(&self, mod_id: &str) -> Result<Option<EclipseComponent>> {
        let row = sqlx::query_as::<_, (String, String)>(
            "SELECT kind, source_key FROM eclipse_packages WHERE mod_id = ?",
        )
        .bind(mod_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(kind, source_key)| EclipseComponent { kind, source_key }))
    }

    pub(crate) async fn eclipse_override_ids(&self, game_id: &str) -> Result<HashSet<String>> {
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT m.id FROM mods m LEFT JOIN eclipse_packages p ON p.mod_id = m.id
             WHERE m.game_id = ? AND (p.kind = 'override' OR
             (p.mod_id IS NULL
              AND EXISTS (SELECT 1 FROM mod_files f WHERE f.mod_id = m.id
                          AND lower(f.game_rel_original) LIKE 'packages/core/override/%')
              AND NOT EXISTS (SELECT 1 FROM mod_files f WHERE f.mod_id = m.id
                              AND lower(f.game_rel_original) LIKE 'addins/%')))",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(ids.into_iter().collect())
    }

    pub(crate) async fn save_eclipse_install(
        &self,
        installs: &[EclipseInstall],
        replacing: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        ensure!(!installs.is_empty(), "No DAO components selected");
        for install in installs {
            let entry = &install.entry;
            sqlx::query("INSERT INTO mods (id, game_id, name, archive_hash, archive_path, installed_at, enabled, priority, nexus_mod_id, nexus_file_id, nexus_domain, install_target, version, author, notes) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)")
                .bind(&entry.id).bind(&entry.game_id).bind(&entry.name)
                .bind(&entry.archive_hash).bind(&entry.archive_path).bind(&entry.installed_at)
                .bind(entry.enabled).bind(entry.priority).bind(entry.nexus_mod_id)
                .bind(entry.nexus_file_id).bind(&entry.nexus_domain)
                .bind(entry.install_target.to_string()).bind(&entry.version).bind(&entry.author)
                .bind(&entry.notes).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO eclipse_packages (mod_id, kind, source_key) VALUES (?, ?, ?)")
                .bind(&entry.id)
                .bind(&install.component.kind)
                .bind(&install.component.source_key)
                .execute(&mut *tx)
                .await?;
            for file in &install.files {
                ensure!(
                    file.mod_id == entry.id,
                    "DAO file belongs to another component"
                );
                sqlx::query("INSERT INTO mod_files (mod_id, game_rel_lowercase, game_rel_original, cache_path) VALUES (?, ?, ?, ?)")
                    .bind(&file.mod_id).bind(&file.game_rel_lowercase).bind(&file.game_rel_original)
                    .bind(&file.cache_path).execute(&mut *tx).await?;
            }
        }
        if let Some(old_id) = replacing {
            for install in installs {
                sqlx::query("INSERT INTO profile_mods (profile_id, mod_id, enabled, priority) SELECT profile_id, ?, enabled, priority FROM profile_mods WHERE mod_id = ?")
                    .bind(&install.entry.id).bind(old_id).execute(&mut *tx).await?;
            }
            sqlx::query("DELETE FROM profile_mods WHERE mod_id = ?")
                .bind(old_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM mod_files WHERE mod_id = ?")
                .bind(old_id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM mods WHERE id = ?")
                .bind(old_id)
                .execute(&mut *tx)
                .await?;
        }
        for install in installs {
            sqlx::query("INSERT OR IGNORE INTO profile_mods (profile_id, mod_id, enabled, priority) SELECT id, ?, ?, ? FROM profiles WHERE game_id = ? AND is_active = 1")
                .bind(&install.entry.id).bind(install.entry.enabled).bind(install.entry.priority).bind(&install.entry.game_id).execute(&mut *tx).await?;
        }
        tx.commit().await.context("Could not commit DAO components")
    }

    pub(crate) async fn set_eclipse_enabled(
        &self,
        ids: &[String],
        enabled: bool,
        profile_id: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for id in ids {
            sqlx::query("UPDATE mods SET enabled = ? WHERE id = ?")
                .bind(enabled)
                .bind(id)
                .execute(&mut *tx)
                .await?;
            if let Some(profile_id) = profile_id {
                sqlx::query("INSERT INTO profile_mods (profile_id, mod_id, enabled, priority) SELECT ?, id, enabled, priority FROM mods WHERE id = ? ON CONFLICT(profile_id, mod_id) DO UPDATE SET enabled=excluded.enabled")
                    .bind(profile_id).bind(id).execute(&mut *tx).await?;
            }
        }
        tx.commit()
            .await
            .context("Could not save DAO enabled state")
    }

    pub(crate) async fn remove_eclipse_mod(&self, mod_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM profile_mods WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM mod_files WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM mods WHERE id = ?")
            .bind(mod_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await.context("Could not remove DAO component")
    }
}
