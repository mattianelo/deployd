use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::core::game;
use crate::models::game::Game;

/// Lowercase all path components and normalize backslashes to forward slashes.
/// "Data/Textures/Foo.DDS" → "data/textures/foo.dds"
pub fn lowercase_path(rel: &Path) -> PathBuf {
    PathBuf::from(lowercase_path_str(rel))
}

/// Like [`lowercase_path`] but returns a [`String`] instead of a [`PathBuf`].
pub fn lowercase_path_str(rel: &Path) -> String {
    rel.to_string_lossy().to_lowercase().replace('\\', "/")
}

/// Return the data directory for Deployd.
///
/// Outside a snap this is `$XDG_DATA_HOME/deployd` (~/.local/share/deployd).
///
/// Inside a snap `$SNAP_USER_DATA` is revision-specific — it changes on every
/// `snap install`, wiping out runtimes, the database, and cached mods. We use
/// `$SNAP_USER_COMMON` instead, which snapd keeps stable across revisions.
pub fn deployd_data_dir() -> Result<PathBuf> {
    if let Some(common) = std::env::var_os("SNAP_USER_COMMON") {
        return Ok(deployd_data_dir_from_snap_common(&PathBuf::from(common)));
    }
    let data_dir = dirs::data_dir().ok_or_else(|| anyhow!("Cannot determine XDG_DATA_HOME"))?;
    Ok(data_dir.join("deployd"))
}

pub(crate) fn deployd_data_dir_from_snap_common(common: &Path) -> PathBuf {
    common.join("deployd")
}

/// Deployd cache root: <data>/deployd/cache
pub fn cache_root() -> Result<PathBuf> {
    Ok(deployd_data_dir()?.join("cache"))
}

/// Resolve the effective cache root for a game.
/// Returns the custom override when provided, otherwise falls back to the global cache_root().
pub fn game_cache_root(custom: Option<&Path>) -> Result<PathBuf> {
    match custom {
        Some(dir) => Ok(dir.to_path_buf()),
        None => cache_root(),
    }
}

/// Per-mod cache directory relative to an explicit cache root.
pub fn mod_cache_dir_in(root: &Path, mod_id: &str) -> PathBuf {
    root.join(mod_id)
}

/// Named-mods symlink directory relative to an explicit cache root.
pub fn named_mods_dir_in(root: &Path) -> PathBuf {
    root.join("named_mods")
}

/// Per-profile save storage root: <data>/deployd/saves
pub fn saves_root() -> Result<PathBuf> {
    Ok(deployd_data_dir()?.join("saves"))
}

/// Database path: <data>/deployd/deployd.db
pub fn db_path() -> Result<PathBuf> {
    Ok(deployd_data_dir()?.join("deployd.db"))
}

/// The target deployment directory for a game.
///
/// For Eclipse engine games this resolves to the Wine prefix user directory
/// rather than the game installation folder. See `game::deploy_dir`.
pub fn game_data_dir(game: &Game) -> PathBuf {
    game::deploy_dir(game)
}

/// Vanilla file backup storage: `<data>/deployd/<game_id>/vanilla-backup/`
///
/// Files here are copies of game files that deployd overwrote with mod files.
/// They are restored when the owning mod is removed or the modlist is purged.
pub fn vanilla_backup_dir(game_id: &str) -> Result<PathBuf> {
    Ok(deployd_data_dir()?.join(game_id).join("vanilla-backup"))
}

/// Pending DB restore path: written by the restore flow, consumed on next launch.
///
/// When this file exists at startup, `init.rs` renames it over `deployd.db`
/// before opening the tracker, completing a full-DB migration restore.
pub fn pending_restore_path() -> Result<PathBuf> {
    Ok(deployd_data_dir()?.join("deployd.db.restore-pending"))
}

/// Marker written after a pending restore is applied, consumed during init.
///
/// When this file exists, `load_init_data` sets `InitData::restored_from_backup`
/// and shows a banner prompting the user to reinstall their mods.
pub fn post_restore_marker_path() -> Result<PathBuf> {
    Ok(deployd_data_dir()?.join("deployd.db.restore-applied"))
}

/// Default downloads directory (~/Downloads or fallback to $HOME/Downloads).
pub fn default_downloads_dir() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Downloads")
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::deployd_data_dir_from_snap_common;

    // @variants: snap
    #[test]
    fn snap_data_dir_uses_user_common() {
        assert_eq!(
            deployd_data_dir_from_snap_common(Path::new("/home/alex/snap/deployd/common")),
            Path::new("/home/alex/snap/deployd/common/deployd")
        );
    }
}
