use anyhow::{Context, Result, ensure};
use sqlx::{Sqlite, Transaction};

use crate::core::tracker::Tracker;
use crate::utils::paths;

use super::catalog::{History, durable};
use super::state::{self, State};

pub(super) async fn initialize(history: &History) -> Result<State> {
    let tracker = history.tracker.clone();
    let game = history.game.clone();
    history
        .lease
        .participant(async move {
            tracker.ensure_location_ready(&game).await?;
            let legacy = paths::saves_root()?.join(&game).join("transition.json");
            match tokio::fs::symlink_metadata(&legacy).await {
                Ok(_) => anyhow::bail!(
                    "Finish existing save recovery before initializing live save ownership"
                ),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).context(
                        "Save recovery storage is unavailable; restore access before continuing",
                    );
                }
            }
            import(&tracker, &game).await
        })
        .await
        .context("Live save ownership initialization stopped")?
}

pub(super) async fn import(tracker: &Tracker, game: &str) -> Result<State> {
    let mut tx = durable(tracker).await?;
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_journals WHERE game_id=?)")
            .bind(game)
            .fetch_one(&mut *tx)
            .await?;
    ensure!(
        !pending,
        "Finish pending generation recovery before initializing live save ownership"
    );
    if let Some(state) = state::read(&mut tx, game).await? {
        tx.rollback().await?;
        return Ok(state);
    }
    let (profile, mode) = selected(&mut tx, game).await?;
    let deployed: Option<String> = sqlx::query_scalar(
        "SELECT p.id FROM settings s JOIN profiles p ON p.id=s.value AND p.game_id=? WHERE s.key=?",
    )
    .bind(game)
    .bind(format!("last_deployed_profile_{game}"))
    .fetch_optional(&mut *tx)
    .await?;
    sqlx::query("INSERT INTO generation_game_state(game_id,deployed_profile_id,live_save_profile_id,live_save_mode,modified) VALUES (?,?,?,?,0)")
        .bind(game).bind(deployed).bind(if mode == "profile" { Some(profile) } else { None })
        .bind(mode).execute(&mut *tx).await?;
    let state = state::read(&mut tx, game)
        .await?
        .context("Live save ownership was not persisted")?;
    tx.commit()
        .await
        .context("Cannot persist live save ownership; activation remains unavailable")?;
    Ok(state)
}

async fn selected(tx: &mut Transaction<'_, Sqlite>, game: &str) -> Result<(String, String)> {
    let profiles: Vec<(String, String)> =
        sqlx::query_as("SELECT id,save_mode FROM profiles WHERE game_id=? AND is_active=1")
            .bind(game)
            .fetch_all(&mut **tx)
            .await?;
    ensure!(
        profiles.len() == 1,
        "Existing live save ownership requires exactly one selected profile; resolve profile selection before continuing"
    );
    let (profile, mode) = profiles
        .into_iter()
        .next()
        .context("Selected profile is missing")?;
    ensure!(
        matches!(mode.as_str(), "global" | "profile"),
        "Selected profile has an unsupported save mode; live saves were preserved"
    );
    let restored: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_drafts WHERE game_id=? AND profile_id=? AND seed_live_saves=1)")
        .bind(game).bind(&profile).fetch_one(&mut **tx).await?;
    ensure!(
        !restored,
        "An uninitialized restored profile cannot establish existing live save ownership"
    );
    Ok((profile, mode))
}

#[cfg(test)]
mod tests;
