use std::path::PathBuf;

use anyhow::Result;

use crate::core::game;
use crate::models::game::Game;
use crate::models::profile::SaveMode;
use crate::utils::paths;

use super::super::App;

impl App {
    pub(crate) fn selected_game(&self) -> Option<&Game> {
        self.session.games.get(self.session.selected_game_idx)
    }

    pub(crate) fn game_shows_plugins(&self) -> bool {
        self.selected_game()
            .is_some_and(|game| panel_features(&game.engine).0)
    }

    pub(crate) fn game_shows_overrides(&self) -> bool {
        self.selected_game()
            .is_some_and(|game| panel_features(&game.engine).1)
    }

    pub(crate) fn game_shows_conflicts(&self) -> bool {
        self.selected_game()
            .is_some_and(|game| panel_features(&game.engine).2)
    }

    /// Resolve the effective cache root for a game.
    pub(crate) fn cache_root_for(&self, game_id: &str) -> Result<PathBuf> {
        let custom = self
            .session
            .game_cache_dirs
            .get(game_id)
            .map(PathBuf::as_path);
        paths::game_cache_root(custom)
    }
    /// True when the selected game supports per-profile save management.
    pub(crate) fn game_has_save_management(&self) -> bool {
        self.selected_game().is_some_and(game::has_save_management)
    }

    /// True when the active profile uses per-profile saves and a manual sync makes sense.
    pub(crate) fn can_sync_saves(&self) -> bool {
        self.game_has_save_management()
            && self.shell.deployment_status.as_ref().is_some_and(|status| {
                !status.recovery_pending
                    && status
                        .live_saves
                        .as_ref()
                        .and_then(crate::core::save_manager::SaveSetId::profile_id)
                        == self
                            .session
                            .profiles
                            .get(self.session.active_profile_idx)
                            .map(|profile| profile.id.as_str())
            })
            && self
                .session
                .profiles
                .get(self.session.active_profile_idx)
                .is_some_and(|p| p.save_mode == SaveMode::ProfileSpecific)
    }

    /// Label for the save mode toggle button based on the active profile.
    /// For ProfileSpecific profiles, appends the age of the last save snapshot.
    pub(crate) fn save_mode_label(&self) -> String {
        let Some(profile) = self.session.profiles.get(self.session.active_profile_idx) else {
            return "Saves: Global".to_string();
        };
        match &profile.save_mode {
            SaveMode::Global => "Saves: Global".to_string(),
            SaveMode::ProfileSpecific => {
                let age = match profile.save_synced_at {
                    None => "never synced".to_string(),
                    Some(t) => {
                        let secs = t.elapsed().unwrap_or_default().as_secs();
                        if secs < 60 {
                            "just now".to_string()
                        } else if secs < 3600 {
                            format!("{}m ago", secs / 60)
                        } else if secs < 86400 {
                            format!("{}h ago", secs / 3600)
                        } else {
                            format!("{}d ago", secs / 86400)
                        }
                    }
                };
                format!("Saves: Profile · {age}")
            }
        }
    }
}

fn panel_features(engine: &crate::models::game::GameEngine) -> (bool, bool, bool) {
    use crate::models::game::GameEngine;
    match engine {
        GameEngine::Bethesda => (true, false, true),
        GameEngine::Eclipse => (false, true, true),
        GameEngine::Aurora | GameEngine::REDEngine => (false, false, true),
        GameEngine::MassEffect => (false, false, false),
    }
}

#[cfg(test)]
mod tests {
    use super::panel_features;
    use crate::models::game::GameEngine;

    // @variants: both
    #[test]
    fn game_panels_match_engine_capabilities() {
        assert_eq!(panel_features(&GameEngine::Bethesda), (true, false, true));
        assert_eq!(panel_features(&GameEngine::Eclipse), (false, true, true));
        for engine in [GameEngine::Aurora, GameEngine::REDEngine] {
            assert_eq!(panel_features(&engine), (false, false, true));
        }
        assert_eq!(
            panel_features(&GameEngine::MassEffect),
            (false, false, false)
        );
    }
}
