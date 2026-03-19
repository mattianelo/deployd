use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;

pub mod downloads;
pub mod files;
pub mod games;
pub mod groups;
pub mod migrations;
pub mod mods;
pub mod order_snapshots;
pub mod plugins;
pub mod profiles;
pub mod settings;
pub mod vanilla;

#[derive(Clone)]
pub struct Tracker {
    pub(super) pool: SqlitePool,
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
        let opts = SqliteConnectOptions::from_str(db_url)?
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
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
                game_id TEXT NOT NULL DEFAULT '',
                game_rel_lowercase TEXT NOT NULL,
                game_rel_original TEXT NOT NULL,
                mod_id TEXT NOT NULL,
                cache_path TEXT NOT NULL,
                PRIMARY KEY (game_id, game_rel_lowercase)
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

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS order_snapshots (
                id TEXT PRIMARY KEY,
                game_id TEXT NOT NULL,
                name TEXT NOT NULL,
                kind TEXT NOT NULL,
                created_at TEXT NOT NULL,
                UNIQUE(game_id, name, kind)
            )",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS order_snapshot_entries (
                snapshot_id TEXT NOT NULL REFERENCES order_snapshots(id) ON DELETE CASCADE,
                entry_id TEXT NOT NULL,
                position INTEGER NOT NULL,
                PRIMARY KEY (snapshot_id, entry_id)
            )",
        )
        .execute(&pool)
        .await?;

        // Migration: add wine_prefix, engine, custom columns to games table
        migrations::migrate_games_columns(&pool).await?;

        // Migration: add Nexus metadata columns to mods table
        migrations::migrate_nexus_columns(&pool).await?;

        // Migration: add file name columns to download_entries
        migrations::migrate_download_columns(&pool).await?;

        // Migration: add size/mtime columns to vanilla_files for replacement detection
        migrations::migrate_vanilla_files_columns(&pool).await?;

        // Migration: add group_id column to mods
        migrations::migrate_group_columns(&pool).await?;

        // Migration: add install_target column to mods
        migrations::migrate_install_target_column(&pool).await?;

        // Migration: add notes column to mods
        migrations::migrate_notes_column(&pool).await?;

        // Migration: fix version/latest_version columns populated by old code
        if let Err(e) = migrations::migrate_version_columns(&pool).await {
            eprintln!("Version column migration failed (non-fatal): {e}");
        }

        // Backfill plugin masters for mods installed before the plugin_masters table was added
        if let Err(e) = migrations::backfill_plugin_masters(&pool).await {
            eprintln!("Plugin master backfill failed (non-fatal): {e}");
        }

        // One-time backfill: set download_entries.status = 'installed' for any entry
        // whose (nexus_mod_id, nexus_file_id) matches an installed mod.
        if let Err(e) = migrations::backfill_download_statuses(&pool).await {
            eprintln!("Download status backfill failed (non-fatal): {e}");
        }

        // One-time backfill: populate archive_hash for mods that were installed before
        // the hash was recorded.
        if let Err(e) = migrations::backfill_archive_hashes(&pool).await {
            eprintln!("Archive hash backfill failed (non-fatal): {e}");
        }

        // Migration: add game_id to deployed_files and update primary key
        if let Err(e) = migrations::migrate_deployed_files_game_id(&pool).await {
            eprintln!("deployed_files migration failed (non-fatal): {e}");
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
}
