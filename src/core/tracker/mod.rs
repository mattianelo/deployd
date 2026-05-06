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
pub mod vanilla_backups;

#[derive(Clone)]
pub struct Tracker {
    pub(super) pool: SqlitePool,
}

impl std::fmt::Debug for Tracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracker").finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub struct PersistedGame {
    pub id: String,
    pub title: String,
    pub path: std::path::PathBuf,
    pub data_subdir: String,
    pub engine: String,
    pub wine_prefix: Option<std::path::PathBuf>,
    pub custom: bool,
}

#[derive(Debug, Default)]
pub struct OverrideInfo {
    pub overrides: usize,
    pub overridden_by: usize,
    pub override_files: Vec<String>,
    pub overridden_files: Vec<String>,
    pub conflicting_mod_names: Vec<String>,
    pub conflicted_by_mod_names: Vec<String>,
}

impl Tracker {
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
            "CREATE TABLE IF NOT EXISTS vanilla_backups (
                game_id       TEXT NOT NULL,
                game_rel_path TEXT NOT NULL,
                backup_path   TEXT NOT NULL,
                PRIMARY KEY (game_id, game_rel_path)
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

        migrations::migrate_game_ids(&pool).await?;
        migrations::migrate_games_columns(&pool).await?;
        migrations::migrate_nexus_columns(&pool).await?;
        migrations::migrate_download_columns(&pool).await?;
        migrations::migrate_vanilla_files_columns(&pool).await?;
        migrations::migrate_group_columns(&pool).await?;
        migrations::migrate_install_target_column(&pool).await?;
        migrations::migrate_notes_column(&pool).await?;

        if let Err(e) = migrations::migrate_version_columns(&pool).await {
            eprintln!("Version column migration failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::backfill_plugin_masters(&pool).await {
            eprintln!("Plugin master backfill failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::backfill_download_statuses(&pool).await {
            eprintln!("Download status backfill failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::backfill_archive_hashes(&pool).await {
            eprintln!("Archive hash backfill failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_deployed_files_game_id(&pool).await {
            eprintln!("deployed_files migration failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_aurora_file_paths(&pool).await {
            eprintln!("Aurora path migration failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_aurora_root_paths(&pool).await {
            eprintln!("Aurora root path migration failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_aurora_data_system_paths(&pool).await {
            eprintln!("Aurora data-system path migration failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_aurora_external_file_paths(&pool).await {
            eprintln!("Aurora external-file path fix failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_aurora_vanilla_root_paths(&pool).await {
            eprintln!("Aurora vanilla root path migration failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_eclipse_file_paths(&pool).await {
            eprintln!("Eclipse path migration failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_fomod_selections_column(&pool).await {
            eprintln!("FOMOD selections column migration failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_archive_path_column(&pool).await {
            eprintln!("archive_path column migration failed (non-fatal): {e}");
        }
        if let Err(e) = migrations::migrate_group_color_column(&pool).await {
            eprintln!("group color column migration failed (non-fatal): {e}");
        }

        let _ =
            sqlx::query("ALTER TABLE profiles ADD COLUMN save_mode TEXT NOT NULL DEFAULT 'global'")
                .execute(&pool)
                .await;

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
