use anyhow::{Context, Result, ensure};

use crate::core::save_manager::SaveSetId;
use crate::core::{game, save_manager};
use crate::models::game::Game;

use super::catalog::{History, durable};
use super::content::Control;
use super::journal::Journal;
use super::manifest::Manifest;
use super::records::text;
use super::state::{self, State};

pub(super) struct Prepared {
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) previous: State,
    pub(super) target: SaveSetId,
    pub(super) journal: Journal,
}

pub(super) async fn prepare(
    history: &History,
    game: &Game,
    journal: Journal,
    manifest: &Manifest,
    control: Control,
) -> Result<Prepared> {
    control.check()?;
    ensure!(
        history.game == game.id && journal.game == game.id && manifest.game_id == game.id,
        "Save preparation belongs to another game"
    );
    ensure!(
        game::has_save_management(game) && game.wine_prefix.is_some(),
        "Save preparation requires a supported game and an accessible Wine prefix"
    );
    ensure!(
        !journal.has_saves(),
        "Activation already has prepared saves"
    );
    manifest.validate()?;
    ensure!(
        manifest.engine == game.engine,
        "Save preparation belongs to another engine"
    );
    history.tracker.ensure_location_ready(&game.id).await?;
    let profile = manifest.profile()?.0;
    let mode = text(&manifest.records[0].rows[0], "save_mode")?;
    let target = match mode {
        "global" => SaveSetId::Global {
            game_id: game.id.clone(),
        },
        "profile" => SaveSetId::Profile {
            game_id: game.id.clone(),
            profile_id: profile.to_owned(),
        },
        _ => anyhow::bail!("Prepared profile has an unsupported save mode"),
    };
    let mut tx = durable(&history.tracker).await?;
    let pending: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_journals WHERE game_id=?)")
            .bind(&game.id)
            .fetch_one(&mut *tx)
            .await?;
    ensure!(
        !pending,
        "Finish pending activation recovery before preparing saves"
    );
    let previous = state::read(&mut tx, &game.id)
        .await?
        .context("Initialize live save ownership before preparing activation")?;
    let current_mode: String =
        sqlx::query_scalar("SELECT save_mode FROM profiles WHERE id=? AND game_id=?")
            .bind(profile)
            .bind(&game.id)
            .fetch_one(&mut *tx)
            .await
            .context("Prepared profile is no longer available")?;
    ensure!(
        mode == current_mode,
        "Profile save mode changed; prepare deployment again"
    );
    let seed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_drafts WHERE game_id=? AND profile_id=? AND seed_live_saves=1)")
        .bind(&game.id).bind(profile).fetch_one(&mut *tx).await?;
    let seed = seed && target.profile_id().is_some();
    ensure!(
        previous.saves != target || !seed,
        "An uninitialized restored save bank cannot own live saves"
    );
    tx.rollback().await?;
    let cap_bytes = save_manager::configured_backup_cap_bytes(&history.tracker).await;
    let game = game.clone();
    history
        .lease
        .participant(async move {
            let _prefix = tokio::fs::read_dir(
                game.wine_prefix
                    .as_deref()
                    .context("Wine prefix is missing")?,
            )
            .await
            .context("Wine prefix is unavailable; reselect it in Settings → Manage Games")?;
            journal.validate(&game)?;
            save_manager::activation::recovery::recover(&game).await?;
            control.check()?;
            if previous.saves == target {
                return Ok(Prepared {
                    previous,
                    target,
                    journal,
                });
            }
            let transition = save_manager::activation::prepare(
                &journal.id,
                &game,
                &previous.saves,
                &target,
                seed,
                cap_bytes,
                control.clone(),
            )
            .await?;
            let result = async {
                control.check()?;
                let mut journal = journal.clone();
                journal.attach_saves(&game, transition)?;
                Ok(Prepared {
                    previous,
                    target,
                    journal,
                })
            }
            .await;
            if let Err(error) = result {
                save_manager::activation::discard_preparation(&game, &journal.id)
                    .await
                    .with_context(|| format!("{error:#}; save preparation cleanup also failed"))?;
                return Err(error);
            }
            result
        })
        .await
        .context("Save preparation participant stopped")?
}

#[cfg(test)]
#[path = "../../../tests/generations/saves.rs"]
mod tests;
