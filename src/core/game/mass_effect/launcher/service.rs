use std::sync::{Arc, atomic::AtomicBool};

use super::*;
use crate::core::tracker::Tracker;
use crate::models::game::Game;
use crate::utils::location::FolderRole;

#[derive(Debug, Clone)]
pub(crate) struct Snapshot {
    pub(crate) entries: Vec<Entry>,
    pub(crate) game: Game,
    location: i64,
    expected: Option<super::super::family::Family>,
}

#[derive(Debug)]
pub(crate) enum Action {
    Add(Inspected),
    Enable(String, bool),
    Move(String, i32),
    Remove(String),
    Restore,
    Repair,
}

pub(crate) async fn load(tracker: &Tracker, game: &Game) -> Result<Snapshot> {
    super::super::library::target(game)?;
    tracker.ensure_location_ready(&game.id).await?;
    tracker.ensure_no_mele_journal(&game.id).await?;
    let location = tracker
        .folder_location(&game.id, FolderRole::Game)
        .await?
        .id;
    let expected = tracker.mele_family(location).await?;
    Ok(Snapshot {
        entries: expected
            .as_ref()
            .map(|family| family.mods.clone())
            .unwrap_or_default(),
        game: game.clone(),
        location,
        expected,
    })
}

pub(crate) async fn apply(
    tracker: Tracker,
    snapshot: Snapshot,
    action: Action,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let data = crate::utils::paths::deployd_data_dir()?;
    apply_in(tracker, snapshot, action, data, cancelled).await
}

pub(in crate::core::game::mass_effect) async fn apply_in(
    tracker: Tracker,
    snapshot: Snapshot,
    action: Action,
    data: PathBuf,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let abandoned = Arc::new(AtomicBool::new(false));
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, std::sync::atomic::Ordering::Release);
        }
    }
    let _guard = Guard(abandoned.clone());
    let control = Control::new(cancelled, abandoned);
    tokio::spawn(async move {
        let _lease = super::super::operation::Lease::acquire(&control).await?;
        tracker.ensure_location_ready(&snapshot.game.id).await?;
        tracker.ensure_no_mele_journal(&snapshot.game.id).await?;
        ensure!(
            tracker
                .folder_location(&snapshot.game.id, FolderRole::Game)
                .await?
                .id
                == snapshot.location
                && tracker.mele_family(snapshot.location).await? == snapshot.expected,
            "Shared launcher mods changed; reopen their list before applying changes"
        );
        let mut entries = snapshot.entries;
        match action {
            Action::Add(mut inspected) => {
                inspected.entry.approval = inspected.entry.source_sha256.clone();
                let entry = inspected.entry.clone();
                ensure!(
                    !entries
                        .iter()
                        .any(|old| old.source_sha256 == entry.source_sha256),
                    "This launcher package is already in the shared list"
                );
                let data = data.clone();
                let control = control.clone();
                tokio::task::spawn_blocking(move || retain(&data, &inspected, &control)).await??;
                entries.push(entry);
            }
            Action::Enable(id, enabled) => {
                entries
                    .iter_mut()
                    .find(|entry| entry.id == id)
                    .context("Launcher mod no longer exists")?
                    .enabled = enabled
            }
            Action::Move(id, offset) => {
                ensure!(offset == -1 || offset == 1, "Invalid launcher order change");
                let index = entries
                    .iter()
                    .position(|entry| entry.id == id)
                    .context("Launcher mod no longer exists")?;
                let destination = index
                    .checked_add_signed(offset as isize)
                    .filter(|index| *index < entries.len())
                    .context("Launcher mod is already at the end of the list")?;
                entries.swap(index, destination);
            }
            Action::Remove(id) => {
                ensure!(
                    entries.iter().any(|entry| entry.id == id),
                    "Launcher mod no longer exists"
                );
                entries.retain(|entry| entry.id != id);
            }
            Action::Restore => {
                for entry in &mut entries {
                    entry.enabled = false;
                }
            }
            Action::Repair => {}
        }
        validate_entries(&entries)?;
        {
            let entries = entries.clone();
            let data = data.clone();
            let control = control.clone();
            tokio::task::spawn_blocking(move || {
                for entry in entries.iter().filter(|entry| entry.enabled) {
                    verify_source(&source_root(&data, &entry.source_sha256), entry, &control)?;
                }
                Ok::<_, anyhow::Error>(())
            })
            .await??;
        }
        let plan =
            super::super::family::edit(&tracker, &snapshot.game, entries, &data, control.clone())
                .await?;
        super::super::journal::shared::publish(&tracker, &snapshot.game, plan, &data, &control)
            .await
    })
    .await
    .context("Launcher deployment worker failed")?
}
