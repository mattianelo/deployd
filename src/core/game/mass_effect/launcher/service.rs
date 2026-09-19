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
    #[cfg(test)]
    Add(Inspected),
    AddBundled {
        entry: Entry,
        source: PathBuf,
    },
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

pub(crate) async fn generation_entries(
    tracker: &Tracker,
    snapshot: Snapshot,
    action: Action,
    cancelled: Arc<AtomicBool>,
) -> Result<(Game, Vec<super::Entry>)> {
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
    let data = crate::utils::paths::deployd_data_dir()?;
    let control = Control::new(cancelled, Arc::new(AtomicBool::new(false)));
    let mut entries = snapshot.entries;
    match action {
        #[cfg(test)]
        Action::Add(mut inspected) => {
            inspected.entry.approval = inspected.entry.source_sha256.clone();
            let entry = inspected.entry.clone();
            ensure!(
                !entries
                    .iter()
                    .any(|old| old.source_sha256 == entry.source_sha256),
                "This launcher package is already in the shared list"
            );
            let retaining = data.clone();
            let source = inspected
                .source
                .as_ref()
                .context("Launcher archive was already released")?
                .path()
                .to_path_buf();
            let retained = entry.clone();
            tokio::task::spawn_blocking(move || retain(&retaining, &source, &retained, &control))
                .await??;
            entries.push(entry);
        }
        Action::AddBundled { mut entry, source } => {
            entry.approval = entry.source_sha256.clone();
            if !entries
                .iter()
                .any(|old| old.source_sha256 == entry.source_sha256)
            {
                let retaining = data.clone();
                let retained = entry.clone();
                tokio::task::spawn_blocking(move || {
                    retain(&retaining, &source, &retained, &control)
                })
                .await??;
                entries.push(entry);
            }
        }
        Action::Enable(id, enabled) => {
            entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .context("Launcher mod no longer exists")?
                .enabled = enabled;
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
    Ok((snapshot.game, entries))
}

pub(crate) async fn add_bundled(
    tracker: Tracker,
    game: Game,
    bundled: Bundled,
    extracted_root: PathBuf,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let snapshot = load(&tracker, &game).await?;
    let (game, entries) = generation_entries(
        &tracker,
        snapshot,
        Action::AddBundled {
            entry: bundled.entry,
            source: extracted_root.join(bundled.source),
        },
        cancelled.clone(),
    )
    .await?;
    let cache = tracker
        .get_setting(&format!("cache_dir_{}", game.id))
        .await?
        .map(PathBuf::from)
        .map_or_else(crate::utils::paths::cache_root, Ok)?;
    let control = crate::core::generations::content::Control {
        cancelled,
        progress: Arc::new(|_, _| {}),
    };
    crate::core::generations::activation::prepare_shared(
        &tracker,
        &game,
        &cache,
        entries,
        control.clone(),
    )
    .await?
    .apply(control)
    .await
}

#[cfg_attr(not(test), allow(dead_code))]
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
            #[cfg(test)]
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
                let retained = entry.clone();
                let source = inspected
                    .source
                    .as_ref()
                    .context("Launcher archive was already released")?
                    .path()
                    .to_path_buf();
                tokio::task::spawn_blocking(move || retain(&data, &source, &retained, &control))
                    .await??;
                entries.push(entry);
            }
            Action::AddBundled { mut entry, source } => {
                entry.approval = entry.source_sha256.clone();
                if entries
                    .iter()
                    .any(|old| old.source_sha256 == entry.source_sha256)
                {
                    return Ok(());
                }
                let data = data.clone();
                let control = control.clone();
                let retained = entry.clone();
                tokio::task::spawn_blocking(move || retain(&data, &source, &retained, &control))
                    .await??;
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
