use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use tempfile::TempDir;
use zip::ZipArchive;

use crate::core::migration_bundle::ExportManifest;

pub(super) struct ExtractedPreview {
    pub(super) _work: TempDir,
    pub(super) manifest: ExportManifest,
    pub(super) export_db: PathBuf,
    pub(super) bundle_files: BundleFileCounts,
}

pub(super) struct ExtractedImport {
    pub(super) _work: TempDir,
    pub(super) manifest: ExportManifest,
    pub(super) export_db: PathBuf,
    pub(super) payload_root: PathBuf,
    pub(super) bundle_files: BundleFileCounts,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct BundleFileCounts {
    pub(super) cache_files: usize,
    pub(super) vanilla_backups: usize,
    pub(super) save_snapshots: usize,
}

pub(super) fn extract_preview_bundle(bundle_path: &Path) -> Result<ExtractedPreview> {
    let file = fs::File::open(bundle_path)
        .with_context(|| format!("Failed to open {}", bundle_path.display()))?;
    let mut archive = ZipArchive::new(file).context("Failed to read export bundle")?;

    let manifest = read_manifest(&mut archive)?;
    require_export_database(&mut archive)?;

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

pub(super) fn extract_import_bundle(bundle_path: &Path) -> Result<ExtractedImport> {
    let file = fs::File::open(bundle_path)
        .with_context(|| format!("Failed to open {}", bundle_path.display()))?;
    let mut archive = ZipArchive::new(file).context("Failed to read export bundle")?;

    let manifest = read_manifest(&mut archive)?;
    require_export_database(&mut archive)?;

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

fn read_manifest<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<ExportManifest> {
    let mut manifest_bytes = Vec::new();
    archive
        .by_name("manifest.json")
        .context("Export bundle is missing manifest.json")?
        .read_to_end(&mut manifest_bytes)
        .context("Failed to read export manifest")?;
    let manifest: ExportManifest =
        serde_json::from_slice(&manifest_bytes).context("Invalid export manifest")?;
    manifest.validate()?;
    Ok(manifest)
}

fn require_export_database<R: std::io::Read + std::io::Seek>(
    archive: &mut ZipArchive<R>,
) -> Result<()> {
    if archive.by_name("data/export.db").is_err() {
        bail!("Export bundle is missing data/export.db");
    }
    Ok(())
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
