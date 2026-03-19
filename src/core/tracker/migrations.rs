use anyhow::{Context, Result};
use sqlx::sqlite::SqlitePool;

use crate::dlog;

pub(super) async fn migrate_games_columns(pool: &SqlitePool) -> Result<()> {
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

pub(super) async fn migrate_nexus_columns(pool: &SqlitePool) -> Result<()> {
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
pub(super) async fn migrate_version_columns(pool: &SqlitePool) -> Result<()> {
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
    Ok(())
}

pub(super) async fn migrate_download_columns(pool: &SqlitePool) -> Result<()> {
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

pub(super) async fn migrate_group_columns(pool: &SqlitePool) -> Result<()> {
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

pub(super) async fn migrate_install_target_column(pool: &SqlitePool) -> Result<()> {
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

pub(super) async fn migrate_notes_column(pool: &SqlitePool) -> Result<()> {
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
pub(super) async fn migrate_vanilla_files_columns(pool: &SqlitePool) -> Result<()> {
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

pub(super) async fn backfill_plugin_masters(pool: &SqlitePool) -> Result<()> {
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

pub(super) async fn backfill_download_statuses(pool: &SqlitePool) -> Result<()> {
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

pub(super) async fn backfill_archive_hashes(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'archive_hash_backfill_v1'")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

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

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('archive_hash_backfill_v1', 'true')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(pool)
    .await
    .context("Failed to record archive hash backfill")?;

    Ok(())
}

pub(super) async fn migrate_deployed_files_game_id(pool: &SqlitePool) -> Result<()> {
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
