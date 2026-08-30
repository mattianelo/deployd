use anyhow::{Context, Result};
use sqlx::sqlite::SqlitePool;

use crate::dlog;

/// Rename legacy game IDs to the new store-agnostic neutral IDs introduced when
/// per-store suffixes (-steam, -goty) were dropped.
///
/// The mapping is applied idempotently: each (old, new) pair is only processed
/// if the old ID still exists in the `games` table.  When both IDs coexist (the
/// user somehow had both a GOG and a Steam copy managed), the variant with more
/// mods wins and the other is deleted.
pub(in crate::core::tracker) async fn migrate_game_ids(pool: &SqlitePool) -> Result<()> {
    let done: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'game_id_migration_v1'")
            .fetch_optional(pool)
            .await
            .context("Failed to read game ID migration state")?;

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
    for table_sql in &[
        "UPDATE mods           SET game_id = ? WHERE game_id = ?",
        "UPDATE deployed_files SET game_id = ? WHERE game_id = ?",
        "UPDATE profiles       SET game_id = ? WHERE game_id = ?",
        "UPDATE tools          SET game_id = ? WHERE game_id = ?",
        "UPDATE mod_groups     SET game_id = ? WHERE game_id = ?",
        "UPDATE vanilla_files  SET game_id = ? WHERE game_id = ?",
        "UPDATE order_snapshots SET game_id = ? WHERE game_id = ?",
    ] {
        sqlx::query(table_sql)
            .bind(new_id)
            .bind(old_id)
            .execute(&mut **tx)
            .await
            .with_context(|| format!("Failed rename step: {table_sql}"))?;
    }
    sqlx::query("UPDATE games SET id = ? WHERE id = ?")
        .bind(new_id)
        .bind(old_id)
        .execute(&mut **tx)
        .await
        .context("Failed to rename games.id")?;

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
    for table_sql in &[
        "DELETE FROM mods           WHERE game_id = ?",
        "DELETE FROM deployed_files WHERE game_id = ?",
        "DELETE FROM profiles       WHERE game_id = ?",
        "DELETE FROM tools          WHERE game_id = ?",
        "DELETE FROM mod_groups     WHERE game_id = ?",
        "DELETE FROM vanilla_files  WHERE game_id = ?",
        "DELETE FROM order_snapshots WHERE game_id = ?",
    ] {
        sqlx::query(table_sql)
            .bind(game_id)
            .execute(&mut **tx)
            .await
            .with_context(|| format!("Failed drop step: {table_sql}"))?;
    }
    sqlx::query("DELETE FROM games WHERE id = ?")
        .bind(game_id)
        .execute(&mut **tx)
        .await
        .context("Failed to delete from games")?;
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
