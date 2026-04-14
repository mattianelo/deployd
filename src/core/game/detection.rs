use crate::models::game::Game;

/// Game auto-detection is disabled — users select game and prefix directories manually.
pub fn detect_games() -> Vec<Game> {
    Vec::new()
}
