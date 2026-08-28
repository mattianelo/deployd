use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tempfile::TempDir;

use crate::dlog;
use crate::utils::{archive, fomod_resolver};

use super::{dazip, file_list};

pub enum PrepareResult {
    Normal {
        file_list: Vec<(PathBuf, PathBuf)>,
        /// Original wrapper dir name stripped by detect_wrapper (e.g. `"modSkipMovies"`).
        /// Used by REDEngine path fixups to preserve the archive's folder name under `Mods/`.
        stripped_wrapper: Option<String>,
        tmp_dir: TempDir,
    },
    Fomod {
        config: fomod_resolver::FomodUiConfig,
        config_path: PathBuf,
        tmp_dir: TempDir,
    },
}

pub async fn prepare_mod(
    archive_path: &Path,
    on_extract_progress: Option<Box<dyn Fn(usize, usize) + Send>>,
    on_processing: Option<Box<dyn FnOnce() + Send>>,
) -> Result<PrepareResult> {
    dlog!("[deployd] prepare_mod: {}", archive_path.display());
    let path = archive_path.to_path_buf();
    let tmp_dir = tokio::task::spawn_blocking(move || {
        archive::extract_archive(&path, on_extract_progress)
            .with_context(|| format!("Extraction failed for: {}", path.display()))
    })
    .await
    .context("Extraction task panicked")??;

    if let Some(cb) = on_processing {
        cb();
    }

    let is_dazip = archive_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("dazip"));
    let stem = if is_dazip {
        archive_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string()
    } else {
        String::new()
    };

    tokio::task::spawn_blocking(move || {
        let extracted_root = tmp_dir.path();
        dlog!("[deployd] extracted to: {}", extracted_root.display());

        if is_dazip {
            dazip::process_dazip_root(extracted_root, &stem)
                .context("Failed to process dazip archive")?;
        } else {
            dazip::expand_dazip_files_in_place(extracted_root)
                .context("Failed to expand nested .dazip files")?;
        }

        if let Some(config_path) = fomod_resolver::detect_fomod(extracted_root) {
            dlog!("[deployd] FOMOD detected: {}", config_path.display());
            let config = fomod_resolver::parse_fomod_config(&config_path).with_context(|| {
                format!("Failed to parse FOMOD config: {}", config_path.display())
            })?;
            dlog!(
                "[deployd] FOMOD config parsed: {} steps",
                config.steps.len()
            );
            Ok(PrepareResult::Fomod {
                config,
                config_path,
                tmp_dir,
            })
        } else {
            let (file_list, stripped_wrapper) = file_list::resolve_file_list(extracted_root)
                .context("Failed to resolve file list from extracted archive")?;
            dlog!("[deployd] normal mod: {} files resolved", file_list.len());
            Ok(PrepareResult::Normal {
                file_list,
                stripped_wrapper,
                tmp_dir,
            })
        }
    })
    .await
    .context("Post-extraction task panicked")?
}
