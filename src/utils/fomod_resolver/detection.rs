use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::dlog;
use crate::utils::paths::lowercase_path_str;

/// Look for fomod/ModuleConfig.xml (case-insensitive) in the extracted directory.
/// Searches up to depth 5 to handle mods with extra wrapper directories.
/// Returns the absolute path to the config if found.
pub fn detect_fomod(extracted_root: &Path) -> Option<PathBuf> {
    let mut info_xml_found = false;

    for entry in WalkDir::new(extracted_root).max_depth(5) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if entry.file_type().is_file() {
            let rel = entry
                .path()
                .strip_prefix(extracted_root)
                .unwrap_or(entry.path());
            let rel_lower = lowercase_path_str(rel);
            if rel_lower.ends_with("fomod/moduleconfig.xml") {
                return Some(entry.path().to_path_buf());
            }
            if rel_lower.ends_with("fomod/info.xml") {
                info_xml_found = true;
            }
        }
    }

    if info_xml_found {
        dlog!("[deployd] Found fomod/info.xml but no ModuleConfig.xml — FOMOD may be incomplete");
    }

    None
}
