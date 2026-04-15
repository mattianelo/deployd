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

/// Re-apply the canonical Aurora Override/ flattening to a stored path string.
///
/// The Aurora engine (NWN-based) resolves Override resources by filename only,
/// so all Override files must be stored flat as `override/<filename>`.  Files
/// under `system/` or `modules/` are left unchanged.
///
/// Mirrors the logic in `installer/paths::route_aurora_paths`.
fn restrip_aurora_path(path: &str) -> Option<String> {
    let lower = path.to_lowercase();
    // system/ and modules/ pass through unchanged.
    if lower.starts_with("system/") || lower.starts_with("modules/") {
        return Some(path.to_owned());
    }
    // Directory sentinels (trailing '/') have no filename — drop them.
    if path.ends_with('/') {
        return None;
    }
    // Extract the bare filename and place it flat inside Override/.
    let filename = std::path::Path::new(path).file_name()?.to_str()?;
    Some(format!("Override/{filename}"))
}

/// One-shot migration that re-applies correct Aurora Override/ path stripping to
/// all Witcher 1 mod files stored with old (pre-fix) paths.
///
/// For each affected `mod_files` row the function:
/// 1. Derives the corrected `game_rel_lowercase` / `game_rel_original`.
/// 2. Moves the cached file on disk to the new path inside the mod cache dir.
/// 3. Updates the DB row.
///
/// Disk moves are attempted before the transaction commits.  If the process is
/// interrupted the on-disk state may be ahead of the DB, but the next run of the
/// migration will not re-run (the settings guard is written inside the same
/// transaction) and re-deploy will recreate hard-links from wherever the cache
/// file ended up.
pub(super) async fn migrate_aurora_file_paths(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'aurora_path_migration_v2'")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if done.is_some() {
        return Ok(());
    }

    // Collect all mod_files rows belonging to witcher-1 mods.
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT mf.mod_id, mf.game_rel_lowercase, mf.game_rel_original, mf.cache_path
         FROM mod_files mf
         JOIN mods m ON mf.mod_id = m.id
         WHERE m.game_id = 'witcher-1'",
    )
    .fetch_all(pool)
    .await
    .context("Failed to query witcher-1 mod_files for aurora migration")?;

    if rows.is_empty() {
        // Nothing to do; still write the guard so this never runs again.
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('aurora_path_migration_v2', 'true')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .execute(pool)
        .await
        .context("Failed to record aurora path migration")?;
        return Ok(());
    }

    // Pre-compute all path changes; perform disk moves outside the transaction.
    struct FileChange {
        mod_id: String,
        old_lowercase: String,
        new_lowercase: String,
        new_original: String,
        old_cache_path: String,
        new_cache_path: String,
    }

    let mut changes: Vec<FileChange> = Vec::new();
    for (mod_id, old_lowercase, old_original, old_cache_path) in rows {
        let Some(new_lowercase) = restrip_aurora_path(&old_lowercase) else {
            // Directory sentinel with no filename — drop the row entirely.
            continue;
        };
        let new_original = restrip_aurora_path(&old_original)
            .unwrap_or_else(|| new_lowercase.clone());

        if new_lowercase == old_lowercase {
            continue; // path already correct
        }

        // Derive new cache path: replace the trailing relative portion.
        // cache_path is an absolute path; the relative part starts where the
        // mod-cache root ends (i.e. the old_lowercase suffix).
        let new_cache_path = if old_cache_path.ends_with(&old_lowercase) {
            let base = &old_cache_path[..old_cache_path.len() - old_lowercase.len()];
            format!("{base}{new_lowercase}")
        } else {
            // Fallback: can't derive; leave cache_path unchanged — deploy will
            // still find the file if it wasn't moved.
            old_cache_path.clone()
        };

        changes.push(FileChange {
            mod_id,
            old_lowercase,
            new_lowercase,
            new_original,
            old_cache_path,
            new_cache_path,
        });
    }

    dlog!(
        "[deployd] aurora path migration: {} file(s) to repath",
        changes.len()
    );

    // Move files on disk before touching the DB.
    for change in &changes {
        if change.old_cache_path == change.new_cache_path {
            continue;
        }
        let old = std::path::Path::new(&change.old_cache_path);
        let new = std::path::Path::new(&change.new_cache_path);
        if !old.exists() {
            continue;
        }
        if let Some(parent) = new.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            dlog!(
                "[deployd] aurora migration: failed to create dir {}: {e}",
                parent.display()
            );
            continue;
        }
        if let Err(e) = std::fs::rename(old, new) {
            dlog!(
                "[deployd] aurora migration: failed to move {} → {}: {e}",
                old.display(),
                new.display()
            );
        }
    }

    // Update DB records and write guard in a single transaction.
    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin aurora path migration transaction")?;

    for change in &changes {
        sqlx::query(
            "UPDATE mod_files
             SET game_rel_lowercase = ?,
                 game_rel_original  = ?,
                 cache_path         = ?
             WHERE mod_id = ? AND game_rel_lowercase = ?",
        )
        .bind(&change.new_lowercase)
        .bind(&change.new_original)
        .bind(&change.new_cache_path)
        .bind(&change.mod_id)
        .bind(&change.old_lowercase)
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!(
                "Failed to update mod_files for mod {} path {}",
                change.mod_id, change.old_lowercase
            )
        })?;
    }

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('aurora_path_migration_v2', 'true')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to record aurora path migration")?;

    tx.commit()
        .await
        .context("Failed to commit aurora path migration")?;

    dlog!("[deployd] aurora path migration complete");
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

/// Recognised first-level content directories inside Dragon Age's
/// `packages/core/override/`.  Must stay in sync with the list in
/// `core/game/eclipse.rs`.
const ECLIPSE_CONTENT_DIRS: &[&str] = &[
    "2da",
    "animationevents",
    "animations",
    "areas",
    "characters",
    "conversations",
    "creatures",
    "environments",
    "gui",
    "items",
    "levels",
    "lights",
    "materials",
    "models",
    "movies",
    "plots",
    "quests",
    "scripts",
    "sound",
    "sounds",
    "spells",
    "store",
    "textures",
    "triggers",
    "vfx",
];

/// Re-apply Eclipse wrapper stripping to a stored `packages/core/override/…` path.
///
/// Strips leading unrecognised wrapper directories from the portion after
/// `packages/core/override/`, preserving the structure below the first
/// recognised content directory.  Non-override paths pass through unchanged.
/// Directory sentinels (trailing `/`) are dropped.
fn restrip_eclipse_path(path: &str) -> Option<String> {
    const OVERRIDE_PREFIX: &str = "packages/core/override/";
    let lower = path.to_lowercase();

    if !lower.starts_with(OVERRIDE_PREFIX) {
        // addins/, settings/, ~docs~/, etc. — pass through as-is.
        if path.ends_with('/') {
            return None; // drop directory sentinels
        }
        return Some(path.to_owned());
    }

    // Drop directory sentinels inside override/ — override needs no pre-created subdirs.
    if path.ends_with('/') {
        return None;
    }

    let after_prefix = &path[OVERRIDE_PREFIX.len()..];
    // Strip leading unrecognised wrappers (same loop as `strip_eclipse_override_wrappers`).
    let mut s = after_prefix.to_owned();
    loop {
        let Some(slash) = s.find('/') else {
            break; // bare filename
        };
        let first = &s[..slash];
        if ECLIPSE_CONTENT_DIRS.contains(&first.to_lowercase().as_str()) {
            break; // recognised content dir — stop
        }
        let rest = &s[slash + 1..];
        if rest.is_empty() {
            s = String::new();
            break;
        }
        s = rest.to_owned();
    }

    Some(format!("{OVERRIDE_PREFIX}{s}"))
}

/// One-shot migration that re-applies correct Eclipse `packages/core/override/`
/// wrapper stripping to all Dragon Age mod files stored with old (unwrapped) paths.
///
/// Mirrors `migrate_aurora_file_paths` in structure and behaviour.
pub(super) async fn migrate_eclipse_file_paths(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'eclipse_path_migration_v1'")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if done.is_some() {
        return Ok(());
    }

    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT mf.mod_id, mf.game_rel_lowercase, mf.game_rel_original, mf.cache_path
         FROM mod_files mf
         JOIN mods m ON mf.mod_id = m.id
         WHERE m.game_id = 'dragon-age'",
    )
    .fetch_all(pool)
    .await
    .context("Failed to query dragon-age mod_files for eclipse migration")?;

    if rows.is_empty() {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES ('eclipse_path_migration_v1', 'true')
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .execute(pool)
        .await
        .context("Failed to record eclipse path migration")?;
        return Ok(());
    }

    struct FileChange {
        mod_id: String,
        old_lowercase: String,
        new_lowercase: String,
        new_original: String,
        old_cache_path: String,
        new_cache_path: String,
    }

    let mut changes: Vec<FileChange> = Vec::new();
    for (mod_id, old_lowercase, old_original, old_cache_path) in rows {
        let Some(new_lowercase) = restrip_eclipse_path(&old_lowercase) else {
            continue; // directory sentinel — drop
        };
        let new_original = restrip_eclipse_path(&old_original)
            .unwrap_or_else(|| new_lowercase.clone());

        if new_lowercase == old_lowercase {
            continue; // already correct
        }

        let new_cache_path = if old_cache_path.ends_with(&old_lowercase) {
            let base = &old_cache_path[..old_cache_path.len() - old_lowercase.len()];
            format!("{base}{new_lowercase}")
        } else {
            old_cache_path.clone()
        };

        changes.push(FileChange {
            mod_id,
            old_lowercase,
            new_lowercase,
            new_original,
            old_cache_path,
            new_cache_path,
        });
    }

    dlog!(
        "[deployd] eclipse path migration: {} file(s) to repath",
        changes.len()
    );

    for change in &changes {
        if change.old_cache_path == change.new_cache_path {
            continue;
        }
        let old = std::path::Path::new(&change.old_cache_path);
        let new = std::path::Path::new(&change.new_cache_path);
        if !old.exists() {
            continue;
        }
        if let Some(parent) = new.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            dlog!(
                "[deployd] eclipse migration: failed to create dir {}: {e}",
                parent.display()
            );
            continue;
        }
        if let Err(e) = std::fs::rename(old, new) {
            dlog!(
                "[deployd] eclipse migration: failed to move {} → {}: {e}",
                old.display(),
                new.display()
            );
        }
    }

    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin eclipse path migration transaction")?;

    for change in &changes {
        sqlx::query(
            "UPDATE mod_files
             SET game_rel_lowercase = ?,
                 game_rel_original  = ?,
                 cache_path         = ?
             WHERE mod_id = ? AND game_rel_lowercase = ?",
        )
        .bind(&change.new_lowercase)
        .bind(&change.new_original)
        .bind(&change.new_cache_path)
        .bind(&change.mod_id)
        .bind(&change.old_lowercase)
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!(
                "Failed to update mod_files for mod {} path {}",
                change.mod_id, change.old_lowercase
            )
        })?;
    }

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('eclipse_path_migration_v1', 'true')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to record eclipse path migration")?;

    tx.commit()
        .await
        .context("Failed to commit eclipse path migration")?;

    dlog!("[deployd] eclipse path migration complete");
    Ok(())
}
