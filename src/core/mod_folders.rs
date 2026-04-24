use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::core::tracker::Tracker;
use crate::utils::paths::{mod_cache_dir_in, named_mods_dir_in};

/// Rebuild the `named_mods/` symlink directory for the given game.
///
/// Each enabled mod gets a subdirectory named `<priority_padded>-<sanitized_name>`
/// that symlinks to its per-mod cache directory. Tools (e.g. NPC Plugin Chooser 2)
/// can be configured to read from this directory via the `M:\` Wine drive.
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

    let mods = tracker.list_mods(game_id).await?;

    for m in mods.iter().filter(|m| m.enabled) {
        let folder_name = format!("{:05}-{}", m.priority, sanitize_name(&m.name));
        let link_path = named_dir.join(&folder_name);
        let target = mod_cache_dir_in(cache_root, &m.id);

        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link_path)?;
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
