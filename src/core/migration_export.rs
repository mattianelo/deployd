use std::fs;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tempfile::TempDir;
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

use crate::core::game;
use crate::core::migration_bundle::{
    EXPORT_SCHEMA_VERSION, ExportManifest, SOURCE_PACKAGE_APPIMAGE,
};
use crate::models::game::Game;
use crate::utils::paths;

#[derive(Debug, Clone)]
pub struct ExportGameRequest {
    pub game: Game,
    pub cache_root: PathBuf,
    pub downloads_dir: PathBuf,
    pub output_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ExportGameResult {
    pub output_path: PathBuf,
    pub warnings: Vec<String>,
}

pub async fn export_game_bundle(request: ExportGameRequest) -> Result<ExportGameResult> {
    let db_path = paths::db_path().context("Cannot resolve Deployd database path")?;
    let work = TempDir::new().context("Failed to create export work directory")?;
    let data_dir = work.path().join("data");
    let cache_dir = work.path().join("cache");
    let export_db = data_dir.join("export.db");

    fs::create_dir_all(&data_dir).context("Failed to create export data directory")?;
    fs::create_dir_all(&cache_dir).context("Failed to create export cache directory")?;

    snapshot_database(&db_path, &export_db).await?;

    let export_url = sqlite_url(&export_db);
    let pool = open_sqlite_pool(&export_url).await?;
    prune_export_database(&pool, &request.game).await?;

    let mut warnings = Vec::new();
    copy_selected_cache(&pool, &request.cache_root, &cache_dir, &mut warnings).await?;
    copy_vanilla_backups(&pool, work.path(), &request.game.id, &mut warnings).await?;
    copy_profile_saves(work.path(), &request.game.id, &mut warnings)?;
    rewrite_bundle_paths(&pool, &request.game, &request.cache_root).await?;

    pool.close().await;

    let manifest = ExportManifest {
        schema_version: EXPORT_SCHEMA_VERSION,
        deployd_version: env!("CARGO_PKG_VERSION").to_string(),
        source_package: SOURCE_PACKAGE_APPIMAGE.to_string(),
        exported_at: chrono::Utc::now().to_rfc3339(),
        game_id: request.game.id.clone(),
        game_title: request.game.title.clone(),
        original_game_path: request.game.path.to_string_lossy().into_owned(),
        original_wine_prefix: request
            .game
            .wine_prefix
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        advisory_downloads_dir: request.downloads_dir.to_string_lossy().into_owned(),
        warnings: warnings.clone(),
    };
    let manifest_json =
        serde_json::to_vec_pretty(&manifest).context("Failed to serialize export manifest")?;
    fs::write(work.path().join("manifest.json"), manifest_json)
        .context("Failed to write export manifest")?;

    write_zip(work.path(), &request.output_path).with_context(|| {
        format!(
            "Failed to write export bundle {}",
            request.output_path.display()
        )
    })?;

    Ok(ExportGameResult {
        output_path: request.output_path,
        warnings,
    })
}

async fn snapshot_database(source: &Path, dest: &Path) -> Result<()> {
    let source_url = sqlite_url(source);
    let pool = open_sqlite_pool(&source_url).await?;
    let dest_str = dest.to_string_lossy().into_owned();
    sqlx::query("VACUUM INTO ?")
        .bind(dest_str)
        .execute(&pool)
        .await
        .context("Failed to snapshot Deployd database")?;
    pool.close().await;
    Ok(())
}

async fn open_sqlite_pool(url: &str) -> Result<sqlx::SqlitePool> {
    let opts = url
        .parse::<SqliteConnectOptions>()
        .with_context(|| format!("Failed to parse SQLite URL: {url}"))?;
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .context("Failed to open SQLite database")
}

fn sqlite_url(path: &Path) -> String {
    format!("sqlite://{}?mode=rwc", path.display())
}

pub(crate) async fn prune_export_database(pool: &sqlx::SqlitePool, game: &Game) -> Result<()> {
    let game_id = game.id.as_str();
    sqlx::query("DELETE FROM games WHERE id != ?")
        .bind(game_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM mods WHERE game_id != ?")
        .bind(game_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM mod_files WHERE mod_id NOT IN (SELECT id FROM mods)")
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM plugins WHERE mod_id NOT IN (SELECT id FROM mods)")
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM plugin_masters WHERE plugin_id NOT IN (SELECT id FROM plugins)")
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM deployed_files WHERE game_id != ?")
        .bind(game_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM profiles WHERE game_id != ?")
        .bind(game_id)
        .execute(pool)
        .await?;
    sqlx::query(
        "DELETE FROM profile_mods
         WHERE profile_id NOT IN (SELECT id FROM profiles)
            OR mod_id NOT IN (SELECT id FROM mods)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "DELETE FROM profile_plugins
         WHERE profile_id NOT IN (SELECT id FROM profiles)
            OR plugin_id NOT IN (SELECT id FROM plugins)",
    )
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM tools WHERE game_id != ?")
        .bind(game_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM mod_groups WHERE game_id != ?")
        .bind(game_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM vanilla_files WHERE game_id != ?")
        .bind(game_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM vanilla_backups WHERE game_id != ?")
        .bind(game_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM order_snapshots WHERE game_id != ?")
        .bind(game_id)
        .execute(pool)
        .await?;
    sqlx::query(
        "DELETE FROM order_snapshot_entries
         WHERE snapshot_id NOT IN (SELECT id FROM order_snapshots)",
    )
    .execute(pool)
    .await?;
    prune_download_entries(pool, game).await?;
    prune_settings(pool, game_id).await?;
    Ok(())
}

async fn prune_download_entries(pool: &sqlx::SqlitePool, game: &Game) -> Result<()> {
    if let Some(domain) = game::nexus_domain(game) {
        sqlx::query(
            "DELETE FROM download_entries
             WHERE COALESCE(game_domain, nexus_domain, '') != ?",
        )
        .bind(domain)
        .execute(pool)
        .await?;
    } else {
        sqlx::query("DELETE FROM download_entries")
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn prune_settings(pool: &sqlx::SqlitePool, game_id: &str) -> Result<()> {
    let cache_key = format!("cache_dir_{game_id}");
    let deployed_profile_key = format!("last_deployed_profile_{game_id}");
    sqlx::query("DELETE FROM settings WHERE key NOT IN (?, ?)")
        .bind(&cache_key)
        .bind(&deployed_profile_key)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('last_game_id', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(game_id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn rewrite_bundle_paths(
    pool: &sqlx::SqlitePool,
    game: &Game,
    cache_root: &Path,
) -> Result<()> {
    let old_cache = cache_root.to_string_lossy();
    for table in ["mod_files", "deployed_files"] {
        let sql = format!("UPDATE {table} SET cache_path = REPLACE(cache_path, ?, 'cache')");
        sqlx::query(&sql)
            .bind(old_cache.as_ref())
            .execute(pool)
            .await?;
    }

    if let Ok(backup_root) = paths::vanilla_backup_dir(&game.id) {
        let old_backup = backup_root.to_string_lossy();
        sqlx::query(
            "UPDATE vanilla_backups SET backup_path = REPLACE(backup_path, ?, 'vanilla-backup')",
        )
        .bind(old_backup.as_ref())
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn copy_selected_cache(
    pool: &sqlx::SqlitePool,
    source_root: &Path,
    dest_root: &Path,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let mod_ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM mods ORDER BY priority, name")
        .fetch_all(pool)
        .await
        .context("Failed to list selected game mods")?;

    for (mod_id,) in mod_ids {
        let source = paths::mod_cache_dir_in(source_root, &mod_id);
        let dest = paths::mod_cache_dir_in(dest_root, &mod_id);
        if !source.exists() {
            warnings.push(format!("Cache folder missing for mod {mod_id}"));
            continue;
        }
        copy_dir_recursive(&source, &dest)
            .with_context(|| format!("Failed to copy cache folder for mod {mod_id}"))?;
    }
    Ok(())
}

async fn copy_vanilla_backups(
    pool: &sqlx::SqlitePool,
    stage_root: &Path,
    game_id: &str,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let backup_root = paths::vanilla_backup_dir(game_id)?;
    let dest_root = stage_root.join("vanilla-backup");
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT game_rel_path, backup_path FROM vanilla_backups")
            .fetch_all(pool)
            .await
            .context("Failed to list vanilla backup records")?;

    for (game_rel, backup_path) in rows {
        let source = PathBuf::from(&backup_path);
        if !source.is_file() {
            warnings.push(format!(
                "Vanilla backup missing for {game_rel}: {}",
                source.display()
            ));
            continue;
        }
        let rel = source
            .strip_prefix(&backup_root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| PathBuf::from(game_rel.replace('/', "__")));
        let dest = dest_root.join(rel);
        copy_file(&source, &dest)
            .with_context(|| format!("Failed to copy vanilla backup for {game_rel}"))?;
    }
    Ok(())
}

fn copy_profile_saves(stage_root: &Path, game_id: &str, warnings: &mut Vec<String>) -> Result<()> {
    let source = paths::saves_root()?.join(game_id);
    if !source.exists() {
        return Ok(());
    }
    let dest = stage_root.join("saves").join(game_id);
    if let Err(e) = copy_dir_recursive(&source, &dest) {
        warnings.push(format!("Profile save snapshots could not be exported: {e}"));
    }
    Ok(())
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)
        .with_context(|| format!("Failed to create directory {}", dest.display()))?;
    for entry in WalkDir::new(source).min_depth(1) {
        let entry = entry?;
        let rel = entry
            .path()
            .strip_prefix(source)
            .context("Failed to compute relative copy path")?;
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("Failed to create directory {}", target.display()))?;
        } else if entry.file_type().is_file() {
            copy_file(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    fs::copy(source, dest)
        .with_context(|| format!("Failed to copy {} to {}", source.display(), dest.display()))?;
    Ok(())
}

fn write_zip(source_root: &Path, output_path: &Path) -> Result<()> {
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory {}", parent.display()))?;
    }
    let file = fs::File::create(output_path)
        .with_context(|| format!("Failed to create {}", output_path.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    add_dir_to_zip(source_root, source_root, &mut zip, options)?;
    zip.finish().context("Failed to finalize export bundle")?;
    Ok(())
}

fn add_dir_to_zip<W: Write + Seek>(
    source_root: &Path,
    dir: &Path,
    zip: &mut zip::ZipWriter<W>,
    options: SimpleFileOptions,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        let rel = path
            .strip_prefix(source_root)
            .context("Failed to compute zip entry path")?;
        let name = rel
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        if entry.file_type()?.is_dir() {
            zip.add_directory(format!("{name}/"), options)
                .with_context(|| format!("Failed to add zip directory {name}"))?;
            add_dir_to_zip(source_root, &path, zip, options)?;
        } else if entry.file_type()?.is_file() {
            zip.start_file(&name, options)
                .with_context(|| format!("Failed to add zip file {name}"))?;
            let mut file = fs::File::open(&path)
                .with_context(|| format!("Failed to open {}", path.display()))?;
            std::io::copy(&mut file, zip)
                .with_context(|| format!("Failed to write zip file {name}"))?;
        } else {
            return Err(anyhow!("Unsupported export entry: {}", path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::game::GameEngine;

    // Regression: AppImage-to-Snap migration deployment state.
    // @variants: both
    #[tokio::test]
    async fn prunes_database_to_selected_game() -> Result<()> {
        let temp = TempDir::new()?;
        let db = temp.path().join("export.db");
        let url = sqlite_url(&db);
        let tracker = crate::core::tracker::Tracker::open(&url).await?;

        tracker
            .upsert_game(
                "skyrim-se",
                "Skyrim",
                Path::new("/games/skyrim"),
                "Data",
                "bethesda",
                Some(Path::new("/prefix/skyrim")),
                true,
            )
            .await?;
        tracker
            .upsert_game(
                "fallout-4",
                "Fallout",
                Path::new("/games/fallout"),
                "Data",
                "bethesda",
                Some(Path::new("/prefix/fallout")),
                true,
            )
            .await?;
        sqlx::query("INSERT INTO mods (id, game_id, name, priority) VALUES ('m1', 'skyrim-se', 'Sky Mod', 0), ('m2', 'fallout-4', 'Fall Mod', 0)")
            .execute(&tracker.pool)
            .await?;
        sqlx::query("INSERT INTO plugins (id, mod_id, filename, load_order) VALUES ('p1', 'm1', 'a.esp', 0), ('p2', 'm2', 'b.esp', 0)")
            .execute(&tracker.pool)
            .await?;
        sqlx::query("INSERT INTO settings (key, value) VALUES ('nexus_api_key', 'secret'), ('cache_dir_skyrim-se', '/cache/skyrim'), ('cache_dir_fallout-4', '/cache/fallout'), ('last_deployed_profile_skyrim-se', 'profile-1'), ('last_deployed_profile_fallout-4', 'profile-2')")
            .execute(&tracker.pool)
            .await?;

        let game = Game {
            id: "skyrim-se".to_string(),
            title: "Skyrim".to_string(),
            path: PathBuf::from("/games/skyrim"),
            data_subdir: "Data".to_string(),
            engine: GameEngine::Bethesda,
            wine_prefix: Some(PathBuf::from("/prefix/skyrim")),
        };

        prune_export_database(&tracker.pool, &game).await?;

        let games: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM games")
            .fetch_one(&tracker.pool)
            .await?;
        let fallout_mods: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM mods WHERE game_id = 'fallout-4'")
                .fetch_one(&tracker.pool)
                .await?;
        let plugins: Vec<(String,)> = sqlx::query_as("SELECT id FROM plugins ORDER BY id")
            .fetch_all(&tracker.pool)
            .await?;
        let secret: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'nexus_api_key'")
                .fetch_optional(&tracker.pool)
                .await?;
        let selected_cache: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'cache_dir_skyrim-se'")
                .fetch_optional(&tracker.pool)
                .await?;
        let deployed_profile: Option<String> = sqlx::query_scalar(
            "SELECT value FROM settings WHERE key = 'last_deployed_profile_skyrim-se'",
        )
        .fetch_optional(&tracker.pool)
        .await?;
        let other_deployed_profile: Option<String> = sqlx::query_scalar(
            "SELECT value FROM settings WHERE key = 'last_deployed_profile_fallout-4'",
        )
        .fetch_optional(&tracker.pool)
        .await?;

        assert_eq!(games, 1, "only the selected game should remain");
        assert_eq!(fallout_mods, 0, "non-selected game mods should be removed");
        assert_eq!(
            plugins,
            vec![("p1".to_string(),)],
            "plugin rows should be pruned through mods"
        );
        assert!(
            secret.is_none(),
            "secret global settings should not be exported"
        );
        assert_eq!(selected_cache.as_deref(), Some("/cache/skyrim"));
        assert_eq!(deployed_profile.as_deref(), Some("profile-1"));
        assert!(
            other_deployed_profile.is_none(),
            "another game's deployment marker must not be exported"
        );

        Ok(())
    }

    #[test]
    fn manifest_marks_appimage_source() -> Result<()> {
        let manifest = ExportManifest {
            schema_version: EXPORT_SCHEMA_VERSION,
            deployd_version: env!("CARGO_PKG_VERSION").to_string(),
            source_package: SOURCE_PACKAGE_APPIMAGE.to_string(),
            exported_at: "2026-05-16T00:00:00Z".to_string(),
            game_id: "skyrim-se".to_string(),
            game_title: "Skyrim".to_string(),
            original_game_path: "/games/skyrim".to_string(),
            original_wine_prefix: Some("/prefix/skyrim".to_string()),
            advisory_downloads_dir: "/home/user/Downloads".to_string(),
            warnings: vec!["Cache folder missing for mod m1".to_string()],
        };

        let json = serde_json::to_value(&manifest)?;
        assert_eq!(json["schema_version"], EXPORT_SCHEMA_VERSION);
        assert_eq!(json["source_package"], "appimage");
        assert!(
            json.get("nexus_api_key").is_none(),
            "manifest must not contain global secrets"
        );
        Ok(())
    }
}
