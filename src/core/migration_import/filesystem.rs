use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::utils::paths;

#[derive(Debug, Clone)]
pub(super) struct ImportPaths {
    cache_root: PathBuf,
    backup_root: PathBuf,
}

impl ImportPaths {
    pub(super) fn new(game_id: &str) -> Result<Self> {
        Ok(Self {
            cache_root: paths::cache_root().context("Cannot resolve Snap cache folder")?,
            backup_root: paths::vanilla_backup_dir(game_id)
                .context("Cannot resolve Snap vanilla backup folder")?,
        })
    }
}

pub(super) async fn validate_export_dependencies(
    pool: &sqlx::SqlitePool,
    payload_root: &Path,
    import_paths: &ImportPaths,
) -> Result<()> {
    for table in ["mod_files", "deployed_files"] {
        let sql = format!("SELECT game_rel_lowercase, cache_path FROM {table}");
        let rows: Vec<(String, Option<String>)> = sqlx::query_as(&sql)
            .fetch_all(pool)
            .await
            .with_context(|| format!("Failed to read {table} cache paths"))?;
        for (game_rel, cache_path) in rows {
            let Some(cache_path) = cache_path else {
                continue;
            };
            if skips_cache_file_validation(&game_rel, &cache_path) {
                continue;
            }
            let staged = staged_cache_path(payload_root, &cache_path)?;
            if !staged.is_file() {
                bail!(
                    "Export bundle is missing a cached mod file referenced by {table}: {}",
                    cache_path
                );
            }
            rewrite_cache_path(&cache_path, import_paths)?;
        }
    }

    let rows: Vec<(String,)> = sqlx::query_as("SELECT backup_path FROM vanilla_backups")
        .fetch_all(pool)
        .await
        .context("Failed to read vanilla backup paths")?;
    for (backup_path,) in rows {
        let staged = staged_backup_path(payload_root, &backup_path)?;
        if !staged.is_file() {
            bail!("Export bundle is missing a vanilla backup referenced by the DB: {backup_path}");
        }
        rewrite_backup_path(&backup_path, import_paths)?;
    }

    Ok(())
}

#[derive(Debug, Default)]
pub(super) struct CopiedPayload {
    paths: Vec<PathBuf>,
}

pub(super) fn copy_payload_to_snap(payload_root: &Path, game_id: &str) -> Result<CopiedPayload> {
    let mut copied = CopiedPayload::default();
    let result = copy_payload_to_snap_inner(payload_root, game_id, &mut copied);
    if let Err(error) = result {
        cleanup_copied_payload(&copied);
        return Err(error);
    }
    Ok(copied)
}

fn copy_payload_to_snap_inner(
    payload_root: &Path,
    game_id: &str,
    copied: &mut CopiedPayload,
) -> Result<()> {
    let cache_root = paths::cache_root().context("Cannot resolve Snap cache folder")?;
    let cache_stage = payload_root.join("cache");
    if cache_stage.exists() {
        fs::create_dir_all(&cache_root)
            .with_context(|| format!("Failed to create {}", cache_root.display()))?;
        for entry in fs::read_dir(&cache_stage)
            .with_context(|| format!("Failed to read {}", cache_stage.display()))?
        {
            let entry = entry?;
            let source = entry.path();
            if !entry.file_type()?.is_dir() {
                bail!(
                    "Export cache contains unsupported file at {}",
                    source.display()
                );
            }
            let dest = cache_root.join(entry.file_name());
            if dest.exists() {
                bail!(
                    "Import would overwrite existing Snap cache folder {}",
                    dest.display()
                );
            }
            copy_dir_recursive(&source, &dest)?;
            copied.paths.push(dest);
        }
    }

    let backup_stage = payload_root.join("vanilla-backup");
    if backup_stage.exists() {
        let dest = paths::vanilla_backup_dir(game_id)?;
        if dest.exists() {
            bail!(
                "Import would overwrite existing Snap vanilla backup folder {}",
                dest.display()
            );
        }
        copy_dir_recursive(&backup_stage, &dest)?;
        copied.paths.push(dest);
    }

    let saves_stage = payload_root.join("saves").join(game_id);
    if saves_stage.exists() {
        let dest = paths::saves_root()?.join(game_id);
        if dest.exists() {
            bail!(
                "Import would overwrite existing Snap save snapshot folder {}",
                dest.display()
            );
        }
        copy_dir_recursive(&saves_stage, &dest)?;
        copied.paths.push(dest);
    }

    Ok(())
}

pub(super) fn cleanup_copied_payload(copied: &CopiedPayload) {
    for path in copied.paths.iter().rev() {
        if let Err(e) = fs::remove_dir_all(path) {
            eprintln!(
                "Failed to clean incomplete import path {}: {e}",
                path.display()
            );
        }
    }
}

fn copy_dir_recursive(source: &Path, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)
        .with_context(|| format!("Failed to create directory {}", dest.display()))?;
    for entry in walkdir::WalkDir::new(source).min_depth(1) {
        let entry = entry?;
        let rel = entry
            .path()
            .strip_prefix(source)
            .context("Failed to compute relative copy path")?;
        let target = dest.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("Failed to create directory {}", target.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create directory {}", parent.display()))?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "Failed to copy {} to {}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        }
    }
    Ok(())
}

fn staged_cache_path(payload_root: &Path, bundle_path: &str) -> Result<PathBuf> {
    Ok(payload_root.join(bundle_relative_path(bundle_path, "cache")?))
}

fn staged_backup_path(payload_root: &Path, bundle_path: &str) -> Result<PathBuf> {
    Ok(payload_root.join(bundle_relative_path(bundle_path, "vanilla-backup")?))
}

fn rewrite_cache_path(bundle_path: &str, import_paths: &ImportPaths) -> Result<PathBuf> {
    let rel = strip_bundle_prefix(bundle_path, "cache")?;
    Ok(import_paths.cache_root.join(rel))
}

pub(super) fn rewrite_cache_path_for_row(
    bundle_path: &str,
    game_rel: &str,
    import_paths: &ImportPaths,
) -> Result<String> {
    if bundle_path.is_empty() {
        return Ok(String::new());
    }
    let rewritten = rewrite_cache_path(bundle_path, import_paths)?;
    let mut value = rewritten.to_string_lossy().into_owned();
    if game_rel.ends_with('/') && !value.ends_with(std::path::MAIN_SEPARATOR) {
        value.push(std::path::MAIN_SEPARATOR);
    }
    Ok(value)
}

pub(super) fn rewrite_backup_path(
    bundle_path: &str,
    import_paths: &ImportPaths,
) -> Result<PathBuf> {
    let rel = strip_bundle_prefix(bundle_path, "vanilla-backup")?;
    Ok(import_paths.backup_root.join(rel))
}

fn bundle_relative_path(bundle_path: &str, prefix: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(prefix).join(strip_bundle_prefix(bundle_path, prefix)?))
}

fn strip_bundle_prefix(bundle_path: &str, prefix: &str) -> Result<PathBuf> {
    let normalized = bundle_path.replace('\\', "/");
    let rel = normalized
        .strip_prefix(&format!("{prefix}/"))
        .ok_or_else(|| anyhow!("Expected bundle-relative {prefix} path, got {bundle_path}"))?;
    let rel = rel.trim_end_matches('/');
    if rel.is_empty() || rel.split('/').any(|part| part == ".." || part.is_empty()) {
        bail!("Unsafe bundle-relative path: {bundle_path}");
    }
    Ok(PathBuf::from(rel))
}

fn skips_cache_file_validation(game_rel: &str, cache_path: &str) -> bool {
    game_rel.ends_with('/') || cache_path.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_traversal_in_bundle_paths() {
        let error = strip_bundle_prefix("cache/mod-1/../outside", "cache")
            .expect_err("parent traversal must be rejected");

        assert!(error.to_string().contains("Unsafe bundle-relative path"));
    }

    #[test]
    fn preserves_directory_sentinel_suffix_when_rewriting_cache_path() -> Result<()> {
        let import_paths = ImportPaths {
            cache_root: PathBuf::from("/snap/cache"),
            backup_root: PathBuf::from("/snap/backup"),
        };

        let rewritten = rewrite_cache_path_for_row("cache/mod-1/empty/", "empty/", &import_paths)?;

        assert!(rewritten.ends_with(std::path::MAIN_SEPARATOR));
        Ok(())
    }
}
