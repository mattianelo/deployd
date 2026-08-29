use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};

use crate::core::game::engine_handler::EngineHandler;
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

    /// Clear all deployed file records for a game.
    pub async fn clear_deployed_files(&self, game_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM deployed_files WHERE game_id = ?")
            .bind(game_id)
            .execute(&self.pool)
            .await
            .context("Failed to clear deployed_files")?;
        Ok(())
    }

    /// Remove specific deployed file records by their lowercase path for a game.
    pub async fn remove_deployed_files(&self, game_id: &str, paths: &[&str]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for path in paths {
            sqlx::query("DELETE FROM deployed_files WHERE game_id = ? AND game_rel_lowercase = ?")
                .bind(game_id)
                .bind(*path)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await.context("Failed to remove deployed files")
    }

    /// Record the currently deployed files in a single transaction.
    pub async fn record_deployed_files(&self, game_id: &str, files: &[ModFile]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for f in files {
            sqlx::query(
                "INSERT INTO deployed_files
                    (game_id, game_rel_lowercase, game_rel_original, mod_id, cache_path)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(game_id)
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

    /// Get all currently deployed files for a game.
    pub async fn get_deployed_files(&self, game_id: &str) -> Result<Vec<ModFile>> {
        let rows = sqlx::query_as::<_, (String, String, String, String)>(
            "SELECT game_rel_lowercase, game_rel_original, mod_id, cache_path
             FROM deployed_files WHERE game_id = ?",
        )
        .bind(game_id)
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
    ///
    /// `handler` supplies the engine-specific conflict key. For most engines
    /// two files conflict only when their full paths match. Aurora overrides
    /// this to detect filename collisions within Override/ regardless of depth.
    pub async fn compute_overrides(
        &self,
        game_id: &str,
        handler: &dyn EngineHandler,
        mod_names: &HashMap<String, String>,
    ) -> Result<HashMap<String, OverrideInfo>> {
        let all_files = self.get_all_mod_files_by_priority(game_id).await?;
        dlog!(
            "[debug] compute_overrides: {} mod-file rows for game {}",
            all_files.len(),
            game_id
        );

        // Group file indices by their engine-specific conflict key.
        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, (game_rel, ..)) in all_files.iter().enumerate() {
            let key = handler.conflict_key(game_rel).to_string();
            groups.entry(key).or_default().push(i);
        }

        let mut result: HashMap<String, OverrideInfo> = HashMap::new();

        for (path_key, mut indices) in groups {
            // Directory sentinels are not real files; two mods sharing an empty folder
            // is not a conflict.
            if path_key.ends_with('/') {
                continue;
            }
            if indices.len() <= 1 {
                continue;
            }
            // Skip common files that are meaningless as conflicts (readme, license, etc.).
            if handler.is_conflict_key_ignored(&path_key) {
                continue;
            }
            // Highest priority wins; game_rel is a stable tiebreaker.
            indices.sort_by(|&a, &b| {
                all_files[b]
                    .4
                    .cmp(&all_files[a].4)
                    .then_with(|| all_files[a].0.cmp(&all_files[b].0))
            });

            // Deduplicate by mod_id: keep only the highest-priority file per mod.
            // This prevents a mod from conflicting with itself when two of its own
            // files share the same conflict key (e.g. two Override/ files with the
            // same filename but different subfolders on the Aurora engine).
            let mut seen_mods: HashSet<&str> = HashSet::new();
            let deduped: Vec<usize> = indices
                .iter()
                .copied()
                .filter(|&i| seen_mods.insert(&all_files[i].1))
                .collect();

            if deduped.len() <= 1 {
                continue;
            }

            let winner_id = &all_files[deduped[0]].1;
            let winner_name = mod_names
                .get(winner_id.as_str())
                .cloned()
                .unwrap_or_else(|| winner_id.clone());

            let winner_info = result.entry(winner_id.clone()).or_default();
            winner_info.overrides += 1;
            winner_info.override_files.push(path_key.clone());

            for &idx in &deduped[1..] {
                let loser_id = &all_files[idx].1;
                let loser_name = mod_names
                    .get(loser_id.as_str())
                    .cloned()
                    .unwrap_or_else(|| loser_id.clone());

                // Record that the winner overrides this particular mod.
                let winner_info = result.entry(winner_id.clone()).or_default();
                if !winner_info.conflicting_mod_names.contains(&loser_name) {
                    winner_info.conflicting_mod_names.push(loser_name.clone());
                }

                let loser_info = result.entry(loser_id.clone()).or_default();
                loser_info.overridden_by += 1;
                loser_info.overridden_files.push(path_key.clone());
                if !loser_info.conflicted_by_mod_names.contains(&winner_name) {
                    loser_info.conflicted_by_mod_names.push(winner_name.clone());
                }
            }
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::game::engine_handler::handler_for;
    use crate::models::game::GameEngine;
    use crate::models::mod_entry::{InstallTarget, ModEntry};

    async fn make_tracker() -> Tracker {
        let tracker = Tracker::open("sqlite::memory:")
            .await
            .expect("in-memory DB");
        sqlx::query("INSERT INTO games (id, title, path, data_subdir) VALUES ('g', 'Test', '/tmp/g', 'Data')")
            .execute(&tracker.pool)
            .await
            .expect("insert test game");
        tracker
    }

    fn mod_entry(id: &str, name: &str, priority: i32) -> ModEntry {
        ModEntry {
            id: id.to_string(),
            game_id: "g".to_string(),
            name: name.to_string(),
            archive_hash: None,
            archive_path: None,
            installed_at: None,
            enabled: true,
            priority,
            nexus_mod_id: None,
            nexus_file_id: None,
            nexus_domain: None,
            version: None,
            author: None,
            nexus_description: None,
            latest_version: None,
            nexus_file_name: None,
            nexus_is_primary: false,
            archive_md5: None,
            install_target: InstallTarget::Data,
            notes: None,
        }
    }

    async fn insert_file(tracker: &Tracker, mod_id: &str, path: &str) {
        sqlx::query(
            "INSERT INTO mod_files (mod_id, game_rel_lowercase, game_rel_original, cache_path) VALUES (?, ?, ?, '')",
        )
        .bind(mod_id)
        .bind(path)
        .bind(path)
        .execute(&tracker.pool)
        .await
        .expect("insert test file");
    }

    /// A single mod whose two Override/ files share a filename must not produce
    /// a self-conflict entry.
    #[tokio::test]
    async fn single_mod_same_filename_no_self_conflict() {
        let tracker = make_tracker().await;
        tracker
            .insert_mod(&mod_entry("a", "ModA", 1))
            .await
            .expect("insert first test mod");
        insert_file(&tracker, "a", "override/readme.xml").await;
        insert_file(&tracker, "a", "override/sub/readme.xml").await;

        let handler = handler_for(&GameEngine::Aurora);
        let mod_names = HashMap::from([("a".to_string(), "ModA".to_string())]);
        let result = tracker
            .compute_overrides("g", handler, &mod_names)
            .await
            .expect("compute single-mod overrides");

        assert!(
            result.is_empty(),
            "single mod must not conflict with itself"
        );
    }

    /// Two distinct mods sharing an Override/ filename must produce a conflict
    /// for both the winner and the loser.
    #[tokio::test]
    async fn two_mods_same_filename_conflict_reported() {
        let tracker = make_tracker().await;
        tracker
            .insert_mod(&mod_entry("a", "ModA", 2))
            .await
            .expect("insert winning test mod");
        tracker
            .insert_mod(&mod_entry("b", "ModB", 1))
            .await
            .expect("insert losing test mod");
        insert_file(&tracker, "a", "override/items.xml").await;
        insert_file(&tracker, "b", "override/sub/items.xml").await;

        let handler = handler_for(&GameEngine::Aurora);
        let mod_names = HashMap::from([
            ("a".to_string(), "ModA".to_string()),
            ("b".to_string(), "ModB".to_string()),
        ]);
        let result = tracker
            .compute_overrides("g", handler, &mod_names)
            .await
            .expect("compute conflicting overrides");

        let winner = result.get("a").expect("winner entry");
        assert_eq!(winner.overrides, 1);
        assert!(winner.conflicting_mod_names.contains(&"ModB".to_string()));

        let loser = result.get("b").expect("loser entry");
        assert_eq!(loser.overridden_by, 1);
        assert!(loser.conflicted_by_mod_names.contains(&"ModA".to_string()));
    }

    /// Override/ files named readme.txt must be silently ignored regardless of
    /// how many mods contain them.
    #[tokio::test]
    async fn ignored_filename_not_reported_as_conflict() {
        let tracker = make_tracker().await;
        tracker
            .insert_mod(&mod_entry("a", "ModA", 2))
            .await
            .expect("insert first ignored-file test mod");
        tracker
            .insert_mod(&mod_entry("b", "ModB", 1))
            .await
            .expect("insert second ignored-file test mod");
        insert_file(&tracker, "a", "override/readme.txt").await;
        insert_file(&tracker, "b", "override/readme.txt").await;

        let handler = handler_for(&GameEngine::Aurora);
        let mod_names = HashMap::from([
            ("a".to_string(), "ModA".to_string()),
            ("b".to_string(), "ModB".to_string()),
        ]);
        let result = tracker
            .compute_overrides("g", handler, &mod_names)
            .await
            .expect("compute ignored-file overrides");

        assert!(
            result.is_empty(),
            "readme.txt must not appear as a conflict"
        );
    }
}
