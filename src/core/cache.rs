use std::io;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::core::tracker::Tracker;
use crate::utils::paths;

/// Move a game's mod cache from `old_cache_root` to `new_cache_root`.
///
/// Steps:
/// 1. Validate that `new_cache_root` is on the same filesystem as `game_path` (required for
///    hardlinks). Returns an error with a clear explanation if the check fails.
/// 2. Create `new_cache_root`.
/// 3. For each mod belonging to this game, move its cache subdirectory from old to new.
///    Attempts `fs::rename` (fast, atomic, same-filesystem) first; falls back to a recursive
///    copy-then-delete when the paths cross device boundaries.
/// 4. Update all `cache_path` entries in the database to reflect the new prefix.
/// 5. Persist the new cache root as the game's custom cache dir in the settings table.
///
/// Partial failure during the file move (some mods moved, some not) is logged per-mod and
/// the operation continues — the DB update and setting write only happen when all moves
/// complete without error.
pub async fn move_game_cache(
    tracker: &Tracker,
    game_id: &str,
    game_path: &Path,
    old_cache_root: &Path,
    new_cache_root: &Path,
) -> Result<()> {
    validate_same_filesystem(new_cache_root, game_path)?;

    std::fs::create_dir_all(new_cache_root)
        .with_context(|| format!("Cannot create cache dir: {}", new_cache_root.display()))?;

    let mods = tracker
        .list_mods(game_id)
        .await
        .context("Failed to list mods for cache move")?;

    let moved = move_cache_directories(&mods, old_cache_root, new_cache_root)?;

    let old_prefix = old_cache_root.to_string_lossy();
    let new_prefix = new_cache_root.to_string_lossy();
    if let Err(error) = tracker
        .commit_game_cache_move(game_id, &old_prefix, &new_prefix, Some(new_cache_root))
        .await
    {
        return Err(cache_move_failure(error, rollback_cache_moves(&moved)));
    }

    Ok(())
}

/// Clear a game's custom cache dir setting and move its mods back to `default_cache_root`.
pub async fn reset_game_cache(
    tracker: &Tracker,
    game_id: &str,
    current_cache_root: &Path,
    default_cache_root: &Path,
) -> Result<()> {
    if current_cache_root == default_cache_root {
        return Ok(());
    }

    std::fs::create_dir_all(default_cache_root)
        .with_context(|| format!("Cannot create cache dir: {}", default_cache_root.display()))?;

    let mods = tracker
        .list_mods(game_id)
        .await
        .context("Failed to list mods for cache reset")?;

    let moved = move_cache_directories(&mods, current_cache_root, default_cache_root)?;

    let old_prefix = current_cache_root.to_string_lossy();
    let new_prefix = default_cache_root.to_string_lossy();
    if let Err(error) = tracker
        .commit_game_cache_move(game_id, &old_prefix, &new_prefix, None)
        .await
    {
        return Err(cache_move_failure(error, rollback_cache_moves(&moved)));
    }

    Ok(())
}

/// Check that `new_dir` and `game_path` are on the same filesystem (same `st_dev`).
///
/// Hardlinks require both ends to share the same filesystem. This is not just a
/// same-device check — BTRFS subvolumes and ZFS datasets each have their own inode
/// space and are distinct filesystems even when sharing a physical device.
fn validate_same_filesystem(new_dir: &Path, game_path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let new_meta = std::fs::metadata(new_dir)
        .or_else(|_| {
            // Directory may not exist yet; check its parent instead.
            new_dir
                .parent()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no parent"))
                .and_then(std::fs::metadata)
        })
        .with_context(|| {
            format!(
                "Cannot stat '{}': directory or parent must exist",
                new_dir.display()
            )
        })?;

    let game_meta = std::fs::metadata(game_path)
        .with_context(|| format!("Cannot stat game path '{}'", game_path.display()))?;

    if new_meta.dev() != game_meta.dev() {
        bail!(
            "The selected cache directory is on a different filesystem than the game folder.\n\
             Hardlinks require both to share the same filesystem.\n\
             \n\
             Note: on BTRFS, hardlinks cannot cross subvolume boundaries even within the\n\
             same physical device. On ZFS, hardlinks cannot cross dataset boundaries even\n\
             within the same pool.\n\
             \n\
             Choose a path that is on the same filesystem as:\n  {}",
            game_path.display()
        );
    }

    Ok(())
}

/// Move `src` directory to `dst`. Tries `rename` first (fast); falls back to recursive
/// copy + delete when `rename` fails with `EXDEV` (cross-device).
fn move_dir(src: &Path, dst: &Path) -> Result<()> {
    // EXDEV = 18 on Linux: cross-device rename, fall back to copy+delete
    match std::fs::rename(src, dst) {
        Ok(()) => return Ok(()),
        Err(e) if e.raw_os_error() == Some(18) => {}
        Err(e) => return Err(e).context("rename failed"),
    }

    copy_dir_recursive(src, dst).context("cross-device copy failed")?;
    std::fs::remove_dir_all(src).context("failed to remove source after copy")?;
    Ok(())
}

fn move_cache_directories(
    mods: &[crate::models::mod_entry::ModEntry],
    old_cache_root: &Path,
    new_cache_root: &Path,
) -> Result<Vec<(std::path::PathBuf, std::path::PathBuf)>> {
    let mut moved = Vec::new();
    for entry in mods {
        let source = paths::mod_cache_dir_in(old_cache_root, &entry.id);
        let destination = paths::mod_cache_dir_in(new_cache_root, &entry.id);
        if !source.exists() {
            continue;
        }
        if let Err(error) = move_dir(&source, &destination) {
            let rollback_errors = rollback_cache_moves(&moved);
            return Err(cache_move_failure(
                anyhow::anyhow!("Cache move failed for '{}': {error:#}", entry.name),
                rollback_errors,
            ));
        }
        moved.push((source, destination));
    }
    Ok(moved)
}

fn rollback_cache_moves(moved: &[(std::path::PathBuf, std::path::PathBuf)]) -> Vec<String> {
    moved
        .iter()
        .rev()
        .filter_map(|(source, destination)| {
            move_dir(destination, source).err().map(|error| {
                format!(
                    "failed to restore '{}' from '{}': {error:#}",
                    source.display(),
                    destination.display()
                )
            })
        })
        .collect()
}

fn cache_move_failure(error: anyhow::Error, rollback_errors: Vec<String>) -> anyhow::Error {
    if rollback_errors.is_empty() {
        return error;
    }
    anyhow::anyhow!(
        "{error:#}. Cache rollback also failed: {}",
        rollback_errors.join("; ")
    )
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::Result;
    use tempfile::tempdir;

    use crate::models::mod_entry::{InstallTarget, ModEntry};

    use super::move_cache_directories;

    fn mod_entry(id: &str, name: &str, priority: i32) -> ModEntry {
        ModEntry {
            id: id.to_string(),
            game_id: "game".to_string(),
            name: name.to_string(),
            archive_hash: None,
            archive_path: None,
            installed_at: None,
            enabled: true,
            priority,
            nexus_mod_id: None,
            nexus_file_id: None,
            nexus_domain: None,
            version: None,
            author: None,
            nexus_description: None,
            latest_version: None,
            nexus_file_name: None,
            nexus_is_primary: false,
            archive_md5: None,
            install_target: InstallTarget::Data,
            notes: None,
        }
    }

    #[test]
    fn rolls_back_partial_cache_move() -> Result<()> {
        let temp = tempdir()?;
        let old_root = temp.path().join("old");
        let new_root = temp.path().join("new");
        std::fs::create_dir_all(old_root.join("first"))?;
        std::fs::create_dir_all(old_root.join("second"))?;
        std::fs::write(old_root.join("first/file"), b"first")?;
        std::fs::write(old_root.join("second/file"), b"second")?;
        std::fs::create_dir_all(new_root.join("second"))?;
        std::fs::write(new_root.join("second/existing"), b"occupied")?;
        let mods = [
            mod_entry("first", "First", 0),
            mod_entry("second", "Second", 1),
        ];

        let error = move_cache_directories(&mods, &old_root, &new_root)
            .expect_err("the occupied second destination must fail the cache move");

        assert!(error.to_string().contains("Second"));
        assert_eq!(std::fs::read(old_root.join("first/file"))?, b"first");
        assert_eq!(std::fs::read(old_root.join("second/file"))?, b"second");
        assert!(!new_root.join("first").exists());
        Ok(())
    }
}
