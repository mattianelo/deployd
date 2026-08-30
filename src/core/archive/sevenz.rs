use std::fs;
use std::io::{self, Write as _};
use std::path::Path;

use anyhow::{Context, Result};
use tempfile::TempDir;

use crate::dlog;

use super::create_tmp;

pub(super) fn extract_7z(
    archive_path: &Path,
    on_progress: Option<&(dyn Fn(usize, usize) + Send)>,
) -> Result<TempDir> {
    // libarchive (liblzma C) is orders of magnitude faster than the pure-Rust
    // lzma-rust2 decoder for solid LZMA archives. Try it first when available.
    #[cfg(feature = "libarchive-fallback")]
    {
        match extract_7z_libarchive(archive_path) {
            Ok(tmp) => return Ok(tmp),
            Err(e) => {
                dlog!("[deployd] libarchive failed for 7z, falling back to sevenz_rust2: {e:#}")
            }
        }
    }

    extract_7z_native(archive_path, on_progress)
}

/// Returns `true` when a 7z archive entry is a genuine directory.
///
/// When a 7z archive lacks the `EmptyFiles` header attribute, `sevenz_rust2` falls back
/// to treating ALL no-stream entries (including zero-byte files) as directories.  This
/// misidentifies sentinel files like `.force-install` that mod authors use to force a
/// directory to be included in the archive.
///
/// Returns `true` if `dest_path` is safely inside `base` with no traversal components.
///
/// sevenz_rust2 passes entry names to the extract callback as-is; unlike the `zip` crate's
/// `mangled_name()`, there is no built-in sanitization, so a crafted archive could supply
/// a path like `../../etc/passwd` and escape the temp directory.
fn is_safe_7z_path(dest_path: &Path, base: &Path) -> bool {
    use std::path::Component;
    let Ok(rel) = dest_path.strip_prefix(base) else {
        return false;
    };
    rel.components().all(|c| matches!(c, Component::Normal(_)))
}

/// We check two more-reliable signals before trusting `is_directory`:
///
/// 1. Windows file attributes — `FILE_ATTRIBUTE_DIRECTORY` (0x10) set → real dir.
/// 2. Name suffix — directory names in well-formed 7z archives end with `\` or `/`.
fn is_genuine_7z_dir(entry: &sevenz_rust2::ArchiveEntry) -> bool {
    if !entry.is_directory {
        return false;
    }
    if entry.has_windows_attributes {
        // FILE_ATTRIBUTE_DIRECTORY = 0x10
        return entry.windows_attributes & 0x10 != 0;
    }
    // Fallback: well-formed 7z archives use a trailing path separator for directories.
    entry.name.ends_with('/') || entry.name.ends_with('\\')
}

fn extract_7z_native(
    archive_path: &Path,
    on_progress: Option<&(dyn Fn(usize, usize) + Send)>,
) -> Result<TempDir> {
    let tmp = create_tmp()?;

    // Count non-directory entries from archive metadata when progress reporting is needed.
    // Use is_genuine_7z_dir so zero-byte files that sevenz_rust2 misidentifies as
    // directories (e.g. .force-install) are counted as files, not directories.
    let total = if on_progress.is_some() {
        let archive = sevenz_rust2::Archive::open(archive_path)
            .map_err(|e| anyhow::anyhow!("Failed to open 7z metadata: {e}"))?;
        let t = archive
            .files
            .iter()
            .filter(|f| !is_genuine_7z_dir(f))
            .count();
        drop(archive);
        dlog!("[deployd] 7z: {t} file entries to extract");
        t
    } else {
        dlog!("[deployd] 7z: extracting (no progress tracking)");
        0
    };

    // Open the archive file once and pass the handle directly so the file is not
    // re-opened inside decompress_with_extract_fn (avoiding a second OS open call).
    let file = fs::File::open(archive_path)
        .with_context(|| format!("Cannot open archive: {}", archive_path.display()))?;

    // Always use a custom extract fn so is_genuine_7z_dir applies regardless of
    // whether progress reporting is requested. Without this, the default decompress_file
    // misidentifies zero-byte sentinel files (e.g. Domains/.force-install) as directories
    // when the 7z archive lacks the EmptyFiles header attribute.
    let base = tmp.path().to_path_buf();
    let mut count = 0usize;
    sevenz_rust2::decompress_with_extract_fn(file, &base, |entry, reader, dest_path| {
        if is_genuine_7z_dir(entry) {
            if !is_safe_7z_path(dest_path, &base) {
                eprintln!(
                    "[deployd] WARNING: skipping 7z directory with traversal path: {}",
                    dest_path.display()
                );
                return Ok(true);
            }
            fs::create_dir_all(dest_path)?;
            return Ok(true);
        }
        if !is_safe_7z_path(dest_path, &base) {
            eprintln!(
                "[deployd] WARNING: skipping 7z entry with traversal path: {}",
                dest_path.display()
            );
            return Ok(true);
        }
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut writer = io::BufWriter::with_capacity(131_072, fs::File::create(dest_path)?);
        io::copy(reader, &mut writer)?;
        writer.flush()?;
        if let Some(cb) = on_progress {
            count += 1;
            cb(count, total);
        }
        Ok(true)
    })
    .map_err(|e| anyhow::anyhow!("Failed to extract 7z '{}': {e}", archive_path.display()))?;

    dlog!("[deployd] 7z extraction complete (native)");
    Ok(tmp)
}

/// Fallback 7z extraction using libarchive (available in org.gnome.Platform 49+).
/// Handles solid LZMA archives that the pure-Rust decoder (`lzma-rust2`) fails on.
#[cfg(feature = "libarchive-fallback")]
fn extract_7z_libarchive(archive_path: &Path) -> Result<TempDir> {
    let tmp = create_tmp()?;
    dlog!(
        "[deployd] 7z libarchive: extracting to {}",
        tmp.path().display()
    );
    let mut source = fs::File::open(archive_path)
        .with_context(|| format!("Cannot open archive: {}", archive_path.display()))?;
    compress_tools::uncompress_archive(&mut source, tmp.path(), compress_tools::Ownership::Ignore)
        .map_err(|e| {
            anyhow::anyhow!(
                "libarchive extraction failed for '{}': {e}",
                archive_path.display()
            )
        })?;
    dlog!("[deployd] 7z libarchive extraction complete");
    Ok(tmp)
}
