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
