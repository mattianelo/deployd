use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::dlog;
use crate::models::manifest::ModFile;
use crate::models::mod_entry::InstallTarget;

use super::{OverrideInfo, Tracker};

impl Tracker {
    /// Record all file entries for a mod in a single transaction.
    pub async fn record_files(&self, files: &[ModFile]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for f in files {
            sqlx::query(
                "INSERT INTO mod_files (mod_id, game_rel_lowercase, game_rel_original, cache_path)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&f.mod_id)
            .bind(&f.game_rel_lowercase)
            .bind(&f.game_rel_original)
            .bind(&f.cache_path)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await.context("Failed to commit mod_files")?;
        Ok(())
    }

    /// Upsert file records for a mod (INSERT OR REPLACE).
    pub async fn upsert_mod_files(&self, files: &[ModFile]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for f in files {
            sqlx::query(
                "INSERT OR REPLACE INTO mod_files
                 (mod_id, game_rel_lowercase, game_rel_original, cache_path)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&f.mod_id)
            .bind(&f.game_rel_lowercase)
            .bind(&f.game_rel_original)
            .bind(&f.cache_path)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit mod_files upsert")?;
        Ok(())
    }

    /// Delete all file records for a mod.
    pub async fn delete_mod_files(&self, mod_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM mod_files WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete mod_files")?;
        Ok(())
    }

    /// Get (game_rel_lowercase, game_rel_original, cache_path) for every plugin file
    /// (.esp/.esm/.esl) currently listed in `deployed_files` for the given game.
    pub async fn get_deployed_plugin_files(
        &self,
        game_id: &str,
    ) -> Result<Vec<(String, String, std::path::PathBuf)>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT df.game_rel_lowercase, df.game_rel_original, df.cache_path
             FROM deployed_files df
             JOIN mods m ON df.mod_id = m.id
             WHERE m.game_id = ?
               AND (df.game_rel_lowercase LIKE '%.esp'
                 OR df.game_rel_lowercase LIKE '%.esm'
                 OR df.game_rel_lowercase LIKE '%.esl')",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query deployed plugin files")?;

        Ok(rows
            .into_iter()
            .map(|(rel, orig, path)| (rel, orig, std::path::PathBuf::from(path)))
            .collect())
    }

    /// Get all tracked lowercase relative paths for all mods of a game.
    pub async fn get_tracked_rel_paths(
        &self,
        game_id: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT mf.game_rel_lowercase FROM mod_files mf
             JOIN mods m ON mf.mod_id = m.id
             WHERE m.game_id = ?",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query tracked rel paths")?;

        Ok(rows.into_iter().map(|(s,)| s).collect())
    }

    /// Get all mod files for enabled mods of a game, ordered by priority descending.
    /// Returns (game_rel_lowercase, mod_id, cache_path, game_rel_original, priority) tuples.
    pub async fn get_all_mod_files_by_priority(
        &self,
        game_id: &str,
    ) -> Result<Vec<(String, String, String, String, i32)>> {
        let rows = sqlx::query_as::<_, (String, String, String, String, i32)>(
            "SELECT mf.game_rel_lowercase, mf.mod_id, mf.cache_path, mf.game_rel_original, m.priority
             FROM mod_files mf
             JOIN mods m ON mf.mod_id = m.id
             WHERE m.game_id = ? AND m.enabled = 1
             ORDER BY mf.game_rel_lowercase, m.priority DESC",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query mod files by priority")?;

        Ok(rows)
    }

    /// Fetch all files tracked for a specific mod, ordered by path.
    pub async fn get_mod_files(&self, mod_id: &str) -> Result<Vec<ModFile>> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT game_rel_lowercase, game_rel_original, cache_path
             FROM mod_files WHERE mod_id = ?
             ORDER BY game_rel_lowercase ASC",
        )
        .bind(mod_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query mod files")?;

        Ok(rows
            .into_iter()
            .map(|(lowercase, original, cache)| ModFile {
                mod_id: mod_id.to_string(),
                game_rel_lowercase: lowercase,
                game_rel_original: original,
                cache_path: cache,
            })
            .collect())
    }

    /// Update per-file install targets for a mod.
    ///
    /// `changes` maps the **current** `game_rel_lowercase` to the desired `InstallTarget`.
    pub async fn update_file_targets(
        &self,
        mod_id: &str,
        changes: &HashMap<String, InstallTarget>,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        for (current_lowercase, target) in changes {
            match target {
                InstallTarget::Root if !current_lowercase.starts_with("../") => {
                    sqlx::query(
                        "UPDATE mod_files
                         SET game_rel_lowercase = '../' || game_rel_lowercase,
                             game_rel_original  = '../' || game_rel_original
                         WHERE mod_id = ? AND game_rel_lowercase = ?",
                    )
                    .bind(mod_id)
                    .bind(current_lowercase)
                    .execute(&mut *tx)
                    .await
                    .context("Failed to set file target to root")?;
                }
                InstallTarget::Data if current_lowercase.starts_with("../") => {
                    sqlx::query(
                        "UPDATE mod_files
                         SET game_rel_lowercase = SUBSTR(game_rel_lowercase, 4),
                             game_rel_original  = SUBSTR(game_rel_original, 4)
                         WHERE mod_id = ? AND game_rel_lowercase = ?",
                    )
                    .bind(mod_id)
                    .bind(current_lowercase)
                    .execute(&mut *tx)
                    .await
                    .context("Failed to set file target to data")?;
                }
                _ => {} // already in correct state — no update needed
            }
        }

        tx.commit()
            .await
            .context("Failed to commit file target updates")?;
        Ok(())
    }

    /// Clear all deployed file records (used before re-deploying).
    pub async fn clear_deployed_files(&self) -> Result<()> {
        sqlx::query("DELETE FROM deployed_files")
            .execute(&self.pool)
            .await
            .context("Failed to clear deployed_files")?;
        Ok(())
    }

    /// Remove specific deployed file records by their lowercase path.
    pub async fn remove_deployed_files(&self, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for path in paths {
            sqlx::query("DELETE FROM deployed_files WHERE game_rel_lowercase = ?")
                .bind(*path)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Failed to remove deployed files")
    }

    /// Record the currently deployed files in a single transaction.
    pub async fn record_deployed_files(&self, files: &[ModFile]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for f in files {
            sqlx::query(
                "INSERT INTO deployed_files (game_rel_lowercase, game_rel_original, mod_id, cache_path)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&f.game_rel_lowercase)
            .bind(&f.game_rel_original)
            .bind(&f.mod_id)
            .bind(&f.cache_path)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit deployed_files")?;
        Ok(())
    }

    /// Get all currently deployed files.
    pub async fn get_deployed_files(&self) -> Result<Vec<ModFile>> {
        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT game_rel_lowercase, game_rel_original, mod_id, cache_path FROM deployed_files",
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to query deployed_files")?;

        Ok(rows
            .into_iter()
            .map(
                |(game_rel_lowercase, game_rel_original, mod_id, cache_path)| ModFile {
                    mod_id,
                    game_rel_lowercase,
                    game_rel_original,
                    cache_path,
                },
            )
            .collect())
    }

    /// Compute per-mod override stats for enabled mods of a game.
    /// Returns a map of mod_id -> OverrideInfo with counts and file paths.
    pub async fn compute_overrides(&self, game_id: &str) -> Result<HashMap<String, OverrideInfo>> {
        let all_files = self.get_all_mod_files_by_priority(game_id).await?;
        dlog!(
            "[debug] compute_overrides: {} mod-file rows for game {}",
            all_files.len(),
            game_id
        );

        let mut result: HashMap<String, OverrideInfo> = HashMap::new();
        let mut last_path: Option<&str> = None;
        let mut group: Vec<&str> = Vec::new();

        let flush = |group: &[&str], path: &str, result: &mut HashMap<String, OverrideInfo>| {
            if group.len() > 1 {
                let info = result
                    .entry(group[0].to_string())
                    .or_insert_with(|| OverrideInfo {
                        overrides: 0,
                        overridden_by: 0,
                        override_files: Vec::new(),
                        overridden_files: Vec::new(),
                    });
                info.overrides += 1;
                info.override_files.push(path.to_string());
                for loser in &group[1..] {
                    let info = result
                        .entry(loser.to_string())
                        .or_insert_with(|| OverrideInfo {
                            overrides: 0,
                            overridden_by: 0,
                            override_files: Vec::new(),
                            overridden_files: Vec::new(),
                        });
                    info.overridden_by += 1;
                    info.overridden_files.push(path.to_string());
                }
            }
        };

        for (game_rel, mod_id, _, _, _) in &all_files {
            if last_path == Some(game_rel.as_str()) {
                group.push(mod_id);
            } else {
                if let Some(prev_path) = last_path {
                    flush(&group, prev_path, &mut result);
                }
                group.clear();
                group.push(mod_id);
                last_path = Some(game_rel);
            }
        }
        if let Some(prev_path) = last_path {
            flush(&group, prev_path, &mut result);
        }

        Ok(result)
    }
}
