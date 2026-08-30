use anyhow::{Context, Result};
use sqlx::sqlite::SqlitePool;

use crate::dlog;

pub(in crate::core::tracker) async fn migrate_games_columns(pool: &SqlitePool) -> Result<()> {
    let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(games)")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
        .collect();
    let existing: std::collections::HashSet<&str> = columns.iter().map(|(n,)| n.as_str()).collect();
    for (col, typedef) in &[
        ("wine_prefix", "TEXT"),
        ("engine", "TEXT DEFAULT 'bethesda'"),
        ("custom", "INTEGER DEFAULT 0"),
        ("hidden", "INTEGER DEFAULT 0"),
    ] {
        if !existing.contains(*col) {
            sqlx::query(&format!("ALTER TABLE games ADD COLUMN {col} {typedef}"))
                .execute(pool)
                .await
                .with_context(|| format!("Failed to add required column games.{col}"))?;
        }
    }
    Ok(())
}

pub(in crate::core::tracker) async fn migrate_nexus_columns(pool: &SqlitePool) -> Result<()> {
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
pub(in crate::core::tracker) async fn migrate_version_columns(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> = sqlx::query_scalar(
        "SELECT value FROM settings WHERE key = 'migration_version_columns_done'",
    )
    .fetch_optional(pool)
    .await?;

    if done.is_some() {
        return Ok(());
    }

    sqlx::query(
        "UPDATE mods SET latest_version = version \
         WHERE latest_version IS NULL AND version IS NOT NULL",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "UPDATE mods SET version = NULL \
         WHERE version IS NOT NULL AND version = latest_version",
    )
    .execute(pool)
    .await?;

    sqlx::query("INSERT INTO settings (key, value) VALUES ('migration_version_columns_done', '1')")
        .execute(pool)
        .await?;

    Ok(())
}

pub(in crate::core::tracker) async fn migrate_download_columns(pool: &SqlitePool) -> Result<()> {
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
        ("archive_md5", "TEXT"),
        ("version", "TEXT"),
        ("author", "TEXT"),
        ("hidden", "INTEGER DEFAULT 0"),
    ];

    for (col, col_type) in &new_columns {
        if !existing.contains(*col) {
            let sql = format!("ALTER TABLE download_entries ADD COLUMN {col} {col_type}");
            sqlx::query(&sql).execute(pool).await?;
        }
    }

    Ok(())
}

pub(in crate::core::tracker) async fn migrate_mod_source_metadata_columns(
    pool: &SqlitePool,
) -> Result<()> {
    let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(mods)")
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
        ("archive_md5", "TEXT"),
    ];

    for (col, col_type) in &new_columns {
        if !existing.contains(*col) {
            let sql = format!("ALTER TABLE mods ADD COLUMN {col} {col_type}");
            sqlx::query(&sql).execute(pool).await?;
        }
    }

    Ok(())
}

pub(in crate::core::tracker) async fn backfill_mod_source_metadata(
    pool: &SqlitePool,
) -> Result<()> {
    sqlx::query(
        "UPDATE mods
         SET nexus_file_name = COALESCE(
                 nexus_file_name,
                 (
                     SELECT de.nexus_file_name
                     FROM download_entries de
                     WHERE de.status = 'installed'
                       AND de.nexus_mod_id = mods.nexus_mod_id
                       AND de.nexus_file_id = mods.nexus_file_id
                       AND de.nexus_domain = mods.nexus_domain
                       AND de.nexus_file_name IS NOT NULL
                     ORDER BY de.metadata_fetched DESC
                     LIMIT 1
                 )
             ),
             nexus_is_primary = CASE
                 WHEN COALESCE(nexus_is_primary, 0) = 0 THEN COALESCE(
                     (
                         SELECT de.nexus_is_primary
                         FROM download_entries de
                         WHERE de.status = 'installed'
                           AND de.nexus_mod_id = mods.nexus_mod_id
                           AND de.nexus_file_id = mods.nexus_file_id
                           AND de.nexus_domain = mods.nexus_domain
                           AND COALESCE(de.nexus_is_primary, 0) != 0
                         LIMIT 1
                     ),
                     0
                 )
                 ELSE nexus_is_primary
             END,
             archive_md5 = COALESCE(
                 archive_md5,
                 (
                     SELECT de.archive_md5
                     FROM download_entries de
                     WHERE de.status = 'installed'
                       AND de.nexus_mod_id = mods.nexus_mod_id
                       AND de.nexus_file_id = mods.nexus_file_id
                       AND de.nexus_domain = mods.nexus_domain
                       AND de.archive_md5 IS NOT NULL
                     ORDER BY de.metadata_fetched DESC
                     LIMIT 1
                 )
             )
         WHERE nexus_mod_id IS NOT NULL
           AND nexus_file_id IS NOT NULL
           AND nexus_domain IS NOT NULL",
    )
    .execute(pool)
    .await
    .context("Failed to backfill mod source metadata")?;

    Ok(())
}

pub(in crate::core::tracker) async fn migrate_group_columns(pool: &SqlitePool) -> Result<()> {
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

pub(in crate::core::tracker) async fn migrate_install_target_column(
    pool: &SqlitePool,
) -> Result<()> {
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

pub(in crate::core::tracker) async fn migrate_notes_column(pool: &SqlitePool) -> Result<()> {
    let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(mods)")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
        .collect();
    if !columns.iter().any(|(n,)| n == "notes") {
        sqlx::query("ALTER TABLE mods ADD COLUMN notes TEXT")
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Rows created before this migration have NULL size/mtime and are deleted to force a fresh snapshot.
pub(in crate::core::tracker) async fn migrate_vanilla_files_columns(
    pool: &SqlitePool,
) -> Result<()> {
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

    sqlx::query("DELETE FROM vanilla_files WHERE file_size IS NULL OR mtime_secs IS NULL")
        .execute(pool)
        .await?;

    Ok(())
}

pub(in crate::core::tracker) async fn backfill_plugin_masters(pool: &SqlitePool) -> Result<()> {
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
                    sqlx::query(
                        "INSERT OR IGNORE INTO plugin_masters (plugin_id, master) VALUES (?, ?)",
                    )
                    .bind(plugin_id)
                    .bind(master)
                    .execute(pool)
                    .await
                    .with_context(|| {
                        format!("Failed to backfill master '{master}' for plugin '{plugin_id}'")
                    })?;
                }
            }
            Err(e) => {
                dlog!("[debug] Could not read masters from {cache_path}: {e}");
            }
        }
    }

    Ok(())
}

pub(in crate::core::tracker) async fn backfill_download_statuses(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'dl_status_backfill_v1'")
            .fetch_optional(pool)
            .await
            .context("Failed to read download status backfill state")?;

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

/// Re-apply the canonical Aurora Override/ flattening to a stored path string.
///
/// The Aurora engine (NWN-based) resolves Override resources by filename only,
/// so all Override files must be stored flat as `override/<filename>`.  Files
/// under `system/` or `modules/` are left unchanged.
///
/// Mirrors the logic in `installer/paths::route_aurora_paths`.
pub(in crate::core::tracker) async fn backfill_archive_hashes(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'archive_hash_backfill_v1'")
            .fetch_optional(pool)
            .await
            .context("Failed to read archive hash backfill state")?;

    if done.is_some() {
        return Ok(());
    }

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
    .context("Failed to query archives for hash backfill")?;

    for (mod_id, archive_path) in rows {
        let path = std::path::PathBuf::from(&archive_path);
        if !path.exists() {
            continue;
        }
        // Hash on a blocking thread — archive files can be several GB.
        let hash =
            tokio::task::spawn_blocking(move || crate::core::archive::hash_archive_file(&path))
                .await
                .context("Archive hash backfill worker failed")?
                .with_context(|| format!("Failed to hash archive for mod '{mod_id}'"))?;

        sqlx::query("UPDATE mods SET archive_hash = ? WHERE id = ?")
            .bind(&hash)
            .bind(&mod_id)
            .execute(pool)
            .await
            .with_context(|| format!("Failed to save archive hash for mod '{mod_id}'"))?;
    }

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('archive_hash_backfill_v1', 'true')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(pool)
    .await
    .context("Failed to record archive hash backfill")?;

    Ok(())
}

pub(in crate::core::tracker) async fn migrate_deployed_files_game_id(
    pool: &SqlitePool,
) -> Result<()> {
    let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(deployed_files)")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
        .collect();

    if columns.iter().any(|(n,)| n == "game_id") {
        return Ok(());
    }

    let mut tx = pool.begin().await?;

    sqlx::query(
        "CREATE TABLE deployed_files_v2 (
            game_id TEXT NOT NULL DEFAULT '',
            game_rel_lowercase TEXT NOT NULL,
            game_rel_original TEXT NOT NULL,
            mod_id TEXT NOT NULL,
            cache_path TEXT NOT NULL,
            PRIMARY KEY (game_id, game_rel_lowercase)
        )",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to create deployed_files_v2")?;

    sqlx::query(
        "INSERT OR IGNORE INTO deployed_files_v2
            (game_id, game_rel_lowercase, game_rel_original, mod_id, cache_path)
         SELECT m.game_id, d.game_rel_lowercase, d.game_rel_original, d.mod_id, d.cache_path
         FROM deployed_files d
         JOIN mods m ON d.mod_id = m.id",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to backfill deployed_files_v2")?;

    sqlx::query("DROP TABLE deployed_files")
        .execute(&mut *tx)
        .await
        .context("Failed to drop old deployed_files")?;

    sqlx::query("ALTER TABLE deployed_files_v2 RENAME TO deployed_files")
        .execute(&mut *tx)
        .await
        .context("Failed to rename deployed_files_v2")?;

    tx.commit()
        .await
        .context("Failed to commit deployed_files migration")?;

    dlog!("[deployd] deployed_files migrated to include game_id");
    Ok(())
}

pub(in crate::core::tracker) async fn migrate_fomod_selections_column(
    pool: &SqlitePool,
) -> Result<()> {
    add_column_if_missing(pool, "mods", "fomod_selections", "TEXT").await?;
    Ok(())
}

pub(in crate::core::tracker) async fn migrate_archive_path_column(pool: &SqlitePool) -> Result<()> {
    add_column_if_missing(pool, "mods", "archive_path", "TEXT").await?;
    Ok(())
}

pub(in crate::core::tracker) async fn migrate_profile_save_mode_column(
    pool: &SqlitePool,
) -> Result<()> {
    add_column_if_missing(
        pool,
        "profiles",
        "save_mode",
        "TEXT NOT NULL DEFAULT 'global'",
    )
    .await?;
    Ok(())
}

pub(in crate::core::tracker) async fn migrate_tools_working_dir_column(
    pool: &SqlitePool,
) -> Result<()> {
    add_column_if_missing(pool, "tools", "working_dir", "TEXT DEFAULT ''").await
}

async fn add_column_if_missing(
    pool: &SqlitePool,
    table: &str,
    column: &str,
    definition: &str,
) -> Result<()> {
    let query = format!("PRAGMA table_info({table})");
    let columns: Vec<(i32, String, String, i32, Option<String>, i32)> =
        sqlx::query_as(&query).fetch_all(pool).await?;
    if columns.iter().any(|row| row.1 == column) {
        return Ok(());
    }

    let statement = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
    sqlx::query(&statement)
        .execute(pool)
        .await
        .with_context(|| format!("Failed to add required column {table}.{column}"))?;
    Ok(())
}

pub(in crate::core::tracker) async fn migrate_group_color_column(pool: &SqlitePool) -> Result<()> {
    let columns: Vec<(String,)> = sqlx::query_as("PRAGMA table_info(mod_groups)")
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|row: (i32, String, String, i32, Option<String>, i32)| (row.1,))
        .collect();
    if !columns.iter().any(|(name,)| name == "color") {
        sqlx::query("ALTER TABLE mod_groups ADD COLUMN color TEXT")
            .execute(pool)
            .await
            .context("Failed to add color column to mod_groups")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;

    use crate::core::tracker::Tracker;

    #[tokio::test]
    async fn backfills_installed_mod_source_metadata_from_download_entries() -> Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;

        sqlx::query(
            "INSERT INTO games (id, title, path, data_subdir)
             VALUES ('g', 'Game', '/tmp/game', 'Data')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO mods
             (id, game_id, name, priority, nexus_mod_id, nexus_file_id, nexus_domain)
             VALUES ('mod-1', 'g', 'Mod', 0, 4598, 123, 'fallout4')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO download_entries
             (id, mod_name, nexus_mod_id, nexus_file_id, nexus_domain, metadata_fetched,
              nexus_file_name, nexus_is_primary, status, archive_md5)
             VALUES ('download-1', 'Mod', 4598, 123, 'fallout4', 1,
                     'Main File', 1, 'installed', 'md5')",
        )
        .execute(&tracker.pool)
        .await?;

        backfill_mod_source_metadata(&tracker.pool).await?;

        let row: (Option<String>, bool, Option<String>) = sqlx::query_as(
            "SELECT nexus_file_name, nexus_is_primary, archive_md5
             FROM mods WHERE id = 'mod-1'",
        )
        .fetch_one(&tracker.pool)
        .await?;

        assert_eq!(row.0.as_deref(), Some("Main File"));
        assert!(row.1, "primary flag should be backfilled");
        assert_eq!(row.2.as_deref(), Some("md5"));
        Ok(())
    }

    async fn isolated_pool() -> Result<SqlitePool> {
        Ok(SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?)
    }

    #[tokio::test]
    async fn upgrades_legacy_deployed_files_with_game_identity() -> Result<()> {
        let pool = isolated_pool().await?;
        sqlx::query("CREATE TABLE mods (id TEXT PRIMARY KEY, game_id TEXT NOT NULL)")
            .execute(&pool)
            .await?;
        sqlx::query(
            "CREATE TABLE deployed_files (
                game_rel_lowercase TEXT PRIMARY KEY,
                game_rel_original TEXT NOT NULL,
                mod_id TEXT NOT NULL,
                cache_path TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO mods (id, game_id) VALUES ('mod-1', 'game-1')")
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO deployed_files
             (game_rel_lowercase, game_rel_original, mod_id, cache_path)
             VALUES ('data/file.txt', 'Data/File.txt', 'mod-1', '/cache/file.txt')",
        )
        .execute(&pool)
        .await?;

        migrate_deployed_files_game_id(&pool).await?;

        let row: (String, String, String) =
            sqlx::query_as("SELECT game_id, game_rel_original, cache_path FROM deployed_files")
                .fetch_one(&pool)
                .await?;
        assert_eq!(row.0, "game-1");
        assert_eq!(row.1, "Data/File.txt");
        assert_eq!(row.2, "/cache/file.txt");
        Ok(())
    }

    #[tokio::test]
    async fn leaves_partially_upgraded_deployed_files_unchanged() -> Result<()> {
        let pool = isolated_pool().await?;
        sqlx::query(
            "CREATE TABLE deployed_files (
                game_id TEXT NOT NULL,
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
            "INSERT INTO deployed_files
             (game_id, game_rel_lowercase, game_rel_original, mod_id, cache_path)
             VALUES ('game-1', 'data/file.txt', 'Data/File.txt', 'mod-1', '/cache/file.txt')",
        )
        .execute(&pool)
        .await?;

        migrate_deployed_files_game_id(&pool).await?;

        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployed_files")
            .fetch_one(&pool)
            .await?;
        assert_eq!(count, 1);
        Ok(())
    }

    #[tokio::test]
    async fn rolls_back_malformed_deployed_files_upgrade() -> Result<()> {
        let pool = isolated_pool().await?;
        sqlx::query("CREATE TABLE mods (id TEXT PRIMARY KEY)")
            .execute(&pool)
            .await?;
        sqlx::query(
            "CREATE TABLE deployed_files (
                game_rel_lowercase TEXT PRIMARY KEY,
                game_rel_original TEXT NOT NULL,
                mod_id TEXT NOT NULL,
                cache_path TEXT NOT NULL
            )",
        )
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO mods (id) VALUES ('mod-1')")
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO deployed_files
             (game_rel_lowercase, game_rel_original, mod_id, cache_path)
             VALUES ('data/file.txt', 'Data/File.txt', 'mod-1', '/cache/file.txt')",
        )
        .execute(&pool)
        .await?;

        let error = migrate_deployed_files_game_id(&pool)
            .await
            .expect_err("malformed mods schema must fail the migration");

        assert!(
            error.to_string().contains("backfill deployed_files_v2"),
            "unexpected error: {error:#}"
        );
        let old_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM deployed_files")
            .fetch_one(&pool)
            .await?;
        assert_eq!(old_count, 1);
        let replacement_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM sqlite_master
                WHERE type = 'table' AND name = 'deployed_files_v2'
            )",
        )
        .fetch_one(&pool)
        .await?;
        assert!(
            !replacement_exists,
            "failed migration must roll back its table"
        );
        Ok(())
    }

    #[tokio::test]
    async fn reports_required_schema_ddl_failure() -> Result<()> {
        let pool = isolated_pool().await?;

        let error = add_column_if_missing(&pool, "sqlite_schema", "required", "TEXT")
            .await
            .expect_err("required schema DDL must not be discarded");

        assert!(error.to_string().contains("sqlite_schema.required"));
        Ok(())
    }
}
