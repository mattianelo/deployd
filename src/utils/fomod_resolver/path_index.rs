use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::dlog;

use super::types::FomodFileMapping;

pub(super) struct FileRef<'a> {
    pub(super) source: &'a str,
    pub(super) destination: Option<&'a str>,
}

/// Build a lookup table mapping lowercased relative paths to real on-disk paths.
/// Covers all files and directories in the extracted tree.
///
/// Keys are added relative to `extracted_root`.  When `content_root` differs
/// (i.e. the archive has a wrapper directory), additional keys relative to
/// `content_root` are added so FOMOD source paths work regardless of whether
/// they include the wrapper prefix.
pub(super) fn build_path_index(extracted_root: &Path, content_root: &Path) -> HashMap<String, PathBuf> {
    let mut index = HashMap::new();
    for entry in WalkDir::new(extracted_root) {
        let Ok(entry) = entry else { continue };
        let abs = entry.path().to_path_buf();

        // Key relative to extracted_root (archive root)
        if let Ok(rel) = entry.path().strip_prefix(extracted_root) {
            let key = rel.to_string_lossy().to_lowercase().replace('\\', "/");
            if !key.is_empty() {
                index.insert(key, abs.clone());
            }
        }

        // Also index relative to content_root for mods with wrapper directories
        if content_root != extracted_root
            && let Ok(rel) = entry.path().strip_prefix(content_root)
        {
            let key = rel.to_string_lossy().to_lowercase().replace('\\', "/");
            if !key.is_empty() {
                index.entry(key).or_insert(abs);
            }
        }
    }
    index
}

/// Collect files from a single file/folder entry using case-insensitive path lookup.
/// If the source is a directory, recursively add all files inside it.
pub(super) fn collect_file(
    ft: &FileRef<'_>,
    extracted_root: &Path,
    path_index: &HashMap<String, PathBuf>,
    mappings: &mut Vec<FomodFileMapping>,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let source_key = ft
        .source
        .to_lowercase()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_string();

    let source_path = match path_index.get(&source_key) {
        Some(p) => p.clone(),
        None => {
            if !source_key.is_empty() {
                warnings.push(format!("FOMOD source not found: {}", ft.source));
            }
            return Ok(());
        }
    };

    // Normalize destination: FOMOD XML uses Windows backslashes and may include a
    // leading ".\" (e.g. ".\DIP") which becomes "./" after normalization and must be stripped.
    // - destination=None  → unwrap_or(source) gives the source path (non-empty) → subfolder
    // - destination=""    → dest_base is empty → install to Data root
    // - destination="./x" → strip "./" → "x"
    let dest_base = ft.destination.unwrap_or(ft.source).replace('\\', "/");
    let dest_base = dest_base.trim_end_matches('/').trim_start_matches("./");

    let src_is_dir = source_path.is_dir();
    let src_is_file = source_path.is_file();
    dlog!(
        "[deployd] FOMOD collect: source={:?} is_dir={src_is_dir} is_file={src_is_file}",
        source_path.display()
    );

    if src_is_dir {
        let mut file_count = 0usize;
        for entry in WalkDir::new(&source_path) {
            let entry = entry?;
            // Never emit the source directory itself as a file entry — WalkDir always yields
            // the root as its first element; guard against any edge case where the root's
            // file_type() could be misreported.
            if entry.path() == source_path {
                continue;
            }
            if entry.file_type().is_file() {
                let rel_to_source = entry.path().strip_prefix(&source_path)?;
                mappings.push(FomodFileMapping {
                    source_relative: entry.path().strip_prefix(extracted_root)?.to_path_buf(),
                    dest_relative: Path::new(dest_base).join(rel_to_source),
                });
                file_count += 1;
            }
        }
        if file_count == 0 {
            dlog!(
                "[deployd] FOMOD: directory source has no files: {}",
                source_path.display()
            );
            // Emit a directory sentinel so the installer still creates the folder in the
            // game directory. The installer identifies sentinels by source_relative pointing
            // to a directory (src_abs.is_dir() check in add_mod_with_file_list).
            mappings.push(FomodFileMapping {
                source_relative: source_path.strip_prefix(extracted_root)?.to_path_buf(),
                dest_relative: PathBuf::from(dest_base),
            });
        }
    } else if src_is_file {
        let dest = if dest_base.is_empty() {
            // No destination — use normalized source path
            PathBuf::from(ft.source.replace('\\', "/"))
        } else if dest_base.ends_with('/') {
            Path::new(dest_base).join(source_path.file_name().unwrap_or_default())
        } else {
            PathBuf::from(dest_base)
        };
        mappings.push(FomodFileMapping {
            source_relative: source_path.strip_prefix(extracted_root)?.to_path_buf(),
            dest_relative: dest,
        });
    } else {
        dlog!(
            "[deployd] FOMOD: source is neither file nor dir (broken symlink or special file?): {}",
            source_path.display()
        );
    }

    Ok(())
}
