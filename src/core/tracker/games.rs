use anyhow::{Context, Result};

use super::{PersistedGame, Tracker};
use crate::utils::location::FolderRole;

impl Tracker {
    pub async fn persist_game_configs(
        &self,
        configs: &[crate::models::game::GameConfig],
        hidden_ids: &[String],
    ) -> Result<()> {
        self.persist_game_configs_with_baselines(configs, hidden_ids, &[])
            .await
    }

    pub(crate) async fn persist_game_configs_with_baselines(
        &self,
        configs: &[crate::models::game::GameConfig],
        hidden_ids: &[String],
        baselines: &[crate::core::game::mass_effect::baseline::Baseline],
    ) -> Result<()> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .context("Failed to begin game settings update")?;
        for config in configs {
            let engine = config.game.engine.as_str();
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
            for (role, path) in [
                (FolderRole::Game, Some(config.game.path.as_path())),
                (FolderRole::Prefix, config.game.wine_prefix.as_deref()),
            ] {
                super::locations::sync_binding(
                    &mut transaction,
                    &config.game.id,
                    role,
                    path,
                    config
                        .locations
                        .iter()
                        .find(|selection| selection.role == role),
                )
                .await?;
            }
        }
        for baseline in baselines {
            anyhow::ensure!(
                configs
                    .iter()
                    .any(|config| config.game.id == baseline.game_id
                        && config.game.engine == crate::models::game::GameEngine::MassEffect),
                "MELE baseline does not belong to a configured game"
            );
            super::mele_baselines::insert(&mut transaction, baseline).await?;
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
        let engine = engine.parse::<crate::models::game::GameEngine>()?;
        let mut transaction = self.pool.begin().await?;
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
        .bind(engine.as_str())
        .bind(wine_prefix.map(|p| p.to_string_lossy().into_owned()))
        .bind(custom as i32)
        .execute(&mut *transaction)
        .await?;
        super::locations::sync_binding(&mut transaction, id, FolderRole::Game, Some(path), None)
            .await?;
        super::locations::sync_binding(&mut transaction, id, FolderRole::Prefix, wine_prefix, None)
            .await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Persist only the game folder path (used by the game folder confirmation dialog).
    pub async fn upsert_game_path(&self, game_id: &str, path: &std::path::Path) -> Result<()> {
        let selected = crate::utils::location::SelectedLocation::capture(path.to_path_buf()).await;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO games (id, path) VALUES (?, ?)
             ON CONFLICT(id) DO UPDATE SET path = excluded.path",
        )
        .bind(game_id)
        .bind(path.to_string_lossy().as_ref())
        .execute(&mut *transaction)
        .await?;
        super::locations::sync_binding(
            &mut transaction,
            game_id,
            FolderRole::Game,
            Some(path),
            None,
        )
        .await?;
        let location = sqlx::query("UPDATE folder_locations SET host_hint=COALESCE(?,host_hint) WHERE root=? AND id IN (SELECT location_id FROM game_locations WHERE game_id=? AND role='game')")
            .bind(selected.host_hint.map(|path| path.to_string_lossy().into_owned())).bind(path.to_string_lossy().into_owned()).bind(game_id);
        location.execute(&mut *transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    /// Load all persisted game configurations from the games table.
    pub async fn load_persisted_games(&self) -> Result<Vec<PersistedGame>> {
        self.load_games(false).await
    }

    pub(crate) async fn load_games(&self, include_hidden: bool) -> Result<Vec<PersistedGame>> {
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
                 FROM games WHERE path IS NOT NULL AND (? OR hidden IS NULL OR hidden = 0)",
        )
        .bind(include_hidden)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(
                |(id, title, path, data_subdir, engine, wine_prefix, custom)| {
                    let engine = crate::models::game::GameEngine::from_persisted(engine.as_deref())
                        .with_context(|| format!("Failed to load engine for game '{id}'"))?;
                    Ok(PersistedGame {
                        id,
                        title: title.unwrap_or_default(),
                        path: std::path::PathBuf::from(path.unwrap_or_default()),
                        data_subdir: data_subdir.unwrap_or_else(|| "Data".to_string()),
                        engine,
                        wine_prefix: wine_prefix.map(std::path::PathBuf::from),
                        custom: custom.unwrap_or(0) != 0,
                    })
                },
            )
            .collect()
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::models::game::{Game, GameConfig, GameEngine};
    use crate::utils::location::{FolderSelection, SelectedLocation};

    fn family() -> Vec<GameConfig> {
        (1..=3)
            .map(|number| GameConfig {
                game: Game {
                    id: format!("mass-effect-le{number}"),
                    title: format!("LE{number}"),
                    path: format!("/selected/mele/Game/ME{number}").into(),
                    data_subdir: "BioGame".into(),
                    engine: GameEngine::MassEffect,
                    wine_prefix: Some("/selected/prefix".into()),
                },
                custom: true,
                locations: vec![
                    FolderSelection {
                        role: FolderRole::Game,
                        location: SelectedLocation {
                            root: "/selected/mele".into(),
                            host_hint: None,
                        },
                        relative: format!("Game/ME{number}").into(),
                    },
                    FolderSelection {
                        role: FolderRole::Prefix,
                        location: SelectedLocation {
                            root: "/selected/prefix".into(),
                            host_hint: None,
                        },
                        relative: PathBuf::new(),
                    },
                ],
            })
            .collect()
    }

    // @variants: both
    #[tokio::test]
    async fn persists_three_games_with_shared_root_and_separate_prefix() -> Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        tracker.persist_game_configs(&family(), &[]).await?;
        let games = tracker.load_persisted_games().await?;
        assert_eq!(games.len(), 3);
        assert!(
            games
                .iter()
                .all(|game| game.engine == GameEngine::MassEffect)
        );
        let roots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM folder_locations")
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(roots, 2);
        let game_roots: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT location_id) FROM game_locations WHERE role='game'",
        )
        .fetch_one(&tracker.pool)
        .await?;
        assert_eq!(game_roots, 1);
        let relatives: Vec<String> = sqlx::query_scalar(
            "SELECT relative_path FROM game_locations WHERE role='game' ORDER BY game_id",
        )
        .fetch_all(&tracker.pool)
        .await?;
        assert_eq!(relatives, ["Game/ME1", "Game/ME2", "Game/ME3"]);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn rolls_back_the_entire_family_when_one_game_cannot_be_saved() -> Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        sqlx::query("CREATE TRIGGER fail_third BEFORE INSERT ON games WHEN NEW.id='mass-effect-le3' BEGIN SELECT RAISE(FAIL, 'injected failure'); END")
            .execute(&tracker.pool).await?;
        assert!(tracker.persist_game_configs(&family(), &[]).await.is_err());
        assert!(tracker.load_persisted_games().await?.is_empty());
        let roots: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM folder_locations")
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(roots, 0);
        assert!(tracker.get_setting("last_game_id").await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn rejects_unknown_explicit_engines_when_loading_configured_games() -> Result<()> {
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        sqlx::query("INSERT INTO games (id, path, engine) VALUES ('legacy', '/game', NULL)")
            .execute(&tracker.pool)
            .await?;
        assert_eq!(
            tracker.load_persisted_games().await?[0].engine,
            GameEngine::Bethesda
        );
        sqlx::query("UPDATE games SET engine='future_engine' WHERE id='legacy'")
            .execute(&tracker.pool)
            .await?;
        let error = tracker.load_persisted_games().await.unwrap_err();
        assert!(format!("{error:#}").contains("future_engine"));
        Ok(())
    }
}
