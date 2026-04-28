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
        if !profiles.is_empty() {
            let last_key = format!("last_profile_{game_id}");
            let preferred_id = self.get_setting(&last_key).await.ok().flatten();
            let target = preferred_id
                .as_deref()
                .and_then(|id| profiles.iter().find(|p| p.id == id))
                .or_else(|| profiles.first())
                .unwrap() // safe: profiles is non-empty
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

    /// Build a portable export snapshot for the given profile.
    pub async fn export_profile(
        &self,
        profile_id: &str,
    ) -> Result<crate::models::profile_export::ProfileExport> {
        use crate::models::profile_export::{ProfileExport, ProfileModExport, ProfilePluginExport};

        let profile_row = sqlx::query_as::<_, (String, String, String)>(
            "SELECT id, game_id, name FROM profiles WHERE id = ?",
        )
        .bind(profile_id)
        .fetch_one(&self.pool)
        .await
        .context("Profile not found")?;

        let game_id = profile_row.1;
        let profile_name = profile_row.2;

        let mod_rows: Vec<(String, bool, i32)> = sqlx::query_as(
            "SELECT m.name, pm.enabled, pm.priority
             FROM profile_mods pm
             JOIN mods m ON m.id = pm.mod_id
             WHERE pm.profile_id = ?
             ORDER BY pm.priority ASC",
        )
        .bind(profile_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to load profile mods for export")?;

        let plugin_rows: Vec<(String, bool, i32)> = sqlx::query_as(
            "SELECT p.filename, pp.enabled, pp.load_order
             FROM profile_plugins pp
             JOIN plugins p ON p.id = pp.plugin_id
             WHERE pp.profile_id = ?
             ORDER BY pp.load_order ASC",
        )
        .bind(profile_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to load profile plugins for export")?;

        Ok(ProfileExport {
            version: 1,
            game_id,
            profile_name,
            mods: mod_rows
                .into_iter()
                .map(|(name, enabled, priority)| ProfileModExport {
                    name,
                    enabled,
                    priority,
                })
                .collect(),
            plugins: plugin_rows
                .into_iter()
                .map(|(filename, enabled, load_order)| ProfilePluginExport {
                    filename,
                    enabled,
                    load_order,
                })
                .collect(),
        })
    }

    /// Import a `ProfileExport`, creating a new profile.
    ///
    /// Matches exported names against live mods/plugins for `game_id`.
    /// Entries that don't match any installed mod/plugin are silently skipped.
    /// If a profile with the exported name already exists, a numeric suffix is appended.
    /// Returns `(profile_id, skipped_mod_count)` — the count of exported mods
    /// that had no matching installed mod and were therefore not imported.
    pub async fn import_profile(
        &self,
        game_id: &str,
        export: &crate::models::profile_export::ProfileExport,
    ) -> Result<(String, usize)> {
        let mut final_name = export.profile_name.clone();
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
            final_name = format!("{} ({})", export.profile_name, counter);
            counter += 1;
        }

        let profile_id = self
            .create_profile(game_id, &final_name)
            .await
            .context("Failed to create profile for import")?;

        let mut tx = self.pool.begin().await?;
        let mut skipped_mods: usize = 0;

        for mod_export in &export.mods {
            let row: Option<(String,)> =
                sqlx::query_as("SELECT id FROM mods WHERE name = ? AND game_id = ? LIMIT 1")
                    .bind(&mod_export.name)
                    .bind(game_id)
                    .fetch_optional(&mut *tx)
                    .await?;

            match row {
                Some((mod_id,)) => {
                    sqlx::query(
                        "INSERT OR IGNORE INTO profile_mods (profile_id, mod_id, enabled, priority)
                         VALUES (?, ?, ?, ?)",
                    )
                    .bind(&profile_id)
                    .bind(&mod_id)
                    .bind(mod_export.enabled)
                    .bind(mod_export.priority)
                    .execute(&mut *tx)
                    .await?;
                }
                None => skipped_mods += 1,
            }
        }

        for plugin_export in &export.plugins {
            let row: Option<(String,)> = sqlx::query_as(
                "SELECT p.id FROM plugins p
                 JOIN mods m ON p.mod_id = m.id
                 WHERE LOWER(p.filename) = LOWER(?) AND m.game_id = ?
                 LIMIT 1",
            )
            .bind(&plugin_export.filename)
            .bind(game_id)
            .fetch_optional(&mut *tx)
            .await?;

            if let Some((plugin_id,)) = row {
                sqlx::query(
                    "INSERT OR IGNORE INTO profile_plugins (profile_id, plugin_id, enabled, load_order)
                     VALUES (?, ?, ?, ?)",
                )
                .bind(&profile_id)
                .bind(&plugin_id)
                .bind(plugin_export.enabled)
                .bind(plugin_export.load_order)
                .execute(&mut *tx)
                .await?;
            }
        }

        tx.commit()
            .await
            .context("Failed to commit imported profile")?;

        Ok((profile_id, skipped_mods))
    }
}
