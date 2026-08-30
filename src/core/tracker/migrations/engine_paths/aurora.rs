use anyhow::{Context, Result};
use sqlx::sqlite::SqlitePool;

use crate::dlog;
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
pub(in crate::core::tracker) async fn migrate_aurora_file_paths(pool: &SqlitePool) -> Result<()> {
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
        let new_original =
            restrip_aurora_path(&old_original).unwrap_or_else(|| new_lowercase.clone());

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

/// One-shot migration that adds `../` prefix to Witcher 1 mod files stored
/// under `system/`, `launcher/`, or `register/`, routing them to the game root
/// instead of inside `Data/`.
///
/// No cache files are moved — only the DB records are updated. A re-deploy
/// will remove the old `Data/system/…` symlinks and place the files at the
/// correct `System/…` location in the game root.
pub(in crate::core::tracker) async fn migrate_aurora_root_paths(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'aurora_root_migration_v3'")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if done.is_some() {
        return Ok(());
    }

    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT mf.mod_id, mf.game_rel_lowercase, mf.game_rel_original
         FROM mod_files mf
         JOIN mods m ON mf.mod_id = m.id
         WHERE m.game_id = 'witcher-1'
           AND (mf.game_rel_lowercase LIKE 'system/%'
                OR mf.game_rel_lowercase LIKE 'launcher/%'
                OR mf.game_rel_lowercase LIKE 'register/%')",
    )
    .fetch_all(pool)
    .await
    .context("Failed to query witcher-1 mod_files for aurora root migration")?;

    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin aurora root migration transaction")?;

    for (mod_id, old_lowercase, old_original) in &rows {
        let new_lowercase = format!("../{old_lowercase}");
        let new_original = format!("../{old_original}");
        sqlx::query(
            "UPDATE mod_files
             SET game_rel_lowercase = ?,
                 game_rel_original  = ?
             WHERE mod_id = ? AND game_rel_lowercase = ?",
        )
        .bind(&new_lowercase)
        .bind(&new_original)
        .bind(mod_id)
        .bind(old_lowercase)
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!("Failed to update mod_files for mod {mod_id} path {old_lowercase}")
        })?;
    }

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('aurora_root_migration_v3', 'true')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to record aurora root migration")?;

    tx.commit()
        .await
        .context("Failed to commit aurora root migration")?;

    dlog!(
        "[deployd] aurora root migration: {} file(s) re-pathed to game root",
        rows.len()
    );

    Ok(())
}

/// One-shot migration that fixes Witcher 1 mod files incorrectly stored as
/// `override/data/system/…` (or launcher/register variants), routing them to
/// the game root with a `../` prefix instead.
///
/// These paths were created by the pre-fix installer when the archive packaged
/// files under a `Data/system/` top-level folder. No cache files are moved;
/// a re-deploy will clean up the old `Data/Override/Data/system/…` symlinks
/// and place files at the correct `system/…` location under the game root.
pub(in crate::core::tracker) async fn migrate_aurora_data_system_paths(
    pool: &SqlitePool,
) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'aurora_root_migration_v4'")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if done.is_some() {
        return Ok(());
    }

    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT mf.mod_id, mf.game_rel_lowercase, mf.game_rel_original
         FROM mod_files mf
         JOIN mods m ON mf.mod_id = m.id
         WHERE m.game_id = 'witcher-1'
           AND (mf.game_rel_lowercase LIKE 'override/data/system/%'
                OR mf.game_rel_lowercase LIKE 'override/data/launcher/%'
                OR mf.game_rel_lowercase LIKE 'override/data/register/%')",
    )
    .fetch_all(pool)
    .await
    .context("Failed to query witcher-1 mod_files for aurora data-system migration")?;

    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin aurora data-system migration transaction")?;

    // "override/data/" is 14 bytes (all ASCII); strip it and prepend "../".
    const STRIP_LEN: usize = "override/data/".len();
    for (mod_id, old_lowercase, old_original) in &rows {
        let new_lowercase = format!("../{}", &old_lowercase[STRIP_LEN..]);
        let new_original = format!("../{}", &old_original[STRIP_LEN..]);
        sqlx::query(
            "UPDATE mod_files
             SET game_rel_lowercase = ?,
                 game_rel_original  = ?
             WHERE mod_id = ? AND game_rel_lowercase = ?",
        )
        .bind(&new_lowercase)
        .bind(&new_original)
        .bind(mod_id)
        .bind(old_lowercase)
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!("Failed to update mod_files for mod {mod_id} path {old_lowercase}")
        })?;
    }

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('aurora_root_migration_v4', 'true')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to record aurora data-system migration")?;

    tx.commit()
        .await
        .context("Failed to commit aurora data-system migration")?;

    dlog!(
        "[deployd] aurora data-system migration: {} file(s) re-pathed to game root",
        rows.len()
    );

    Ok(())
}

/// Fix Witcher 1 external-file rows whose rooted paths were routed twice.
///
/// The affected paths were passed directly to `route_aurora_paths`, producing
/// values like
/// `../system/../System/Scripts/foo.ws` instead of `../System/Scripts/foo.ws`.
///
/// The malformed key never matched the scanner's output, so the file was
/// reported as external on every scan even after being absorbed into a mod.
///
/// All corrupt rows share the prefix `"../system/../"` (13 chars) because the
/// root branch of `route_aurora_paths` always wraps unrecognised paths in
/// `PathBuf::from("..").join("system").join(stripped)`.
pub(in crate::core::tracker) async fn migrate_aurora_external_file_paths(
    pool: &SqlitePool,
) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'aurora_root_migration_v5'")
            .fetch_optional(pool)
            .await
            .unwrap_or(None);

    if done.is_some() {
        return Ok(());
    }

    // "../system/../" is 13 ASCII chars; SUBSTR is 1-indexed, so offset 14 skips it.
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT mf.mod_id, mf.game_rel_lowercase, mf.game_rel_original
         FROM mod_files mf
         JOIN mods m ON mf.mod_id = m.id
         WHERE m.game_id = 'witcher-1'
           AND mf.game_rel_lowercase LIKE '../system/../%'",
    )
    .fetch_all(pool)
    .await
    .context("Failed to query witcher-1 mod_files for external-file path fix")?;

    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin aurora external-file path migration")?;

    const STRIP_LEN: usize = "../system/../".len(); // 13
    for (mod_id, old_lowercase, old_original) in &rows {
        let new_lowercase = format!("../{}", &old_lowercase[STRIP_LEN..]);
        let new_original = format!("../{}", &old_original[STRIP_LEN..]);
        sqlx::query(
            "UPDATE mod_files
             SET game_rel_lowercase = ?,
                 game_rel_original  = ?
             WHERE mod_id = ? AND game_rel_lowercase = ?",
        )
        .bind(&new_lowercase)
        .bind(&new_original)
        .bind(mod_id)
        .bind(old_lowercase)
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!("Failed to fix mod_files path {old_lowercase} for mod {mod_id}")
        })?;
    }

    // Also fix deployed_files for consistency (rebuilt on next deploy, but clean is better).
    sqlx::query(
        "UPDATE deployed_files
         SET game_rel_lowercase = '../' || SUBSTR(game_rel_lowercase, 14),
             game_rel_original  = '../' || SUBSTR(game_rel_original,  14)
         WHERE game_id = 'witcher-1'
           AND game_rel_lowercase LIKE '../system/../%'",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to fix deployed_files paths for external-file path migration")?;

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('aurora_root_migration_v5', 'true')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to record aurora external-file path migration")?;

    tx.commit()
        .await
        .context("Failed to commit aurora external-file path migration")?;

    dlog!(
        "[deployd] aurora external-file path fix: {} mod_file(s) corrected",
        rows.len()
    );

    Ok(())
}

/// One-shot migration that adds the `../` prefix to Witcher 1 vanilla_files
/// entries for `system/`, `launcher/`, and `register/` paths.
///
/// These entries were stored without the prefix when the vanilla snapshot was
/// taken while Witcher 1 still had `data_subdir = "."`. In that mode, all
/// files (including System/) were walked as data-dir files and stored with
/// bare paths like `"system/foo.dll"`. After the migration to
/// `data_subdir = "Data"`, the external-file scanner generates
/// `"../system/foo.dll"` for those same root-level files, so the vanilla
/// lookup key no longer matched and they were always reported as external.
pub(in crate::core::tracker) async fn migrate_aurora_vanilla_root_paths(
    pool: &SqlitePool,
) -> Result<()> {
    let done: Option<String> = sqlx::query_scalar(
        "SELECT value FROM settings WHERE key = 'aurora_vanilla_root_migration_v1'",
    )
    .fetch_optional(pool)
    .await
    .unwrap_or(None);

    if done.is_some() {
        return Ok(());
    }

    let mut tx = pool
        .begin()
        .await
        .context("Failed to begin aurora vanilla root migration")?;

    let affected = sqlx::query(
        "UPDATE vanilla_files
         SET game_rel_lowercase = '../' || game_rel_lowercase
         WHERE game_id = 'witcher-1'
           AND (  game_rel_lowercase LIKE 'system/%'
               OR game_rel_lowercase LIKE 'launcher/%'
               OR game_rel_lowercase LIKE 'register/%')",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to update vanilla_files paths for witcher-1")?
    .rows_affected();

    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('aurora_vanilla_root_migration_v1', 'true')
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .execute(&mut *tx)
    .await
    .context("Failed to record aurora vanilla root migration")?;

    tx.commit()
        .await
        .context("Failed to commit aurora vanilla root migration")?;

    dlog!(
        "[deployd] aurora vanilla root migration: {} vanilla_file(s) re-pathed to game root",
        affected
    );

    Ok(())
}
