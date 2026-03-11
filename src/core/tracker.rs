use std::collections::HashMap;

use anyhow::{Context, Result};

use crate::dlog;
use sqlx::sqlite::{SqlitePool, SqlitePoolOptions};

use crate::models::manifest::ModFile;
use crate::models::mod_entry::{InstallTarget, ModEntry};
use crate::models::plugin::Plugin;
use crate::models::profile::{Profile, SaveMode};

#[derive(Clone)]
pub struct Tracker {
    pool: SqlitePool,
}

impl std::fmt::Debug for Tracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracker").finish_non_exhaustive()
    }
}

/// A game record loaded from the `games` table, including optional user overrides.
#[derive(Debug, Clone)]
pub struct PersistedGame {
    pub id: String,
    pub title: String,
    pub path: std::path::PathBuf,
    pub data_subdir: String,
    /// "bethesda" or "redengine"
    pub engine: String,
    /// User-specified Wine prefix; `None` means auto-detect.
    pub wine_prefix: Option<std::path::PathBuf>,
    /// `true` if the user manually added this game (not auto-detected).
    pub custom: bool,
}

/// Per-mod override stats: counts + file paths.
#[derive(Debug, Default)]
pub struct OverrideInfo {
    pub overrides: usize,
    pub overridden_by: usize,
    pub override_files: Vec<String>,
    pub overridden_files: Vec<String>,
}

impl Tracker {
    /// Open (or create) the SQLite database and ensure tables exist.
    pub async fn open(db_url: &str) -> Result<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect(db_url)
            .await
            .context("Failed to open SQLite database")?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS games (
                id TEXT PRIMARY KEY,
                title TEXT,
                path TEXT UNIQUE,
                data_subdir TEXT DEFAULT 'Data'
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS mods (
                id TEXT PRIMARY KEY,
                game_id TEXT,
                name TEXT,
                archive_hash TEXT,
                installed_at TEXT,
                enabled BOOLEAN DEFAULT TRUE,
                priority INTEGER DEFAULT 0
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS mod_files (
                mod_id TEXT,
                game_rel_lowercase TEXT,
                game_rel_original TEXT NOT NULL,
                cache_path TEXT,
                PRIMARY KEY (mod_id, game_rel_lowercase)
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS plugins (
                id TEXT PRIMARY KEY,
                mod_id TEXT NOT NULL,
                filename TEXT NOT NULL,
                load_order INTEGER NOT NULL,
                enabled BOOLEAN DEFAULT TRUE,
                FOREIGN KEY (mod_id) REFERENCES mods(id) ON DELETE CASCADE
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS plugin_masters (
                plugin_id TEXT NOT NULL,
                master    TEXT NOT NULL,
                PRIMARY KEY (plugin_id, master),
                FOREIGN KEY (plugin_id) REFERENCES plugins(id) ON DELETE CASCADE
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS deployed_files (
                game_rel_lowercase TEXT PRIMARY KEY,
                game_rel_original TEXT NOT NULL,
                mod_id TEXT NOT NULL,
                cache_path TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS profiles (
                id TEXT PRIMARY KEY,
                game_id TEXT NOT NULL,
                name TEXT NOT NULL,
                is_active BOOLEAN DEFAULT FALSE,
                UNIQUE(game_id, name)
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS profile_mods (
                profile_id TEXT NOT NULL,
                mod_id TEXT NOT NULL,
                enabled BOOLEAN DEFAULT TRUE,
                priority INTEGER DEFAULT 0,
                PRIMARY KEY (profile_id, mod_id),
                FOREIGN KEY (profile_id) REFERENCES profiles(id) ON DELETE CASCADE
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS profile_plugins (
                profile_id TEXT NOT NULL,
                plugin_id TEXT NOT NULL,
                enabled BOOLEAN DEFAULT TRUE,
                load_order INTEGER DEFAULT 0,
                PRIMARY KEY (profile_id, plugin_id),
                FOREIGN KEY (profile_id) REFERENCES profiles(id) ON DELETE CASCADE
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS tools (
                id TEXT PRIMARY KEY,
                game_id TEXT NOT NULL,
                name TEXT NOT NULL,
                exe_path TEXT NOT NULL,
                icon_name TEXT DEFAULT 'application-x-executable-symbolic',
                custom_args TEXT DEFAULT '',
                sort_order INTEGER DEFAULT 0,
                working_dir TEXT DEFAULT ''
            )",
        )
        .execute(&pool)
        .await?;

        // Migration: add working_dir to existing databases that predate this column.
        let _ = sqlx::query("ALTER TABLE tools ADD COLUMN working_dir TEXT DEFAULT ''")
            .execute(&pool)
            .await;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS download_entries (
                id TEXT PRIMARY KEY,
                mod_name TEXT NOT NULL,
                archive_path TEXT,
                nexus_mod_id INTEGER,
                nexus_file_id INTEGER,
                nexus_domain TEXT,
                game_domain TEXT,
                metadata_fetched BOOLEAN DEFAULT FALSE
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS mod_groups (
                id       TEXT PRIMARY KEY,
                game_id  TEXT NOT NULL,
                name     TEXT NOT NULL,
                position REAL NOT NULL DEFAULT 0.0,
                collapsed INTEGER NOT NULL DEFAULT 0
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS vanilla_files (
                game_id            TEXT NOT NULL,
                game_rel_lowercase TEXT NOT NULL,
                file_size          INTEGER,
                mtime_secs         INTEGER,
                PRIMARY KEY (game_id, game_rel_lowercase)
            )",
        )
        .execute(&pool)
        .await?;

        // Migration: add wine_prefix, engine, custom columns to games table
        Self::migrate_games_columns(&pool).await?;

        // Migration: add Nexus metadata columns to mods table
        Self::migrate_nexus_columns(&pool).await?;

        // Migration: add file name columns to download_entries
        Self::migrate_download_columns(&pool).await?;

        // Migration: add size/mtime columns to vanilla_files for replacement detection
        Self::migrate_vanilla_files_columns(&pool).await?;

        // Migration: add group_id column to mods
        Self::migrate_group_columns(&pool).await?;

        // Migration: add install_target column to mods
        Self::migrate_install_target_column(&pool).await?;

        // Migration: fix version/latest_version columns populated by old code
        if let Err(e) = Self::migrate_version_columns(&pool).await {
            eprintln!("Version column migration failed (non-fatal): {e}");
        }

        // Backfill plugin masters for mods installed before the plugin_masters table was added
        if let Err(e) = Self::backfill_plugin_masters(&pool).await {
            eprintln!("Plugin master backfill failed (non-fatal): {e}");
        }

        // One-time backfill: set download_entries.status = 'installed' for any entry
        // whose (nexus_mod_id, nexus_file_id) matches an installed mod. This fixes rows
        // that were created before the status column existed (they received DEFAULT 'downloaded')
        // and rows that were incorrectly reset by the over-eager reset-on-removal bug.
        if let Err(e) = Self::backfill_download_statuses(&pool).await {
            eprintln!("Download status backfill failed (non-fatal): {e}");
        }

        // One-time backfill: populate archive_hash for mods that were installed before
        // the hash was recorded. Joins mods with download_entries via nexus IDs and hashes
        // any archive files that are still present on disk.
        if let Err(e) = Self::backfill_archive_hashes(&pool).await {
            eprintln!("Archive hash backfill failed (non-fatal): {e}");
        }

        // Migration: add save_mode column to profiles table
        let _ =
            sqlx::query("ALTER TABLE profiles ADD COLUMN save_mode TEXT NOT NULL DEFAULT 'global'")
                .execute(&pool)
                .await;

        // Indexes on frequently-joined foreign-key columns (safe to add to existing DBs).
        for stmt in &[
            "CREATE INDEX IF NOT EXISTS idx_mods_game_id      ON mods(game_id)",
            "CREATE INDEX IF NOT EXISTS idx_mod_files_mod_id  ON mod_files(mod_id)",
            "CREATE INDEX IF NOT EXISTS idx_plugins_mod_id    ON plugins(mod_id)",
            "CREATE INDEX IF NOT EXISTS idx_deployed_mod_id   ON deployed_files(mod_id)",
            "CREATE INDEX IF NOT EXISTS idx_profile_mods_prof ON profile_mods(profile_id)",
        ] {
            let _ = sqlx::query(stmt).execute(&pool).await;
        }

        Ok(Self { pool })
    }

    /// Add wine_prefix, engine, and custom columns to the games table if they don't exist.
    async fn migrate_games_columns(pool: &SqlitePool) -> Result<()> {
        let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(games)")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
            .collect();
        let existing: std::collections::HashSet<&str> =
            columns.iter().map(|(n,)| n.as_str()).collect();
        for (col, typedef) in &[
            ("wine_prefix", "TEXT"),
            ("engine", "TEXT DEFAULT 'bethesda'"),
            ("custom", "INTEGER DEFAULT 0"),
            ("hidden", "INTEGER DEFAULT 0"),
        ] {
            if !existing.contains(*col) {
                let _ = sqlx::query(&format!("ALTER TABLE games ADD COLUMN {col} {typedef}"))
                    .execute(pool)
                    .await;
            }
        }
        Ok(())
    }

    /// Add Nexus metadata columns if they don't exist yet.
    async fn migrate_nexus_columns(pool: &SqlitePool) -> Result<()> {
        // Check which columns already exist
        let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(mods)")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
            .collect();

        let existing: std::collections::HashSet<&str> =
            columns.iter().map(|(name,)| name.as_str()).collect();

        let new_columns = [
            ("nexus_mod_id", "INTEGER"),
            ("nexus_file_id", "INTEGER"),
            ("nexus_domain", "TEXT"),
            ("version", "TEXT"),
            ("author", "TEXT"),
            ("nexus_description", "TEXT"),
            ("latest_version", "TEXT"),
        ];

        for (col, col_type) in &new_columns {
            if !existing.contains(*col) {
                let sql = format!("ALTER TABLE mods ADD COLUMN {col} {col_type}");
                sqlx::query(&sql).execute(pool).await?;
            }
        }

        Ok(())
    }

    /// One-time fix for mods installed with the old code that stored the Nexus mod-page
    /// version (latest available) in the `version` column instead of `latest_version`.
    ///
    /// Strategy:
    ///  1. Copy `version` → `latest_version` where `latest_version` is NULL (old mods never
    ///     had `latest_version` set — the old `update_mod_nexus_metadata` wrote to `version`).
    ///  2. Clear `version` for those same mods (version = latest_version after step 1 signals
    ///     that the value came from the mod page, not the installed file). This leaves `version`
    ///     NULL so the UI shows nothing rather than the wrong value; future installs via the new
    ///     code will populate it correctly from the specific Nexus file entry.
    async fn migrate_version_columns(pool: &SqlitePool) -> Result<()> {
        // Step 1: backfill latest_version from the incorrectly-stored version
        sqlx::query(
            "UPDATE mods SET latest_version = version \
             WHERE latest_version IS NULL AND version IS NOT NULL",
        )
        .execute(pool)
        .await?;
        // Step 2: clear version where it equals latest_version — these are mods where
        // the old code wrote the Nexus mod-page version into the wrong column.
        sqlx::query(
            "UPDATE mods SET version = NULL \
             WHERE version IS NOT NULL AND version = latest_version",
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Add nexus_file_name and nexus_is_primary columns to download_entries if missing.
    async fn migrate_download_columns(pool: &SqlitePool) -> Result<()> {
        let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(download_entries)")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
            .collect();

        let existing: std::collections::HashSet<&str> =
            columns.iter().map(|(name,)| name.as_str()).collect();

        let new_columns = [
            ("nexus_file_name", "TEXT"),
            ("nexus_is_primary", "BOOLEAN DEFAULT FALSE"),
            ("status", "TEXT DEFAULT 'downloaded'"),
            ("archive_hash", "TEXT"),
        ];

        for (col, col_type) in &new_columns {
            if !existing.contains(*col) {
                let sql = format!("ALTER TABLE download_entries ADD COLUMN {col} {col_type}");
                sqlx::query(&sql).execute(pool).await?;
            }
        }

        Ok(())
    }

    /// Add group_id column to mods table if it doesn't exist yet.
    async fn migrate_group_columns(pool: &SqlitePool) -> Result<()> {
        let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(mods)")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
            .collect();

        let has_group_id = columns.iter().any(|(name,)| name == "group_id");
        if !has_group_id {
            sqlx::query("ALTER TABLE mods ADD COLUMN group_id TEXT")
                .execute(pool)
                .await?;
        }

        Ok(())
    }

    /// Add install_target column to mods table if it doesn't exist yet.
    async fn migrate_install_target_column(pool: &SqlitePool) -> Result<()> {
        let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(mods)")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
            .collect();

        let has_col = columns.iter().any(|(name,)| name == "install_target");
        if !has_col {
            sqlx::query("ALTER TABLE mods ADD COLUMN install_target TEXT NOT NULL DEFAULT 'data'")
                .execute(pool)
                .await?;
        }

        Ok(())
    }

    /// Add file_size and mtime_secs columns to vanilla_files if missing.
    ///
    /// Rows created before this migration have NULL size/mtime and cannot be used
    /// for replacement detection, so they are deleted to force a fresh snapshot.
    async fn migrate_vanilla_files_columns(pool: &SqlitePool) -> Result<()> {
        let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(vanilla_files)")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
            .collect();

        let existing: std::collections::HashSet<&str> =
            columns.iter().map(|(name,)| name.as_str()).collect();

        for col in &["file_size INTEGER", "mtime_secs INTEGER"] {
            let col_name = col.split_whitespace().next().unwrap_or("");
            if !existing.contains(col_name) {
                let sql = format!("ALTER TABLE vanilla_files ADD COLUMN {col}");
                sqlx::query(&sql).execute(pool).await?;
            }
        }

        // Remove rows that still have NULL attributes — they are from the old schema
        // and will be re-snapshotted with proper values on next game load.
        sqlx::query("DELETE FROM vanilla_files WHERE file_size IS NULL OR mtime_secs IS NULL")
            .execute(pool)
            .await?;

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Mod CRUD
    // -----------------------------------------------------------------------

    /// Insert a new mod record.
    pub async fn insert_mod(&self, entry: &ModEntry) -> Result<()> {
        sqlx::query(
            "INSERT INTO mods (id, game_id, name, archive_hash, installed_at, enabled, priority,
                               nexus_mod_id, nexus_file_id, nexus_domain, install_target)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&entry.id)
        .bind(&entry.game_id)
        .bind(&entry.name)
        .bind(&entry.archive_hash)
        .bind(&entry.installed_at)
        .bind(entry.enabled)
        .bind(entry.priority)
        .bind(entry.nexus_mod_id)
        .bind(entry.nexus_file_id)
        .bind(&entry.nexus_domain)
        .bind(entry.install_target.to_string())
        .execute(&self.pool)
        .await
        .context("Failed to insert mod entry")?;
        Ok(())
    }

    /// Delete a mod entry.
    pub async fn delete_mod(&self, mod_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM mods WHERE id = ?")
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete mod")?;
        Ok(())
    }

    /// List all mods for a given game, ordered by priority ascending (lowest priority first).
    pub async fn list_mods(&self, game_id: &str) -> Result<Vec<ModEntry>> {
        let rows = sqlx::query_as::<
            _,
            (
                String,
                String,
                String,
                Option<String>,
                Option<String>,
                bool,
                i32,
                Option<i64>,
                Option<i64>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
                Option<String>,
            ),
        >(
            "SELECT id, game_id, name, archive_hash, installed_at, enabled, priority,
                    nexus_mod_id, nexus_file_id, nexus_domain, version, author,
                    nexus_description, latest_version, install_target
             FROM mods WHERE game_id = ? ORDER BY priority ASC",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list mods")?;

        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    game_id,
                    name,
                    archive_hash,
                    installed_at,
                    enabled,
                    priority,
                    nexus_mod_id,
                    nexus_file_id,
                    nexus_domain,
                    version,
                    author,
                    nexus_description,
                    latest_version,
                    install_target,
                )| {
                    ModEntry {
                        id,
                        game_id,
                        name,
                        archive_hash,
                        installed_at,
                        enabled,
                        priority,
                        nexus_mod_id,
                        nexus_file_id,
                        nexus_domain,
                        version,
                        author,
                        nexus_description,
                        latest_version,
                        install_target: InstallTarget::from(install_target.as_deref()),
                    }
                },
            )
            .collect())
    }

    /// Get the next priority value for a game (one higher than current max).
    pub async fn next_priority(&self, game_id: &str) -> Result<i32> {
        let row: (i32,) =
            sqlx::query_as("SELECT COALESCE(MAX(priority), -1) + 1 FROM mods WHERE game_id = ?")
                .bind(game_id)
                .fetch_one(&self.pool)
                .await
                .context("Failed to query next priority")?;

        Ok(row.0)
    }

    /// Batch-update priority values. Each tuple is (mod_id, new_priority).
    pub async fn update_priorities(&self, updates: &[(String, i32)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (mod_id, priority) in updates {
            sqlx::query("UPDATE mods SET priority = ? WHERE id = ?")
                .bind(priority)
                .bind(mod_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit priority updates")?;
        Ok(())
    }

    /// Toggle a mod's enabled state.
    pub async fn toggle_mod(&self, mod_id: &str, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE mods SET enabled = ? WHERE id = ?")
            .bind(enabled)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to toggle mod")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Mod files
    // -----------------------------------------------------------------------

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
    /// Used when merging new files into an existing mod — existing rows for the
    /// same (mod_id, game_rel_lowercase) key are overwritten with the new cache path.
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
    /// Uses the actual on-disk path (`game_rel_original`) so inode comparisons in the
    /// external-modification detector work correctly on case-sensitive Linux filesystems.
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
    /// Used by the external-file detector to skip already-managed files.
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
    /// `changes` maps the **current** `game_rel_lowercase` (as stored in the DB)
    /// to the desired `InstallTarget`.  Only files whose target differs from their
    /// current `../` prefix state are rewritten; others are skipped.
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

    // -----------------------------------------------------------------------
    // Plugin CRUD
    // -----------------------------------------------------------------------

    /// Insert plugin records in a single transaction.
    pub async fn insert_plugins(&self, plugins: &[Plugin]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for p in plugins {
            sqlx::query(
                "INSERT INTO plugins (id, mod_id, filename, load_order, enabled)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&p.id)
            .bind(&p.mod_id)
            .bind(&p.filename)
            .bind(p.load_order)
            .bind(p.enabled)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await.context("Failed to commit plugins")?;
        Ok(())
    }

    /// Delete plugin records that no longer have a corresponding file in mod_files.
    ///
    /// This happens when a user manually deletes a plugin file from the mod cache and
    /// then triggers a reload. Without this cleanup the plugin would remain visible in
    /// the plugin order panel even though it can't be deployed.
    pub async fn cleanup_orphaned_plugins(&self, game_id: &str) -> Result<()> {
        sqlx::query(
            "DELETE FROM plugins
             WHERE mod_id IN (SELECT id FROM mods WHERE game_id = ?)
               AND NOT EXISTS (
                 SELECT 1 FROM mod_files
                 WHERE mod_files.mod_id = plugins.mod_id
                   AND LOWER(mod_files.game_rel_lowercase) = LOWER(plugins.filename)
               )",
        )
        .bind(game_id)
        .execute(&self.pool)
        .await
        .context("Failed to cleanup orphaned plugins")?;
        Ok(())
    }

    /// List all plugins for a game, ordered by load_order ascending.
    ///
    /// When two mods provide the same plugin filename, only the record from the
    /// highest-priority mod (highest `priority` value = wins conflicts) is returned.
    ///
    /// Plugins belonging to a disabled mod are returned with `enabled = false` so
    /// they appear unchecked in the UI and are never written as active in Plugins.txt.
    pub async fn list_plugins(&self, game_id: &str) -> Result<Vec<Plugin>> {
        let rows = sqlx::query_as::<_, (String, String, String, i32, bool)>(
            "WITH ranked AS (
               SELECT p.id, p.mod_id, p.filename, p.load_order,
                      CASE WHEN m.enabled = 1 THEN p.enabled ELSE 0 END AS enabled,
                      ROW_NUMBER() OVER (
                        PARTITION BY LOWER(p.filename)
                        ORDER BY m.priority DESC
                      ) AS rn
               FROM plugins p
               JOIN mods m ON p.mod_id = m.id
               WHERE m.game_id = ?
             )
             SELECT id, mod_id, filename, load_order, enabled
             FROM ranked
             WHERE rn = 1
             ORDER BY load_order ASC",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list plugins")?;

        Ok(rows
            .into_iter()
            .map(|(id, mod_id, filename, load_order, enabled)| Plugin {
                id,
                mod_id,
                filename,
                load_order,
                enabled,
            })
            .collect())
    }

    /// Get the next plugin load_order value for a game.
    pub async fn next_load_order(&self, game_id: &str) -> Result<i32> {
        let row: (i32,) = sqlx::query_as(
            "SELECT COALESCE(MAX(p.load_order), -1) + 1
             FROM plugins p
             JOIN mods m ON p.mod_id = m.id
             WHERE m.game_id = ?",
        )
        .bind(game_id)
        .fetch_one(&self.pool)
        .await
        .context("Failed to query next load_order")?;

        Ok(row.0)
    }

    /// Batch-update plugin load_order values. Each tuple is (plugin_id, new_load_order).
    pub async fn update_plugin_order(&self, updates: &[(String, i32)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (plugin_id, load_order) in updates {
            sqlx::query("UPDATE plugins SET load_order = ? WHERE id = ?")
                .bind(load_order)
                .bind(plugin_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit plugin order updates")?;
        Ok(())
    }

    /// Toggle a plugin's enabled state.
    pub async fn toggle_plugin(&self, plugin_id: &str, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE plugins SET enabled = ? WHERE id = ?")
            .bind(enabled)
            .bind(plugin_id)
            .execute(&self.pool)
            .await
            .context("Failed to toggle plugin")?;
        Ok(())
    }

    /// Sync plugin enabled state from a parsed Plugins.txt.
    ///
    /// Only updates the `enabled` flag for plugins already in the DB.
    /// `load_order` is intentionally left untouched — the DB order is the
    /// source of truth and must not be overwritten by Plugins.txt on every reload.
    pub async fn sync_plugins_from_txt(
        &self,
        game_id: &str,
        txt_entries: &[(String, bool)],
    ) -> Result<()> {
        let plugins = self.list_plugins(game_id).await?;
        if plugins.is_empty() || txt_entries.is_empty() {
            return Ok(());
        }

        // Build lookup: lowercase filename → enabled
        let txt_enabled: std::collections::HashMap<String, bool> = txt_entries
            .iter()
            .map(|(filename, enabled)| (filename.to_lowercase(), *enabled))
            .collect();

        let mut tx = self.pool.begin().await?;
        for p in &plugins {
            if let Some(&enabled) = txt_enabled.get(&p.filename.to_lowercase()) {
                // Only update plugins whose parent mod is currently enabled.
                // Deployd writes disabled-mod plugins as disabled in Plugins.txt via the
                // CASE cascade in list_plugins; reading that state back would permanently
                // corrupt plugins.enabled and prevent re-enabling after the mod is turned
                // back on (e.g. on app restart after a deploy-with-disabled-mods).
                sqlx::query(
                    "UPDATE plugins SET enabled = ? WHERE id = ?
                     AND mod_id IN (SELECT id FROM mods WHERE enabled = 1)",
                )
                .bind(enabled)
                .bind(&p.id)
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit()
            .await
            .context("Failed to commit plugin enabled sync from Plugins.txt")?;

        Ok(())
    }

    /// Delete all plugins belonging to a mod.
    pub async fn delete_plugins_for_mod(&self, mod_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM plugins WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete plugins for mod")?;
        Ok(())
    }

    /// Return (plugin_id, filename, load_order, enabled) for every plugin belonging to a mod.
    pub async fn get_plugins_for_mod(
        &self,
        mod_id: &str,
    ) -> Result<Vec<(String, String, i32, bool)>> {
        let rows: Vec<(String, String, i32, bool)> = sqlx::query_as(
            "SELECT id, filename, load_order, enabled FROM plugins WHERE mod_id = ?",
        )
        .bind(mod_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to get plugins for mod")?;
        Ok(rows)
    }

    /// Batch-update load_order and enabled state for a list of plugins.
    /// Each tuple is (plugin_id, new_load_order, new_enabled).
    pub async fn update_plugin_states(&self, updates: &[(String, i32, bool)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (plugin_id, load_order, enabled) in updates {
            sqlx::query("UPDATE plugins SET load_order = ?, enabled = ? WHERE id = ?")
                .bind(load_order)
                .bind(enabled)
                .bind(plugin_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit plugin state updates")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Override computation
    // -----------------------------------------------------------------------

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

    /// Update a mod's display name.
    pub async fn update_mod_name(&self, mod_id: &str, new_name: &str) -> Result<()> {
        sqlx::query("UPDATE mods SET name = ? WHERE id = ?")
            .bind(new_name)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to update mod name")?;
        Ok(())
    }

    /// Update only the `mods.install_target` column — no path rewriting.
    ///
    /// Use this when per-file paths are managed separately via `update_file_targets`.
    pub async fn set_mod_install_target_column(
        &self,
        mod_id: &str,
        target: &InstallTarget,
    ) -> Result<()> {
        sqlx::query("UPDATE mods SET install_target = ? WHERE id = ?")
            .bind(target.to_string())
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to update install_target column")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Deployed files tracking
    // -----------------------------------------------------------------------

    /// Clear all deployed file records (used before re-deploying).
    pub async fn clear_deployed_files(&self) -> Result<()> {
        sqlx::query("DELETE FROM deployed_files")
            .execute(&self.pool)
            .await
            .context("Failed to clear deployed_files")?;
        Ok(())
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

    // -----------------------------------------------------------------------
    // Profile CRUD
    // -----------------------------------------------------------------------

    /// Create a new profile for a game. Returns the profile id.
    pub async fn create_profile(&self, game_id: &str, name: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO profiles (id, game_id, name, is_active) VALUES (?, ?, ?, 0)")
            .bind(&id)
            .bind(game_id)
            .bind(name)
            .execute(&self.pool)
            .await
            .context("Failed to create profile")?;
        Ok(id)
    }

    /// Atomically create a new profile and snapshot all current mods/plugins as disabled.
    ///
    /// Unlike calling `create_profile` + `save_clean_to_profile` separately, this method
    /// uses a single transaction so a failure leaves no orphaned profile row.
    pub async fn create_clean_profile(&self, game_id: &str, name: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await?;

        sqlx::query("INSERT INTO profiles (id, game_id, name, is_active) VALUES (?, ?, ?, 0)")
            .bind(&id)
            .bind(game_id)
            .bind(name)
            .execute(&mut *tx)
            .await
            .context("Failed to create profile")?;

        sqlx::query(
            "INSERT INTO profile_mods (profile_id, mod_id, enabled, priority)
             SELECT ?, id, 0, priority FROM mods WHERE game_id = ?",
        )
        .bind(&id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
             SELECT ?, p.id, 0, p.load_order
             FROM plugins p JOIN mods m ON p.mod_id = m.id
             WHERE m.game_id = ?",
        )
        .bind(&id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        tx.commit()
            .await
            .context("Failed to create clean profile")?;
        Ok(id)
    }

    /// Clone a profile: create a new profile with the same mod/plugin snapshot as the source.
    /// The new profile starts inactive; call `switch_profile` to activate it.
    /// If `new_name` is already taken, a numeric suffix is appended (e.g. "Name (Copy) (2)").
    pub async fn clone_profile(
        &self,
        source_profile_id: &str,
        new_name: &str,
        game_id: &str,
    ) -> Result<String> {
        // Resolve a unique name to avoid the UNIQUE(game_id, name) constraint.
        let mut final_name = new_name.to_string();
        let mut counter = 2u32;
        loop {
            let taken: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM profiles WHERE game_id = ? AND name = ?",
            )
            .bind(game_id)
            .bind(&final_name)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(false);

            if !taken {
                break;
            }
            final_name = format!("{new_name} ({counter})");
            counter += 1;
        }

        let id = uuid::Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await?;

        sqlx::query("INSERT INTO profiles (id, game_id, name, is_active) VALUES (?, ?, ?, 0)")
            .bind(&id)
            .bind(game_id)
            .bind(&final_name)
            .execute(&mut *tx)
            .await
            .context("Failed to create cloned profile")?;

        sqlx::query(
            "INSERT INTO profile_mods (profile_id, mod_id, enabled, priority)
             SELECT ?, mod_id, enabled, priority FROM profile_mods WHERE profile_id = ?",
        )
        .bind(&id)
        .bind(source_profile_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
             SELECT ?, plugin_id, enabled, load_order FROM profile_plugins WHERE profile_id = ?",
        )
        .bind(&id)
        .bind(source_profile_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await.context("Failed to clone profile")?;
        Ok(id)
    }

    /// Rename an existing profile.
    pub async fn rename_profile(&self, profile_id: &str, new_name: &str) -> Result<()> {
        sqlx::query("UPDATE profiles SET name = ? WHERE id = ?")
            .bind(new_name)
            .bind(profile_id)
            .execute(&self.pool)
            .await
            .context("Failed to rename profile")?;
        Ok(())
    }

    /// List all profiles for a game.
    pub async fn list_profiles(&self, game_id: &str) -> Result<Vec<Profile>> {
        let rows = sqlx::query_as::<_, (String, String, String, bool, String)>(
            "SELECT id, game_id, name, is_active, save_mode FROM profiles WHERE game_id = ? ORDER BY name",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list profiles")?;

        Ok(rows
            .into_iter()
            .map(|(id, game_id, name, is_active, save_mode)| Profile {
                id,
                game_id,
                name,
                is_active,
                save_mode: SaveMode::from_db(&save_mode),
                save_synced_at: None,
            })
            .collect())
    }

    /// Get the active profile for a game (if any).
    pub async fn get_active_profile(&self, game_id: &str) -> Result<Option<Profile>> {
        let row = sqlx::query_as::<_, (String, String, String, bool, String)>(
            "SELECT id, game_id, name, is_active, save_mode FROM profiles
             WHERE game_id = ? AND is_active = TRUE LIMIT 1",
        )
        .bind(game_id)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to query active profile")?;

        Ok(
            row.map(|(id, game_id, name, is_active, save_mode)| Profile {
                id,
                game_id,
                name,
                is_active,
                save_mode: SaveMode::from_db(&save_mode),
                save_synced_at: None,
            }),
        )
    }

    /// Update the save mode for a profile.
    pub async fn set_profile_save_mode(&self, profile_id: &str, mode: SaveMode) -> Result<()> {
        sqlx::query("UPDATE profiles SET save_mode = ? WHERE id = ?")
            .bind(mode.to_db())
            .bind(profile_id)
            .execute(&self.pool)
            .await
            .context("Failed to update profile save mode")?;
        Ok(())
    }

    /// Save the current mods/plugins state into the given profile (snapshot).
    pub async fn save_to_profile(&self, profile_id: &str, game_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Clear existing profile state
        sqlx::query("DELETE FROM profile_mods WHERE profile_id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM profile_plugins WHERE profile_id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;

        // Snapshot current mods
        sqlx::query(
            "INSERT INTO profile_mods (profile_id, mod_id, enabled, priority)
             SELECT ?, id, enabled, priority FROM mods WHERE game_id = ?",
        )
        .bind(profile_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        // Snapshot current plugins
        sqlx::query(
            "INSERT INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
             SELECT ?, p.id, p.enabled, p.load_order
             FROM plugins p JOIN mods m ON p.mod_id = m.id
             WHERE m.game_id = ?",
        )
        .bind(profile_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        tx.commit()
            .await
            .context("Failed to save profile snapshot")?;
        Ok(())
    }

    /// Switch to a profile: load its state into the live mods/plugins tables.
    pub async fn switch_profile(&self, game_id: &str, profile_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        // Deactivate all profiles for this game, activate the target
        sqlx::query("UPDATE profiles SET is_active = FALSE WHERE game_id = ?")
            .bind(game_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE profiles SET is_active = TRUE WHERE id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;

        // Restore mod enabled/priority from profile snapshot
        // Only updates mods that exist in both live table and profile snapshot
        sqlx::query(
            "UPDATE mods SET enabled = pm.enabled, priority = pm.priority
             FROM profile_mods pm
             WHERE mods.id = pm.mod_id AND pm.profile_id = ? AND mods.game_id = ?",
        )
        .bind(profile_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        // Disable mods installed after this profile was last saved (not in snapshot).
        // Without this, a mod installed while another profile was active would inherit
        // its enabled=true state and appear enabled in every profile it wasn't part of.
        sqlx::query(
            "UPDATE mods SET enabled = 0
             WHERE game_id = ?
               AND id NOT IN (SELECT mod_id FROM profile_mods WHERE profile_id = ?)",
        )
        .bind(game_id)
        .bind(profile_id)
        .execute(&mut *tx)
        .await?;

        // Restore plugin enabled/load_order from profile snapshot
        sqlx::query(
            "UPDATE plugins SET enabled = pp.enabled, load_order = pp.load_order
             FROM profile_plugins pp
             WHERE plugins.id = pp.plugin_id AND pp.profile_id = ?
               AND plugins.mod_id IN (SELECT id FROM mods WHERE game_id = ?)",
        )
        .bind(profile_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        // For plugins not in profile_plugins (e.g. installed after the profile was saved),
        // default their enabled state to match their mod's state in the profile snapshot.
        sqlx::query(
            "UPDATE plugins SET enabled = (
                SELECT COALESCE(pm.enabled, 0)
                FROM profile_mods pm
                WHERE pm.mod_id = plugins.mod_id AND pm.profile_id = ?
             )
             WHERE plugins.mod_id IN (SELECT id FROM mods WHERE game_id = ?)
               AND plugins.id NOT IN (SELECT plugin_id FROM profile_plugins WHERE profile_id = ?)",
        )
        .bind(profile_id)
        .bind(game_id)
        .bind(profile_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await.context("Failed to switch profile")?;
        Ok(())
    }

    /// Delete a profile and its snapshot data (CASCADE handles profile_mods/profile_plugins).
    pub async fn delete_profile(&self, profile_id: &str) -> Result<()> {
        // CASCADE may not be enabled by default in SQLite, so delete explicitly
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM profile_mods WHERE profile_id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM profile_plugins WHERE profile_id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM profiles WHERE id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await.context("Failed to delete profile")?;
        Ok(())
    }

    /// Ensure a "Default" profile exists for a game, creating one if needed.
    /// Returns the active profile (or the newly created Default).
    pub async fn ensure_default_profile(&self, game_id: &str) -> Result<Profile> {
        if let Some(active) = self.get_active_profile(game_id).await? {
            return Ok(active);
        }

        let profiles = self.list_profiles(game_id).await?;
        if !profiles.is_empty() {
            // Prefer the last-used profile if it still exists; fall back to first alphabetical.
            let last_key = format!("last_profile_{game_id}");
            let preferred_id = self.get_setting(&last_key).await.ok().flatten();
            let target = preferred_id
                .as_deref()
                .and_then(|id| profiles.iter().find(|p| p.id == id))
                .or_else(|| profiles.first())
                .unwrap() // safe: profiles is non-empty
                .clone();
            sqlx::query("UPDATE profiles SET is_active = TRUE WHERE id = ?")
                .bind(&target.id)
                .execute(&self.pool)
                .await?;
            return Ok(Profile {
                is_active: true,
                ..target
            });
        }

        // No profiles at all — create Default
        let id = self.create_profile(game_id, "Default").await?;
        sqlx::query("UPDATE profiles SET is_active = TRUE WHERE id = ?")
            .bind(&id)
            .execute(&self.pool)
            .await?;
        self.save_to_profile(&id, game_id).await?;
        Ok(Profile {
            id,
            game_id: game_id.to_string(),
            name: "Default".to_string(),
            is_active: true,
            save_mode: SaveMode::Global,
            save_synced_at: None,
        })
    }

    /// Set `enabled` for every mod belonging to a game in one statement.
    pub async fn set_all_mods_enabled(&self, game_id: &str, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE mods SET enabled = ? WHERE game_id = ?")
            .bind(enabled)
            .bind(game_id)
            .execute(&self.pool)
            .await
            .context("Failed to set all mods enabled")?;
        Ok(())
    }

    /// Set `enabled` for every plugin belonging to a game's mods in one statement.
    pub async fn set_all_plugins_enabled(&self, game_id: &str, enabled: bool) -> Result<()> {
        sqlx::query(
            "UPDATE plugins SET enabled = ?
             WHERE mod_id IN (SELECT id FROM mods WHERE game_id = ?)",
        )
        .bind(enabled)
        .bind(game_id)
        .execute(&self.pool)
        .await
        .context("Failed to set all plugins enabled")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Profile export / import
    // -----------------------------------------------------------------------

    /// Build a portable export snapshot for the given profile.
    pub async fn export_profile(
        &self,
        profile_id: &str,
    ) -> Result<crate::models::profile_export::ProfileExport> {
        use crate::models::profile_export::{ProfileExport, ProfileModExport, ProfilePluginExport};

        let profile_row = sqlx::query_as::<_, (String, String, String)>(
            "SELECT id, game_id, name FROM profiles WHERE id = ?",
        )
        .bind(profile_id)
        .fetch_one(&self.pool)
        .await
        .context("Profile not found")?;

        let game_id = profile_row.1;
        let profile_name = profile_row.2;

        let mod_rows: Vec<(String, bool, i32)> = sqlx::query_as(
            "SELECT m.name, pm.enabled, pm.priority
             FROM profile_mods pm
             JOIN mods m ON m.id = pm.mod_id
             WHERE pm.profile_id = ?
             ORDER BY pm.priority ASC",
        )
        .bind(profile_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to load profile mods for export")?;

        let plugin_rows: Vec<(String, bool, i32)> = sqlx::query_as(
            "SELECT p.filename, pp.enabled, pp.load_order
             FROM profile_plugins pp
             JOIN plugins p ON p.id = pp.plugin_id
             WHERE pp.profile_id = ?
             ORDER BY pp.load_order ASC",
        )
        .bind(profile_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to load profile plugins for export")?;

        Ok(ProfileExport {
            version: 1,
            game_id,
            profile_name,
            mods: mod_rows
                .into_iter()
                .map(|(name, enabled, priority)| ProfileModExport {
                    name,
                    enabled,
                    priority,
                })
                .collect(),
            plugins: plugin_rows
                .into_iter()
                .map(|(filename, enabled, load_order)| ProfilePluginExport {
                    filename,
                    enabled,
                    load_order,
                })
                .collect(),
        })
    }

    /// Import a `ProfileExport`, creating a new profile.
    ///
    /// Matches exported names against live mods/plugins for `game_id`.
    /// Entries that don't match any installed mod/plugin are silently skipped.
    /// If a profile with the exported name already exists, a numeric suffix is
    /// appended (e.g. "Vanilla (2)") so the import never fails on duplicates.
    /// Returns the new profile's ID.
    pub async fn import_profile(
        &self,
        game_id: &str,
        export: &crate::models::profile_export::ProfileExport,
    ) -> Result<String> {
        // Resolve a unique name to avoid the UNIQUE(game_id, name) constraint.
        let mut final_name = export.profile_name.clone();
        let mut counter = 2u32;
        loop {
            let taken: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM profiles WHERE game_id = ? AND name = ?",
            )
            .bind(game_id)
            .bind(&final_name)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(false);

            if !taken {
                break;
            }
            final_name = format!("{} ({})", export.profile_name, counter);
            counter += 1;
        }

        let profile_id = self
            .create_profile(game_id, &final_name)
            .await
            .context("Failed to create profile for import")?;

        let mut tx = self.pool.begin().await?;

        for mod_export in &export.mods {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT id FROM mods WHERE name = ? AND game_id = ? LIMIT 1")
                    .bind(&mod_export.name)
                    .bind(game_id)
                    .fetch_optional(&mut *tx)
                    .await?;

            if let Some((mod_id,)) = row {
                sqlx::query(
                    "INSERT OR IGNORE INTO profile_mods (profile_id, mod_id, enabled, priority)
                     VALUES (?, ?, ?, ?)",
                )
                .bind(&profile_id)
                .bind(&mod_id)
                .bind(mod_export.enabled)
                .bind(mod_export.priority)
                .execute(&mut *tx)
                .await?;
            }
        }

        for plugin_export in &export.plugins {
            let row: Option<(String,)> = sqlx::query_as(
                "SELECT p.id FROM plugins p
                 JOIN mods m ON p.mod_id = m.id
                 WHERE LOWER(p.filename) = LOWER(?) AND m.game_id = ?
                 LIMIT 1",
            )
            .bind(&plugin_export.filename)
            .bind(game_id)
            .fetch_optional(&mut *tx)
            .await?;

            if let Some((plugin_id,)) = row {
                sqlx::query(
                    "INSERT OR IGNORE INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
                     VALUES (?, ?, ?, ?)",
                )
                .bind(&profile_id)
                .bind(&plugin_id)
                .bind(plugin_export.enabled)
                .bind(plugin_export.load_order)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit()
            .await
            .context("Failed to commit imported profile")?;

        Ok(profile_id)
    }

    // -----------------------------------------------------------------------
    // Tool CRUD
    // -----------------------------------------------------------------------

    /// Insert a new tool configuration.
    pub async fn insert_tool(&self, tool: &crate::models::tool::Tool) -> Result<()> {
        sqlx::query(
            "INSERT INTO tools (id, game_id, name, exe_path, icon_name, custom_args, sort_order, working_dir)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&tool.id)
        .bind(&tool.game_id)
        .bind(&tool.name)
        .bind(&tool.exe_path)
        .bind(&tool.icon_name)
        .bind(&tool.custom_args)
        .bind(tool.sort_order)
        .bind(&tool.working_dir)
        .execute(&self.pool)
        .await
        .context("Failed to insert tool")?;
        Ok(())
    }

    /// List all tools for a game, ordered by sort_order ascending.
    pub async fn list_tools(&self, game_id: &str) -> Result<Vec<crate::models::tool::Tool>> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String, i32, String)>(
            "SELECT id, game_id, name, exe_path, icon_name, custom_args, sort_order, working_dir
             FROM tools WHERE game_id = ? ORDER BY sort_order ASC",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list tools")?;

        Ok(rows
            .into_iter()
            .map(
                |(id, game_id, name, exe_path, icon_name, custom_args, sort_order, working_dir)| {
                    crate::models::tool::Tool {
                        id,
                        game_id,
                        name,
                        exe_path,
                        icon_name,
                        custom_args,
                        sort_order,
                        working_dir,
                    }
                },
            )
            .collect())
    }

    /// Update the working directory for an existing tool.
    pub async fn update_tool_working_dir(&self, tool_id: &str, working_dir: &str) -> Result<()> {
        sqlx::query("UPDATE tools SET working_dir = ? WHERE id = ?")
            .bind(working_dir)
            .bind(tool_id)
            .execute(&self.pool)
            .await
            .context("Failed to update tool working dir")?;
        Ok(())
    }

    /// Delete a tool by its ID.
    pub async fn delete_tool(&self, tool_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM tools WHERE id = ?")
            .bind(tool_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete tool")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Settings
    // -----------------------------------------------------------------------

    /// Get a setting value by key.
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query_as::<_, (String,)>("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .context("Failed to query setting")?;
        Ok(row.map(|(v,)| v))
    }

    // -----------------------------------------------------------------------
    // Nexus metadata
    // -----------------------------------------------------------------------

    /// Update Nexus metadata for a mod after fetching from the API.
    /// `latest_version` is the current version on Nexus (used for update-badge comparison);
    /// the installed `version` column is written separately via `set_mod_installed_version`.
    pub async fn update_mod_nexus_metadata(
        &self,
        mod_id: &str,
        latest_version: &str,
        author: &str,
        description: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE mods SET latest_version = ?, author = ?, nexus_description = ? WHERE id = ?",
        )
        .bind(latest_version)
        .bind(author)
        .bind(description)
        .bind(mod_id)
        .execute(&self.pool)
        .await
        .context("Failed to update Nexus metadata")?;
        Ok(())
    }

    /// Set the installed version for a mod (from the specific Nexus file entry).
    pub async fn set_mod_installed_version(&self, mod_id: &str, version: &str) -> Result<()> {
        sqlx::query("UPDATE mods SET version = ? WHERE id = ?")
            .bind(version)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to set installed version")?;
        Ok(())
    }

    /// Set the latest known version for a mod (from Nexus update check).
    pub async fn set_latest_version(&self, mod_id: &str, latest_version: &str) -> Result<()> {
        sqlx::query("UPDATE mods SET latest_version = ? WHERE id = ?")
            .bind(latest_version)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to set latest version")?;
        Ok(())
    }

    /// Get all mods that have Nexus IDs (for update checking).
    pub async fn mods_with_nexus_ids(&self, game_id: &str) -> Result<Vec<ModEntry>> {
        let all = self.list_mods(game_id).await?;
        Ok(all
            .into_iter()
            .filter(|m| m.nexus_mod_id.is_some())
            .collect())
    }

    /// Set a setting value (upsert).
    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .context("Failed to save setting")?;
        Ok(())
    }

    /// Persist rate limit info as JSON under key "nexus_rate_limits".
    pub async fn save_rate_limits(
        &self,
        info: &crate::core::nexus_api::RateLimitInfo,
    ) -> Result<()> {
        let json = serde_json::to_string(info).context("Failed to serialize rate limits")?;
        self.set_setting("nexus_rate_limits", &json).await
    }

    /// Load rate limit info from the settings table. Returns None if not stored.
    pub async fn load_rate_limits(&self) -> Result<Option<crate::core::nexus_api::RateLimitInfo>> {
        match self.get_setting("nexus_rate_limits").await? {
            Some(json) => {
                let info =
                    serde_json::from_str(&json).context("Failed to parse stored rate limits")?;
                Ok(Some(info))
            }
            None => Ok(None),
        }
    }

    // -----------------------------------------------------------------------
    // Download entries (persisted across restarts)
    // -----------------------------------------------------------------------

    /// Save a download entry to the database.
    pub async fn save_download_entry(
        &self,
        entry: &crate::models::download::DownloadEntry,
    ) -> Result<()> {
        let (nexus_mod_id, nexus_file_id, nexus_domain) = match &entry.nexus_ids {
            Some((mid, fid, dom)) => (Some(*mid), Some(*fid), Some(dom.as_str())),
            None => (None, None, None),
        };
        sqlx::query(
            "INSERT INTO download_entries (id, mod_name, archive_path, nexus_mod_id, nexus_file_id, nexus_domain, game_domain, metadata_fetched, nexus_file_name, nexus_is_primary, status, archive_hash)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                mod_name = excluded.mod_name,
                archive_path = excluded.archive_path,
                nexus_mod_id = excluded.nexus_mod_id,
                nexus_file_id = excluded.nexus_file_id,
                nexus_domain = excluded.nexus_domain,
                game_domain = excluded.game_domain,
                metadata_fetched = excluded.metadata_fetched,
                nexus_file_name = excluded.nexus_file_name,
                nexus_is_primary = excluded.nexus_is_primary,
                status = excluded.status,
                archive_hash = excluded.archive_hash",
        )
        .bind(&entry.id)
        .bind(&entry.mod_name)
        .bind(entry.archive_path.as_ref().map(|p| p.to_string_lossy().to_string()))
        .bind(nexus_mod_id)
        .bind(nexus_file_id)
        .bind(nexus_domain)
        .bind(entry.game_domain.as_deref())
        .bind(entry.metadata_fetched)
        .bind(entry.nexus_file_name.as_deref())
        .bind(entry.nexus_is_primary)
        .bind(entry.status.as_db_str())
        .bind(entry.archive_hash.as_deref())
        .execute(&self.pool)
        .await
        .context("Failed to save download entry")?;
        Ok(())
    }

    /// Load all persisted download entries.
    pub async fn load_download_entries(
        &self,
    ) -> Result<Vec<crate::models::download::DownloadEntry>> {
        use crate::models::download::{DownloadEntry, DownloadStatus};
        let rows: Vec<(String, String, Option<String>, Option<i64>, Option<i64>, Option<String>, Option<String>, bool, Option<String>, bool, Option<String>, Option<String>)> =
            sqlx::query_as(
                "SELECT id, mod_name, archive_path, nexus_mod_id, nexus_file_id, nexus_domain, game_domain, metadata_fetched, nexus_file_name, nexus_is_primary, status, archive_hash
                 FROM download_entries"
            )
            .fetch_all(&self.pool)
            .await
            .context("Failed to load download entries")?;

        let entries = rows
            .into_iter()
            .filter_map(
                |(
                    id,
                    mod_name,
                    archive_path,
                    nexus_mod_id,
                    nexus_file_id,
                    nexus_domain,
                    game_domain,
                    metadata_fetched,
                    nexus_file_name,
                    nexus_is_primary,
                    status_str,
                    archive_hash,
                )| {
                    let path = archive_path.map(std::path::PathBuf::from);
                    let is_installed = status_str.as_deref().unwrap_or("downloaded") == "installed";
                    // Always load Installed entries (the archive may have been deleted
                    // or the downloads folder may have changed — the mod is still installed).
                    // Filter out other entries whose archive no longer exists.
                    if !is_installed
                        && let Some(ref p) = path
                        && !p.exists()
                    {
                        return None;
                    }
                    let nexus_ids = nexus_mod_id
                        .zip(nexus_file_id)
                        .zip(nexus_domain)
                        .map(|((mid, fid), dom)| (mid, fid, dom));
                    let status =
                        DownloadStatus::from_db_str(status_str.as_deref().unwrap_or("downloaded"));
                    let status_msg = status.default_status_msg().to_string();
                    Some(DownloadEntry {
                        id,
                        mod_name,
                        status,
                        progress: 1.0,
                        status_msg,
                        error_msg: None,
                        nexus_ids,
                        archive_path: path,
                        metadata_fetched,
                        game_domain,
                        nexus_file_name,
                        nexus_is_primary,
                        archive_hash,
                    })
                },
            )
            .collect();

        Ok(entries)
    }

    // -----------------------------------------------------------------------
    // Plugin master dependencies
    // -----------------------------------------------------------------------

    /// Store the master-file requirements for a plugin.
    pub async fn insert_plugin_masters(&self, plugin_id: &str, masters: &[String]) -> Result<()> {
        if masters.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin().await?;
        for master in masters {
            sqlx::query("INSERT OR IGNORE INTO plugin_masters (plugin_id, master) VALUES (?, ?)")
                .bind(plugin_id)
                .bind(master)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit plugin masters")?;
        Ok(())
    }

    /// Return a map of `plugin_id → [master, ...]` for all plugins of a game.
    pub async fn list_all_plugin_masters(
        &self,
        game_id: &str,
    ) -> Result<HashMap<String, Vec<String>>> {
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT pm.plugin_id, pm.master
             FROM plugin_masters pm
             JOIN plugins p ON pm.plugin_id = p.id
             JOIN mods m ON p.mod_id = m.id
             WHERE m.game_id = ?",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list plugin masters")?;

        let mut map: HashMap<String, Vec<String>> = HashMap::new();
        for (plugin_id, master) in rows {
            map.entry(plugin_id).or_default().push(master);
        }
        Ok(map)
    }

    /// Backfill `plugin_masters` for plugins installed before master tracking was added.
    ///
    /// Finds all plugin records whose cache file can be located but that have no rows in
    /// `plugin_masters`, reads the TES4 header, and inserts any MAST records found.
    /// Safe to run on every startup — the NOT EXISTS guard is idempotent for plugins that
    /// already have master rows, and plugins with no masters are fast to re-scan.
    async fn backfill_plugin_masters(pool: &SqlitePool) -> Result<()> {
        use std::path::Path;

        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT p.id, mf.cache_path
             FROM plugins p
             JOIN mod_files mf ON p.mod_id = mf.mod_id
             WHERE (mf.game_rel_lowercase = LOWER(p.filename)
                    OR mf.game_rel_lowercase LIKE '%/' || LOWER(p.filename))
               AND NOT EXISTS (SELECT 1 FROM plugin_masters pm WHERE pm.plugin_id = p.id)",
        )
        .fetch_all(pool)
        .await
        .context("Failed to query plugins for master backfill")?;

        if rows.is_empty() {
            return Ok(());
        }

        dlog!(
            "[debug] Backfilling plugin masters for {} plugins",
            rows.len()
        );

        for (plugin_id, cache_path) in &rows {
            match crate::utils::plugin_header::read_masters(Path::new(cache_path)) {
                Ok(masters) => {
                    for master in &masters {
                        let _ = sqlx::query(
                            "INSERT OR IGNORE INTO plugin_masters (plugin_id, master) VALUES (?, ?)",
                        )
                        .bind(plugin_id)
                        .bind(master)
                        .execute(pool)
                        .await;
                    }
                }
                Err(e) => {
                    dlog!("[debug] Could not read masters from {cache_path}: {e}");
                }
            }
        }

        Ok(())
    }

    /// One-time startup migration: mark download entries as 'installed' when their
    /// (nexus_mod_id, nexus_file_id) matches a row in the mods table.
    ///
    /// This fixes two historical issues:
    /// 1. Rows created before the `status` column existed received DEFAULT 'downloaded',
    ///    so already-installed mods showed "Ready to Install" after the column was added.
    /// 2. A bug in the removal path reset ALL installed downloads instead of just the one
    ///    corresponding to the removed mod.
    ///
    /// The backfill is gated by the settings key `dl_status_backfill_v1` so it runs only
    /// once per database.
    async fn backfill_download_statuses(pool: &SqlitePool) -> Result<()> {
        let done: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'dl_status_backfill_v1'")
                .fetch_optional(pool)
                .await
                .unwrap_or(None);

        if done.is_some() {
            return Ok(());
        }

        sqlx::query(
            "UPDATE download_entries
             SET status = 'installed'
             WHERE (status IS NULL OR status = 'downloaded')
               AND nexus_mod_id IS NOT NULL
               AND nexus_file_id IS NOT NULL
               AND EXISTS (
                   SELECT 1 FROM mods
                   WHERE mods.nexus_mod_id = download_entries.nexus_mod_id
                     AND mods.nexus_file_id = download_entries.nexus_file_id
               )",
        )
        .execute(pool)
        .await
        .context("Failed to backfill download statuses")?;

        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('dl_status_backfill_v1', 'true')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .execute(pool)
        .await
        .context("Failed to record download status backfill")?;

        Ok(())
    }

    /// One-time startup backfill: populate `mods.archive_hash` for mods that were
    /// installed before archive hashing was introduced.
    ///
    /// Strategy:
    /// - Join `mods` (where `archive_hash IS NULL` and Nexus IDs are present) with
    ///   `download_entries` on `(nexus_mod_id, nexus_file_id)` to locate the archive.
    /// - Hash each archive file that still exists on disk (SHA-256, blocking I/O).
    /// - `UPDATE mods SET archive_hash = ?` for the matched mod.
    ///
    /// Gated by the settings key `archive_hash_backfill_v1` so it runs only once per
    /// database, even if some mods are left un-hashed because their archives were deleted.
    async fn backfill_archive_hashes(pool: &SqlitePool) -> Result<()> {
        let done: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'archive_hash_backfill_v1'")
                .fetch_optional(pool)
                .await
                .unwrap_or(None);

        if done.is_some() {
            return Ok(());
        }

        // Find mods that can be matched to a download entry with a known archive path.
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT m.id, de.archive_path
             FROM mods m
             JOIN download_entries de
               ON m.nexus_mod_id = de.nexus_mod_id
              AND m.nexus_file_id = de.nexus_file_id
             WHERE m.archive_hash IS NULL
               AND m.nexus_mod_id IS NOT NULL
               AND m.nexus_file_id IS NOT NULL
               AND de.archive_path IS NOT NULL",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();

        for (mod_id, archive_path) in rows {
            let path = std::path::PathBuf::from(&archive_path);
            if !path.exists() {
                continue;
            }
            // Hash on a blocking thread — archive files can be several GB.
            let hash = tokio::task::spawn_blocking(move || {
                crate::utils::archive::hash_archive_file(&path).ok()
            })
            .await
            .unwrap_or(None);

            if let Some(hash) = hash {
                let _ = sqlx::query("UPDATE mods SET archive_hash = ? WHERE id = ?")
                    .bind(&hash)
                    .bind(&mod_id)
                    .execute(pool)
                    .await;
            }
        }

        // Mark the backfill as done even if some archives were missing — those mods
        // simply won't be caught by the hash-based dedup check (Nexus-ID check still works).
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('archive_hash_backfill_v1', 'true')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .execute(pool)
        .await
        .context("Failed to record archive hash backfill")?;

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Mod Groups
    // -----------------------------------------------------------------------

    /// List all groups for a game, ordered by position.
    pub async fn list_groups(&self, game_id: &str) -> Result<Vec<crate::models::group::ModGroup>> {
        let rows: Vec<(String, String, f64, i32)> = sqlx::query_as(
            "SELECT id, name, position, collapsed
             FROM mod_groups
             WHERE game_id = ?
             ORDER BY position ASC",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list mod groups")?;

        Ok(rows
            .into_iter()
            .map(
                |(id, name, position, collapsed)| crate::models::group::ModGroup {
                    id,
                    name,
                    position,
                    collapsed: collapsed != 0,
                },
            )
            .collect())
    }

    /// Create a new group and return it.
    pub async fn create_group(
        &self,
        game_id: &str,
        name: &str,
        position: f64,
    ) -> Result<crate::models::group::ModGroup> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO mod_groups (id, game_id, name, position, collapsed)
             VALUES (?, ?, ?, ?, 0)",
        )
        .bind(&id)
        .bind(game_id)
        .bind(name)
        .bind(position)
        .execute(&self.pool)
        .await
        .context("Failed to create mod group")?;

        Ok(crate::models::group::ModGroup {
            id,
            name: name.to_string(),
            position,
            collapsed: false,
        })
    }

    /// Rename a group.
    pub async fn rename_group(&self, group_id: &str, name: &str) -> Result<()> {
        sqlx::query("UPDATE mod_groups SET name = ? WHERE id = ?")
            .bind(name)
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to rename mod group")?;
        Ok(())
    }

    /// Persist the collapsed state of a group.
    pub async fn set_group_collapsed(&self, group_id: &str, collapsed: bool) -> Result<()> {
        sqlx::query("UPDATE mod_groups SET collapsed = ? WHERE id = ?")
            .bind(collapsed as i32)
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to update group collapsed state")?;
        Ok(())
    }

    /// Delete a group. Mods that belonged to it become ungrouped (group_id = NULL).
    pub async fn delete_group(&self, group_id: &str) -> Result<()> {
        sqlx::query("UPDATE mods SET group_id = NULL WHERE group_id = ?")
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to ungroup mods before group deletion")?;
        sqlx::query("DELETE FROM mod_groups WHERE id = ?")
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete mod group")?;
        Ok(())
    }

    /// Move a group to a new position in the list.
    pub async fn move_group(&self, group_id: &str, new_position: f64) -> Result<()> {
        sqlx::query("UPDATE mod_groups SET position = ? WHERE id = ?")
            .bind(new_position)
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to move mod group")?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Vanilla file snapshot
    // -----------------------------------------------------------------------

    /// Record the pre-mod state of a game's file tree as the vanilla baseline.
    ///
    /// Each entry is `(game_rel_lowercase, size_bytes, mtime_secs)` as produced by
    /// `detector::snapshot_game_files`. Idempotent: skips recording if a snapshot
    /// already exists for this game.
    pub async fn ensure_vanilla_snapshot(
        &self,
        game_id: &str,
        entries: &[(String, u64, i64)],
    ) -> Result<()> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM vanilla_files WHERE game_id = ?")
                .bind(game_id)
                .fetch_one(&self.pool)
                .await
                .context("Failed to count vanilla_files")?;

        if count > 0 {
            return Ok(());
        }

        let mut tx = self.pool.begin().await?;
        for (path, size, mtime) in entries {
            sqlx::query(
                "INSERT OR IGNORE INTO vanilla_files
                 (game_id, game_rel_lowercase, file_size, mtime_secs)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(game_id)
            .bind(path)
            .bind(*size as i64)
            .bind(mtime)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit vanilla snapshot")?;
        Ok(())
    }

    /// Return a map of `game_rel_lowercase → (size_bytes, mtime_secs)` for the
    /// vanilla snapshot. Used by `detector::scan_external_files` to identify
    /// files that have been replaced (same path, different attributes).
    pub async fn get_vanilla_metadata(
        &self,
        game_id: &str,
    ) -> Result<std::collections::HashMap<String, (u64, i64)>> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT game_rel_lowercase, file_size, mtime_secs
             FROM vanilla_files WHERE game_id = ?",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to query vanilla_files")?;

        Ok(rows
            .into_iter()
            .map(|(path, size, mtime)| (path, (size as u64, mtime)))
            .collect())
    }

    /// Delete the existing vanilla snapshot for a game and record a fresh one.
    ///
    /// Use after a clean game reinstall to stop vanilla files appearing as
    /// external changes (their mtimes changed even though content is identical).
    pub async fn reset_vanilla_snapshot(
        &self,
        game_id: &str,
        entries: &[(String, u64, i64)],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM vanilla_files WHERE game_id = ?")
            .bind(game_id)
            .execute(&mut *tx)
            .await
            .context("Failed to delete old vanilla snapshot")?;
        for (path, size, mtime) in entries {
            sqlx::query(
                "INSERT OR IGNORE INTO vanilla_files
                 (game_id, game_rel_lowercase, file_size, mtime_secs)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(game_id)
            .bind(path)
            .bind(*size as i64)
            .bind(mtime)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to commit new vanilla snapshot")
    }

    /// Upsert vanilla baseline entries for specific files.
    ///
    /// Use when individual files are incorrectly flagged as external changes
    /// (e.g. after a partial reinstall). Overwrites existing rows for the same
    /// key so the detector accepts the new attributes as the vanilla state.
    pub async fn update_vanilla_entries(
        &self,
        game_id: &str,
        entries: &[(String, u64, i64)],
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (path, size, mtime) in entries {
            sqlx::query(
                "INSERT OR REPLACE INTO vanilla_files
                 (game_id, game_rel_lowercase, file_size, mtime_secs)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(game_id)
            .bind(path)
            .bind(*size as i64)
            .bind(mtime)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit()
            .await
            .context("Failed to update vanilla entries")
    }

    /// Persist full game configuration (path, wine prefix, engine, custom flag).
    ///
    /// Used both by the Flatpak portal path-confirmation flow and the game setup dialog.
    pub async fn upsert_game(
        &self,
        id: &str,
        title: &str,
        path: &std::path::Path,
        data_subdir: &str,
        engine: &str,
        wine_prefix: Option<&std::path::Path>,
        custom: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO games (id, title, path, data_subdir, engine, wine_prefix, custom, hidden)
             VALUES (?, ?, ?, ?, ?, ?, ?, 0)
             ON CONFLICT(id) DO UPDATE SET
               title        = excluded.title,
               path         = excluded.path,
               data_subdir  = excluded.data_subdir,
               engine       = excluded.engine,
               wine_prefix  = excluded.wine_prefix,
               custom       = excluded.custom,
               hidden       = 0",
        )
        .bind(id)
        .bind(title)
        .bind(path.to_string_lossy().as_ref())
        .bind(data_subdir)
        .bind(engine)
        .bind(wine_prefix.map(|p| p.to_string_lossy().into_owned()))
        .bind(custom as i32)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Backwards-compatible shim: persist only the game folder path (Flatpak portal flow).
    pub async fn upsert_game_path(&self, game_id: &str, path: &std::path::Path) -> Result<()> {
        sqlx::query(
            "INSERT INTO games (id, path) VALUES (?, ?)
             ON CONFLICT(id) DO UPDATE SET path = excluded.path",
        )
        .bind(game_id)
        .bind(path.to_string_lossy().as_ref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Load all persisted game configurations from the games table.
    pub async fn load_persisted_games(&self) -> Result<Vec<PersistedGame>> {
        let rows: Vec<(String, Option<String>, Option<String>, Option<String>, Option<String>, Option<String>, Option<i32>)> =
            sqlx::query_as(
                "SELECT id, title, path, data_subdir, engine, wine_prefix, custom
                 FROM games WHERE path IS NOT NULL AND (hidden IS NULL OR hidden = 0)",
            )
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|(id, title, path, data_subdir, engine, wine_prefix, custom)| PersistedGame {
                id,
                title: title.unwrap_or_default(),
                path: std::path::PathBuf::from(path.unwrap_or_default()),
                data_subdir: data_subdir.unwrap_or_else(|| "Data".to_string()),
                engine: engine.unwrap_or_else(|| "bethesda".to_string()),
                wine_prefix: wine_prefix.map(std::path::PathBuf::from),
                custom: custom.unwrap_or(0) != 0,
            })
            .collect())
    }

    /// Mark a game as hidden so it is excluded from the managed list and not re-added on rescan.
    pub async fn hide_game(&self, game_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO games (id, hidden) VALUES (?, 1)
             ON CONFLICT(id) DO UPDATE SET hidden = 1",
        )
        .bind(game_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Return the IDs of all games the user has explicitly hidden.
    pub async fn load_hidden_game_ids(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT id FROM games WHERE hidden = 1")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }

    /// Remove a game record from the games table (used when user removes a custom game).
    pub async fn remove_game(&self, game_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM games WHERE id = ?")
            .bind(game_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
