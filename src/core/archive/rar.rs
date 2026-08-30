use std::path::Path;

use anyhow::Result;
use tempfile::TempDir;

use crate::dlog;

use super::create_tmp;

pub(super) fn extract_rar(archive_path: &Path) -> Result<TempDir> {
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
