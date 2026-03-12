use std::path::PathBuf;

use crate::models::game::Game;
use super::known_games::KNOWN_GAMES;
use super::wine::find_wine_user_dir;

/// Detect the game's save directory inside the Wine prefix.
///
/// Returns `None` for Bethesda games (saves are not managed by Deployd) or when
/// the Wine prefix cannot be located.
pub fn detect_save_dir(game: &Game) -> Option<PathBuf> {
    let known = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id)?;
    let subpath = known.save_game_subpath?;
    let user_dir = find_wine_user_dir(known, game)?;
    Some(user_dir.join(subpath))
}

/// Returns `true` if this game has a save directory configured in `KNOWN_GAMES`.
/// Unlike [`detect_save_dir`], this performs no filesystem I/O and is safe to call
/// in UI helpers.
pub fn has_save_management(game: &Game) -> bool {
    KNOWN_GAMES
        .iter()
        .find(|k| k.deployd_id == game.id)
        .is_some_and(|k| k.save_game_subpath.is_some())
}

/// Look up the Nexus Mods domain name for a game (e.g. "skyrimspecialedition").
pub fn nexus_domain(game: &Game) -> Option<&'static str> {
    KNOWN_GAMES
        .iter()
        .find(|k| k.deployd_id == game.id)
        .map(|k| k.nexus_domain)
}

/// Find the deployd game ID that matches a Nexus domain name.
/// Returns the first match (GOG before Steam for backward compatibility).
pub fn game_id_for_nexus_domain(domain: &str) -> Option<&'static str> {
    KNOWN_GAMES
        .iter()
        .find(|k| k.nexus_domain == domain)
        .map(|k| k.deployd_id)
}

/// Return all known Nexus domain names (for scanning per-game download subfolders).
pub fn all_nexus_domains() -> Vec<&'static str> {
    let mut seen = std::collections::HashSet::new();
    KNOWN_GAMES
        .iter()
        .filter_map(|k| {
            if seen.insert(k.nexus_domain) {
                Some(k.nexus_domain)
            } else {
                None
            }
        })
        .collect()
}
