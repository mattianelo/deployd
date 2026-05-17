use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tempfile::TempDir;
use zip::ZipArchive;

use crate::core::migration_bundle::ExportManifest;
use crate::core::tracker::Tracker;

#[derive(Debug, Clone)]
pub struct PreviewImportRequest {
    pub bundle_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct PreviewImportResult {
    pub manifest: ExportManifest,
    pub counts: PreviewCounts,
    pub conflict: PreviewConflict,
    pub validation_items: Vec<ValidationItem>,
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
    NeedsDownloadsFolderConfirmation,
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

    let exists = tracker
        .load_persisted_games()
        .await
        .context("Failed to check existing Snap games")?
        .into_iter()
        .any(|game| game.id == extracted.manifest.game_id);

    let mut warnings = extracted.manifest.warnings.clone();
    if exists {
        warnings.push(
            "This game is already managed in the Snap; a later import phase will skip it by default."
                .to_string(),
        );
    }

    let mut validation_items = vec![
        ValidationItem::NeedsGameFolderConfirmation,
        ValidationItem::NeedsWinePrefixConfirmation,
        ValidationItem::NeedsDownloadsFolderConfirmation,
    ];
    if counts.tools > 0 {
        validation_items.push(ValidationItem::ToolsNeedSnapRuntimeRebind);
    }

    Ok(PreviewImportResult {
        manifest: extracted.manifest,
        counts,
        conflict: if exists {
            PreviewConflict::ExistingGame
        } else {
            PreviewConflict::NewGame
        },
        validation_items,
        warnings,
    })
}

struct ExtractedPreview {
    _work: TempDir,
    manifest: ExportManifest,
    export_db: PathBuf,
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
    use zip::write::SimpleFileOptions;

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

    struct PreviewFixture {
        _temp: TempDir,
        bundle_path: PathBuf,
        export_db: PathBuf,
        snap_tracker: Tracker,
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
        let pool = open_sqlite_pool(url).await?;
        for stmt in [
            "CREATE TABLE mods (id TEXT PRIMARY KEY, game_id TEXT, name TEXT)",
            "CREATE TABLE plugins (id TEXT PRIMARY KEY, mod_id TEXT, filename TEXT)",
            "CREATE TABLE profiles (id TEXT PRIMARY KEY, game_id TEXT, name TEXT)",
            "CREATE TABLE tools (id TEXT PRIMARY KEY, game_id TEXT, name TEXT)",
            "CREATE TABLE download_entries (id TEXT PRIMARY KEY, mod_name TEXT)",
        ] {
            sqlx::query(stmt).execute(&pool).await?;
        }
        sqlx::query("INSERT INTO mods (id, game_id, name) VALUES ('mod-1', 'skyrim-se', 'Mod')")
            .execute(&pool)
            .await?;
        sqlx::query(
            "INSERT INTO plugins (id, mod_id, filename) VALUES ('plugin-1', 'mod-1', 'a.esp')",
        )
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO profiles (id, game_id, name) VALUES ('profile-1', 'skyrim-se', 'Default')",
        )
        .execute(&pool)
        .await?;
        sqlx::query("INSERT INTO tools (id, game_id, name) VALUES ('tool-1', 'skyrim-se', 'Tool')")
            .execute(&pool)
            .await?;
        sqlx::query("INSERT INTO download_entries (id, mod_name) VALUES ('download-1', 'Mod')")
            .execute(&pool)
            .await?;
        pool.close().await;
        Ok(())
    }
}
