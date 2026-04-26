use std::fs;
use std::io::{self, Read as _, Seek, Write as _};
use std::path::Path;

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::paths;
use crate::dlog;

/// Compute a SHA-256 hex digest of an archive file.
///
/// Used to detect duplicate installations: if the same archive is chosen
/// twice, the hash matches and we can offer "already installed" feedback.
/// Reads the entire file sequentially; for large archives this is I/O-bound
/// but much cheaper than a full extraction.
pub fn hash_archive_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)
        .with_context(|| format!("Cannot open archive for hashing: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

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
        "zip" | "dazip" => extract_zip(archive_path, cb),
        "7z" => extract_7z(archive_path, cb),
        "rar" => extract_rar(archive_path),
        other => bail!("Unsupported archive format: .{other}. Supported: .zip, .7z, .rar, .dazip"),
    }
}

/// Create a temp directory on the real filesystem (under app data dir).
/// Falls back to the system default if the app data dir is unavailable.
fn create_tmp() -> Result<TempDir> {
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

/// Outer ZIP dispatcher: tries the native decoder; if any entry uses LZMA
/// compression (which the pure-Rust decoder cannot handle without hanging),
/// falls back to libarchive when the `libarchive-fallback` feature is enabled.
fn extract_zip(
    archive_path: &Path,
    on_progress: Option<&(dyn Fn(usize, usize) + Send)>,
) -> Result<TempDir> {
    let result = extract_zip_native(archive_path, on_progress);

    if let Err(ref e) = result {
        let err_msg = format!("{e:#}");
        if err_msg.contains("LZMA compression (unsupported)") {
            dlog!("[deployd] ZIP native decoder hit LZMA entry: {e:#}");
            #[cfg(feature = "libarchive-fallback")]
            {
                dlog!(
                    "[deployd] trying libarchive fallback for ZIP: {}",
                    archive_path.display()
                );
                return extract_zip_libarchive(archive_path).with_context(|| {
                    format!(
                        "All ZIP methods failed for '{}' (native: {e:#})",
                        archive_path.display()
                    )
                });
            }
        }
    }

    result
}

/// Extract a ZIP archive into an existing directory (no TempDir created).
/// Used for in-place expansion of nested archives (e.g. `.dazip` files found inside a mod).
pub fn extract_zip_to(archive_path: &Path, dest: &Path) -> Result<()> {
    let mut data = fs::read(archive_path)
        .with_context(|| format!("Cannot read archive: {}", archive_path.display()))?;

    let mut archive = match zip::ZipArchive::new(io::Cursor::new(data.clone())) {
        Ok(a) => a,
        Err(zip_err) => {
            let err_msg = zip_err.to_string();
            let is_unicode_field_err = err_msg.contains("CRC32 checksum")
                || err_msg.contains("Unicode extra field")
                || (err_msg.contains("Invalid UTF-8") && !err_msg.contains("symlink"));
            if !is_unicode_field_err {
                return Err(zip_err)
                    .with_context(|| format!("Invalid ZIP: {}", archive_path.display()));
            }
            if !strip_zip_unicode_extra_fields(&mut data) {
                return Err(zip_err).with_context(|| {
                    format!(
                        "Invalid ZIP (malformed Unicode extra field, no patch applied): {}",
                        archive_path.display()
                    )
                });
            }
            zip::ZipArchive::new(io::Cursor::new(data)).with_context(|| {
                format!(
                    "Invalid ZIP (still invalid after stripping Unicode extra fields): {}",
                    archive_path.display()
                )
            })?
        }
    };

    let total = archive.len();
    for i in 0..total {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("Failed to read ZIP entry #{i}"))?;
        let entry_path = entry.mangled_name();
        if entry_path.as_os_str().is_empty() {
            continue;
        }
        let out_path = dest.join(&entry_path);
        if entry.is_dir() {
            let _ = fs::create_dir_all(&out_path);
        } else {
            if matches!(entry.compression(), zip::CompressionMethod::Lzma) {
                return Err(anyhow::anyhow!(
                    "ZIP entry '{}' uses LZMA compression (unsupported) in '{}'",
                    entry.name(),
                    archive_path.display()
                ));
            }
            if let Some(parent) = out_path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                dlog!(
                    "[deployd] extract_zip_to: skipping '{}' — cannot create parent: {e}",
                    entry.name()
                );
                continue;
            }
            match fs::File::create(&out_path) {
                Ok(outfile) => {
                    let mut writer = io::BufWriter::with_capacity(131_072, outfile);
                    io::copy(&mut entry, &mut writer)
                        .with_context(|| format!("Failed to write file: {}", out_path.display()))?;
                    writer
                        .flush()
                        .with_context(|| format!("Failed to flush file: {}", out_path.display()))?;
                }
                Err(e) if e.raw_os_error() == Some(21) => {
                    dlog!(
                        "[deployd] extract_zip_to: skipping '{}' (EISDIR)",
                        entry.name()
                    );
                }
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Failed to create file: {}", out_path.display()));
                }
            }
        }
    }
    Ok(())
}

/// Native ZIP extraction: handles the Info-ZIP Unicode Path extra-field patch,
/// then extracts all entries. Bails immediately if an entry uses LZMA compression
/// to avoid an infinite hang in the decoder.
fn extract_zip_native(
    archive_path: &Path,
    on_progress: Option<&(dyn Fn(usize, usize) + Send)>,
) -> Result<TempDir> {
    let file = fs::File::open(archive_path)
        .with_context(|| format!("Cannot open archive: {}", archive_path.display()))?;

    match zip::ZipArchive::new(file) {
        Ok(archive) => run_zip_extraction(archive, archive_path, on_progress),
        Err(zip_err) => {
            // Some ZIP creation tools (older WinZip, certain 7-Zip settings) include an
            // Info-ZIP Unicode Path Extra Field (0x7075) but write an incorrect CRC32 for it.
            // The `zip` crate validates this checksum strictly and rejects the entire archive;
            // system `unzip` silently ignores the mismatch and falls back to the filename in
            // the regular central-directory entry.
            //
            // When we hit that specific failure, read the file into memory, neutralise the
            // offending extra-field tags (replace 0x7075/0x6375 with 0xFFFF so the crate
            // treats them as unknown and skips them), then retry.
            let err_msg = zip_err.to_string();
            dlog!("[deployd] initial ZipArchive::new failed: {err_msg}");

            let is_unicode_field_err = err_msg.contains("CRC32 checksum")
                || err_msg.contains("Unicode extra field")
                || (err_msg.contains("Invalid UTF-8") && !err_msg.contains("symlink"));

            if !is_unicode_field_err {
                return Err(zip_err)
                    .with_context(|| format!("Invalid ZIP: {}", archive_path.display()));
            }

            dlog!(
                "[deployd] Attempting ZIP unicode-field patch for: {}",
                archive_path.display()
            );
            let mut data = fs::read(archive_path)
                .with_context(|| format!("Cannot read archive: {}", archive_path.display()))?;

            if !strip_zip_unicode_extra_fields(&mut data) {
                // Nothing was patched; surface the original error.
                return Err(zip_err).with_context(|| {
                    format!(
                        "Invalid ZIP (malformed Unicode extra field, no patch applied): {}",
                        archive_path.display()
                    )
                });
            }

            dlog!("[deployd] ZIP unicode-field patch applied, retrying");
            let cursor = io::Cursor::new(data);
            let archive = zip::ZipArchive::new(cursor).with_context(|| {
                format!(
                    "Invalid ZIP (still invalid after stripping Unicode extra fields): {}",
                    archive_path.display()
                )
            })?;
            run_zip_extraction(archive, archive_path, on_progress)
        }
    }
}

/// Fallback ZIP extraction using libarchive (available in org.gnome.Platform 49+).
/// Handles ZIP files that contain LZMA-compressed entries which the pure-Rust
/// decoder cannot decompress without hanging.
#[cfg(feature = "libarchive-fallback")]
fn extract_zip_libarchive(archive_path: &Path) -> Result<TempDir> {
    let tmp = create_tmp()?;
    dlog!(
        "[deployd] ZIP libarchive: extracting to {}",
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
    dlog!("[deployd] ZIP libarchive extraction complete");
    Ok(tmp)
}

/// Core extraction loop, generic over the reader type so it works for both
/// `fs::File` (normal path) and `io::Cursor<Vec<u8>>` (lenient fallback path).
///
/// Returns an error immediately if any entry uses LZMA compression (method 14),
/// which the pure-Rust decoder cannot handle and would hang indefinitely.
fn run_zip_extraction<R: io::Read + Seek>(
    mut archive: zip::ZipArchive<R>,
    archive_path: &Path,
    on_progress: Option<&(dyn Fn(usize, usize) + Send)>,
) -> Result<TempDir> {
    let tmp = create_tmp()?;
    let total = archive.len();
    dlog!("[deployd] ZIP: {total} entries to extract");

    for i in 0..total {
        let mut entry = archive
            .by_index(i)
            .with_context(|| format!("Failed to read ZIP entry #{i}"))?;

        let raw_name = entry.name().to_string();
        // mangled_name() strips path-traversal attacks (leading /, .., etc.)
        let entry_path = entry.mangled_name();

        // Guard: mangled_name can return an empty path when the entry name is
        // entirely path-traversal (e.g. "../foo") and gets sanitized away.
        // Creating a file at the temp-dir root would fail with IsADirectory.
        if entry_path.as_os_str().is_empty() {
            dlog!(
                "[deployd] ZIP: skipping entry #{i} with empty sanitized name (raw: {raw_name:?})"
            );
            if let Some(cb) = on_progress {
                cb(i + 1, total);
            }
            continue;
        }

        let out_path = tmp.path().join(&entry_path);

        if entry.is_dir() {
            // If the path already exists as a regular file (archive has both a file entry
            // and a directory entry with the same name), skip the directory creation rather
            // than aborting the extraction. The file entry wins.
            if let Err(e) = fs::create_dir_all(&out_path)
                && !out_path.is_dir()
            {
                dlog!(
                    "[deployd] ZIP: skipping directory entry '{raw_name}' — \
                         path already exists as a non-directory: {e}"
                );
            }
            // If it IS already a dir, create_dir_all should have returned Ok;
            // if we still get an error here, fall through silently.
        } else {
            let compression = entry.compression();

            // LZMA (ZIP method 14) hangs the pure-Rust decoder indefinitely.
            // Bail now so the outer extract_zip can try the libarchive fallback.
            if matches!(compression, zip::CompressionMethod::Lzma) {
                return Err(anyhow::anyhow!(
                    "ZIP entry '{}' uses LZMA compression (unsupported) in archive '{}'",
                    raw_name,
                    archive_path.display()
                ));
            }

            dlog!(
                "[deployd] ZIP: [{}/{}] {raw_name} ({} B, method {:?})",
                i + 1,
                total,
                entry.compressed_size(),
                compression,
            );

            // A parent component may already exist as a file (malformed archive where a file
            // and a same-named directory entry coexist). Skip this entry instead of aborting.
            if let Some(parent) = out_path.parent()
                && let Err(e) = fs::create_dir_all(parent)
            {
                dlog!(
                    "[deployd] ZIP: skipping entry '{raw_name}' — \
                         cannot create parent dir: {e}"
                );
                if let Some(cb) = on_progress {
                    cb(i + 1, total);
                }
                continue;
            }

            // If the target path is already a directory (directory entry processed before
            // this file entry), skip creating the file rather than failing with EISDIR.
            match fs::File::create(&out_path) {
                Ok(outfile) => {
                    let mut writer = io::BufWriter::with_capacity(131_072, outfile);
                    io::copy(&mut entry, &mut writer)
                        .with_context(|| format!("Failed to write file: {}", out_path.display()))?;
                    writer
                        .flush()
                        .with_context(|| format!("Failed to flush file: {}", out_path.display()))?;
                }
                Err(e) if e.raw_os_error() == Some(21) => {
                    // EISDIR — a directory with this name was already created.
                    dlog!(
                        "[deployd] ZIP: skipping file entry '{raw_name}' — \
                         path already exists as a directory (EISDIR)"
                    );
                }
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("Failed to create file: {}", out_path.display()));
                }
            }

            dlog!("[deployd] ZIP: [{}/{}] ok", i + 1, total);
        }

        if let Some(cb) = on_progress {
            cb(i + 1, total);
        }
    }

    dlog!("[deployd] ZIP extraction complete");
    Ok(tmp)
}

/// Neutralise Info-ZIP Unicode Path (0x7075) and Unicode Comment (0x6375) extra fields
/// in the ZIP central directory by replacing their tag bytes with 0xFFFF.
///
/// Some tools write these fields with an incorrect CRC32.  The `zip` crate validates the
/// checksum and rejects the archive; system `unzip` ignores the mismatch.  Replacing the
/// tag with an unknown value (0xFFFF) causes the crate to skip the sub-record entirely,
/// falling back to the CP437-decoded filename stored in the regular central-directory entry.
///
/// Only the central directory is patched; local file headers are left untouched because
/// the `zip` crate already ignores extra-field errors in local headers.
///
/// Returns `true` if at least one field was patched.
fn strip_zip_unicode_extra_fields(data: &mut [u8]) -> bool {
    const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    const CD_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
    const UNICODE_PATH: u16 = 0x7075;
    const UNICODE_COMMENT: u16 = 0x6375;

    let len = data.len();
    if len < 22 {
        return false;
    }

    // Locate the End of Central Directory record by scanning backward.
    // The EOCD lives within the last (22 + 65535) bytes because the archive comment
    // is at most 65535 bytes long.
    let scan_end = len - 22;
    let scan_start = scan_end.saturating_sub(65535);
    let eocd_pos = (scan_start..=scan_end).rev().find(|&pos| {
        data[pos..pos + 4] == EOCD_SIG && {
            let comment_len = u16::from_le_bytes([data[pos + 20], data[pos + 21]]) as usize;
            pos + 22 + comment_len == len
        }
    });

    let eocd_pos = match eocd_pos {
        Some(p) => p,
        None => return false,
    };

    // EOCD layout (offsets relative to signature):
    //  +12  size of central directory (4 bytes LE)
    //  +16  offset of central directory from start of disk (4 bytes LE)
    if eocd_pos + 20 > data.len() {
        return false;
    }
    let cd_size =
        u32::from_le_bytes(data[eocd_pos + 12..eocd_pos + 16].try_into().unwrap()) as usize;
    let cd_offset =
        u32::from_le_bytes(data[eocd_pos + 16..eocd_pos + 20].try_into().unwrap()) as usize;

    // Skip ZIP64 archives (offset sentinel = 0xFFFFFFFF) and perform a basic sanity check.
    if cd_offset == 0xFFFF_FFFF || cd_offset.saturating_add(cd_size) > len {
        return false;
    }

    let mut patched = false;
    let mut pos = cd_offset;
    let cd_end = cd_offset + cd_size;

    while pos + 46 <= cd_end {
        if data[pos..pos + 4] != CD_SIG {
            break; // Not a central-directory file header — stop walking.
        }

        // Central-directory file header layout (offsets from signature):
        //  +28  file name length   (2 bytes LE)
        //  +30  extra field length (2 bytes LE)
        //  +32  file comment length(2 bytes LE)
        //  +46  file name          (fn_len bytes)
        //  +46+fn_len  extra field (ef_len bytes)
        //  ...  file comment       (fc_len bytes)
        let fn_len = u16::from_le_bytes([data[pos + 28], data[pos + 29]]) as usize;
        let ef_len = u16::from_le_bytes([data[pos + 30], data[pos + 31]]) as usize;
        let fc_len = u16::from_le_bytes([data[pos + 32], data[pos + 33]]) as usize;

        let ef_start = pos + 46 + fn_len;
        let ef_end = ef_start + ef_len;
        if ef_end > cd_end {
            break; // Corrupt entry.
        }

        // Walk extra-field sub-records: [tag 2B LE][size 2B LE][data size B]
        let mut ef_pos = ef_start;
        while ef_pos + 4 <= ef_end {
            let tag = u16::from_le_bytes([data[ef_pos], data[ef_pos + 1]]);
            let sub_len = u16::from_le_bytes([data[ef_pos + 2], data[ef_pos + 3]]) as usize;
            if ef_pos + 4 + sub_len > ef_end {
                break; // Corrupt sub-record.
            }
            if tag == UNICODE_PATH || tag == UNICODE_COMMENT {
                // Replace with 0xFFFF — an unrecognised tag the crate will ignore.
                data[ef_pos] = 0xFF;
                data[ef_pos + 1] = 0xFF;
                patched = true;
            }
            ef_pos += 4 + sub_len;
        }

        pos = ef_end + fc_len;
    }

    patched
}

fn extract_7z(
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
/// We check two more-reliable signals before trusting `is_directory`:
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
    let mut count = 0usize;
    sevenz_rust2::decompress_with_extract_fn(file, tmp.path(), |entry, reader, dest_path| {
        if is_genuine_7z_dir(entry) {
            fs::create_dir_all(dest_path)?;
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

fn extract_rar(archive_path: &Path) -> Result<TempDir> {
    let tmp = create_tmp()?;
    let dest = tmp.path();
    dlog!("[deployd] RAR: extracting to {}", dest.display());
    let mut archive = unrar::Archive::new(archive_path)
        .open_for_processing()
        .map_err(|e| anyhow::anyhow!("Failed to open RAR '{}': {e}", archive_path.display()))?;
    loop {
        archive = match archive.read_header() {
            Ok(Some(header)) => header.extract_with_base(dest).map_err(|e| {
                anyhow::anyhow!(
                    "Failed to extract RAR entry from '{}': {e}",
                    archive_path.display()
                )
            })?,
            Ok(None) => break,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "Failed to read RAR header from '{}': {e}",
                    archive_path.display()
                ));
            }
        };
    }
    dlog!("[deployd] RAR extraction complete");
    Ok(tmp)
}
