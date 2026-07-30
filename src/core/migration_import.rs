use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, Sqlite, Transaction};
use tempfile::TempDir;
use zip::ZipArchive;

use crate::core::tracker::Tracker;
use crate::core::{game as game_core, migration_bundle::ExportManifest};
use crate::models::download::DownloadEntry;
use crate::models::game::{Game, GameEngine};
use crate::utils::paths;

#[derive(Debug, Clone)]
pub struct PreviewImportRequest {
    pub bundle_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PreviewImportResult {
    pub bundle_path: PathBuf,
    pub manifest: ExportManifest,
    pub counts: PreviewCounts,
    pub conflict: PreviewConflict,
    pub validation_items: Vec<ValidationItem>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ImportBundleRequest {
    pub bundle_path: PathBuf,
    pub confirmed_game_path: PathBuf,
    pub confirmed_wine_prefix: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ImportBundleResult {
    pub game: Game,
    pub download_entries: Vec<DownloadEntry>,
    pub counts: PreviewCounts,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PreviewCounts {
    pub mods: i64,
    pub plugins: i64,
    pub profiles: i64,
    pub tools: i64,
    pub downloads: i64,
    pub cache_files: usize,
    pub vanilla_backups: usize,
    pub save_snapshots: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewConflict {
    NewGame,
    ExistingGame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationItem {
    NeedsGameFolderConfirmation,
    NeedsWinePrefixConfirmation,
    ToolsNeedSnapRuntimeRebind,
}

pub async fn preview_import_bundle(
    tracker: &Tracker,
    request: PreviewImportRequest,
) -> Result<PreviewImportResult> {
    let extracted = tokio::task::spawn_blocking({
        let bundle_path = request.bundle_path.clone();
        move || extract_preview_bundle(&bundle_path)
    })
    .await
    .context("Import preview task failed")??;

    let db_url = sqlite_url_read_only(&extracted.export_db);
    let pool = open_sqlite_pool(&db_url).await?;
    let counts = read_preview_counts(&pool, extracted.bundle_files).await?;
    pool.close().await;

    let existing_state = tracker_game_state(tracker, &extracted.manifest.game_id).await?;

    let mut warnings = extracted.manifest.warnings.clone();
    if existing_state == ExistingGameState::Active {
        warnings.push(
            "This game is already managed in the Snap; a later import phase will skip it by default."
                .to_string(),
        );
    } else if existing_state == ExistingGameState::Hidden {
        warnings.push(
            "This game was previously stopped in the Snap; import will replace that hidden state."
                .to_string(),
        );
    }

    let mut validation_items = vec![
        ValidationItem::NeedsGameFolderConfirmation,
        ValidationItem::NeedsWinePrefixConfirmation,
    ];
    if counts.tools > 0 {
        validation_items.push(ValidationItem::ToolsNeedSnapRuntimeRebind);
    }

    Ok(PreviewImportResult {
        bundle_path: request.bundle_path,
        manifest: extracted.manifest,
        counts,
        conflict: if existing_state == ExistingGameState::Active {
            PreviewConflict::ExistingGame
        } else {
            PreviewConflict::NewGame
        },
        validation_items,
        warnings,
    })
}

pub async fn import_bundle(
    tracker: &Tracker,
    request: ImportBundleRequest,
) -> Result<ImportBundleResult> {
    validate_required_confirmation(&request.confirmed_game_path, "game folder")?;
    validate_required_confirmation(&request.confirmed_wine_prefix, "Wine prefix")?;

    let staged = tokio::task::spawn_blocking({
        let bundle_path = request.bundle_path.clone();
        move || extract_import_bundle(&bundle_path)
    })
    .await
    .context("Import task failed")??;

    let existing_state = tracker_game_state(tracker, &staged.manifest.game_id).await?;
    if existing_state == ExistingGameState::Active {
        bail!(
            "Game {} is already managed in this Snap. Import was skipped.",
            staged.manifest.game_title
        );
    }

    let export_url = sqlite_url_read_only(&staged.export_db);
    let export_pool = open_sqlite_pool(&export_url).await?;
    let counts = read_preview_counts(&export_pool, staged.bundle_files).await?;

    let import_paths = ImportPaths::new(&staged.manifest.game_id)?;
    let imported_game = read_imported_game(
        &export_pool,
        &staged.manifest,
        &request.confirmed_game_path,
        &request.confirmed_wine_prefix,
    )
    .await?;
    if existing_state == ExistingGameState::Hidden {
        drop_hidden_game_state(tracker, &imported_game).await?;
    }
    validate_export_dependencies(&export_pool, &staged.payload_root, &import_paths).await?;
    validate_import_collisions(tracker, &export_pool, &staged.manifest.game_id).await?;

    let copied_paths = copy_payload_to_snap(&staged.payload_root, &staged.manifest.game_id)?;
    let mut warnings = staged.manifest.warnings.clone();
    if counts.tools > 0 {
        warnings.push(format!(
            "{} external tool(s) were skipped. Re-add tools in the Snap so they use the Snap Wine runtime.",
            counts.tools
        ));
    }
    let import_result = import_database_rows(
        tracker,
        &export_pool,
        &staged.manifest,
        &imported_game,
        &import_paths,
    )
    .await;
    export_pool.close().await;

    if let Err(error) = import_result {
        cleanup_copied_payload(&copied_paths);
        return Err(error);
    }

    let download_entries = tracker
        .load_download_entries()
        .await
        .context("Failed to load imported download entries")?;

    Ok(ImportBundleResult {
        game: imported_game,
        download_entries,
        counts,
        warnings,
    })
}

struct ExtractedPreview {
    _work: TempDir,
    manifest: ExportManifest,
    export_db: PathBuf,
    bundle_files: BundleFileCounts,
}

struct ExtractedImport {
    _work: TempDir,
    manifest: ExportManifest,
    export_db: PathBuf,
    payload_root: PathBuf,
    bundle_files: BundleFileCounts,
}

#[derive(Debug, Clone, Copy, Default)]
struct BundleFileCounts {
    cache_files: usize,
    vanilla_backups: usize,
    save_snapshots: usize,
}

fn extract_preview_bundle(bundle_path: &Path) -> Result<ExtractedPreview> {
    let file = fs::File::open(bundle_path)
        .with_context(|| format!("Failed to open {}", bundle_path.display()))?;
    let mut archive = ZipArchive::new(file).context("Failed to read export bundle")?;

    let mut manifest_bytes = Vec::new();
    archive
        .by_name("manifest.json")
        .context("Export bundle is missing manifest.json")?
        .read_to_end(&mut manifest_bytes)
        .context("Failed to read export manifest")?;
    let manifest: ExportManifest =
        serde_json::from_slice(&manifest_bytes).context("Invalid export manifest")?;
    manifest.validate()?;

    if archive.by_name("data/export.db").is_err() {
        bail!("Export bundle is missing data/export.db");
    }

    let work = TempDir::new().context("Failed to create import preview work directory")?;
    let data_dir = work.path().join("data");
    fs::create_dir_all(&data_dir).context("Failed to create preview data directory")?;
    let export_db = data_dir.join("export.db");

    {
        let mut db_entry = archive
            .by_name("data/export.db")
            .context("Export bundle is missing data/export.db")?;
        let mut db_file =
            fs::File::create(&export_db).context("Failed to stage export database")?;
        std::io::copy(&mut db_entry, &mut db_file).context("Failed to extract export database")?;
    }
    let bundle_files = count_bundle_files(&mut archive, &manifest.game_id)?;

    Ok(ExtractedPreview {
        _work: work,
        manifest,
        export_db,
        bundle_files,
    })
}

fn extract_import_bundle(bundle_path: &Path) -> Result<ExtractedImport> {
    let file = fs::File::open(bundle_path)
        .with_context(|| format!("Failed to open {}", bundle_path.display()))?;
    let mut archive = ZipArchive::new(file).context("Failed to read export bundle")?;

    let mut manifest_bytes = Vec::new();
    archive
        .by_name("manifest.json")
        .context("Export bundle is missing manifest.json")?
        .read_to_end(&mut manifest_bytes)
        .context("Failed to read export manifest")?;
    let manifest: ExportManifest =
        serde_json::from_slice(&manifest_bytes).context("Invalid export manifest")?;
    manifest.validate()?;

    if archive.by_name("data/export.db").is_err() {
        bail!("Export bundle is missing data/export.db");
    }

    let work = TempDir::new().context("Failed to create import work directory")?;
    let payload_root = work.path().join("payload");
    fs::create_dir_all(&payload_root).context("Failed to create import payload directory")?;
    let data_dir = payload_root.join("data");
    fs::create_dir_all(&data_dir).context("Failed to create import data directory")?;
    let export_db = data_dir.join("export.db");

    let mut bundle_files = BundleFileCounts::default();
    for idx in 0..archive.len() {
        let mut entry = archive.by_index(idx)?;
        if entry.is_dir() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            bail!("Export bundle contains an unsafe path: {}", entry.name());
        };
        let name = enclosed.to_string_lossy().replace('\\', "/");
        if name == "manifest.json" {
            continue;
        }
        if name == "data/export.db" {
            copy_zip_entry(&mut entry, &export_db)?;
        } else if name.starts_with("cache/") {
            bundle_files.cache_files += 1;
            copy_zip_entry(&mut entry, &payload_root.join(enclosed))?;
        } else if name.starts_with("vanilla-backup/") {
            bundle_files.vanilla_backups += 1;
            copy_zip_entry(&mut entry, &payload_root.join(enclosed))?;
        } else if name.starts_with(&format!("saves/{}/", manifest.game_id)) {
            bundle_files.save_snapshots += 1;
            copy_zip_entry(&mut entry, &payload_root.join(enclosed))?;
        } else {
            bail!("Export bundle contains unsupported entry: {name}");
        }
    }

    if !export_db.is_file() {
        bail!("Export bundle is missing data/export.db");
    }

    Ok(ExtractedImport {
        _work: work,
        manifest,
        export_db,
        payload_root,
        bundle_files,
    })
}

fn copy_zip_entry(entry: &mut zip::read::ZipFile<'_>, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let mut file = fs::File::create(target)
        .with_context(|| format!("Failed to create {}", target.display()))?;
    std::io::copy(entry, &mut file)
        .with_context(|| format!("Failed to extract {}", target.display()))?;
    Ok(())
}

async fn read_preview_counts(
    pool: &sqlx::SqlitePool,
    bundle_files: BundleFileCounts,
) -> Result<PreviewCounts> {
    Ok(PreviewCounts {
        mods: count_rows(pool, "mods").await?,
        plugins: count_rows(pool, "plugins").await?,
        profiles: count_rows(pool, "profiles").await?,
        tools: count_rows(pool, "tools").await?,
        downloads: count_rows(pool, "download_entries").await?,
        cache_files: bundle_files.cache_files,
        vanilla_backups: bundle_files.vanilla_backups,
        save_snapshots: bundle_files.save_snapshots,
    })
}

fn count_bundle_files<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
    game_id: &str,
) -> Result<BundleFileCounts> {
    let saves_prefix = format!("saves/{game_id}/");
    let mut counts = BundleFileCounts::default();
    for idx in 0..archive.len() {
        let entry = archive.by_index(idx)?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name();
        if name.starts_with("cache/") {
            counts.cache_files += 1;
        } else if name.starts_with("vanilla-backup/") {
            counts.vanilla_backups += 1;
        } else if name.starts_with(&saves_prefix) {
            counts.save_snapshots += 1;
        }
    }
    Ok(counts)
}

async fn count_rows(pool: &sqlx::SqlitePool, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    sqlx::query_scalar(&sql)
        .fetch_one(pool)
        .await
        .with_context(|| format!("Failed to count {table}"))
}

fn validate_required_confirmation(path: &Path, label: &str) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("Import requires a confirmed {label}");
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExistingGameState {
    Absent,
    Active,
    Hidden,
}

async fn tracker_game_state(tracker: &Tracker, game_id: &str) -> Result<ExistingGameState> {
    let hidden: Option<Option<i32>> = sqlx::query_scalar("SELECT hidden FROM games WHERE id = ?")
        .bind(game_id)
        .fetch_optional(&tracker.pool)
        .await
        .context("Failed to check existing Snap game")?;
    Ok(match hidden {
        None => ExistingGameState::Absent,
        Some(Some(1)) => ExistingGameState::Hidden,
        Some(_) => ExistingGameState::Active,
    })
}

#[derive(Debug, Clone)]
struct ImportPaths {
    cache_root: PathBuf,
    backup_root: PathBuf,
}

impl ImportPaths {
    fn new(game_id: &str) -> Result<Self> {
        Ok(Self {
            cache_root: paths::cache_root().context("Cannot resolve Snap cache folder")?,
            backup_root: paths::vanilla_backup_dir(game_id)
                .context("Cannot resolve Snap vanilla backup folder")?,
        })
    }
}

async fn read_imported_game(
    pool: &sqlx::SqlitePool,
    manifest: &ExportManifest,
    confirmed_game_path: &Path,
    confirmed_wine_prefix: &Path,
) -> Result<Game> {
    let row = sqlx::query(
        "SELECT id, title, data_subdir, engine
         FROM games
         WHERE id = ?",
    )
    .bind(&manifest.game_id)
    .fetch_optional(pool)
    .await
    .context("Failed to read exported game row")?
    .ok_or_else(|| anyhow!("Export database does not contain the exported game row"))?;

    let id: String = row.get("id");
    let title: Option<String> = row.get("title");
    let data_subdir: Option<String> = row.get("data_subdir");
    let engine: Option<String> = row.get("engine");
    Ok(Game {
        id,
        title: title.unwrap_or_else(|| manifest.game_title.clone()),
        path: confirmed_game_path.to_path_buf(),
        data_subdir: data_subdir.unwrap_or_else(|| "Data".to_string()),
        engine: parse_game_engine(engine.as_deref()),
        wine_prefix: Some(confirmed_wine_prefix.to_path_buf()),
    })
}

fn parse_game_engine(engine: Option<&str>) -> GameEngine {
    match engine {
        Some("redengine") => GameEngine::REDEngine,
        Some("eclipse") => GameEngine::Eclipse,
        Some("aurora") => GameEngine::Aurora,
        _ => GameEngine::Bethesda,
    }
}

fn game_engine_db_value(engine: &GameEngine) -> &'static str {
    match engine {
        GameEngine::REDEngine => "redengine",
        GameEngine::Eclipse => "eclipse",
        GameEngine::Aurora => "aurora",
        GameEngine::Bethesda => "bethesda",
    }
}

async fn validate_export_dependencies(
    pool: &sqlx::SqlitePool,
    payload_root: &Path,
    import_paths: &ImportPaths,
) -> Result<()> {
    for table in ["mod_files", "deployed_files"] {
        let sql = format!("SELECT game_rel_lowercase, cache_path FROM {table}");
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(&sql)
            .fetch_all(pool)
            .await
            .with_context(|| format!("Failed to read {table} cache paths"))?;
        for (game_rel, cache_path) in rows {
            let Some(cache_path) = cache_path else {
                continue;
            };
            if skips_cache_file_validation(&game_rel, &cache_path) {
                continue;
            }
            let staged = staged_cache_path(payload_root, &cache_path)?;
            if !staged.is_file() {
                bail!(
                    "Export bundle is missing a cached mod file referenced by {table}: {}",
                    cache_path
                );
            }
            rewrite_cache_path(&cache_path, import_paths)?;
        }
    }

    let rows: Vec<(String,)> = sqlx::query_as("SELECT backup_path FROM vanilla_backups")
        .fetch_all(pool)
        .await
        .context("Failed to read vanilla backup paths")?;
    for (backup_path,) in rows {
        let staged = staged_backup_path(payload_root, &backup_path)?;
        if !staged.is_file() {
            bail!("Export bundle is missing a vanilla backup referenced by the DB: {backup_path}");
        }
        rewrite_backup_path(&backup_path, import_paths)?;
    }

    Ok(())
}

async fn drop_hidden_game_state(tracker: &Tracker, game: &Game) -> Result<()> {
    if tracker_game_state(tracker, &game.id).await? != ExistingGameState::Hidden {
        return Ok(());
    }

    let cache_root = paths::cache_root().context("Cannot resolve Snap cache folder")?;
    let mod_ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM mods WHERE game_id = ?")
        .bind(&game.id)
        .fetch_all(&tracker.pool)
        .await
        .context("Failed to list hidden game mods before reimport")?;
    for (mod_id,) in &mod_ids {
        let cache = paths::mod_cache_dir_in(&cache_root, mod_id);
        if cache.exists() {
            fs::remove_dir_all(&cache).with_context(|| {
                format!(
                    "Failed to remove old hidden-game cache folder {}",
                    cache.display()
                )
            })?;
        }
    }
    let backup_dir = paths::vanilla_backup_dir(&game.id)?;
    if backup_dir.exists() {
        fs::remove_dir_all(&backup_dir).with_context(|| {
            format!(
                "Failed to remove old hidden-game vanilla backups {}",
                backup_dir.display()
            )
        })?;
    }
    let saves_dir = paths::saves_root()?.join(&game.id);
    if saves_dir.exists() {
        fs::remove_dir_all(&saves_dir).with_context(|| {
            format!(
                "Failed to remove old hidden-game save snapshots {}",
                saves_dir.display()
            )
        })?;
    }

    let mut tx = tracker
        .pool
        .begin()
        .await
        .context("Failed to begin hidden game cleanup")?;
    let mod_ids: Vec<(String,)> = sqlx::query_as("SELECT id FROM mods WHERE game_id = ?")
        .bind(&game.id)
        .fetch_all(&mut *tx)
        .await
        .context("Failed to list hidden game mods for cleanup")?;
    for (mod_id,) in &mod_ids {
        sqlx::query("DELETE FROM plugin_masters WHERE plugin_id IN (SELECT id FROM plugins WHERE mod_id = ?)")
            .bind(mod_id)
            .execute(&mut *tx)
            .await
            .context("Failed to delete hidden game plugin masters")?;
        sqlx::query("DELETE FROM profile_mods WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&mut *tx)
            .await
            .context("Failed to delete hidden game profile mods")?;
        sqlx::query("DELETE FROM profile_plugins WHERE plugin_id IN (SELECT id FROM plugins WHERE mod_id = ?)")
            .bind(mod_id)
            .execute(&mut *tx)
            .await
            .context("Failed to delete hidden game profile plugins")?;
        sqlx::query("DELETE FROM plugins WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&mut *tx)
            .await
            .context("Failed to delete hidden game plugins")?;
        sqlx::query("DELETE FROM mod_files WHERE mod_id = ?")
            .bind(mod_id)
            .execute(&mut *tx)
            .await
            .context("Failed to delete hidden game mod files")?;
    }
    for sql in [
        "DELETE FROM deployed_files WHERE game_id = ?",
        "DELETE FROM mods WHERE game_id = ?",
        "DELETE FROM profiles WHERE game_id = ?",
        "DELETE FROM tools WHERE game_id = ?",
        "DELETE FROM mod_groups WHERE game_id = ?",
        "DELETE FROM vanilla_files WHERE game_id = ?",
        "DELETE FROM vanilla_backups WHERE game_id = ?",
        "DELETE FROM order_snapshot_entries WHERE snapshot_id IN (SELECT id FROM order_snapshots WHERE game_id = ?)",
        "DELETE FROM order_snapshots WHERE game_id = ?",
        "DELETE FROM games WHERE id = ?",
    ] {
        sqlx::query(sql)
            .bind(&game.id)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("Failed hidden game cleanup step: {sql}"))?;
    }
    if let Some(domain) = game_core::nexus_domain(game) {
        sqlx::query(
            "DELETE FROM download_entries
             WHERE COALESCE(game_domain, nexus_domain, '') = ?",
        )
        .bind(domain)
        .execute(&mut *tx)
        .await
        .context("Failed to delete hidden game download entries")?;
    }
    for key in [
        format!("cache_dir_{}", game.id),
        format!("last_profile_{}", game.id),
        format!("last_deployed_profile_{}", game.id),
    ] {
        sqlx::query("DELETE FROM settings WHERE key = ?")
            .bind(key)
            .execute(&mut *tx)
            .await
            .context("Failed to delete hidden game setting")?;
    }
    tx.commit()
        .await
        .context("Failed to commit hidden game cleanup")?;
    Ok(())
}

async fn validate_import_collisions(
    tracker: &Tracker,
    export_pool: &sqlx::SqlitePool,
    game_id: &str,
) -> Result<()> {
    if tracker_game_state(tracker, game_id).await? != ExistingGameState::Absent {
        bail!("Game {game_id} already exists in the Snap database");
    }
    if let Some((table, id)) = [
        id_collision(tracker, export_pool, "mods", "id").await?,
        id_collision(tracker, export_pool, "plugins", "id").await?,
        id_collision(tracker, export_pool, "profiles", "id").await?,
        id_collision(tracker, export_pool, "mod_groups", "id").await?,
        id_collision(tracker, export_pool, "order_snapshots", "id").await?,
        id_collision(tracker, export_pool, "download_entries", "id").await?,
    ]
    .into_iter()
    .flatten()
    .next()
    {
        bail!("Import would overwrite existing Snap {table} row with id {id}");
    }
    Ok(())
}

async fn id_collision(
    tracker: &Tracker,
    export_pool: &sqlx::SqlitePool,
    table: &str,
    column: &str,
) -> Result<Option<(String, String)>> {
    let export_sql = format!("SELECT {column} FROM {table}");
    let ids: Vec<(String,)> = sqlx::query_as(&export_sql)
        .fetch_all(export_pool)
        .await
        .with_context(|| format!("Failed to read exported {table} ids"))?;
    let active_sql = format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?");
    for (id,) in ids {
        let count: i64 = sqlx::query_scalar(&active_sql)
            .bind(&id)
            .fetch_one(&tracker.pool)
            .await
            .with_context(|| format!("Failed to check active {table} id collisions"))?;
        if count > 0 {
            return Ok(Some((table.to_string(), id)));
        }
    }
    Ok(None)
}

#[derive(Debug, Default)]
struct CopiedPayload {
    paths: Vec<PathBuf>,
}

fn copy_payload_to_snap(payload_root: &Path, game_id: &str) -> Result<CopiedPayload> {
    let mut copied = CopiedPayload::default();
    let result = copy_payload_to_snap_inner(payload_root, game_id, &mut copied);
    if let Err(error) = result {
        cleanup_copied_payload(&copied);
        return Err(error);
    }
    Ok(copied)
}

fn copy_payload_to_snap_inner(
    payload_root: &Path,
    game_id: &str,
    copied: &mut CopiedPayload,
) -> Result<()> {
    let cache_root = paths::cache_root().context("Cannot resolve Snap cache folder")?;
    let cache_stage = payload_root.join("cache");
    if cache_stage.exists() {
        fs::create_dir_all(&cache_root)
            .with_context(|| format!("Failed to create {}", cache_root.display()))?;
        for entry in fs::read_dir(&cache_stage)
            .with_context(|| format!("Failed to read {}", cache_stage.display()))?
        {
            let entry = entry?;
            let source = entry.path();
            if !entry.file_type()?.is_dir() {
                bail!(
                    "Export cache contains unsupported file at {}",
                    source.display()
                );
            }
            let dest = cache_root.join(entry.file_name());
            if dest.exists() {
                bail!(
                    "Import would overwrite existing Snap cache folder {}",
                    dest.display()
                );
            }
            copy_dir_recursive(&source, &dest)?;
            copied.paths.push(dest);
        }
    }

    let backup_stage = payload_root.join("vanilla-backup");
    if backup_stage.exists() {
        let dest = paths::vanilla_backup_dir(game_id)?;
        if dest.exists() {
            bail!(
                "Import would overwrite existing Snap vanilla backup folder {}",
                dest.display()
            );
        }
        copy_dir_recursive(&backup_stage, &dest)?;
        copied.paths.push(dest);
    }

    let saves_stage = payload_root.join("saves").join(game_id);
    if saves_stage.exists() {
        let dest = paths::saves_root()?.join(game_id);
        if dest.exists() {
            bail!(
                "Import would overwrite existing Snap save snapshot folder {}",
                dest.display()
            );
        }
        copy_dir_recursive(&saves_stage, &dest)?;
        copied.paths.push(dest);
    }

    Ok(())
}

fn cleanup_copied_payload(copied: &CopiedPayload) {
    for path in copied.paths.iter().rev() {
        if let Err(e) = fs::remove_dir_all(path) {
            eprintln!(
                "Failed to clean incomplete import path {}: {e}",
                path.display()
            );
        }
    }
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)
        .with_context(|| format!("Failed to create directory {}", dest.display()))?;
    for entry in walkdir::WalkDir::new(source).min_depth(1) {
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
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory {}", parent.display()))?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "Failed to copy {} to {}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        }
    }
    Ok(())
}

async fn import_database_rows(
    tracker: &Tracker,
    export_pool: &sqlx::SqlitePool,
    manifest: &ExportManifest,
    game: &Game,
    import_paths: &ImportPaths,
) -> Result<()> {
    let mut tx = tracker
        .pool
        .begin()
        .await
        .context("Failed to begin import")?;

    import_game_row(&mut tx, game).await?;
    import_mod_groups(&mut tx, export_pool).await?;
    import_mods(&mut tx, export_pool).await?;
    import_mod_files(&mut tx, export_pool, import_paths).await?;
    import_plugins(&mut tx, export_pool).await?;
    import_plugin_masters(&mut tx, export_pool).await?;
    import_deployed_files(&mut tx, export_pool, import_paths).await?;
    import_profiles(&mut tx, export_pool).await?;
    import_profile_mods(&mut tx, export_pool).await?;
    import_profile_plugins(&mut tx, export_pool).await?;
    import_vanilla_files(&mut tx, export_pool).await?;
    import_order_snapshots(&mut tx, export_pool).await?;
    import_order_snapshot_entries(&mut tx, export_pool).await?;
    import_vanilla_backups(&mut tx, export_pool, import_paths).await?;
    import_download_entries(&mut tx, export_pool).await?;
    backfill_imported_mod_source_metadata(&mut tx).await?;
    import_settings(&mut tx, export_pool, &manifest.game_id).await?;

    tx.commit()
        .await
        .context("Failed to commit AppImage export import")?;
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

fn staged_cache_path(payload_root: &Path, bundle_path: &str) -> Result<PathBuf> {
    Ok(payload_root.join(bundle_relative_path(bundle_path, "cache")?))
}

fn staged_backup_path(payload_root: &Path, bundle_path: &str) -> Result<PathBuf> {
    Ok(payload_root.join(bundle_relative_path(bundle_path, "vanilla-backup")?))
}

fn rewrite_cache_path(bundle_path: &str, import_paths: &ImportPaths) -> Result<PathBuf> {
    let rel = strip_bundle_prefix(bundle_path, "cache")?;
    Ok(import_paths.cache_root.join(rel))
}

fn rewrite_cache_path_for_row(
    bundle_path: &str,
    game_rel: &str,
    import_paths: &ImportPaths,
) -> Result<String> {
    if bundle_path.is_empty() {
        return Ok(String::new());
    }
    let rewritten = rewrite_cache_path(bundle_path, import_paths)?;
    let mut value = rewritten.to_string_lossy().into_owned();
    if game_rel.ends_with('/') && !value.ends_with(std::path::MAIN_SEPARATOR) {
        value.push(std::path::MAIN_SEPARATOR);
    }
    Ok(value)
}

fn rewrite_backup_path(bundle_path: &str, import_paths: &ImportPaths) -> Result<PathBuf> {
    let rel = strip_bundle_prefix(bundle_path, "vanilla-backup")?;
    Ok(import_paths.backup_root.join(rel))
}

fn bundle_relative_path(bundle_path: &str, prefix: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(prefix).join(strip_bundle_prefix(bundle_path, prefix)?))
}

fn strip_bundle_prefix(bundle_path: &str, prefix: &str) -> Result<PathBuf> {
    let normalized = bundle_path.replace('\\', "/");
    let rel = normalized
        .strip_prefix(&format!("{prefix}/"))
        .ok_or_else(|| anyhow!("Expected bundle-relative {prefix} path, got {bundle_path}"))?;
    let rel = rel.trim_end_matches('/');
    if rel.is_empty() || rel.split('/').any(|part| part == ".." || part.is_empty()) {
        bail!("Unsafe bundle-relative path: {bundle_path}");
    }
    Ok(PathBuf::from(rel))
}

fn skips_cache_file_validation(game_rel: &str, cache_path: &str) -> bool {
    game_rel.ends_with('/') || cache_path.is_empty()
}

async fn open_sqlite_pool(url: &str) -> Result<sqlx::SqlitePool> {
    let opts = url
        .parse::<SqliteConnectOptions>()
        .with_context(|| format!("Failed to parse SQLite URL: {url}"))?;
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .context("Failed to open export database")
}

fn sqlite_url_read_only(path: &Path) -> String {
    format!("sqlite://{}?mode=ro", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::migration_bundle::{EXPORT_SCHEMA_VERSION, SOURCE_PACKAGE_APPIMAGE};
    use crate::core::tracker::Tracker;
    use std::io::Write;
    use std::sync::Mutex;
    use zip::write::SimpleFileOptions;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn previews_valid_bundle() -> Result<()> {
        let fixture = PreviewFixture::new().await?;
        fixture.write_bundle(|manifest| manifest).await?;

        let result = preview_import_bundle(
            &fixture.snap_tracker,
            PreviewImportRequest {
                bundle_path: fixture.bundle_path.clone(),
            },
        )
        .await?;

        assert_eq!(result.manifest.game_id, "skyrim-se");
        assert_eq!(result.counts.mods, 1);
        assert_eq!(result.counts.plugins, 1);
        assert_eq!(result.counts.profiles, 1);
        assert_eq!(result.counts.tools, 1);
        assert_eq!(result.counts.downloads, 1);
        assert_eq!(result.counts.cache_files, 1);
        assert_eq!(result.counts.vanilla_backups, 1);
        assert_eq!(result.counts.save_snapshots, 1);
        assert_eq!(result.conflict, PreviewConflict::NewGame);
        assert!(
            result
                .validation_items
                .contains(&ValidationItem::ToolsNeedSnapRuntimeRebind)
        );

        Ok(())
    }

    #[tokio::test]
    async fn detects_existing_game_without_writing_snap_state() -> Result<()> {
        let fixture = PreviewFixture::new().await?;
        fixture.write_bundle(|manifest| manifest).await?;
        fixture
            .snap_tracker
            .upsert_game(
                "skyrim-se",
                "Skyrim",
                Path::new("/snap-visible/skyrim"),
                "Data",
                "bethesda",
                Some(Path::new("/snap-visible/prefix")),
                true,
            )
            .await?;

        let before = fixture.snap_tracker.load_persisted_games().await?.len();
        let result = preview_import_bundle(
            &fixture.snap_tracker,
            PreviewImportRequest {
                bundle_path: fixture.bundle_path.clone(),
            },
        )
        .await?;
        let after = fixture.snap_tracker.load_persisted_games().await?.len();

        assert_eq!(result.conflict, PreviewConflict::ExistingGame);
        assert_eq!(before, after, "preview must not mutate Snap games");

        Ok(())
    }

    #[tokio::test]
    async fn previews_hidden_game_as_importable() -> Result<()> {
        let fixture = PreviewFixture::new().await?;
        fixture.write_bundle(|manifest| manifest).await?;
        fixture
            .snap_tracker
            .upsert_game(
                "skyrim-se",
                "Skyrim",
                Path::new("/snap-visible/skyrim"),
                "Data",
                "bethesda",
                Some(Path::new("/snap-visible/prefix")),
                true,
            )
            .await?;
        fixture.snap_tracker.hide_game("skyrim-se").await?;

        let result = preview_import_bundle(
            &fixture.snap_tracker,
            PreviewImportRequest {
                bundle_path: fixture.bundle_path.clone(),
            },
        )
        .await?;

        assert_eq!(result.conflict, PreviewConflict::NewGame);
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("previously stopped"))
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_missing_manifest() -> Result<()> {
        let fixture = PreviewFixture::new().await?;
        fixture
            .write_raw_bundle(false, true, valid_manifest())
            .await?;

        let err = preview_import_bundle(
            &fixture.snap_tracker,
            PreviewImportRequest {
                bundle_path: fixture.bundle_path.clone(),
            },
        )
        .await
        .expect_err("bundle without manifest should be rejected");

        assert!(err.to_string().contains("manifest.json"));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_missing_export_db() -> Result<()> {
        let fixture = PreviewFixture::new().await?;
        fixture
            .write_raw_bundle(true, false, valid_manifest())
            .await?;

        let err = preview_import_bundle(
            &fixture.snap_tracker,
            PreviewImportRequest {
                bundle_path: fixture.bundle_path.clone(),
            },
        )
        .await
        .expect_err("bundle without export DB should be rejected");

        assert!(err.to_string().contains("data/export.db"));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unsupported_schema() -> Result<()> {
        let fixture = PreviewFixture::new().await?;
        fixture
            .write_bundle(|mut manifest| {
                manifest.schema_version = EXPORT_SCHEMA_VERSION + 1;
                manifest
            })
            .await?;

        let err = preview_import_bundle(
            &fixture.snap_tracker,
            PreviewImportRequest {
                bundle_path: fixture.bundle_path.clone(),
            },
        )
        .await
        .expect_err("unsupported schema should be rejected");

        assert!(
            err.to_string()
                .contains("Unsupported export schema version")
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_invalid_manifest_json() -> Result<()> {
        let fixture = PreviewFixture::new().await?;
        let file = fs::File::create(&fixture.bundle_path)?;
        let mut zip = zip::ZipWriter::new(file);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file("manifest.json", options)?;
        zip.write_all(b"{not-json")?;
        zip.start_file("data/export.db", options)?;
        zip.write_all(b"not a real db")?;
        zip.finish()?;

        let err = preview_import_bundle(
            &fixture.snap_tracker,
            PreviewImportRequest {
                bundle_path: fixture.bundle_path.clone(),
            },
        )
        .await
        .expect_err("invalid manifest should be rejected before DB open");

        assert!(err.to_string().contains("Invalid export manifest"));
        Ok(())
    }

    // Regression: AppImage-to-Snap migration metadata and deployment state.
    // @variants: snap
    #[tokio::test]
    async fn imports_new_game_into_snap_owned_state() -> Result<()> {
        let _guard = ENV_LOCK
            .lock()
            .map_err(|_| anyhow!("test environment lock was poisoned"))?;
        let fixture = PreviewFixture::new().await?;
        let snap_common = fixture._temp.path().join("snap-common");
        let _env = EnvVarGuard::set("SNAP_USER_COMMON", &snap_common);
        fixture.write_bundle(|manifest| manifest).await?;

        let result = import_bundle(
            &fixture.snap_tracker,
            ImportBundleRequest {
                bundle_path: fixture.bundle_path.clone(),
                confirmed_game_path: fixture._temp.path().join("snap-visible-game"),
                confirmed_wine_prefix: fixture._temp.path().join("snap-visible-prefix"),
            },
        )
        .await?;

        assert_eq!(result.game.id, "skyrim-se");
        assert_eq!(result.counts.mods, 1);
        let imported_download = result
            .download_entries
            .iter()
            .find(|entry| entry.id == "download-1")
            .expect("import result should include imported download metadata");
        assert!(
            imported_download.metadata_fetched,
            "import result should expose fetched download metadata to the UI"
        );
        assert_eq!(
            imported_download.nexus_file_name.as_deref(),
            Some("mod.zip")
        );
        assert_eq!(imported_download.archive_md5.as_deref(), Some("md5"));
        assert_eq!(imported_download.version.as_deref(), Some("1.0"));
        assert_eq!(imported_download.author.as_deref(), Some("Author"));
        let games = fixture.snap_tracker.load_persisted_games().await?;
        assert_eq!(games.len(), 1);
        assert_eq!(
            games[0].path,
            fixture._temp.path().join("snap-visible-game")
        );
        let tools: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tools")
            .fetch_one(&fixture.snap_tracker.pool)
            .await?;
        assert_eq!(tools, 0, "external tools must be skipped during import");
        let archive_path: Option<String> =
            sqlx::query_scalar("SELECT archive_path FROM download_entries WHERE id = 'download-1'")
                .fetch_one(&fixture.snap_tracker.pool)
                .await?;
        assert!(
            archive_path.is_none(),
            "download archive paths must be cleared"
        );
        let metadata = sqlx::query(
            "SELECT metadata_fetched, nexus_file_name, archive_md5, version, author
             FROM download_entries
             WHERE id = 'download-1'",
        )
        .fetch_one(&fixture.snap_tracker.pool)
        .await?;
        assert!(
            metadata.get::<bool, _>("metadata_fetched"),
            "fetched download metadata must stay attached to the imported row"
        );
        assert_eq!(
            metadata
                .get::<Option<String>, _>("nexus_file_name")
                .as_deref(),
            Some("mod.zip")
        );
        assert_eq!(
            metadata.get::<Option<String>, _>("archive_md5").as_deref(),
            Some("md5")
        );
        assert_eq!(
            metadata.get::<Option<String>, _>("version").as_deref(),
            Some("1.0")
        );
        assert_eq!(
            metadata.get::<Option<String>, _>("author").as_deref(),
            Some("Author")
        );
        let mod_metadata = sqlx::query(
            "SELECT nexus_file_name, nexus_is_primary, archive_md5
             FROM mods WHERE id = 'mod-1'",
        )
        .fetch_one(&fixture.snap_tracker.pool)
        .await?;
        assert_eq!(
            mod_metadata
                .get::<Option<String>, _>("nexus_file_name")
                .as_deref(),
            Some("source-mod.zip")
        );
        assert!(mod_metadata.get::<bool, _>("nexus_is_primary"));
        assert_eq!(
            mod_metadata
                .get::<Option<String>, _>("archive_md5")
                .as_deref(),
            Some("source-md5")
        );
        let deployed_profile: Option<String> = fixture
            .snap_tracker
            .get_setting("last_deployed_profile_skyrim-se")
            .await?;
        assert_eq!(deployed_profile.as_deref(), Some("profile-1"));
        let deployed_files: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM deployed_files
             WHERE game_id = 'skyrim-se' AND mod_id = 'mod-1'",
        )
        .fetch_one(&fixture.snap_tracker.pool)
        .await?;
        assert_eq!(deployed_files, 1, "deployed file state must remain intact");
        let cache_path: String =
            sqlx::query_scalar("SELECT cache_path FROM mod_files WHERE mod_id = 'mod-1'")
                .fetch_one(&fixture.snap_tracker.pool)
                .await?;
        let expected_cache = snap_common
            .join("deployd")
            .join("cache")
            .join("mod-1")
            .join("file.txt");
        assert_eq!(PathBuf::from(cache_path), expected_cache);
        assert!(expected_cache.is_file(), "cache file should be copied");
        let backup_path: String = sqlx::query_scalar(
            "SELECT backup_path FROM vanilla_backups WHERE game_id = 'skyrim-se'",
        )
        .fetch_one(&fixture.snap_tracker.pool)
        .await?;
        let expected_backup = snap_common
            .join("deployd")
            .join("skyrim-se")
            .join("vanilla-backup")
            .join("file.txt");
        assert_eq!(PathBuf::from(backup_path), expected_backup);
        assert!(expected_backup.is_file(), "vanilla backup should be copied");
        assert!(
            snap_common
                .join("deployd")
                .join("saves")
                .join("skyrim-se")
                .join("profile-1")
                .join("save.ess")
                .is_file(),
            "save snapshots should be copied"
        );

        Ok(())
    }

    // Regression: AppImage-to-Snap migration deployment state.
    // @variants: snap
    #[tokio::test]
    async fn skips_stale_last_deployed_profile() -> Result<()> {
        let _guard = ENV_LOCK
            .lock()
            .map_err(|_| anyhow!("test environment lock was poisoned"))?;
        let fixture = PreviewFixture::new().await?;
        let snap_common = fixture._temp.path().join("snap-common");
        let _env = EnvVarGuard::set("SNAP_USER_COMMON", &snap_common);
        let export_url = format!("sqlite://{}?mode=rwc", fixture.export_db.display());
        let export_tracker = Tracker::open(&export_url).await?;
        export_tracker
            .set_setting("last_deployed_profile_skyrim-se", "missing-profile")
            .await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&export_tracker.pool)
            .await?;
        export_tracker.pool.close().await;
        fixture.write_bundle(|manifest| manifest).await?;

        import_bundle(
            &fixture.snap_tracker,
            ImportBundleRequest {
                bundle_path: fixture.bundle_path.clone(),
                confirmed_game_path: fixture._temp.path().join("snap-visible-game"),
                confirmed_wine_prefix: fixture._temp.path().join("snap-visible-prefix"),
            },
        )
        .await?;

        let deployed_profile = fixture
            .snap_tracker
            .get_setting("last_deployed_profile_skyrim-se")
            .await?;
        assert!(
            deployed_profile.is_none(),
            "a marker that does not reference an imported profile must be omitted"
        );
        Ok(())
    }

    #[tokio::test]
    async fn imports_directory_sentinels_without_cached_files() -> Result<()> {
        let _guard = ENV_LOCK
            .lock()
            .map_err(|_| anyhow!("test environment lock was poisoned"))?;
        let fixture = PreviewFixture::new().await?;
        let snap_common = fixture._temp.path().join("snap-common");
        let _env = EnvVarGuard::set("SNAP_USER_COMMON", &snap_common);
        add_directory_sentinel(&fixture.export_db).await?;
        fixture.write_bundle(|manifest| manifest).await?;

        import_bundle(
            &fixture.snap_tracker,
            ImportBundleRequest {
                bundle_path: fixture.bundle_path.clone(),
                confirmed_game_path: fixture._temp.path().join("snap-visible-game"),
                confirmed_wine_prefix: fixture._temp.path().join("snap-visible-prefix"),
            },
        )
        .await?;

        let sentinel_cache_path: String = sqlx::query_scalar(
            "SELECT cache_path FROM mod_files
             WHERE mod_id = 'mod-1' AND game_rel_lowercase = 'empty/'",
        )
        .fetch_one(&fixture.snap_tracker.pool)
        .await?;
        assert!(
            sentinel_cache_path.contains("deployd/cache/mod-1/empty"),
            "directory sentinel cache path should be rewritten into Snap cache"
        );
        Ok(())
    }

    async fn add_directory_sentinel(export_db: &Path) -> Result<()> {
        let url = format!("sqlite://{}?mode=rwc", export_db.display());
        let pool = open_sqlite_pool(&url).await?;
        sqlx::query(
            "INSERT INTO mod_files (mod_id, game_rel_lowercase, game_rel_original, cache_path)
             VALUES ('mod-1', 'empty/', 'empty/', 'cache/mod-1/empty/')",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO deployed_files
             (game_id, game_rel_lowercase, game_rel_original, mod_id, cache_path)
             VALUES ('skyrim-se', 'empty/', 'empty/', 'mod-1', 'cache/mod-1/empty/')",
        )
        .execute(&pool)
        .await?;
        pool.close().await;
        Ok(())
    }

    #[tokio::test]
    async fn refuses_existing_game_import_without_writing_snap_state() -> Result<()> {
        let _guard = ENV_LOCK
            .lock()
            .map_err(|_| anyhow!("test environment lock was poisoned"))?;
        let fixture = PreviewFixture::new().await?;
        let snap_common = fixture._temp.path().join("snap-common");
        let _env = EnvVarGuard::set("SNAP_USER_COMMON", &snap_common);
        fixture.write_bundle(|manifest| manifest).await?;
        fixture
            .snap_tracker
            .upsert_game(
                "skyrim-se",
                "Skyrim",
                Path::new("/snap-visible/skyrim"),
                "Data",
                "bethesda",
                Some(Path::new("/snap-visible/prefix")),
                true,
            )
            .await?;

        let err = import_bundle(
            &fixture.snap_tracker,
            ImportBundleRequest {
                bundle_path: fixture.bundle_path.clone(),
                confirmed_game_path: fixture._temp.path().join("snap-visible-game"),
                confirmed_wine_prefix: fixture._temp.path().join("snap-visible-prefix"),
            },
        )
        .await
        .expect_err("existing Snap games must not be imported over");

        assert!(err.to_string().contains("already managed"));
        let games = fixture.snap_tracker.load_persisted_games().await?;
        assert_eq!(games.len(), 1);
        assert!(
            !snap_common
                .join("deployd")
                .join("cache")
                .join("mod-1")
                .exists(),
            "existing-game refusal must happen before copying files"
        );
        Ok(())
    }

    #[tokio::test]
    async fn imports_over_hidden_game_state() -> Result<()> {
        let _guard = ENV_LOCK
            .lock()
            .map_err(|_| anyhow!("test environment lock was poisoned"))?;
        let fixture = PreviewFixture::new().await?;
        let snap_common = fixture._temp.path().join("snap-common");
        let _env = EnvVarGuard::set("SNAP_USER_COMMON", &snap_common);
        fixture.write_bundle(|manifest| manifest).await?;
        fixture
            .snap_tracker
            .upsert_game(
                "skyrim-se",
                "Old Skyrim",
                Path::new("/old/skyrim"),
                "Data",
                "bethesda",
                Some(Path::new("/old/prefix")),
                true,
            )
            .await?;
        sqlx::query(
            "INSERT INTO mods (id, game_id, name, priority)
             VALUES ('mod-1', 'skyrim-se', 'Old Mod', 0)",
        )
        .execute(&fixture.snap_tracker.pool)
        .await?;
        fixture.snap_tracker.hide_game("skyrim-se").await?;
        let old_cache = snap_common.join("deployd").join("cache").join("mod-1");
        std::fs::create_dir_all(&old_cache)?;
        std::fs::write(old_cache.join("old.txt"), b"old")?;

        let result = import_bundle(
            &fixture.snap_tracker,
            ImportBundleRequest {
                bundle_path: fixture.bundle_path.clone(),
                confirmed_game_path: fixture._temp.path().join("snap-visible-game"),
                confirmed_wine_prefix: fixture._temp.path().join("snap-visible-prefix"),
            },
        )
        .await?;

        assert_eq!(result.game.id, "skyrim-se");
        let hidden: i32 = sqlx::query_scalar("SELECT hidden FROM games WHERE id = 'skyrim-se'")
            .fetch_one(&fixture.snap_tracker.pool)
            .await?;
        assert_eq!(hidden, 0);
        assert!(
            !old_cache.join("old.txt").exists(),
            "old hidden cache should be replaced"
        );
        assert!(
            old_cache.join("file.txt").is_file(),
            "new imported cache should be copied"
        );
        Ok(())
    }

    #[tokio::test]
    async fn rejects_import_without_required_confirmations() -> Result<()> {
        let fixture = PreviewFixture::new().await?;
        let err = import_bundle(
            &fixture.snap_tracker,
            ImportBundleRequest {
                bundle_path: fixture.bundle_path.clone(),
                confirmed_game_path: PathBuf::new(),
                confirmed_wine_prefix: fixture._temp.path().join("prefix"),
            },
        )
        .await
        .expect_err("missing game folder confirmation should reject");
        assert!(err.to_string().contains("confirmed game folder"));
        Ok(())
    }

    #[tokio::test]
    async fn rejects_primary_key_collision_without_writing_game() -> Result<()> {
        let _guard = ENV_LOCK
            .lock()
            .map_err(|_| anyhow!("test environment lock was poisoned"))?;
        let fixture = PreviewFixture::new().await?;
        let snap_common = fixture._temp.path().join("snap-common");
        let _env = EnvVarGuard::set("SNAP_USER_COMMON", &snap_common);
        fixture.write_bundle(|manifest| manifest).await?;
        sqlx::query(
            "INSERT INTO mods (id, game_id, name, priority)
             VALUES ('mod-1', 'other-game', 'Existing Mod', 0)",
        )
        .execute(&fixture.snap_tracker.pool)
        .await?;

        let err = import_bundle(
            &fixture.snap_tracker,
            ImportBundleRequest {
                bundle_path: fixture.bundle_path.clone(),
                confirmed_game_path: fixture._temp.path().join("snap-visible-game"),
                confirmed_wine_prefix: fixture._temp.path().join("snap-visible-prefix"),
            },
        )
        .await
        .expect_err("colliding imported IDs should reject");

        assert!(
            err.to_string()
                .contains("would overwrite existing Snap mods")
        );
        let imported_game: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM games WHERE id = 'skyrim-se'")
                .fetch_one(&fixture.snap_tracker.pool)
                .await?;
        assert_eq!(imported_game, 0);
        assert!(
            !snap_common
                .join("deployd")
                .join("cache")
                .join("mod-1")
                .exists(),
            "collision refusal must happen before copying files"
        );
        Ok(())
    }

    struct PreviewFixture {
        _temp: TempDir,
        bundle_path: PathBuf,
        export_db: PathBuf,
        snap_tracker: Tracker,
    }

    struct EnvVarGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &Path) -> Self {
            let original = std::env::var_os(key);
            // SAFETY: tests that mutate process environment hold ENV_LOCK, so
            // no other migration_import test reads or writes SNAP_USER_COMMON concurrently.
            unsafe {
                std::env::set_var(key, value);
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: EnvVarGuard is only created while ENV_LOCK is held by the
            // owning test, preventing concurrent environment access in this module.
            unsafe {
                match &self.original {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    impl PreviewFixture {
        async fn new() -> Result<Self> {
            let temp = TempDir::new()?;
            let export_db = temp.path().join("export.db");
            let export_url = format!("sqlite://{}?mode=rwc", export_db.display());
            create_export_db(&export_url).await?;

            let snap_db = temp.path().join("snap.db");
            let snap_url = format!("sqlite://{}?mode=rwc", snap_db.display());
            let snap_tracker = Tracker::open(&snap_url).await?;

            Ok(Self {
                bundle_path: temp.path().join("preview.deployd-export.zip"),
                _temp: temp,
                export_db,
                snap_tracker,
            })
        }

        async fn write_bundle(
            &self,
            mutate: impl FnOnce(ExportManifest) -> ExportManifest,
        ) -> Result<()> {
            self.write_raw_bundle(true, true, mutate(valid_manifest()))
                .await
        }

        async fn write_raw_bundle(
            &self,
            include_manifest: bool,
            include_db: bool,
            manifest: ExportManifest,
        ) -> Result<()> {
            let file = fs::File::create(&self.bundle_path)?;
            let mut zip = zip::ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            if include_manifest {
                zip.start_file("manifest.json", options)?;
                zip.write_all(&serde_json::to_vec(&manifest)?)?;
            }
            if include_db {
                zip.start_file("data/export.db", options)?;
                zip.write_all(&fs::read(&self.export_db)?)?;
            }
            zip.start_file("cache/mod-1/file.txt", options)?;
            zip.write_all(b"cache")?;
            zip.start_file("vanilla-backup/file.txt", options)?;
            zip.write_all(b"backup")?;
            zip.start_file("saves/skyrim-se/profile-1/save.ess", options)?;
            zip.write_all(b"save")?;
            zip.finish()?;
            Ok(())
        }
    }

    fn valid_manifest() -> ExportManifest {
        ExportManifest {
            schema_version: EXPORT_SCHEMA_VERSION,
            deployd_version: env!("CARGO_PKG_VERSION").to_string(),
            source_package: SOURCE_PACKAGE_APPIMAGE.to_string(),
            exported_at: "2026-05-17T00:00:00Z".to_string(),
            game_id: "skyrim-se".to_string(),
            game_title: "Skyrim Special Edition".to_string(),
            original_game_path: "/games/skyrim".to_string(),
            original_wine_prefix: Some("/prefix/skyrim".to_string()),
            advisory_downloads_dir: "/home/user/Downloads".to_string(),
            warnings: vec!["Export warning".to_string()],
        }
    }

    async fn create_export_db(url: &str) -> Result<()> {
        let tracker = Tracker::open(url).await?;
        tracker
            .upsert_game(
                "skyrim-se",
                "Skyrim Special Edition",
                Path::new("/games/skyrim"),
                "Data",
                "bethesda",
                Some(Path::new("/prefix/skyrim")),
                true,
            )
            .await?;
        sqlx::query(
            "INSERT INTO mod_groups (id, game_id, name, position, collapsed, color)
             VALUES ('group-1', 'skyrim-se', 'Group', 0.0, 0, 'blue')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO mods
             (id, game_id, name, archive_hash, archive_path, installed_at, enabled, priority,
              nexus_mod_id, nexus_file_id, nexus_domain, version, author, latest_version,
              nexus_description, group_id, install_target, notes, fomod_selections,
              nexus_file_name, nexus_is_primary, archive_md5)
             VALUES
             ('mod-1', 'skyrim-se', 'Mod', 'hash', '/downloads/mod.zip', 'now', 1, 0,
              101, 202, 'skyrimspecialedition', '1.0', 'Author', '1.1',
              'Description', 'group-1', 'data', 'Notes', '{\"choices\":[]}',
              'source-mod.zip', 1, 'source-md5')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO mod_files (mod_id, game_rel_lowercase, game_rel_original, cache_path)
             VALUES ('mod-1', 'file.txt', 'file.txt', 'cache/mod-1/file.txt')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO plugins (id, mod_id, filename, load_order, enabled)
             VALUES ('plugin-1', 'mod-1', 'a.esp', 0, 1)",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO plugin_masters (plugin_id, master)
             VALUES ('plugin-1', 'Skyrim.esm')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO deployed_files
             (game_id, game_rel_lowercase, game_rel_original, mod_id, cache_path)
             VALUES ('skyrim-se', 'file.txt', 'file.txt', 'mod-1', 'cache/mod-1/file.txt')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO profiles (id, game_id, name, is_active, save_mode)
             VALUES ('profile-1', 'skyrim-se', 'Default', 1, 'profile')",
        )
        .execute(&tracker.pool)
        .await?;
        tracker
            .set_setting("last_deployed_profile_skyrim-se", "profile-1")
            .await?;
        sqlx::query(
            "INSERT INTO profile_mods (profile_id, mod_id, enabled, priority)
             VALUES ('profile-1', 'mod-1', 1, 0)",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
             VALUES ('profile-1', 'plugin-1', 1, 0)",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO vanilla_files (game_id, game_rel_lowercase, file_size, mtime_secs)
             VALUES ('skyrim-se', 'skyrim.esm', 42, 123)",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO order_snapshots (id, game_id, name, kind, created_at)
             VALUES ('snapshot-1', 'skyrim-se', 'Snapshot', 'mods', 'now')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO order_snapshot_entries (snapshot_id, entry_id, position)
             VALUES ('snapshot-1', 'mod-1', 0)",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO vanilla_backups (game_id, game_rel_path, backup_path)
             VALUES ('skyrim-se', 'file.txt', 'vanilla-backup/file.txt')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO download_entries
             (id, mod_name, archive_path, nexus_mod_id, nexus_file_id, nexus_domain, game_domain,
              metadata_fetched, nexus_file_name, nexus_is_primary, status, archive_hash,
              archive_md5, version, author, hidden)
             VALUES
             ('download-1', 'Mod', '/downloads/mod.zip', 101, 202, 'skyrimspecialedition',
              'skyrimspecialedition', 1, 'mod.zip', 1, 'installed', 'hash', 'md5',
              '1.0', 'Author', 0)",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query(
            "INSERT INTO tools (id, game_id, name, exe_path, icon_name, custom_args, sort_order, working_dir)
             VALUES ('tool-1', 'skyrim-se', 'Tool', '/tools/tool.exe', 'application-x-executable-symbolic', '', 0, '')",
        )
        .execute(&tracker.pool)
        .await?;
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&tracker.pool)
            .await?;
        tracker.pool.close().await;
        Ok(())
    }
}
