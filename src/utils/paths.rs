use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::core::game;
use crate::models::game::Game;

/// Lowercase all path components and normalize backslashes to forward slashes.
/// "Data/Textures/Foo.DDS" → "data/textures/foo.dds"
pub fn lowercase_path(rel: &Path) -> PathBuf {
    let s = rel.to_string_lossy().to_lowercase().replace('\\', "/");
    PathBuf::from(s)
}

/// Return the data directory for Deployd.
///
/// Outside a snap this is `$XDG_DATA_HOME/deployd` (~/.local/share/deployd).
///
/// Inside a snap `$SNAP_USER_DATA` is revision-specific — it changes on every
/// `snap install`, wiping out runtimes, the database, and cached mods. We use
/// `$SNAP_USER_COMMON` instead, which snapd keeps stable across revisions.
fn deployd_data_dir() -> Result<PathBuf> {
    if let Some(common) = std::env::var_os("SNAP_USER_COMMON") {
        return Ok(PathBuf::from(common).join("deployd"));
    }
    let data_dir = dirs::data_dir().ok_or_else(|| anyhow!("Cannot determine XDG_DATA_HOME"))?;
    Ok(data_dir.join("deployd"))
}

/// Deployd cache root: <data>/deployd/cache
pub fn cache_root() -> Result<PathBuf> {
    Ok(deployd_data_dir()?.join("cache"))
}

/// Per-profile save storage root: <data>/deployd/saves
pub fn saves_root() -> Result<PathBuf> {
    Ok(deployd_data_dir()?.join("saves"))
}

/// Per-mod cache directory: cache_root/<mod_id>/
pub fn mod_cache_dir(mod_id: &str) -> Result<PathBuf> {
    Ok(cache_root()?.join(mod_id))
}

/// Database path: <data>/deployd/deployd.db
pub fn db_path() -> Result<PathBuf> {
    Ok(deployd_data_dir()?.join("deployd.db"))
}

/// Named mod folders directory: cache_root/named_mods/
/// Contains human-readable symlinks for each enabled mod, used for tool compatibility.
pub fn named_mods_dir() -> Result<PathBuf> {
    Ok(cache_root()?.join("named_mods"))
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

/// Default downloads directory (~/Downloads or fallback to $HOME/Downloads).
pub fn default_downloads_dir() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Downloads).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Downloads")
    })
}
