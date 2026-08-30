use anyhow::{Context, Result};

use super::{PersistedGame, Tracker};

impl Tracker {
    pub async fn persist_game_configs(
        &self,
        configs: &[crate::models::game::GameConfig],
        hidden_ids: &[String],
    ) -> Result<()> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .context("Failed to begin game settings update")?;
        for config in configs {
            let engine = match config.game.engine {
                crate::models::game::GameEngine::REDEngine => "redengine",
                crate::models::game::GameEngine::Eclipse => "eclipse",
                crate::models::game::GameEngine::Aurora => "aurora",
                crate::models::game::GameEngine::Bethesda => "bethesda",
            };
            sqlx::query(
                "INSERT INTO games
                 (id, title, path, data_subdir, engine, wine_prefix, custom, hidden)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0)
                 ON CONFLICT(id) DO UPDATE SET
                   title = excluded.title, path = excluded.path,
                   data_subdir = excluded.data_subdir, engine = excluded.engine,
                   wine_prefix = excluded.wine_prefix, custom = excluded.custom, hidden = 0",
            )
            .bind(&config.game.id)
            .bind(&config.game.title)
            .bind(config.game.path.to_string_lossy().as_ref())
            .bind(&config.game.data_subdir)
            .bind(engine)
            .bind(
                config
                    .game
                    .wine_prefix
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
            )
            .bind(config.custom)
            .execute(&mut *transaction)
            .await
            .with_context(|| format!("Failed to save game '{}'", config.game.title))?;
        }
        for game_id in hidden_ids {
            sqlx::query(
                "INSERT INTO games (id, hidden) VALUES (?, 1)
                 ON CONFLICT(id) DO UPDATE SET hidden = 1",
            )
            .bind(game_id)
            .execute(&mut *transaction)
            .await
            .with_context(|| format!("Failed to hide game '{game_id}'"))?;
        }
        if let Some(first) = configs.first() {
            sqlx::query(
                "INSERT INTO settings (key, value) VALUES ('last_game_id', ?)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            )
            .bind(&first.game.id)
            .execute(&mut *transaction)
            .await
            .context("Failed to save the selected game")?;
        }
        transaction
            .commit()
            .await
            .context("Failed to commit game settings")
    }

    /// Persist full game configuration (path, wine prefix, engine, custom flag).
    #[allow(clippy::too_many_arguments)] // All fields map directly to DB columns; a struct would require its own validation layer.
    pub async fn upsert_game(
        &self,
        id: &str,
        title: &str,
        path: &std::path::Path,
        data_subdir: &str,
        engine: &str,
        wine_prefix: Option<&std::path::Path>,
        custom: bool,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO games (id, title, path, data_subdir, engine, wine_prefix, custom, hidden)
             VALUES (?, ?, ?, ?, ?, ?, ?, 0)
             ON CONFLICT(id) DO UPDATE SET
               title        = excluded.title,
               path         = excluded.path,
               data_subdir  = excluded.data_subdir,
               engine       = excluded.engine,
               wine_prefix  = excluded.wine_prefix,
               custom       = excluded.custom,
               hidden       = 0",
        )
        .bind(id)
        .bind(title)
        .bind(path.to_string_lossy().as_ref())
        .bind(data_subdir)
        .bind(engine)
        .bind(wine_prefix.map(|p| p.to_string_lossy().into_owned()))
        .bind(custom as i32)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Persist only the game folder path (used by the game folder confirmation dialog).
    pub async fn upsert_game_path(&self, game_id: &str, path: &std::path::Path) -> Result<()> {
        sqlx::query(
            "INSERT INTO games (id, path) VALUES (?, ?)
             ON CONFLICT(id) DO UPDATE SET path = excluded.path",
        )
        .bind(game_id)
        .bind(path.to_string_lossy().as_ref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Load all persisted game configurations from the games table.
    pub async fn load_persisted_games(&self) -> Result<Vec<PersistedGame>> {
        #[allow(clippy::type_complexity)]
        // Flat SQLx row tuple — a struct would need manual FromRow impl.
        let rows: Vec<(
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i32>,
        )> = sqlx::query_as(
            "SELECT id, title, path, data_subdir, engine, wine_prefix, custom
                 FROM games WHERE path IS NOT NULL AND (hidden IS NULL OR hidden = 0)",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, title, path, data_subdir, engine, wine_prefix, custom)| PersistedGame {
                    id,
                    title: title.unwrap_or_default(),
                    path: std::path::PathBuf::from(path.unwrap_or_default()),
                    data_subdir: data_subdir.unwrap_or_else(|| "Data".to_string()),
                    engine: engine.unwrap_or_else(|| "bethesda".to_string()),
                    wine_prefix: wine_prefix.map(std::path::PathBuf::from),
                    custom: custom.unwrap_or(0) != 0,
                },
            )
            .collect())
    }

    /// Mark a game as hidden so it is excluded from the managed list and not re-added on rescan.
    pub async fn hide_game(&self, game_id: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO games (id, hidden) VALUES (?, 1)
             ON CONFLICT(id) DO UPDATE SET hidden = 1",
        )
        .bind(game_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn remove_managed_game(
        &self,
        game_id: &str,
        delete_mods: bool,
    ) -> Result<Vec<String>> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .context("Failed to begin game removal")?;
        let mod_ids: Vec<String> = if delete_mods {
            sqlx::query_scalar("SELECT id FROM mods WHERE game_id = ?")
                .bind(game_id)
                .fetch_all(&mut *transaction)
                .await
                .context("Failed to list mods for game removal")?
        } else {
            Vec::new()
        };
        for mod_id in &mod_ids {
            sqlx::query("DELETE FROM plugins WHERE mod_id = ?")
                .bind(mod_id)
                .execute(&mut *transaction)
                .await
                .with_context(|| format!("Failed to delete plugins for mod '{mod_id}'"))?;
            sqlx::query("DELETE FROM mod_files WHERE mod_id = ?")
                .bind(mod_id)
                .execute(&mut *transaction)
                .await
                .with_context(|| format!("Failed to delete files for mod '{mod_id}'"))?;
            sqlx::query("DELETE FROM mods WHERE id = ?")
                .bind(mod_id)
                .execute(&mut *transaction)
                .await
                .with_context(|| format!("Failed to delete mod '{mod_id}'"))?;
        }
        sqlx::query(
            "INSERT INTO games (id, hidden) VALUES (?, 1)
             ON CONFLICT(id) DO UPDATE SET hidden = 1",
        )
        .bind(game_id)
        .execute(&mut *transaction)
        .await
        .context("Failed to hide removed game")?;
        transaction
            .commit()
            .await
            .context("Failed to commit game removal")?;
        Ok(mod_ids)
    }

    /// Return the IDs of all games the user has explicitly hidden.
    pub async fn load_hidden_game_ids(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM games WHERE hidden = 1")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }
}
