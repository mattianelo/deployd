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

    let mut move_errors: Vec<String> = Vec::new();
    for m in &mods {
        let src = paths::mod_cache_dir_in(old_cache_root, &m.id);
        let dst = paths::mod_cache_dir_in(new_cache_root, &m.id);
        if !src.exists() {
            continue;
        }
        if let Err(e) = move_dir(&src, &dst) {
            move_errors.push(format!("  {}: {e}", m.name));
        }
    }

    if !move_errors.is_empty() {
        bail!(
            "Cache move failed for {} mod(s):\n{}",
            move_errors.len(),
            move_errors.join("\n")
        );
    }

    let old_prefix = old_cache_root.to_string_lossy();
    let new_prefix = new_cache_root.to_string_lossy();
    tracker
        .update_game_cache_paths(game_id, &old_prefix, &new_prefix)
        .await
        .context("Failed to update cache paths in database")?;

    tracker
        .set_game_cache_dir(game_id, new_cache_root)
        .await
        .context("Failed to save new cache dir setting")?;

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

    let mut move_errors: Vec<String> = Vec::new();
    for m in &mods {
        let src = paths::mod_cache_dir_in(current_cache_root, &m.id);
        let dst = paths::mod_cache_dir_in(default_cache_root, &m.id);
        if !src.exists() {
            continue;
        }
        if let Err(e) = move_dir(&src, &dst) {
            move_errors.push(format!("  {}: {e}", m.name));
        }
    }

    if !move_errors.is_empty() {
        bail!(
            "Cache reset failed for {} mod(s):\n{}",
            move_errors.len(),
            move_errors.join("\n")
        );
    }

    let old_prefix = current_cache_root.to_string_lossy();
    let new_prefix = default_cache_root.to_string_lossy();
    tracker
        .update_game_cache_paths(game_id, &old_prefix, &new_prefix)
        .await
        .context("Failed to update cache paths in database")?;

    tracker
        .clear_game_cache_dir(game_id)
        .await
        .context("Failed to clear custom cache dir setting")?;

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
