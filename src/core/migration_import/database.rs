use anyhow::{Context, Result};
use sqlx::{Row, Sqlite, Transaction};

use crate::core::migration_bundle::ExportManifest;
use crate::models::game::{Game, GameEngine};

use super::filesystem::{ImportPaths, rewrite_backup_path, rewrite_cache_path_for_row};

fn game_engine_db_value(engine: &GameEngine) -> &'static str {
    match engine {
        GameEngine::REDEngine => "redengine",
        GameEngine::Eclipse => "eclipse",
        GameEngine::Aurora => "aurora",
        GameEngine::Bethesda => "bethesda",
    }
}

pub(super) async fn import_database_rows(
    tx: &mut Transaction<'_, Sqlite>,
    export_pool: &sqlx::SqlitePool,
    manifest: &ExportManifest,
    game: &Game,
    import_paths: &ImportPaths,
) -> Result<()> {
    import_game_row(tx, game).await?;
    import_mod_groups(tx, export_pool).await?;
    import_mods(tx, export_pool).await?;
    import_mod_files(tx, export_pool, import_paths).await?;
    import_plugins(tx, export_pool).await?;
    import_plugin_masters(tx, export_pool).await?;
    import_deployed_files(tx, export_pool, import_paths).await?;
    import_profiles(tx, export_pool).await?;
    import_profile_mods(tx, export_pool).await?;
    import_profile_plugins(tx, export_pool).await?;
    import_vanilla_files(tx, export_pool).await?;
    import_order_snapshots(tx, export_pool).await?;
    import_order_snapshot_entries(tx, export_pool).await?;
    import_vanilla_backups(tx, export_pool, import_paths).await?;
    import_download_entries(tx, export_pool).await?;
    backfill_imported_mod_source_metadata(tx).await?;
    import_settings(tx, export_pool, &manifest.game_id).await?;
    Ok(())
}

async fn backfill_imported_mod_source_metadata(tx: &mut Transaction<'_, Sqlite>) -> Result<()> {
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
    .execute(&mut **tx)
    .await
    .context("Failed to backfill imported mod source metadata")?;
    Ok(())
}

async fn import_game_row(tx: &mut Transaction<'_, Sqlite>, game: &Game) -> Result<()> {
    sqlx::query(
        "INSERT INTO games (id, title, path, data_subdir, wine_prefix, engine, custom, hidden)
         VALUES (?, ?, ?, ?, ?, ?, 1, 0)",
    )
    .bind(&game.id)
    .bind(&game.title)
    .bind(game.path.to_string_lossy().as_ref())
    .bind(&game.data_subdir)
    .bind(
        game.wine_prefix
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
    )
    .bind(game_engine_db_value(&game.engine))
    .execute(&mut **tx)
    .await
    .context("Failed to import game row")?;
    Ok(())
}

async fn import_mod_groups(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    let rows = sqlx::query("SELECT id, game_id, name, position, collapsed, color FROM mod_groups")
        .fetch_all(pool)
        .await
        .context("Failed to read exported mod groups")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO mod_groups (id, game_id, name, position, collapsed, color)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("id"))
        .bind(row.get::<String, _>("game_id"))
        .bind(row.get::<String, _>("name"))
        .bind(row.get::<f64, _>("position"))
        .bind(row.get::<i32, _>("collapsed"))
        .bind(row.get::<Option<String>, _>("color"))
        .execute(&mut **tx)
        .await
        .context("Failed to import mod group")?;
    }
    Ok(())
}

async fn import_mods(tx: &mut Transaction<'_, Sqlite>, pool: &sqlx::SqlitePool) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, game_id, name, archive_hash, archive_path, installed_at, enabled, priority,
                nexus_mod_id, nexus_file_id, nexus_domain, version, author, latest_version,
                nexus_description, group_id, install_target, notes, fomod_selections,
                nexus_file_name, nexus_is_primary, archive_md5
         FROM mods",
    )
    .fetch_all(pool)
    .await
    .context("Failed to read exported mods")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO mods
             (id, game_id, name, archive_hash, archive_path, installed_at, enabled, priority,
              nexus_mod_id, nexus_file_id, nexus_domain, version, author, latest_version,
              nexus_description, group_id, install_target, notes, fomod_selections,
              nexus_file_name, nexus_is_primary, archive_md5)
             VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("id"))
        .bind(row.get::<Option<String>, _>("game_id"))
        .bind(row.get::<Option<String>, _>("name"))
        .bind(row.get::<Option<String>, _>("archive_hash"))
        .bind(row.get::<Option<String>, _>("installed_at"))
        .bind(row.get::<bool, _>("enabled"))
        .bind(row.get::<i32, _>("priority"))
        .bind(row.get::<Option<i64>, _>("nexus_mod_id"))
        .bind(row.get::<Option<i64>, _>("nexus_file_id"))
        .bind(row.get::<Option<String>, _>("nexus_domain"))
        .bind(row.get::<Option<String>, _>("version"))
        .bind(row.get::<Option<String>, _>("author"))
        .bind(row.get::<Option<String>, _>("latest_version"))
        .bind(row.get::<Option<String>, _>("nexus_description"))
        .bind(row.get::<Option<String>, _>("group_id"))
        .bind(row.get::<Option<String>, _>("install_target"))
        .bind(row.get::<Option<String>, _>("notes"))
        .bind(row.get::<Option<String>, _>("fomod_selections"))
        .bind(row.get::<Option<String>, _>("nexus_file_name"))
        .bind(row.get::<bool, _>("nexus_is_primary"))
        .bind(row.get::<Option<String>, _>("archive_md5"))
        .execute(&mut **tx)
        .await
        .context("Failed to import mod")?;
    }
    Ok(())
}

async fn import_mod_files(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
    import_paths: &ImportPaths,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT mod_id, game_rel_lowercase, game_rel_original, cache_path FROM mod_files",
    )
    .fetch_all(pool)
    .await
    .context("Failed to read exported mod files")?;
    for row in rows {
        let game_rel = row.get::<String, _>("game_rel_lowercase");
        let cache_path = row.get::<Option<String>, _>("cache_path");
        let rewritten = cache_path
            .as_deref()
            .map(|path| rewrite_cache_path_for_row(path, &game_rel, import_paths))
            .transpose()?;
        sqlx::query(
            "INSERT INTO mod_files
             (mod_id, game_rel_lowercase, game_rel_original, cache_path)
             VALUES (?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("mod_id"))
        .bind(game_rel)
        .bind(row.get::<String, _>("game_rel_original"))
        .bind(rewritten)
        .execute(&mut **tx)
        .await
        .context("Failed to import mod file")?;
    }
    Ok(())
}

async fn import_plugins(tx: &mut Transaction<'_, Sqlite>, pool: &sqlx::SqlitePool) -> Result<()> {
    let rows = sqlx::query("SELECT id, mod_id, filename, load_order, enabled FROM plugins")
        .fetch_all(pool)
        .await
        .context("Failed to read exported plugins")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO plugins (id, mod_id, filename, load_order, enabled)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("id"))
        .bind(row.get::<String, _>("mod_id"))
        .bind(row.get::<String, _>("filename"))
        .bind(row.get::<i32, _>("load_order"))
        .bind(row.get::<bool, _>("enabled"))
        .execute(&mut **tx)
        .await
        .context("Failed to import plugin")?;
    }
    Ok(())
}

async fn import_plugin_masters(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    let rows = sqlx::query("SELECT plugin_id, master FROM plugin_masters")
        .fetch_all(pool)
        .await
        .context("Failed to read exported plugin masters")?;
    for row in rows {
        sqlx::query("INSERT INTO plugin_masters (plugin_id, master) VALUES (?, ?)")
            .bind(row.get::<String, _>("plugin_id"))
            .bind(row.get::<String, _>("master"))
            .execute(&mut **tx)
            .await
            .context("Failed to import plugin master")?;
    }
    Ok(())
}

async fn import_deployed_files(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
    import_paths: &ImportPaths,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT game_id, game_rel_lowercase, game_rel_original, mod_id, cache_path
         FROM deployed_files",
    )
    .fetch_all(pool)
    .await
    .context("Failed to read exported deployed files")?;
    for row in rows {
        let game_rel = row.get::<String, _>("game_rel_lowercase");
        let cache_path = rewrite_cache_path_for_row(
            &row.get::<String, _>("cache_path"),
            &game_rel,
            import_paths,
        )?;
        sqlx::query(
            "INSERT INTO deployed_files
             (game_id, game_rel_lowercase, game_rel_original, mod_id, cache_path)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("game_id"))
        .bind(game_rel)
        .bind(row.get::<String, _>("game_rel_original"))
        .bind(row.get::<String, _>("mod_id"))
        .bind(cache_path)
        .execute(&mut **tx)
        .await
        .context("Failed to import deployed file")?;
    }
    Ok(())
}

async fn import_profiles(tx: &mut Transaction<'_, Sqlite>, pool: &sqlx::SqlitePool) -> Result<()> {
    let rows = sqlx::query("SELECT id, game_id, name, is_active, save_mode FROM profiles")
        .fetch_all(pool)
        .await
        .context("Failed to read exported profiles")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO profiles (id, game_id, name, is_active, save_mode)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("id"))
        .bind(row.get::<String, _>("game_id"))
        .bind(row.get::<String, _>("name"))
        .bind(row.get::<bool, _>("is_active"))
        .bind(row.get::<String, _>("save_mode"))
        .execute(&mut **tx)
        .await
        .context("Failed to import profile")?;
    }
    Ok(())
}

async fn import_profile_mods(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    let rows = sqlx::query("SELECT profile_id, mod_id, enabled, priority FROM profile_mods")
        .fetch_all(pool)
        .await
        .context("Failed to read exported profile mods")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO profile_mods (profile_id, mod_id, enabled, priority)
             VALUES (?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("profile_id"))
        .bind(row.get::<String, _>("mod_id"))
        .bind(row.get::<bool, _>("enabled"))
        .bind(row.get::<i32, _>("priority"))
        .execute(&mut **tx)
        .await
        .context("Failed to import profile mod")?;
    }
    Ok(())
}

async fn import_profile_plugins(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    let rows =
        sqlx::query("SELECT profile_id, plugin_id, enabled, load_order FROM profile_plugins")
            .fetch_all(pool)
            .await
            .context("Failed to read exported profile plugins")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
             VALUES (?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("profile_id"))
        .bind(row.get::<String, _>("plugin_id"))
        .bind(row.get::<bool, _>("enabled"))
        .bind(row.get::<i32, _>("load_order"))
        .execute(&mut **tx)
        .await
        .context("Failed to import profile plugin")?;
    }
    Ok(())
}

async fn import_vanilla_files(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    let rows =
        sqlx::query("SELECT game_id, game_rel_lowercase, file_size, mtime_secs FROM vanilla_files")
            .fetch_all(pool)
            .await
            .context("Failed to read exported vanilla files")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO vanilla_files
             (game_id, game_rel_lowercase, file_size, mtime_secs)
             VALUES (?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("game_id"))
        .bind(row.get::<String, _>("game_rel_lowercase"))
        .bind(row.get::<Option<i64>, _>("file_size"))
        .bind(row.get::<Option<i64>, _>("mtime_secs"))
        .execute(&mut **tx)
        .await
        .context("Failed to import vanilla file")?;
    }
    Ok(())
}

async fn import_order_snapshots(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    let rows = sqlx::query("SELECT id, game_id, name, kind, created_at FROM order_snapshots")
        .fetch_all(pool)
        .await
        .context("Failed to read exported order snapshots")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO order_snapshots (id, game_id, name, kind, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("id"))
        .bind(row.get::<String, _>("game_id"))
        .bind(row.get::<String, _>("name"))
        .bind(row.get::<String, _>("kind"))
        .bind(row.get::<String, _>("created_at"))
        .execute(&mut **tx)
        .await
        .context("Failed to import order snapshot")?;
    }
    Ok(())
}

async fn import_order_snapshot_entries(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    let rows = sqlx::query("SELECT snapshot_id, entry_id, position FROM order_snapshot_entries")
        .fetch_all(pool)
        .await
        .context("Failed to read exported order snapshot entries")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO order_snapshot_entries (snapshot_id, entry_id, position)
             VALUES (?, ?, ?)",
        )
        .bind(row.get::<String, _>("snapshot_id"))
        .bind(row.get::<String, _>("entry_id"))
        .bind(row.get::<i32, _>("position"))
        .execute(&mut **tx)
        .await
        .context("Failed to import order snapshot entry")?;
    }
    Ok(())
}

async fn import_vanilla_backups(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
    import_paths: &ImportPaths,
) -> Result<()> {
    let rows = sqlx::query("SELECT game_id, game_rel_path, backup_path FROM vanilla_backups")
        .fetch_all(pool)
        .await
        .context("Failed to read exported vanilla backups")?;
    for row in rows {
        let backup_path = rewrite_backup_path(&row.get::<String, _>("backup_path"), import_paths)?;
        sqlx::query(
            "INSERT INTO vanilla_backups (game_id, game_rel_path, backup_path)
             VALUES (?, ?, ?)",
        )
        .bind(row.get::<String, _>("game_id"))
        .bind(row.get::<String, _>("game_rel_path"))
        .bind(backup_path.to_string_lossy().into_owned())
        .execute(&mut **tx)
        .await
        .context("Failed to import vanilla backup")?;
    }
    Ok(())
}

async fn import_download_entries(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT id, mod_name, nexus_mod_id, nexus_file_id, nexus_domain, game_domain,
                metadata_fetched, nexus_file_name, nexus_is_primary, status, archive_hash,
                archive_md5, version, author, hidden
         FROM download_entries",
    )
    .fetch_all(pool)
    .await
    .context("Failed to read exported download entries")?;
    for row in rows {
        sqlx::query(
            "INSERT INTO download_entries
             (id, mod_name, archive_path, nexus_mod_id, nexus_file_id, nexus_domain, game_domain,
              metadata_fetched, nexus_file_name, nexus_is_primary, status, archive_hash,
              archive_md5, version, author, hidden)
             VALUES (?, ?, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(row.get::<String, _>("id"))
        .bind(row.get::<String, _>("mod_name"))
        .bind(row.get::<Option<i64>, _>("nexus_mod_id"))
        .bind(row.get::<Option<i64>, _>("nexus_file_id"))
        .bind(row.get::<Option<String>, _>("nexus_domain"))
        .bind(row.get::<Option<String>, _>("game_domain"))
        .bind(row.get::<bool, _>("metadata_fetched"))
        .bind(row.get::<Option<String>, _>("nexus_file_name"))
        .bind(row.get::<bool, _>("nexus_is_primary"))
        .bind(row.get::<Option<String>, _>("status"))
        .bind(row.get::<Option<String>, _>("archive_hash"))
        .bind(row.get::<Option<String>, _>("archive_md5"))
        .bind(row.get::<Option<String>, _>("version"))
        .bind(row.get::<Option<String>, _>("author"))
        .bind(row.get::<bool, _>("hidden"))
        .execute(&mut **tx)
        .await
        .context("Failed to import download entry")?;
    }
    Ok(())
}

async fn import_settings(
    tx: &mut Transaction<'_, Sqlite>,
    pool: &sqlx::SqlitePool,
    game_id: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('last_game_id', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(game_id)
    .execute(&mut **tx)
    .await
    .context("Failed to store imported game selection")?;

    let deployed_profile_key = format!("last_deployed_profile_{game_id}");
    let deployed_profile_id: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(&deployed_profile_key)
            .fetch_optional(pool)
            .await
            .context("Failed to read exported last-deployed profile")?;
    if let Some(profile_id) = deployed_profile_id {
        let profile_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM profiles WHERE id = ? AND game_id = ?)",
        )
        .bind(&profile_id)
        .bind(game_id)
        .fetch_one(&mut **tx)
        .await
        .context("Failed to validate imported last-deployed profile")?;
        if profile_exists {
            sqlx::query(
                "INSERT INTO settings (key, value) VALUES (?, ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(&deployed_profile_key)
            .bind(profile_id)
            .execute(&mut **tx)
            .await
            .context("Failed to import last-deployed profile")?;
        }
    }

    let cache_key = format!("cache_dir_{game_id}");
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(cache_key)
        .execute(&mut **tx)
        .await
        .context("Failed to clear imported AppImage cache override")?;
    Ok(())
}
