use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use crate::core::save_manager::SaveSetId;
use crate::models::game::Game;

use super::staging::Purpose;
use super::{bank, preparation, staging};

fn entries(parent: &Path) -> Result<Vec<PathBuf>> {
    match fs::symlink_metadata(parent) {
        Ok(metadata) => ensure!(
            metadata.is_dir(),
            "Save recovery storage is not a directory"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).context("Save recovery storage is unavailable"),
    }
    let mut paths = fs::read_dir(parent)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    paths.sort();
    Ok(paths)
}

fn name(path: &Path) -> Option<&str> {
    path.file_name()?.to_str()
}

fn bank_owner(game: &str, bank: &str, profiles: bool) -> Result<SaveSetId> {
    super::super::validate_component("save bank", bank)?;
    if profiles {
        Ok(SaveSetId::Profile {
            game_id: game.to_owned(),
            profile_id: bank.to_owned(),
        })
    } else {
        ensure!(
            bank == "global",
            "Unexpected save bank in global storage; recovery was preserved"
        );
        Ok(SaveSetId::Global {
            game_id: game.to_owned(),
        })
    }
}

fn recover_banks(parent: &Path, game: &str, profiles: bool) -> Result<()> {
    for path in entries(parent)? {
        if let Some(bank) = name(&path)
            .and_then(|name| name.strip_prefix('.'))
            .and_then(|name| name.strip_suffix(".generation-bank.json"))
        {
            let owner = bank_owner(game, bank, profiles)?;
            bank::recover(&parent.join(bank), &owner)?;
        }
    }
    Ok(())
}

fn discard_banks(parent: &Path, game: &str, profiles: bool) -> Result<()> {
    for path in entries(parent)? {
        if let Some(root) = name(&path)
            .filter(|name| name.starts_with(".generation-bank-"))
            .and_then(|name| name.strip_suffix(".preparation.json"))
        {
            let owned = staging::read(&parent.join(root))?
                .context("Save staging record disappeared during recovery")?;
            let expected = bank_owner(
                game,
                owned.owner().profile_id().unwrap_or("global"),
                profiles,
            )?;
            ensure!(
                owned.purpose() == Purpose::Bank && owned.owner() == &expected,
                "Save staging belongs to a different bank; recovery was preserved"
            );
            owned.discard(parent)?;
        }
    }
    Ok(())
}

fn discard_live(live: &Path, game: &str) -> Result<()> {
    let parent = live
        .parent()
        .context("Live saves have no parent directory")?;
    for path in entries(parent)? {
        let Some(name) = name(&path) else {
            continue;
        };
        if let Some(operation) = name
            .strip_prefix(".deployd-saves-")
            .and_then(|name| name.strip_suffix("-preparation.json"))
        {
            let transition = preparation::read(live, operation)?
                .context("Save preparation disappeared during recovery")?;
            if transition.source.game_id() == game {
                preparation::discard(live, operation, game)?;
            }
        } else if let Some(root) = name
            .strip_suffix(".preparation.json")
            .filter(|name| name.starts_with(".deployd-save-snapshot-"))
        {
            let owned = staging::read(&parent.join(root))?
                .context("Snapshot ownership disappeared during recovery")?;
            ensure!(
                owned.purpose() == Purpose::Snapshot,
                "Unexpected staging purpose in live-save storage"
            );
            if owned.owner().game_id() == game {
                owned.discard(parent)?;
            }
        }
    }
    Ok(())
}

fn recover_at(game: &str, save_root: &Path, live: Option<&Path>) -> Result<()> {
    entries(save_root)?;
    ensure!(
        fs::symlink_metadata(save_root.join("transition.json"))
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "Finish legacy save-transition recovery before generation preparation cleanup"
    );
    let sets = save_root.join("sets");
    let profiles = sets.join("profiles");
    recover_banks(&sets, game, false)?;
    recover_banks(&profiles, game, true)?;
    discard_banks(&sets, game, false)?;
    discard_banks(&profiles, game, true)?;
    if let Some(live) = live {
        discard_live(live, game)?;
    }
    Ok(())
}

pub(crate) async fn recover(game: &Game) -> Result<()> {
    let game = game.clone();
    tokio::task::spawn_blocking(move || {
        let root = super::super::game_root(&game.id)?;
        let live = if game.wine_prefix.is_some() && crate::core::game::has_save_management(&game) {
            Some(super::super::validate_live_save_access(&game)?)
        } else {
            None
        };
        recover_at(&game.id, &root, live.as_deref())
    })
    .await
    .context("Save preparation recovery worker stopped")?
}

#[cfg(test)]
mod tests {
    use crate::core::generations::content::Control;

    use super::*;

    fn owner() -> SaveSetId {
        SaveSetId::Global {
            game_id: "game".into(),
        }
    }

    fn snapshot(parent: &Path) -> Result<(PathBuf, PathBuf)> {
        fs::create_dir_all(parent)?;
        let live = parent.join("live");
        fs::create_dir(&live)?;
        fs::write(live.join("save.dat"), b"current progress")?;
        let snapshot = super::super::Snapshot::capture(&live, &owner(), &Control::default())?;
        Ok((live, snapshot.directory))
    }

    // @variants: both
    #[test]
    fn recovery_publishes_pending_banks_before_discarding_abandoned_copies() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let save_root = temp.path().join("game");
        let source = temp.path().join("source");
        fs::create_dir(&source)?;
        fs::write(source.join("save.dat"), b"bank progress")?;
        let bank_root = save_root.join("sets/global");
        bank::leave_pending(&bank_root, &owner(), &source)?;
        let profile = SaveSetId::Profile {
            game_id: "game".into(),
            profile_id: "deleted-profile".into(),
        };
        let profile_root = save_root.join("sets/profiles/deleted-profile");
        bank::leave_pending(&profile_root, &profile, &source)?;
        let abandoned = staging::Staging::new(&owner(), Purpose::Bank, super::super::tree::empty());
        let abandoned_root = abandoned.persist(bank_root.parent().context("Bank parent")?)?;
        let (live, snapshot_root) = snapshot(&temp.path().join("prefix"))?;
        let transition = super::super::stage(
            &uuid::Uuid::new_v4().to_string(),
            owner(),
            profile,
            &live,
            &source,
            &Control::default(),
        )?;
        let staged = transition.paths(&live)?.0;
        recover_at("game", &save_root, Some(&live))?;
        recover_at("game", &save_root, Some(&live))?;
        assert_eq!(fs::read(bank_root.join("data/save.dat"))?, b"bank progress");
        assert_eq!(
            fs::read(profile_root.join("data/save.dat"))?,
            b"bank progress"
        );
        assert_eq!(fs::read(live.join("save.dat"))?, b"current progress");
        assert!(!snapshot_root.exists() && !staged.exists());
        assert!(staging::read(&abandoned_root)?.is_none());
        Ok(())
    }

    // @variants: both
    #[test]
    fn invalid_publications_block_snapshot_cleanup() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let save_root = temp.path().join("game");
        fs::create_dir_all(save_root.join("sets"))?;
        let journal = bank::journal(&save_root.join("sets/global"))?;
        fs::write(&journal, b"invalid publication")?;
        let (live, snapshot_root) = snapshot(&temp.path().join("prefix"))?;
        assert!(recover_at("game", &save_root, Some(&live)).is_err());
        assert!(snapshot_root.exists() && staging::read(&snapshot_root)?.is_some());
        assert_eq!(fs::read(journal)?, b"invalid publication");
        Ok(())
    }

    // @variants: both
    #[test]
    fn recovery_preserves_unowned_files_and_other_games_snapshots() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (live, own) = snapshot(&temp.path().join("prefix"))?;
        let foreign = super::super::Snapshot::capture(
            &live,
            &SaveSetId::Global {
                game_id: "other".into(),
            },
            &Control::default(),
        )?;
        let unowned = live
            .parent()
            .context("Live parent")?
            .join(".deployd-save-snapshot-unowned");
        fs::create_dir(&unowned)?;
        fs::write(unowned.join("keep.dat"), b"unowned")?;
        recover_at("game", &temp.path().join("game"), Some(&live))?;
        assert!(!own.exists());
        assert!(foreign.directory.exists());
        assert_eq!(fs::read(unowned.join("keep.dat"))?, b"unowned");
        Ok(())
    }

    // @variants: both
    #[test]
    fn bank_staging_with_another_game_owner_is_preserved() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sets = temp.path().join("sets");
        fs::create_dir(&sets)?;
        let staging = staging::Staging::new(
            &SaveSetId::Global {
                game_id: "other".into(),
            },
            Purpose::Bank,
            super::super::tree::empty(),
        );
        let root = staging.persist(&sets)?;
        assert!(recover_at("game", temp.path(), None).is_err());
        assert!(staging::read(&root)?.is_some());
        Ok(())
    }

    // @variants: both
    #[test]
    fn missing_storage_is_not_created_and_symlinked_storage_is_rejected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let missing = temp.path().join("missing");
        recover_at("game", &missing, None)?;
        assert!(!missing.exists());
        let target = temp.path().join("target");
        fs::create_dir(&target)?;
        std::os::unix::fs::symlink(&target, &missing)?;
        assert!(recover_at("game", &missing, None).is_err());
        assert_eq!(fs::read_dir(&target)?.count(), 0);
        Ok(())
    }

    // @variants: both
    #[test]
    fn legacy_save_transitions_block_generation_cleanup() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("transition.json"), b"legacy recovery")?;
        let (live, snapshot_root) = snapshot(&temp.path().join("prefix"))?;
        assert!(recover_at("game", temp.path(), Some(&live)).is_err());
        assert!(snapshot_root.exists());
        assert_eq!(
            fs::read(temp.path().join("transition.json"))?,
            b"legacy recovery"
        );
        Ok(())
    }
}
