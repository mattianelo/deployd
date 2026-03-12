use anyhow::{Context, Result};

use crate::models::mod_entry::InstallTarget;

use super::Tracker;

impl Tracker {
    /// Get a setting value by key.
    pub async fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let row = sqlx::query_as::<_, (String,)>("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .context("Failed to query setting")?;
        Ok(row.map(|(v,)| v))
    }

    /// Set a setting value (upsert).
    pub async fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .context("Failed to save setting")?;
        Ok(())
    }

    /// Persist rate limit info as JSON under key "nexus_rate_limits".
    pub async fn save_rate_limits(
        &self,
        info: &crate::core::nexus_api::RateLimitInfo,
    ) -> Result<()> {
        let json = serde_json::to_string(info).context("Failed to serialize rate limits")?;
        self.set_setting("nexus_rate_limits", &json).await
    }

    /// Load rate limit info from the settings table. Returns None if not stored.
    pub async fn load_rate_limits(&self) -> Result<Option<crate::core::nexus_api::RateLimitInfo>> {
        match self.get_setting("nexus_rate_limits").await? {
            Some(json) => {
                let info =
                    serde_json::from_str(&json).context("Failed to parse stored rate limits")?;
                Ok(Some(info))
            }
            None => Ok(None),
        }
    }

    /// Update Nexus metadata for a mod after fetching from the API.
    pub async fn update_mod_nexus_metadata(
        &self,
        mod_id: &str,
        latest_version: &str,
        author: &str,
        description: &str,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE mods SET latest_version = ?, author = ?, nexus_description = ? WHERE id = ?",
        )
        .bind(latest_version)
        .bind(author)
        .bind(description)
        .bind(mod_id)
        .execute(&self.pool)
        .await
        .context("Failed to update Nexus metadata")?;
        Ok(())
    }

    /// Set the installed version for a mod (from the specific Nexus file entry).
    pub async fn set_mod_installed_version(&self, mod_id: &str, version: &str) -> Result<()> {
        sqlx::query("UPDATE mods SET version = ? WHERE id = ?")
            .bind(version)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to set installed version")?;
        Ok(())
    }

    /// Set the latest known version for a mod (from Nexus update check).
    pub async fn set_latest_version(&self, mod_id: &str, latest_version: &str) -> Result<()> {
        sqlx::query("UPDATE mods SET latest_version = ? WHERE id = ?")
            .bind(latest_version)
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to set latest version")?;
        Ok(())
    }

    /// Insert a new tool configuration.
    pub async fn insert_tool(&self, tool: &crate::models::tool::Tool) -> Result<()> {
        sqlx::query(
            "INSERT INTO tools (id, game_id, name, exe_path, icon_name, custom_args, sort_order, working_dir)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&tool.id)
        .bind(&tool.game_id)
        .bind(&tool.name)
        .bind(&tool.exe_path)
        .bind(&tool.icon_name)
        .bind(&tool.custom_args)
        .bind(tool.sort_order)
        .bind(&tool.working_dir)
        .execute(&self.pool)
        .await
        .context("Failed to insert tool")?;
        Ok(())
    }

    /// List all tools for a game, ordered by sort_order ascending.
    pub async fn list_tools(&self, game_id: &str) -> Result<Vec<crate::models::tool::Tool>> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String, i32, String)>(
            "SELECT id, game_id, name, exe_path, icon_name, custom_args, sort_order, working_dir
             FROM tools WHERE game_id = ? ORDER BY sort_order ASC",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list tools")?;

        Ok(rows
            .into_iter()
            .map(
                |(id, game_id, name, exe_path, icon_name, custom_args, sort_order, working_dir)| {
                    crate::models::tool::Tool {
                        id,
                        game_id,
                        name,
                        exe_path,
                        icon_name,
                        custom_args,
                        sort_order,
                        working_dir,
                    }
                },
            )
            .collect())
    }

    /// Update the working directory for an existing tool.
    pub async fn update_tool_working_dir(&self, tool_id: &str, working_dir: &str) -> Result<()> {
        sqlx::query("UPDATE tools SET working_dir = ? WHERE id = ?")
            .bind(working_dir)
            .bind(tool_id)
            .execute(&self.pool)
            .await
            .context("Failed to update tool working dir")?;
        Ok(())
    }

    /// Delete a tool by its ID.
    pub async fn delete_tool(&self, tool_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM tools WHERE id = ?")
            .bind(tool_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete tool")?;
        Ok(())
    }

    /// Update only the `mods.install_target` column — no path rewriting.
    pub async fn set_mod_install_target_column(
        &self,
        mod_id: &str,
        target: &InstallTarget,
    ) -> Result<()> {
        sqlx::query("UPDATE mods SET install_target = ? WHERE id = ?")
            .bind(target.to_string())
            .bind(mod_id)
            .execute(&self.pool)
            .await
            .context("Failed to update install_target column")?;
        Ok(())
    }
}
