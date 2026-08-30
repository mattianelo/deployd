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
