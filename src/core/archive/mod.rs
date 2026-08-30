use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use tempfile::TempDir;

use crate::dlog;
use crate::utils::paths;

mod hashing;
mod rar;
mod sevenz;
mod zip;

pub use hashing::{compute_md5, hash_archive_file};
pub use zip::extract_zip_to;

/// Extract an archive into a new temporary directory.
/// Supports ZIP, 7z, and RAR formats (detected by file extension).
/// The caller owns the returned `TempDir`; dropping it removes all extracted files.
///
/// The temp directory is placed under the app data directory (not `/tmp`) for reliability.
///
/// `on_progress` is called with `(done, total)` as each file is extracted.
/// Total is always known for ZIP and 7z; for RAR the callback is not invoked.
pub fn extract_archive(
    archive_path: &Path,
    on_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
) -> Result<TempDir> {
    let ext = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    dlog!(
        "[deployd] extract_archive: {} (format: .{})",
        archive_path.display(),
        ext
    );

    // Verify the file is readable before attempting extraction.
    let meta = fs::metadata(archive_path)
        .with_context(|| format!("Cannot stat archive: {}", archive_path.display()))?;
    dlog!("[deployd] archive size: {} bytes", meta.len());

    let cb: Option<&(dyn Fn(usize, usize) + Send)> = on_progress.as_deref();

    match ext.as_str() {
        "zip" | "dazip" => zip::extract_zip(archive_path, cb),
        "7z" => sevenz::extract_7z(archive_path, cb),
        "rar" => rar::extract_rar(archive_path),
        other => bail!("Unsupported archive format: .{other}. Supported: .zip, .7z, .rar, .dazip"),
    }
}

/// Create a temp directory on the real filesystem (under app data dir).
/// Falls back to the system default if the app data dir is unavailable.
pub(super) fn create_tmp() -> Result<TempDir> {
    match paths::cache_root() {
        Ok(data_dir) => {
            fs::create_dir_all(&data_dir)
                .with_context(|| format!("Cannot create cache root: {}", data_dir.display()))?;
            let tmp = tempfile::Builder::new()
                .prefix("extract_")
                .tempdir_in(&data_dir)
                .with_context(|| {
                    format!(
                        "Failed to create temp dir in cache root: {}",
                        data_dir.display()
                    )
                })?;
            dlog!("[deployd] temp dir: {}", tmp.path().display());
            Ok(tmp)
        }
        Err(e) => {
            dlog!("[deployd] cache_root unavailable ({e}), falling back to system /tmp");
            TempDir::new().context("Failed to create temp directory")
        }
    }
}
