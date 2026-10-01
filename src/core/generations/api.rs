use std::path::Path;

use anyhow::Result;

use crate::core::tracker::Tracker;
use crate::models::game::Game;

use super::{catalog::History, content::Control, history, recovery, restore};

#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) timestamp: String,
    pub(crate) deployed: bool,
    pub(crate) modified: bool,
    pub(crate) recovery_pending: bool,
    pub(crate) deletion: Result<u64, String>,
}

#[derive(Debug)]
pub(crate) struct Overview {
    pub(crate) entries: Vec<Entry>,
    pub(crate) bytes: u64,
}

pub(crate) async fn list(tracker: &Tracker, game: &str) -> Result<Overview> {
    let mut entries = Vec::new();
    for entry in history::list(tracker, game).await? {
        let deletion = history::deletion_size(tracker, game, &entry.id)
            .await
            .map_err(|error| error.to_string());
        entries.push(Entry {
            id: entry.id,
            name: entry.profile_name,
            timestamp: entry.created_at,
            deployed: entry.deployed,
            modified: entry.modified,
            recovery_pending: entry.recovery_pending,
            deletion,
        });
    }
    Ok(Overview {
        entries,
        bytes: history::usage(tracker, game).await?,
    })
}

pub(crate) async fn restore(
    tracker: &Tracker,
    game: &Game,
    cache: &Path,
    generation: &str,
    name: &str,
    control: Control,
) -> Result<String> {
    let history = History::open(tracker, &game.id, cache, false).await?;
    recovery::recover(&history, game).await?;
    restore::restore(&history, generation, name, control).await
}

pub(crate) async fn delete(
    tracker: &Tracker,
    game: &Game,
    cache: &Path,
    generation: &str,
) -> Result<()> {
    let history = History::open(tracker, &game.id, cache, false).await?;
    recovery::recover(&history, game).await?;
    history::delete(&history, generation).await
}

#[derive(Debug)]
pub(crate) struct IntegrityReport {
    pub(crate) generations: usize,
    pub(crate) differing_files: usize,
    pub(crate) hashed_bytes: u64,
}

pub(crate) async fn verify_integrity(
    tracker: &Tracker,
    game: &Game,
    cache: &Path,
    control: Control,
) -> Result<IntegrityReport> {
    let tracker = tracker.clone();
    let game = game.clone();
    let cache = cache.to_owned();
    tokio::spawn(async move {
        let history = History::open(&tracker, &game.id, &cache, false).await?;
        tracker.ensure_location_ready(&game.id).await?;
        let pending: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM generation_journals WHERE game_id=?)")
                .bind(&game.id)
                .fetch_one(&tracker.pool)
                .await?;
        anyhow::ensure!(
            !pending,
            "Finish pending deployment recovery before checking integrity"
        );
        control.check()?;
        crate::utils::verified_files::invalidate()?;
        let before = crate::utils::verified_files::metrics();
        let generations: Vec<String> =
            sqlx::query_scalar("SELECT id FROM generations WHERE game_id=?")
                .bind(&game.id)
                .fetch_all(&tracker.pool)
                .await?;
        (control.phase)("Checking all retained generations…");
        for generation in &generations {
            history
                .load_with_control(generation, control.clone())
                .await?;
        }
        (control.phase)("Checking deployed files…");
        let inspection = super::divergence::inspect(&history, &game, control.clone()).await?;
        control.check()?;
        let after = crate::utils::verified_files::metrics();
        Ok(IntegrityReport {
            generations: generations.len(),
            differing_files: inspection.map_or(0, |inspection| inspection.differences.len()),
            hashed_bytes: after.hashed_bytes.saturating_sub(before.hashed_bytes),
        })
    })
    .await?
}
