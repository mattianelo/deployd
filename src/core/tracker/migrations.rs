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
    let existing: std::collections::HashSet<&str> = columns.iter().map(|(n,)| n.as_str()).collect();
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

/// Rename legacy game IDs to the new store-agnostic neutral IDs introduced when
/// per-store suffixes (-steam, -goty) were dropped.
///
/// The mapping is applied idempotently: each (old, new) pair is only processed
/// if the old ID still exists in the `games` table.  When both IDs coexist (the
/// user somehow had both a GOG and a Steam copy managed), the variant with more
/// mods wins and the other is deleted.
pub(super) async fn migrate_game_ids(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'game_id_migration_v1'")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if done.is_some() {
        return Ok(());
    }

    // (old_id, new_id) — multiple old IDs may map to the same new ID.
    const RENAMES: &[(&str, &str)] = &[
        ("skyrimse", "skyrim-se"),
        ("skyrimse-steam", "skyrim-se"),
        ("fallout4", "fallout-4"),
        ("fallout4-steam", "fallout-4"),
        ("falloutnv", "fallout-nv"),
        ("falloutnv-steam", "fallout-nv"),
        ("witcher3", "witcher-3"),
        ("witcher3-goty", "witcher-3"),
        ("witcher3-steam", "witcher-3"),
        ("cyberpunk2077", "cyberpunk-2077"),
        ("cyberpunk2077-steam", "cyberpunk-2077"),
        ("dragonage", "dragon-age"),
        ("dragonage-steam", "dragon-age"),
    ];

    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin game-id migration transaction")?;

    for &(old_id, new_id) in RENAMES {
        let old_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM games WHERE id = ?)")
                .bind(old_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap_or(false);

        if !old_exists {
            continue;
        }

        let new_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM games WHERE id = ?)")
                .bind(new_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap_or(false);

        if new_exists {
            // Both variants exist. Keep the one with more mods; delete the other.
            let old_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE game_id = ?")
                .bind(old_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap_or(0);
            let new_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE game_id = ?")
                .bind(new_id)
                .fetch_one(&mut *tx)
                .await
                .unwrap_or(0);

            if old_count > new_count {
                // Old variant wins — rename old → new, then drop the weaker new entry.
                drop_game_id(&mut tx, new_id).await?;
                rename_game_id(&mut tx, old_id, new_id).await?;
            } else {
                // New variant wins (or tie) — just drop the old entry.
                drop_game_id(&mut tx, old_id).await?;
            }
        } else {
            rename_game_id(&mut tx, old_id, new_id).await?;
        }

        dlog!("[deployd] game-id migration: {} → {}", old_id, new_id);
    }

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('game_id_migration_v1', 'true')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to record game-id migration")?;

    tx.commit()
        .await
        .context("Failed to commit game-id migration")?;

    Ok(())
}

async fn rename_game_id(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    old_id: &str,
    new_id: &str,
) -> Result<()> {
    // Cascade the rename across every table that references game_id.
    for stmt in &[
        format!("UPDATE mods          SET game_id = '{new_id}' WHERE game_id = '{old_id}'"),
        format!("UPDATE deployed_files SET game_id = '{new_id}' WHERE game_id = '{old_id}'"),
        format!("UPDATE profiles      SET game_id = '{new_id}' WHERE game_id = '{old_id}'"),
        format!("UPDATE tools         SET game_id = '{new_id}' WHERE game_id = '{old_id}'"),
        format!("UPDATE mod_groups    SET game_id = '{new_id}' WHERE game_id = '{old_id}'"),
        format!("UPDATE vanilla_files SET game_id = '{new_id}' WHERE game_id = '{old_id}'"),
        format!("UPDATE order_snapshots SET game_id = '{new_id}' WHERE game_id = '{old_id}'"),
        format!("UPDATE games         SET id       = '{new_id}' WHERE id       = '{old_id}'"),
    ] {
        sqlx::query(stmt)
            .execute(&mut **tx)
            .await
            .with_context(|| format!("Failed rename step: {stmt}"))?;
    }

    // Update per-game settings keys.
    rename_settings_key(
        tx,
        &format!("last_profile_{old_id}"),
        &format!("last_profile_{new_id}"),
    )
    .await?;
    rename_settings_key(
        tx,
        &format!("last_deployed_profile_{old_id}"),
        &format!("last_deployed_profile_{new_id}"),
    )
    .await?;

    // Update the last_game_id pointer if it pointed at the old ID.
    sqlx::query("UPDATE settings SET value = ? WHERE key = 'last_game_id' AND value = ?")
        .bind(new_id)
        .bind(old_id)
        .execute(&mut **tx)
        .await
        .context("Failed to update last_game_id setting")?;

    Ok(())
}

async fn drop_game_id(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, game_id: &str) -> Result<()> {
    for stmt in &[
        format!("DELETE FROM mods          WHERE game_id = '{game_id}'"),
        format!("DELETE FROM deployed_files WHERE game_id = '{game_id}'"),
        format!("DELETE FROM profiles      WHERE game_id = '{game_id}'"),
        format!("DELETE FROM tools         WHERE game_id = '{game_id}'"),
        format!("DELETE FROM mod_groups    WHERE game_id = '{game_id}'"),
        format!("DELETE FROM vanilla_files WHERE game_id = '{game_id}'"),
        format!("DELETE FROM order_snapshots WHERE game_id = '{game_id}'"),
        format!("DELETE FROM games         WHERE id       = '{game_id}'"),
    ] {
        sqlx::query(stmt)
            .execute(&mut **tx)
            .await
            .with_context(|| format!("Failed drop step: {stmt}"))?;
    }
    Ok(())
}

async fn rename_settings_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    old_key: &str,
    new_key: &str,
) -> Result<()> {
    // If new key doesn't exist yet, copy old → new.
    let new_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM settings WHERE key = ?)")
            .bind(new_key)
            .fetch_one(&mut **tx)
            .await
            .unwrap_or(false);

    if !new_exists {
        sqlx::query(
            "INSERT INTO settings (key, value)
             SELECT ?, value FROM settings WHERE key = ?",
        )
        .bind(new_key)
        .bind(old_key)
        .execute(&mut **tx)
        .await
        .context("Failed to copy settings key")?;
    }

    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(old_key)
        .execute(&mut **tx)
        .await
        .context("Failed to delete old settings key")?;

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
