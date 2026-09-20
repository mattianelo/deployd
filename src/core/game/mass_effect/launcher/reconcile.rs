use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, atomic::AtomicBool};

use anyhow::{Result, ensure};

use crate::core::{
    archive,
    game::mass_effect::operation::{Control, Lease},
    tracker::{LauncherComponentAssociation, Tracker},
};
use crate::dlog;
use crate::models::game::Game;
use crate::utils::location::FolderRole;

use super::{Entry, PersistedEntry};

pub(crate) async fn reconcile(
    tracker: &Tracker,
    game: &Game,
    data: &Path,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let control = Control::new(cancelled, Arc::new(AtomicBool::new(false)));
    let _lease = Lease::acquire(&control).await?;
    tracker.ensure_location_ready(&game.id).await?;
    tracker.ensure_no_mele_journal(&game.id).await?;
    let location = tracker.folder_location(&game.id, FolderRole::Game).await?;
    let Some(family) = tracker.mele_family(location.id).await? else {
        return Ok(());
    };
    let legacy: Vec<_> = family
        .mods
        .iter()
        .filter(|entry| matches!(entry, PersistedEntry::Legacy(_)))
        .cloned()
        .collect();
    let removed = tracker
        .mele_launcher_reconciliation_candidates(location.id)
        .await?;
    if legacy.is_empty() {
        if !removed.is_empty() {
            tracker
                .clear_mele_launcher_reconciliation_candidates(location.id)
                .await?;
        }
        return Ok(());
    }

    let mut download_paths: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for download in tracker.load_download_entries().await? {
        if let (Some(hash), Some(path)) = (download.archive_hash, download.archive_path) {
            download_paths.entry(hash).or_default().push(path);
        }
    }
    for binding in location
        .bindings
        .iter()
        .filter(|binding| binding.role == FolderRole::Game)
    {
        for installed in tracker.list_mods(&binding.game_id).await? {
            if let (Some(hash), Some(path)) = (installed.archive_hash, installed.archive_path) {
                download_paths
                    .entry(hash)
                    .or_default()
                    .push(PathBuf::from(path));
            }
        }
    }
    for candidate in &removed {
        if let Some(path) = &candidate.archive_path {
            download_paths
                .entry(candidate.archive_sha256.clone())
                .or_default()
                .push(PathBuf::from(path));
        }
    }
    let mut inspected: BTreeMap<String, Option<Entry>> = BTreeMap::new();
    for binding in location
        .bindings
        .iter()
        .filter(|binding| binding.role == FolderRole::Game)
    {
        for installed in tracker.list_mods(&binding.game_id).await? {
            control.check()?;
            let Some(record) = tracker.mele_package(&installed.id).await? else {
                continue;
            };
            let mut matching_legacy = None;
            if let Some(component) = record.launcher() {
                let matches: Vec<_> = legacy
                    .iter()
                    .filter(|entry| entry.matches(component))
                    .collect();
                if let [legacy] = matches.as_slice() {
                    matching_legacy = Some((*legacy).clone());
                    tracker
                        .bind_mele_launcher_component(
                            &LauncherComponentAssociation {
                                location_id: location.id,
                                legacy_id: legacy.id(),
                                source_sha256: legacy.source_sha256(),
                                archive_sha256: installed.archive_hash.as_deref(),
                                game_id: &binding.game_id,
                                mod_id: &installed.id,
                            },
                            &record,
                            &record,
                        )
                        .await?;
                    if super::source_root(data, legacy.source_sha256()).try_exists()? {
                        continue;
                    }
                }
            }
            let Some(hash) = installed.archive_hash.as_deref() else {
                continue;
            };
            if !inspected.contains_key(hash) {
                let inspection = inspect_archive(
                    hash.to_owned(),
                    download_paths.get(hash).cloned().unwrap_or_default(),
                    legacy.clone(),
                    data.to_path_buf(),
                    control.clone(),
                )
                .await?;
                inspected.insert(hash.to_owned(), inspection);
            }
            let Some(component) = inspected.get(hash).and_then(Option::as_ref) else {
                continue;
            };
            let matched = if let Some(legacy) = matching_legacy {
                legacy
            } else {
                let matches: Vec<_> = legacy
                    .iter()
                    .filter(|entry| entry.matches(component))
                    .collect();
                let [legacy] = matches.as_slice() else {
                    continue;
                };
                (*legacy).clone()
            };
            let desired = if record.launcher().is_some() {
                record.clone()
            } else {
                record
                    .bind_launcher(matched.owned(installed.id.clone(), binding.game_id.clone()))?
            };
            tracker
                .bind_mele_launcher_component(
                    &LauncherComponentAssociation {
                        location_id: location.id,
                        legacy_id: matched.id(),
                        source_sha256: matched.source_sha256(),
                        archive_sha256: Some(hash),
                        game_id: &binding.game_id,
                        mod_id: &installed.id,
                    },
                    &record,
                    &desired,
                )
                .await?;
        }
    }
    for candidate in removed {
        control.check()?;
        let hash = candidate.archive_sha256.as_str();
        if !inspected.contains_key(hash) {
            let inspection = inspect_archive(
                hash.to_owned(),
                download_paths.get(hash).cloned().unwrap_or_default(),
                legacy.clone(),
                data.to_path_buf(),
                control.clone(),
            )
            .await?;
            inspected.insert(hash.to_owned(), inspection);
        }
        let Some(component) = inspected.get(hash).and_then(Option::as_ref) else {
            continue;
        };
        let matches: Vec<_> = legacy
            .iter()
            .filter(|entry| entry.matches(component))
            .collect();
        let [legacy] = matches.as_slice() else {
            continue;
        };
        tracker
            .bind_removed_mele_launcher_component(
                location.id,
                &candidate,
                legacy.id(),
                legacy.source_sha256(),
            )
            .await?;
    }
    Ok(())
}

async fn inspect_archive(
    expected: String,
    mut paths: Vec<PathBuf>,
    candidates: Vec<PersistedEntry>,
    data: PathBuf,
    control: Control,
) -> Result<Option<Entry>> {
    paths.sort();
    paths.dedup();
    tokio::task::spawn_blocking(move || -> Result<Option<Entry>> {
        for path in paths {
            let result = (|| -> Result<Option<Entry>> {
                let before = fs::symlink_metadata(&path)?;
                ensure!(
                    before.is_file() && !before.file_type().is_symlink(),
                    "MELE reconciliation archives must be regular files"
                );
                if archive::hash_archive_file(&path)? != expected {
                    return Ok(None);
                }
                let hashed = fs::symlink_metadata(&path)?;
                ensure!(
                    hashed.dev() == before.dev()
                        && hashed.ino() == before.ino()
                        && hashed.len() == before.len(),
                    "MELE reconciliation archive changed while hashing"
                );
                let extracted = archive::extract_archive(&path, None)?;
                ensure!(
                    archive::hash_archive_file(&path)? == expected,
                    "MELE reconciliation archive changed while inspecting"
                );
                let inspected = fs::symlink_metadata(&path)?;
                ensure!(
                    inspected.is_file()
                        && !inspected.file_type().is_symlink()
                        && inspected.dev() == before.dev()
                        && inspected.ino() == before.ino()
                        && inspected.len() == before.len(),
                    "MELE reconciliation archive changed while inspecting"
                );
                let Some(bundle) = super::inspect_bundle(extracted.path())? else {
                    return Ok(None);
                };
                let matches: Vec<_> = candidates
                    .iter()
                    .filter(|entry| entry.matches(&bundle.entry))
                    .collect();
                let [legacy] = matches.as_slice() else {
                    return Ok(None);
                };
                super::retain_persisted(
                    &data,
                    &extracted.path().join(bundle.source),
                    legacy,
                    &control,
                )?;
                Ok(Some(bundle.entry))
            })();
            match result {
                Ok(Some(entry)) => return Ok(Some(entry)),
                Ok(None) => {}
                Err(error) => {
                    control.check()?;
                    dlog!(
                        "[deployd] skipped unavailable MELE reconciliation archive '{}': {error:#}",
                        path.display()
                    );
                }
            }
        }
        Ok(None)
    })
    .await?
}
