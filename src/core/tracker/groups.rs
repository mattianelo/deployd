use anyhow::{Context, Result};

use super::Tracker;

impl Tracker {
    /// List all groups for a game, ordered by position.
    pub async fn list_groups(&self, game_id: &str) -> Result<Vec<crate::models::group::ModGroup>> {
        let rows: Vec<(String, String, f64, i32, Option<String>)> = sqlx::query_as(
            "SELECT id, name, position, collapsed, color
             FROM mod_groups
             WHERE game_id = ?
             ORDER BY position ASC",
        )
        .bind(game_id)
        .fetch_all(&self.pool)
        .await
        .context("Failed to list mod groups")?;

        Ok(rows
            .into_iter()
            .map(
                |(id, name, position, collapsed, color)| crate::models::group::ModGroup {
                    id,
                    name,
                    position,
                    collapsed: collapsed != 0,
                    color,
                },
            )
            .collect())
    }

    /// Create a new group and return it.
    pub async fn create_group(
        &self,
        game_id: &str,
        name: &str,
        position: f64,
    ) -> Result<crate::models::group::ModGroup> {
        let id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO mod_groups (id, game_id, name, position, collapsed)
             VALUES (?, ?, ?, ?, 0)",
        )
        .bind(&id)
        .bind(game_id)
        .bind(name)
        .bind(position)
        .execute(&self.pool)
        .await
        .context("Failed to create mod group")?;

        Ok(crate::models::group::ModGroup {
            id,
            name: name.to_string(),
            position,
            collapsed: false,
            color: None,
        })
    }

    /// Rename a group.
    pub async fn rename_group(&self, group_id: &str, name: &str) -> Result<()> {
        sqlx::query("UPDATE mod_groups SET name = ? WHERE id = ?")
            .bind(name)
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to rename mod group")?;
        Ok(())
    }

    /// Persist the collapsed state of a group.
    pub async fn set_group_collapsed(&self, group_id: &str, collapsed: bool) -> Result<()> {
        sqlx::query("UPDATE mod_groups SET collapsed = ? WHERE id = ?")
            .bind(collapsed as i32)
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to update group collapsed state")?;
        Ok(())
    }

    /// Delete a group. Mods that belonged to it become ungrouped (group_id = NULL).
    pub async fn delete_group(&self, group_id: &str) -> Result<()> {
        sqlx::query("UPDATE mods SET group_id = NULL WHERE group_id = ?")
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to ungroup mods before group deletion")?;
        sqlx::query("DELETE FROM mod_groups WHERE id = ?")
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to delete mod group")?;
        Ok(())
    }

    /// Move a group to a new position in the list.
    pub async fn move_group(&self, group_id: &str, new_position: f64) -> Result<()> {
        sqlx::query("UPDATE mod_groups SET position = ? WHERE id = ?")
            .bind(new_position)
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to move mod group")?;
        Ok(())
    }

    /// Set (or clear) the color label for a group.
    pub async fn set_group_color(&self, group_id: &str, color: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE mod_groups SET color = ? WHERE id = ?")
            .bind(color)
            .bind(group_id)
            .execute(&self.pool)
            .await
            .context("Failed to set group color")?;
        Ok(())
    }
}
