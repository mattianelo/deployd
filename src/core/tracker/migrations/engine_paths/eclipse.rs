use anyhow::{Context, Result};
use sqlx::sqlite::SqlitePool;

use crate::dlog;
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
/// recognised content directory. Non-override paths pass through unchanged.
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
    while let Some(slash) = s.find('/') {
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
pub(in crate::core::tracker) async fn migrate_eclipse_file_paths(pool: &SqlitePool) -> Result<()> {
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
        let new_original =
            restrip_eclipse_path(&old_original).unwrap_or_else(|| new_lowercase.clone());

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
