use anyhow::{Context, Result, ensure};

use crate::core::save_manager::{BackupTrigger, SaveSyncResult};
use crate::core::{game, save_manager, tracker::Tracker};
use crate::models::game::{Game, GameEngine};

use super::super::{operation::Lease, ownership};

pub(crate) async fn select_profile(tracker: &Tracker, game: &Game, profile: &str) -> Result<()> {
    run(tracker, game, profile, true).await?;
    Ok(())
}

pub(crate) async fn sync_live_saves(
    tracker: &Tracker,
    game: &Game,
    profile: &str,
) -> Result<SaveSyncResult> {
    run(tracker, game, profile, false).await
}

async fn run(
    tracker: &Tracker,
    selected: &Game,
    profile: &str,
    select: bool,
) -> Result<SaveSyncResult> {
    let tracker = tracker.clone();
    let selected = selected.clone();
    let profile = profile.to_owned();
    let lease = Lease::acquire().await?;
    lease.participant(async move {
        let parts = tracker.profile_parts(&profile).await?;
        ensure!(parts.iter().any(|(game, id)| game == &selected.id && id == &profile), "Profile does not belong to the selected game");
        let games = tracker.load_persisted_games().await?;
        let mut captures = Vec::new();
        for (id, _) in parts {
            let member = if id == selected.id { selected.clone() } else {
                let stored = games.iter().find(|game| game.id == id).context("Restore the missing trilogy game before syncing saves")?;
                Game { id: stored.id.clone(), title: stored.title.clone(), path: stored.path.clone(), data_subdir: stored.data_subdir.clone(), engine: stored.engine.clone(), wine_prefix: stored.wine_prefix.clone() }
            };
            if !game::has_save_management(&member) {
                continue;
            }
            if member.wine_prefix.is_none() {
                ensure!(select && member.engine != GameEngine::MassEffect, "Configure the Wine prefix for {} before syncing or changing trilogy profiles", member.title);
                continue;
            }
            tracker.ensure_location_ready(&member.id).await?;
            if member.engine == GameEngine::MassEffect { tracker.ensure_no_mele_journal(&member.id).await?; }
            let owner = ownership::import(&tracker, &member.id).await?.saves;
            let checking = member.clone();
            tokio::task::spawn_blocking(move || save_manager::validate_sync_access(&checking)).await??;
            captures.push((member, owner));
        }
        let cap = save_manager::configured_backup_cap_bytes(&tracker).await;
        let mut total = SaveSyncResult::default();
        for (member, owner) in captures {
            save_manager::activation::recovery::recover(&member).await?;
            let result = if select {
                save_manager::capture_save_set(&member, &owner, BackupTrigger::ProfileSwitch, cap).await
            } else {
                save_manager::sync_save_set(&member, &owner, cap).await
            }.with_context(|| format!("Could not sync {} saves to their current bank; profile selection was not changed", member.title))?;
            total.added += result.added;
            total.modified += result.modified;
            total.removed += result.removed;
        }
        if select {
            tracker.switch_profile(&selected.id, &profile).await?;
        }
        Ok(total)
    }).await.context("Save sync worker stopped")?
}
