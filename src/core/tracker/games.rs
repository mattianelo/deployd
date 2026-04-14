use anyhow::Result;

use super::{PersistedGame, Tracker};

impl Tracker {
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

    /// Return the IDs of all games the user has explicitly hidden.
    pub async fn load_hidden_game_ids(&self) -> Result<Vec<String>> {
        let rows: Vec<(String,)> = sqlx::query_as("SELECT id FROM games WHERE hidden = 1")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(|(id,)| id).collect())
    }
}
