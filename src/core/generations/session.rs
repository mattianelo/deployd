use anyhow::{Context, Result, ensure};

use crate::core::{game, save_manager, tracker::Tracker};
use crate::models::game::Game;

use super::catalog::{History, durable};
use super::{divergence, ownership, recovery, state};

pub(crate) async fn initialize(tracker: &Tracker, game: &Game) -> Result<()> {
    super::relocation::recover(tracker, &game.id).await?;
    let cache: Option<String> =
        sqlx::query_scalar("SELECT cache_root FROM generation_stores WHERE game_id=?")
            .bind(&game.id)
            .fetch_optional(&tracker.pool)
            .await?;
    if let Some(cache) = cache {
        let history = History::open(tracker, &game.id, std::path::Path::new(&cache), false).await?;
        recovery::recover_journal(&history, game).await?;
    }
    ownership::import(tracker, &game.id).await?;
    Ok(())
}

pub(crate) async fn inspect_live(tracker: &Tracker, game: &Game) -> Result<Option<usize>> {
    let cache: Option<String> =
        sqlx::query_scalar("SELECT cache_root FROM generation_stores WHERE game_id=?")
            .bind(&game.id)
            .fetch_optional(&tracker.pool)
            .await?;
    let Some(cache) = cache else {
        return Ok(None);
    };
    let history = History::open(tracker, &game.id, std::path::Path::new(&cache), false).await?;
    Ok(
        divergence::inspect(&history, game, super::content::Control::default())
            .await?
            .map(|inspection| {
                let _generation = inspection.generation;
                inspection.differences.len()
            }),
    )
}

pub(crate) async fn live_saves(tracker: &Tracker, game: &Game) -> Result<save_manager::SaveSetId> {
    let mut tx = durable(tracker).await?;
    Ok(state::read(&mut tx, &game.id)
        .await?
        .context("Initialize live save ownership before continuing")?
        .saves)
}

pub(crate) async fn can_delete_profile(tracker: &Tracker, profile: &str) -> Result<()> {
    let owns: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_game_state WHERE live_save_mode='profile' AND live_save_profile_id=?)")
        .bind(profile).fetch_one(&tracker.pool).await?;
    ensure!(
        !owns,
        "This profile owns the live saves. Explicitly Deploy another profile before deleting it."
    );
    Ok(())
}

pub(crate) async fn reconcile_plugins(tracker: &Tracker, game: &Game) -> Result<()> {
    if game::plugins_txt_paths(game).is_empty() {
        return Ok(());
    }
    let selected: Option<String> =
        sqlx::query_scalar("SELECT id FROM profiles WHERE game_id=? AND is_active=1")
            .bind(&game.id)
            .fetch_optional(&tracker.pool)
            .await?;
    let deployed: Option<String> =
        sqlx::query_scalar("SELECT deployed_profile_id FROM generation_game_state WHERE game_id=?")
            .bind(&game.id)
            .fetch_optional(&tracker.pool)
            .await?
            .flatten();
    let Some(deployed) = deployed else {
        return Ok(());
    };
    let paths = game::plugins_txt_paths(game);
    let entries = tokio::task::spawn_blocking(move || -> Result<Vec<(String, bool)>> {
        for path in paths {
            let entries = crate::utils::plugins_txt::read_plugins_txt(&path)?;
            if !entries.is_empty() {
                return Ok(entries);
            }
        }
        Ok(Vec::new())
    })
    .await??;
    if entries.is_empty() {
        return Ok(());
    }
    import_plugins(tracker, &game.id, &deployed, selected.as_deref(), &entries).await
}

async fn import_plugins(
    tracker: &Tracker,
    game: &str,
    deployed: &str,
    selected: Option<&str>,
    entries: &[(String, bool)],
) -> Result<()> {
    let mut tx = durable(tracker).await?;
    for (index, (filename, enabled)) in entries.iter().enumerate() {
        sqlx::query("UPDATE profile_plugins SET load_order=?,enabled=? WHERE profile_id=? AND plugin_id IN (SELECT p.id FROM plugins p JOIN mods m ON m.id=p.mod_id JOIN profile_mods pm ON pm.mod_id=m.id WHERE m.game_id=? AND pm.profile_id=? AND pm.enabled=1 AND lower(p.filename)=lower(?))")
            .bind(i64::try_from(index)?).bind(enabled).bind(deployed).bind(game).bind(deployed).bind(filename).execute(&mut *tx).await?;
    }
    if selected == Some(deployed) {
        sqlx::query("UPDATE plugins SET load_order=pp.load_order,enabled=pp.enabled FROM profile_plugins pp WHERE pp.profile_id=? AND pp.plugin_id=plugins.id AND plugins.mod_id IN (SELECT id FROM mods WHERE game_id=?)")
            .bind(deployed).bind(game).execute(&mut *tx).await?;
    }
    tx.commit()
        .await
        .context("Cannot save the deployed profile's live plugin configuration")
}

#[cfg(test)]
mod tests;
