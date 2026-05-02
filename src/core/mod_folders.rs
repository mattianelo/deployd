use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::core::tracker::Tracker;
use crate::utils::paths::{mod_cache_dir_in, named_mods_dir_in};

/// Rebuild the `named_mods/` symlink directory for the given game.
///
/// Each enabled mod gets:
/// - A priority-prefixed symlink at `named_mods/<priority_padded>-<name>` (for ordered access).
/// - A stable symlink at `named_mods/by-name/<name>` (for tools needing a fixed path).
///
/// The `by-name/` subtree never changes its paths when mods are reordered, making it
/// suitable as a fixed output folder target for tools like PGPatcher.
pub async fn refresh_named_mod_folders(
    tracker: &Tracker,
    game_id: &str,
    cache_root: &Path,
) -> Result<()> {
    let named_dir = named_mods_dir_in(cache_root);

    if named_dir.exists() {
        fs::remove_dir_all(&named_dir)?;
    }
    fs::create_dir_all(&named_dir)?;

    let by_name_dir = named_dir.join("by-name");
    fs::create_dir_all(&by_name_dir)?;

    let mods = tracker.list_mods(game_id).await?;

    for m in mods.iter().filter(|m| m.enabled) {
        let sanitized = sanitize_name(&m.name);
        let target = mod_cache_dir_in(cache_root, &m.id);

        let priority_link = named_dir.join(format!("{:05}-{sanitized}", m.priority));
        let stable_link = by_name_dir.join(&sanitized);

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&target, &priority_link)?;
            std::os::unix::fs::symlink(&target, &stable_link)?;
        }
    }

    Ok(())
}

/// Replace filesystem-unsafe characters and trim to a reasonable length.
fn sanitize_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect();
    sanitized.chars().take(64).collect()
}
