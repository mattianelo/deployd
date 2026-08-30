use anyhow::{Context, Result};

use crate::models::profile::{Profile, SaveMode};

use super::Tracker;

impl Tracker {
    /// Create a new profile for a game. Returns the profile id.
    pub async fn create_profile(&self, game_id: &str, name: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO profiles (id, game_id, name, is_active) VALUES (?, ?, ?, 0)")
            .bind(&id)
            .bind(game_id)
            .bind(name)
            .execute(&self.pool)
            .await
            .context("Failed to create profile")?;
        Ok(id)
    }

    /// Atomically create a new profile and snapshot all current mods/plugins as disabled.
    pub async fn create_clean_profile(&self, game_id: &str, name: &str) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await?;

        sqlx::query("INSERT INTO profiles (id, game_id, name, is_active) VALUES (?, ?, ?, 0)")
            .bind(&id)
            .bind(game_id)
            .bind(name)
            .execute(&mut *tx)
            .await
            .context("Failed to create profile")?;

        sqlx::query(
            "INSERT INTO profile_mods (profile_id, mod_id, enabled, priority)
             SELECT ?, id, 0, priority FROM mods WHERE game_id = ?",
        )
        .bind(&id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
             SELECT ?, p.id, 0, p.load_order
             FROM plugins p JOIN mods m ON p.mod_id = m.id
             WHERE m.game_id = ?",
        )
        .bind(&id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        tx.commit()
            .await
            .context("Failed to create clean profile")?;
        Ok(id)
    }

    /// Clone a profile: create a new profile with the same mod/plugin snapshot as the source.
    /// If `new_name` is already taken, a numeric suffix is appended.
    pub async fn clone_profile(
        &self,
        source_profile_id: &str,
        new_name: &str,
        game_id: &str,
    ) -> Result<String> {
        let mut final_name = new_name.to_string();
        let mut counter = 2u32;
        loop {
            let taken: bool = sqlx::query_scalar(
                "SELECT COUNT(*) > 0 FROM profiles WHERE game_id = ? AND name = ?",
            )
            .bind(game_id)
            .bind(&final_name)
            .fetch_one(&self.pool)
            .await
            .unwrap_or(false);

            if !taken {
                break;
            }
            final_name = format!("{new_name} ({counter})");
            counter += 1;
        }

        let id = uuid::Uuid::new_v4().to_string();
        let mut tx = self.pool.begin().await?;

        sqlx::query("INSERT INTO profiles (id, game_id, name, is_active) VALUES (?, ?, ?, 0)")
            .bind(&id)
            .bind(game_id)
            .bind(&final_name)
            .execute(&mut *tx)
            .await
            .context("Failed to create cloned profile")?;

        sqlx::query(
            "INSERT INTO profile_mods (profile_id, mod_id, enabled, priority)
             SELECT ?, mod_id, enabled, priority FROM profile_mods WHERE profile_id = ?",
        )
        .bind(&id)
        .bind(source_profile_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
             SELECT ?, plugin_id, enabled, load_order FROM profile_plugins WHERE profile_id = ?",
        )
        .bind(&id)
        .bind(source_profile_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await.context("Failed to clone profile")?;
        Ok(id)
    }

    /// Rename an existing profile.
    pub async fn rename_profile(&self, profile_id: &str, new_name: &str) -> Result<()> {
        sqlx::query("UPDATE profiles SET name = ? WHERE id = ?")
            .bind(new_name)
            .bind(profile_id)
            .execute(&self.pool)
            .await
            .context("Failed to rename profile")?;
        Ok(())
    }

    /// List all profiles for a game.
    pub async fn list_profiles(&self, game_id: &str) -> Result<Vec<Profile>> {
        let rows = sqlx::query_as::<_, (String, String, bool, String)>(
            "SELECT id, name, is_active, save_mode FROM profiles WHERE game_id = ? ORDER BY name",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list profiles")?;

        Ok(rows
            .into_iter()
            .map(|(id, name, is_active, save_mode)| Profile {
                id,
                name,
                is_active,
                save_mode: SaveMode::from_db(&save_mode),
                save_synced_at: None,
            })
            .collect())
    }

    /// Get the active profile for a game (if any).
    pub async fn get_active_profile(&self, game_id: &str) -> Result<Option<Profile>> {
        let row = sqlx::query_as::<_, (String, String, bool, String)>(
            "SELECT id, name, is_active, save_mode FROM profiles
             WHERE game_id = ? AND is_active = TRUE LIMIT 1",
        )
        .bind(game_id)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to query active profile")?;

        Ok(row.map(|(id, name, is_active, save_mode)| Profile {
            id,
            name,
            is_active,
            save_mode: SaveMode::from_db(&save_mode),
            save_synced_at: None,
        }))
    }

    /// Resolve the profile used by the most recent successful deploy for this game.
    /// Missing and stale settings deliberately leave the current profile unchanged.
    async fn get_last_deployed_profile(&self, game_id: &str) -> Result<Option<Profile>> {
        let key = format!("last_deployed_profile_{game_id}");
        let Some(profile_id) = self.get_setting(&key).await? else {
            return Ok(None);
        };

        Ok(self
            .list_profiles(game_id)
            .await?
            .into_iter()
            .find(|profile| profile.id == profile_id))
    }

    pub(crate) async fn restore_last_deployed_profile(
        &self,
        game_id: &str,
    ) -> Result<Option<(Profile, Profile)>> {
        let Some(active_profile) = self.get_active_profile(game_id).await? else {
            return Ok(None);
        };
        let Some(deployed_profile) = self.get_last_deployed_profile(game_id).await? else {
            return Ok(None);
        };
        if active_profile.id == deployed_profile.id {
            return Ok(None);
        }

        self.save_to_profile(&active_profile.id, game_id).await?;
        self.switch_profile(game_id, &deployed_profile.id).await?;
        Ok(Some((active_profile, deployed_profile)))
    }

    pub(crate) async fn record_deployed_profile(
        &self,
        game_id: &str,
        profile_id: &str,
    ) -> Result<()> {
        let key = format!("last_deployed_profile_{game_id}");
        self.set_setting(&key, profile_id).await
    }

    /// Update the save mode for a profile.
    pub async fn set_profile_save_mode(&self, profile_id: &str, mode: SaveMode) -> Result<()> {
        sqlx::query("UPDATE profiles SET save_mode = ? WHERE id = ?")
            .bind(mode.to_db())
            .bind(profile_id)
            .execute(&self.pool)
            .await
            .context("Failed to update profile save mode")?;
        Ok(())
    }

    /// Save the current mods/plugins state into the given profile (snapshot).
    pub async fn save_to_profile(&self, profile_id: &str, game_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM profile_mods WHERE profile_id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM profile_plugins WHERE profile_id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "INSERT INTO profile_mods (profile_id, mod_id, enabled, priority)
             SELECT ?, id, enabled, priority FROM mods WHERE game_id = ?",
        )
        .bind(profile_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "INSERT INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
             SELECT ?, p.id, p.enabled, p.load_order
             FROM plugins p JOIN mods m ON p.mod_id = m.id
             WHERE m.game_id = ?",
        )
        .bind(profile_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        tx.commit()
            .await
            .context("Failed to save profile snapshot")?;
        Ok(())
    }

    /// Switch to a profile: load its state into the live mods/plugins tables.
    pub async fn switch_profile(&self, game_id: &str, profile_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("UPDATE profiles SET is_active = FALSE WHERE game_id = ?")
            .bind(game_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE profiles SET is_active = TRUE WHERE id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "UPDATE mods SET enabled = pm.enabled, priority = pm.priority
             FROM profile_mods pm
             WHERE mods.id = pm.mod_id AND pm.profile_id = ? AND mods.game_id = ?",
        )
        .bind(profile_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE mods SET enabled = 0
             WHERE game_id = ?
               AND id NOT IN (SELECT mod_id FROM profile_mods WHERE profile_id = ?)",
        )
        .bind(game_id)
        .bind(profile_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE plugins SET enabled = pp.enabled, load_order = pp.load_order
             FROM profile_plugins pp
             WHERE plugins.id = pp.plugin_id AND pp.profile_id = ?
               AND plugins.mod_id IN (SELECT id FROM mods WHERE game_id = ?)",
        )
        .bind(profile_id)
        .bind(game_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            "UPDATE plugins SET enabled = (
                SELECT COALESCE(pm.enabled, 0)
                FROM profile_mods pm
                WHERE pm.mod_id = plugins.mod_id AND pm.profile_id = ?
             )
             WHERE plugins.mod_id IN (SELECT id FROM mods WHERE game_id = ?)
               AND plugins.id NOT IN (SELECT plugin_id FROM profile_plugins WHERE profile_id = ?)",
        )
        .bind(profile_id)
        .bind(game_id)
        .bind(profile_id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await.context("Failed to switch profile")?;
        Ok(())
    }

    /// Delete a profile and its snapshot data (CASCADE handles profile_mods/profile_plugins).
    pub async fn delete_profile(&self, profile_id: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM profile_mods WHERE profile_id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM profile_plugins WHERE profile_id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM profiles WHERE id = ?")
            .bind(profile_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await.context("Failed to delete profile")?;
        Ok(())
    }

    /// Ensure a "Default" profile exists for a game, creating one if needed.
    /// Returns the active profile (or the newly created Default).
    pub async fn ensure_default_profile(&self, game_id: &str) -> Result<Profile> {
        if let Some(active) = self.get_active_profile(game_id).await? {
            return Ok(active);
        }

        let profiles = self.list_profiles(game_id).await?;
        if let Some(first_profile) = profiles.first() {
            let last_key = format!("last_profile_{game_id}");
            let preferred_id = self.get_setting(&last_key).await.ok().flatten();
            let target = preferred_id
                .as_deref()
                .and_then(|id| profiles.iter().find(|p| p.id == id))
                .unwrap_or(first_profile)
                .clone();
            sqlx::query("UPDATE profiles SET is_active = TRUE WHERE id = ?")
                .bind(&target.id)
                .execute(&self.pool)
                .await?;
            return Ok(Profile {
                is_active: true,
                ..target
            });
        }

        let id = self.create_profile(game_id, "Default").await?;
        sqlx::query("UPDATE profiles SET is_active = TRUE WHERE id = ?")
            .bind(&id)
            .execute(&self.pool)
            .await?;
        self.save_to_profile(&id, game_id).await?;
        Ok(Profile {
            id,
            name: "Default".to_string(),
            is_active: true,
            save_mode: SaveMode::Global,
            save_synced_at: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn profile_tracker() -> Result<Tracker> {
        Ok(Tracker::open("sqlite::memory:").await?.tracker)
    }

    // @variants: both
    #[tokio::test]
    async fn resolves_last_deployed_profiles_independently_per_game() -> Result<()> {
        let tracker = profile_tracker().await?;
        let first_a = tracker.create_profile("game-a", "Alpha").await?;
        let deployed_a = tracker.create_profile("game-a", "Zulu").await?;
        let first_b = tracker.create_profile("game-b", "Alpha").await?;
        let deployed_b = tracker.create_profile("game-b", "Zulu").await?;
        sqlx::query(
            "INSERT INTO mods (id, game_id, name, enabled, priority) VALUES (?, ?, ?, 0, 0)",
        )
        .bind("mod-a")
        .bind("game-a")
        .bind("Mod A")
        .execute(&tracker.pool)
        .await?;
        tracker.save_to_profile(&first_a, "game-a").await?;
        sqlx::query("UPDATE mods SET enabled = 1 WHERE id = ?")
            .bind("mod-a")
            .execute(&tracker.pool)
            .await?;
        tracker.save_to_profile(&deployed_a, "game-a").await?;
        tracker.switch_profile("game-a", &first_a).await?;
        tracker.switch_profile("game-b", &first_b).await?;
        tracker
            .record_deployed_profile("game-a", &deployed_a)
            .await?;
        tracker
            .record_deployed_profile("game-b", &deployed_b)
            .await?;

        let transition_a = tracker.restore_last_deployed_profile("game-a").await?;
        let transition_b = tracker.restore_last_deployed_profile("game-b").await?;
        let active_a = tracker.get_active_profile("game-a").await?;
        let active_b = tracker.get_active_profile("game-b").await?;
        let mod_a_enabled: bool = sqlx::query_scalar("SELECT enabled FROM mods WHERE id = ?")
            .bind("mod-a")
            .fetch_one(&tracker.pool)
            .await?;

        assert_eq!(
            transition_a.map(|(_, deployed)| deployed.id),
            Some(deployed_a.clone()),
        );
        assert_eq!(
            transition_b.map(|(_, deployed)| deployed.id),
            Some(deployed_b.clone()),
        );
        assert_eq!(active_a.map(|profile| profile.id), Some(deployed_a));
        assert_eq!(active_b.map(|profile| profile.id), Some(deployed_b));
        assert!(mod_a_enabled, "the deployed profile snapshot was restored");
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn missing_deploy_record_keeps_current_profile_available() -> Result<()> {
        let tracker = profile_tracker().await?;
        let current = tracker.create_profile("game", "Current").await?;
        tracker.switch_profile("game", &current).await?;

        let transition = tracker.restore_last_deployed_profile("game").await?;
        let active = tracker.get_active_profile("game").await?;

        assert!(
            transition.is_none(),
            "a never-deployed game has no preferred profile",
        );
        assert_eq!(active.map(|profile| profile.id), Some(current));
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn stale_deploy_record_keeps_current_profile_available() -> Result<()> {
        let tracker = profile_tracker().await?;
        let current = tracker.create_profile("game", "Current").await?;
        tracker.switch_profile("game", &current).await?;
        tracker
            .set_setting("last_deployed_profile_game", "deleted-profile")
            .await?;

        let transition = tracker.restore_last_deployed_profile("game").await?;
        let active = tracker.get_active_profile("game").await?;

        assert!(transition.is_none(), "a stale deploy record is ignored");
        assert_eq!(active.map(|profile| profile.id), Some(current));
        Ok(())
    }
}
