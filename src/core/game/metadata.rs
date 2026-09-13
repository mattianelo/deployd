use std::path::PathBuf;

use super::known_games::KNOWN_GAMES;
use super::wine::find_wine_user_dir;
use crate::models::game::Game;

/// Detect the game's save directory inside the Wine prefix.
///
/// Returns `None` when the game has no configured save path or the Wine prefix
/// cannot be located.
pub fn detect_save_dir(game: &Game) -> Option<PathBuf> {
    let known = KNOWN_GAMES.iter().find(|k| k.deployd_id == game.id)?;
    let subpath = known.save_game_subpath?;
    let user_dir = find_wine_user_dir(game)?;
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

pub fn game_ids_for_nexus_domain(domain: &str) -> Vec<&'static str> {
    KNOWN_GAMES
        .iter()
        .filter(|game| game.nexus_domain == domain)
        .map(|game| game.deployd_id)
        .collect()
}

/// Return the canonical `data_subdir` for a known game ID.
///
/// Used to recover the correct value when the persisted DB record pre-dates a
/// `KNOWN_GAMES` update (e.g. Witcher 1 going from `"."` to `"Data"`).
/// Returns `None` for custom games that have no `KNOWN_GAMES` entry.
pub fn known_data_subdir(id: &str) -> Option<&'static str> {
    KNOWN_GAMES
        .iter()
        .find(|k| k.deployd_id == id)
        .map(|k| k.data_subdir)
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

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn resolves_the_shared_nexus_domain_without_picking_one_game() {
        assert_eq!(
            game_ids_for_nexus_domain("masseffectlegendaryedition"),
            vec!["mass-effect-le1", "mass-effect-le2", "mass-effect-le3"]
        );
        assert_eq!(
            all_nexus_domains()
                .iter()
                .filter(|domain| **domain == "masseffectlegendaryedition")
                .count(),
            1
        );
        assert_eq!(
            game_ids_for_nexus_domain("skyrimspecialedition"),
            vec!["skyrim-se"]
        );
        assert!(game_ids_for_nexus_domain("unknown").is_empty());
    }
}
