use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::dlog;
use crate::utils::fomod_resolver;

/// `(file_list, stripped_wrapper)` — the file list plus the optional wrapper dir name.
type FileListResult = (Vec<(PathBuf, PathBuf)>, Option<String>);

/// Returns (file_list, stripped_wrapper) where stripped_wrapper is the name of the
/// single wrapper directory removed from the archive root (if any).
pub(super) fn resolve_file_list(extracted_root: &Path) -> Result<FileListResult> {
    // Check for FOMOD first
    if let Some(config_path) = fomod_resolver::detect_fomod(extracted_root) {
        let mappings = fomod_resolver::resolve_fomod_default(extracted_root, &config_path)?;
        let result = mappings
            .into_iter()
            .map(|m| (extracted_root.join(&m.source_relative), m.dest_relative))
            .collect();
        return Ok((result, None));
    }

    // Normal mod: apply wrapper stripping, then collect all files
    let (effective_root, stripped_wrapper) = detect_wrapper(extracted_root);

    let mut files: Vec<(PathBuf, PathBuf)> = Vec::new();
    // Track every non-fomod subdirectory and which ones have at least one tracked file
    // anywhere in their subtree. Used below to emit directory sentinels.
    let mut all_dirs: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut dirs_with_files: HashSet<PathBuf> = HashSet::new();

    for entry in WalkDir::new(&effective_root) {
        let entry = entry?;
        let rel = entry.path().strip_prefix(&effective_root)?;
        // Skip fomod metadata even in non-FOMOD mods
        let rel_lower = rel.to_string_lossy().to_lowercase();
        if rel_lower.starts_with("fomod/") || rel_lower.starts_with("fomod\\") {
            continue;
        }

        if entry.file_type().is_file() {
            // Mark every ancestor directory as having at least one tracked file.
            let mut parent = rel.parent();
            while let Some(p) = parent {
                if p.as_os_str().is_empty() {
                    break;
                }
                dirs_with_files.insert(p.to_path_buf());
                parent = p.parent();
            }
            files.push((entry.path().to_path_buf(), rel.to_path_buf()));
        } else if entry.file_type().is_dir() && !rel.as_os_str().is_empty() {
            all_dirs.push((entry.path().to_path_buf(), rel.to_path_buf()));
        }
    }

    // Emit directory sentinels for folders whose entire subtree has no tracked files.
    // These folders may contain only hidden/system files excluded by the archiving tool
    // (e.g. JContainers' Domains/ folder) or must simply exist at runtime.
    // The installer detects sentinels via src_abs.is_dir() and creates the directory
    // in the game folder during deployment even with no file to hardlink.
    for (dir_abs, dir_rel) in all_dirs {
        if !dirs_with_files.contains(&dir_rel) {
            dlog!("[deployd] empty-dir sentinel: {}", dir_rel.display());
            files.push((dir_abs, dir_rel));
        }
    }

    Ok((files, stripped_wrapper))
}

/// Detect a single wrapper directory.
///
/// A wrapper exists when the root contains exactly one non-fomod subdirectory
/// and zero meaningful files (ignoring readmes, changelogs, etc.), AND the
/// subdirectory is not a known Bethesda content directory (SKSE, Meshes, etc.).
///
/// Returns `(effective_root, stripped_wrapper_name)` where `stripped_wrapper_name`
/// is the original directory name if a wrapper was stripped (`Some`), else `None`.
fn detect_wrapper(extracted_root: &Path) -> (PathBuf, Option<String>) {
    let entries: Vec<_> = match fs::read_dir(extracted_root) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return (extracted_root.to_path_buf(), None),
    };

    let mut dirs = Vec::new();
    let mut has_meaningful_file = false;

    for entry in &entries {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let name_lower = entry.file_name().to_string_lossy().to_lowercase();
        if ft.is_dir() {
            if name_lower != "fomod" {
                // Store original name alongside lowercase for later use
                let orig_name = entry.file_name().to_string_lossy().to_string();
                dirs.push((entry.path(), name_lower, orig_name));
            }
        } else if ft.is_file() && !is_ignorable_file(&name_lower) {
            has_meaningful_file = true;
        }
    }

    if dirs.len() == 1 && !has_meaningful_file {
        let (path, name_lower, orig_name) = dirs.into_iter().next().unwrap();
        if is_known_content_dir(&name_lower) {
            (extracted_root.to_path_buf(), None)
        } else {
            (path, Some(orig_name))
        }
    } else {
        (extracted_root.to_path_buf(), None)
    }
}

/// Check whether a directory name is a known game content directory
/// that should never be stripped as a wrapper.
/// Covers both Bethesda (SKSE, Meshes, …) and REDEngine (archive, mods, r6, …) layouts.
fn is_known_content_dir(name_lower: &str) -> bool {
    matches!(
        name_lower,
        // Bethesda
        "data"
            | "skse"
            | "f4se"
            | "nvse"
            | "fose"
            | "obse"
            | "mwse"
            | "meshes"
            | "textures"
            | "sound"
            | "music"
            | "scripts"
            | "source"
            | "interface"
            | "strings"
            | "seq"
            | "grass"
            | "lodsettings"
            | "shadersfx"
            | "vis"
            | "materials"
            | "geometries"
            | "animations"
            | "plugins"
            | "docs"
            | "tools"
            | "edit scripts"
            | "calientetools"
            | "netscriptframework"
            | "dllplugins"
            | "asi"
            | "video"
            | "videos"
            | "mcm"   // MCM (Mod Configuration Menu) — Config/ and Settings/ live inside
            // REDEngine (Cyberpunk 2077 / The Witcher 3)
            | "archive"   // archive/pc/mod/ — CP2077 & W3 mod archives
            | "mods"      // REDmod directory (CP2077) and Mods/ (W3)
            | "r6"        // CP2077 scripts, tweaks, config
            | "red4ext"   // CP2077 REDscript extensions
            | "bin"       // CP2077 binary plugins (CET: bin/x64/plugins/…)
            | "content"   // W3 content subdirectories
            | "dlc" // W3 DLC-style mods
    )
}

fn is_ignorable_file(name_lower: &str) -> bool {
    matches!(
        name_lower,
        "readme.txt"
            | "readme.md"
            | "readme"
            | "changelog.txt"
            | "changelog.md"
            | "license.txt"
            | "license"
            | "credits.txt"
            | "version.txt"
    )
}
