use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use crate::models::plugin::Plugin;

/// Write the Plugins.txt file for a Bethesda game.
///
/// Format:
/// ```text
/// # This file is managed by Deployd
/// *EnabledPlugin.esp
/// DisabledPlugin.esp
/// ```
pub fn write_plugins_txt(path: &Path, plugins: &[Plugin]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("Cannot create Plugins.txt parent dir: {}", parent.display())
        })?;
    }

    let mut file = std::fs::File::create(path)
        .with_context(|| format!("Cannot create Plugins.txt: {}", path.display()))?;

    writeln!(file, "# This file is managed by Deployd")?;
    for plugin in plugins {
        if plugin.enabled {
            writeln!(file, "*{}", plugin.filename)?;
        } else {
            writeln!(file, "{}", plugin.filename)?;
        }
    }

    Ok(())
}

/// Parse a Plugins.txt file, returning (filename, enabled) tuples in file order.
///
/// Lines starting with `#` are comments. Lines starting with `*` are enabled plugins.
/// Returns `Ok(vec![])` if the file does not exist.
pub fn read_plugins_txt(path: &Path) -> Result<Vec<(String, bool)>> {
    if !path.exists() {
        return Ok(vec![]);
    }

    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read Plugins.txt: {}", path.display()))?;

    let mut entries = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(name) = trimmed.strip_prefix('*') {
            entries.push((name.to_string(), true));
        } else {
            entries.push((trimmed.to_string(), false));
        }
    }

    Ok(entries)
}

/// Ensure the ArchiveInvalidation custom INI file exists so the game engine
/// loads loose files from the Data directory instead of only BSA archives.
///
/// Only creates the file if it does not already exist — never overwrites
/// user customizations.
pub fn ensure_archive_invalidation(path: &Path) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create INI parent dir: {}", parent.display()))?;
    }

    let mut file = std::fs::File::create(path)
        .with_context(|| format!("Cannot create INI: {}", path.display()))?;

    writeln!(file, "[Archive]")?;
    writeln!(file, "bInvalidateOlderFiles=1")?;
    writeln!(file, "sResourceDataDirsFinal=")?;

    Ok(())
}
