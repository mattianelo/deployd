use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::core::generations::content::Control;
use crate::models::game::Game;

use super::SaveSetId;

mod bank;
mod preparation;
pub(crate) mod recovery;
mod staging;
mod tree;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Transition {
    version: u32,
    operation: String,
    pub(crate) source: SaveSetId,
    pub(crate) target: SaveSetId,
    before: Option<tree::Tree>,
    after: tree::Tree,
}

impl Transition {
    fn paths(&self, live: &Path) -> Result<(PathBuf, PathBuf)> {
        ensure!(
            self.version == 1 && uuid::Uuid::parse_str(&self.operation).is_ok(),
            "Unsupported activation save journal"
        );
        ensure!(
            self.source != self.target && self.source.game_id() == self.target.game_id(),
            "Save transition belongs to different games"
        );
        tree::validate(&self.after)?;
        if let Some(before) = &self.before {
            tree::validate(before)?;
        }
        let parent = live
            .parent()
            .context("Live saves have no parent directory")?;
        ensure!(
            fs::symlink_metadata(parent)?.is_dir(),
            "Save parent is unavailable; restore Wine-prefix access"
        );
        Ok((
            parent.join(format!(".deployd-saves-{}-staged", self.operation)),
            parent.join(format!(".deployd-saves-{}-rollback", self.operation)),
        ))
    }

    pub(crate) fn validate(&self, operation: &str, game: &Game) -> Result<()> {
        ensure!(
            self.operation == operation
                && self.source.game_id() == game.id
                && self.target.game_id() == game.id,
            "Save participant belongs to another activation"
        );
        self.paths(&super::validate_live_save_access(game)?)?;
        Ok(())
    }

    pub(crate) async fn adopt_preparation(&self, game: &Game) -> Result<()> {
        let transition = self.clone();
        let game = game.clone();
        tokio::task::spawn_blocking(move || {
            let live = super::validate_live_save_access(&game)?;
            preparation::adopt(&live, &transition)
        })
        .await
        .context("Save preparation recovery worker stopped")?
    }

    pub(crate) async fn apply(&self, game: &Game) -> Result<()> {
        let live = super::validate_live_save_access(game)?;
        let transition = self.clone();
        tokio::task::spawn_blocking(move || transition.apply_at(&live))
            .await
            .context("Save activation worker stopped")?
    }

    fn apply_at(&self, live: &Path) -> Result<()> {
        let (staged, rollback) = self.paths(live)?;
        ensure!(
            tree::scan(live)? == self.before,
            "Live saves changed after preparation; activation stopped"
        );
        ensure!(
            tree::scan(&staged)?.as_ref() == Some(&self.after),
            "Staged saves changed; activation stopped"
        );
        ensure!(
            !rollback.try_exists()?,
            "Save rollback location is occupied"
        );
        if self.before.is_some() {
            rename(live, &rollback)?;
        }
        rename(&staged, live)?;
        self.verify_at(live)
    }

    pub(crate) async fn verify(&self, game: &Game) -> Result<()> {
        let live = super::validate_live_save_access(game)?;
        let transition = self.clone();
        tokio::task::spawn_blocking(move || transition.verify_at(&live))
            .await
            .context("Save verification worker stopped")?
    }

    fn verify_at(&self, live: &Path) -> Result<()> {
        let (_, rollback) = self.paths(live)?;
        ensure!(
            tree::scan(live)?.as_ref() == Some(&self.after),
            "Live saves changed before activation committed; recovery is blocked"
        );
        ensure!(
            tree::scan(&rollback)? == self.before,
            "Preserved saves changed; recovery is blocked"
        );
        Ok(())
    }

    pub(crate) async fn recover(&self, game: &Game, committed: bool) -> Result<()> {
        let live = super::validate_live_save_access(game)?;
        let transition = self.clone();
        tokio::task::spawn_blocking(move || transition.recover_at(&live, committed))
            .await
            .context("Save recovery worker stopped")?
    }

    fn recover_at(&self, live: &Path, committed: bool) -> Result<()> {
        let (staged, rollback) = self.paths(live)?;
        let current = tree::scan(live)?;
        if committed {
            ensure!(
                current.as_ref() == Some(&self.after),
                "External live-save edits were preserved; recovery is blocked"
            );
            if let Some(before) = &self.before {
                tree::remove(&rollback, before)?;
            } else {
                ensure!(
                    tree::scan(&rollback)?.is_none(),
                    "Unexpected save rollback directory was preserved"
                );
            }
            tree::remove(&staged, &self.after)?;
            return Ok(());
        }
        let preserved = tree::scan(&rollback)?;
        if preserved.is_some() {
            ensure!(
                preserved == self.before,
                "Preserved saves changed; rollback is blocked"
            );
            if current.is_some() {
                ensure!(
                    current.as_ref() == Some(&self.after) && tree::scan(&staged)?.is_none(),
                    "External live-save edits were preserved; rollback is blocked"
                );
                rename(live, &staged)?;
            }
            rename(&rollback, live)?;
        } else if self.before.is_none() && current.is_some() {
            ensure!(
                current.as_ref() == Some(&self.after) && tree::scan(&staged)?.is_none(),
                "Unexpected live saves were preserved"
            );
            rename(live, &staged)?;
        } else {
            ensure!(
                current == self.before,
                "Original live saves are unavailable; rollback is blocked"
            );
        }
        tree::remove(&staged, &self.after)?;
        ensure!(
            tree::scan(live)? == self.before,
            "Save rollback verification failed"
        );
        Ok(())
    }
}

fn rename(source: &Path, destination: &Path) -> Result<()> {
    fs::rename(source, destination)?;
    fs::File::open(
        destination
            .parent()
            .context("Save directory has no parent")?,
    )?
    .sync_all()?;
    Ok(())
}

#[cfg(test)]
pub(crate) fn prepare_staged(
    operation: &str,
    source: SaveSetId,
    target: SaveSetId,
    live: &Path,
    bank: &Path,
) -> Result<Transition> {
    stage(operation, source, target, live, bank, &Control::default())
}

fn stage(
    operation: &str,
    source: SaveSetId,
    target: SaveSetId,
    live: &Path,
    bank: &Path,
    control: &Control,
) -> Result<Transition> {
    let before = tree::scan_with_control(live, control)?;
    let after = tree::scan_with_control(bank, control)?
        .context("The target save bank is unavailable; initialize it before activation")?;
    let transition = Transition {
        version: 1,
        operation: operation.to_owned(),
        source,
        target,
        before,
        after,
    };
    let (staged, rollback) = transition.paths(live)?;
    ensure!(
        !staged.try_exists()? && !rollback.try_exists()?,
        "A save operation already occupies its staging locations"
    );
    preparation::persist(live, &transition)?;
    let result = (|| -> Result<()> {
        tree::copy(bank, &staged, &transition.after, control)?;
        ensure!(
            tree::scan_with_control(live, control)? == transition.before,
            "Live saves changed during preparation"
        );
        control.check()?;
        fs::File::open(live.parent().context("Save parent is missing")?)?.sync_all()?;
        Ok(())
    })();
    if let Err(error) = result {
        preparation::discard(live, operation, transition.source.game_id())
            .with_context(|| format!("{error:#}; save preparation cleanup also failed"))?;
        return Err(error);
    }
    Ok(transition)
}

struct Snapshot {
    directory: PathBuf,
    ownership: staging::Staging,
    before: Option<tree::Tree>,
}

impl Snapshot {
    fn capture(live: &Path, save_set: &SaveSetId, control: &Control) -> Result<Self> {
        let before = tree::scan_with_control(live, control)?;
        let inventory = before.clone().unwrap_or_else(tree::empty);
        let parent = live.parent().context("Save parent is missing")?;
        let ownership = staging::Staging::new(save_set, staging::Purpose::Snapshot, inventory);
        control.check()?;
        let directory = ownership.persist(parent)?;
        let result = (|| -> Result<()> {
            if let Some(before) = &before {
                tree::copy(live, &directory, before, control)?;
            } else {
                use std::os::unix::fs::PermissionsExt;
                fs::create_dir(&directory)?;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
                fs::File::open(&directory)?.sync_all()?;
            }
            control.check()?;
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if let Err(error) = result {
            ownership
                .discard(parent)
                .with_context(|| format!("{error:#}; snapshot cleanup also failed"))?;
            return Err(error);
        }
        Ok(Self {
            directory,
            ownership,
            before,
        })
    }

    fn discard(&self) -> Result<()> {
        self.ownership.discard(
            self.directory
                .parent()
                .context("Snapshot parent is missing")?,
        )
    }

    fn verify(&self, live: &Path, control: &Control) -> Result<()> {
        ensure!(
            tree::scan_with_control(live, control)? == self.before,
            "Live saves changed while preparing save banks; activation stopped"
        );
        Ok(())
    }
}

pub(crate) async fn discard_preparation(game: &Game, operation: &str) -> Result<()> {
    let game = game.clone();
    let operation = operation.to_owned();
    tokio::task::spawn_blocking(move || {
        let live = super::validate_live_save_access(&game)?;
        preparation::discard(&live, &operation, &game.id)
    })
    .await
    .context("Save preparation cleanup worker stopped")?
}

pub(crate) async fn prepare(
    operation: &str,
    game: &Game,
    source: &SaveSetId,
    target: &SaveSetId,
    seed: bool,
    cap_bytes: u64,
    control: Control,
) -> Result<Transition> {
    let runtime = tokio::runtime::Handle::current();
    let operation = operation.to_owned();
    let game = game.clone();
    let source = source.clone();
    let target = target.clone();
    tokio::task::spawn_blocking(move || {
        runtime.block_on(prepare_inner(
            &operation, &game, &source, &target, seed, cap_bytes, control,
        ))
    })
    .await
    .context("Save preparation worker stopped")?
}

async fn prepare_inner(
    operation: &str,
    game: &Game,
    source: &SaveSetId,
    target: &SaveSetId,
    seed: bool,
    cap_bytes: u64,
    control: Control,
) -> Result<Transition> {
    ensure!(
        source != target && source.game_id() == game.id && target.game_id() == game.id,
        "Invalid activation save transition"
    );
    uuid::Uuid::parse_str(operation).context("Invalid activation identity")?;
    let live = super::validate_live_save_access(game)?;
    ensure!(
        !super::game_root(&game.id)?
            .join("transition.json")
            .try_exists()?,
        "Finish the existing save transition before deploying"
    );
    control.check()?;
    let snapshot = Snapshot::capture(&live, source, &control)?;
    let result = async {
        bank::recover(&super::bank_root(source)?, source)?;
        bank::recover(&super::bank_root(target)?, target)?;
        super::migrate_legacy_profile_bank(source).await?;
        super::migrate_legacy_profile_bank(target).await?;
        control.check()?;
        super::create_backup_from_dir(
            source,
            snapshot.directory.as_path(),
            super::BackupTrigger::ProfileSwitch,
            None,
        )
        .await?;
        control.check()?;
        bank::replace(
            &super::bank_root(source)?,
            source,
            snapshot.directory.as_path(),
            &control,
        )?;
        if seed {
            ensure!(
                target.profile_id().is_some(),
                "Only a restored profile can seed an isolated save bank"
            );
            let bank = super::bank_root(target)?;
            if super::bank_manifest(&bank).try_exists()? {
                super::create_backup_from_dir(
                    target,
                    &super::bank_data(&bank),
                    super::BackupTrigger::ProfileSwitch,
                    None,
                )
                .await?;
            }
            control.check()?;
            bank::replace(
                &super::bank_root(target)?,
                target,
                snapshot.directory.as_path(),
                &control,
            )?;
        }
        let bank = super::bank_root(target)?;
        super::load_bank(target).await?;
        super::prune_automatic_backups(&game.id, cap_bytes).await?;
        snapshot.verify(&live, &control)?;
        let transition = stage(
            operation,
            source.clone(),
            target.clone(),
            &live,
            &super::bank_data(&bank),
            &control,
        )?;
        if transition.before != snapshot.before {
            preparation::discard(&live, operation, source.game_id())?;
            anyhow::bail!("Live saves changed after save-bank preparation; activation stopped");
        }
        Ok(transition)
    }
    .await;
    snapshot.discard().with_context(|| match &result {
        Ok(_) => "Snapshot cleanup failed; preparation was preserved".to_owned(),
        Err(error) => format!("{error:#}; snapshot cleanup also failed"),
    })?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(root: &Path) -> Result<(Transition, PathBuf)> {
        let live = root.join("live");
        let bank = root.join("bank");
        fs::create_dir(&live)?;
        fs::create_dir(&bank)?;
        fs::write(live.join("save.dat"), b"current progress")?;
        fs::write(bank.join("save.dat"), b"other profile")?;
        let transition = prepare_staged(
            &uuid::Uuid::new_v4().to_string(),
            SaveSetId::Global {
                game_id: "game".into(),
            },
            SaveSetId::Profile {
                game_id: "game".into(),
                profile_id: "profile".into(),
            },
            &live,
            &bank,
        )?;
        Ok((transition, live))
    }

    // @variants: both
    #[test]
    fn missing_live_saves_produce_an_owned_empty_snapshot() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let live = temp.path().join("missing");
        let snapshot = Snapshot::capture(
            &live,
            &SaveSetId::Global {
                game_id: "game".into(),
            },
            &Control::default(),
        )?;
        assert!(snapshot.before.is_none());
        assert_eq!(fs::read_dir(&snapshot.directory)?.count(), 0);
        snapshot.verify(&live, &Control::default())?;
        snapshot.discard()?;
        assert_eq!(fs::read_dir(temp.path())?.count(), 0);
        Ok(())
    }

    // @variants: both
    #[test]
    fn snapshot_cleanup_preserves_external_changes_and_ownership() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let live = temp.path().join("live");
        fs::create_dir(&live)?;
        fs::write(live.join("save.dat"), b"current")?;
        let snapshot = Snapshot::capture(
            &live,
            &SaveSetId::Global {
                game_id: "game".into(),
            },
            &Control::default(),
        )?;
        fs::write(snapshot.directory.join("save.dat"), b"external")?;
        assert!(snapshot.discard().is_err());
        assert!(staging::read(&snapshot.directory)?.is_some());
        assert_eq!(fs::read(live.join("save.dat"))?, b"current");
        assert_eq!(fs::read(snapshot.directory.join("save.dat"))?, b"external");
        Ok(())
    }

    // @variants: both
    #[test]
    fn frozen_saves_remain_independent_and_reject_later_live_changes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let live = temp.path().join("live");
        fs::create_dir(&live)?;
        fs::write(live.join("save.dat"), b"original progress")?;
        let snapshot = Snapshot::capture(
            &live,
            &SaveSetId::Global {
                game_id: "game".into(),
            },
            &Control::default(),
        )?;
        snapshot.verify(&live, &Control::default())?;
        fs::write(live.join("save.dat"), b"later progress")?;
        assert!(snapshot.verify(&live, &Control::default()).is_err());
        assert_eq!(
            fs::read(snapshot.directory.as_path().join("save.dat"))?,
            b"original progress"
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn cancelled_staging_preserves_live_saves_and_removes_temporary_copies() -> Result<()> {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let temp = tempfile::tempdir()?;
        let live = temp.path().join("live");
        let bank = temp.path().join("bank");
        fs::create_dir(&live)?;
        fs::create_dir(&bank)?;
        fs::write(live.join("save.dat"), b"live")?;
        fs::write(bank.join("save.dat"), b"bank")?;
        let mut control = Control::default();
        let cancelled = control.cancelled.clone();
        let hashed = AtomicUsize::new(0);
        control.progress = Arc::new(move |_, _| {
            if hashed.fetch_add(1, Ordering::AcqRel) == 2 {
                cancelled.store(true, Ordering::Release);
            }
        });
        assert!(
            stage(
                &uuid::Uuid::new_v4().to_string(),
                SaveSetId::Global {
                    game_id: "game".into()
                },
                SaveSetId::Profile {
                    game_id: "game".into(),
                    profile_id: "profile".into()
                },
                &live,
                &bank,
                &control,
            )
            .is_err()
        );
        assert_eq!(fs::read(live.join("save.dat"))?, b"live");
        assert_eq!(fs::read_dir(temp.path())?.count(), 2);
        Ok(())
    }

    // @variants: both
    #[test]
    fn missing_live_saves_return_to_absence_after_rollback() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let live = temp.path().join("live");
        let bank = temp.path().join("bank");
        fs::create_dir(&bank)?;
        fs::write(bank.join("save.dat"), b"progress")?;
        let transition = prepare_staged(
            &uuid::Uuid::new_v4().to_string(),
            SaveSetId::Global {
                game_id: "game".into(),
            },
            SaveSetId::Profile {
                game_id: "game".into(),
                profile_id: "profile".into(),
            },
            &live,
            &bank,
        )?;
        transition.apply_at(&live)?;
        transition.recover_at(&live, false)?;
        transition.recover_at(&live, false)?;
        assert!(!live.exists());
        Ok(())
    }

    // @variants: both
    #[test]
    fn staging_preserves_save_timestamps_and_read_only_permissions() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir()?;
        let live = temp.path().join("live");
        let bank = temp.path().join("bank");
        fs::create_dir(&bank)?;
        let file = bank.join("save.dat");
        fs::write(&file, b"progress")?;
        let timestamp = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_234_567_890);
        fs::File::options()
            .write(true)
            .open(&file)?
            .set_modified(timestamp)?;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o400))?;
        let transition = prepare_staged(
            &uuid::Uuid::new_v4().to_string(),
            SaveSetId::Global {
                game_id: "game".into(),
            },
            SaveSetId::Profile {
                game_id: "game".into(),
                profile_id: "profile".into(),
            },
            &live,
            &bank,
        )?;
        transition.apply_at(&live)?;
        assert_eq!(fs::metadata(live.join("save.dat"))?.modified()?, timestamp);
        assert_eq!(
            fs::metadata(live.join("save.dat"))?.permissions().mode() & 0o777,
            0o400
        );
        transition.recover_at(&live, true)?;
        Ok(())
    }

    // @variants: both
    #[test]
    fn preparation_leaves_live_saves_untouched_and_rollback_is_repeatable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (transition, live) = fixture(temp.path())?;
        assert_eq!(fs::read(live.join("save.dat"))?, b"current progress");
        transition.apply_at(&live)?;
        assert_eq!(fs::read(live.join("save.dat"))?, b"other profile");
        transition.recover_at(&live, false)?;
        transition.recover_at(&live, false)?;
        assert_eq!(fs::read(live.join("save.dat"))?, b"current progress");
        Ok(())
    }

    // @variants: both
    #[test]
    fn interrupted_save_rename_restores_the_original_directory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (transition, live) = fixture(temp.path())?;
        let (_, rollback) = transition.paths(&live)?;
        rename(&live, &rollback)?;
        transition.recover_at(&live, false)?;
        assert_eq!(fs::read(live.join("save.dat"))?, b"current progress");
        Ok(())
    }

    // @variants: both
    #[test]
    fn committed_cleanup_resumes_after_partial_rollback_deletion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (transition, live) = fixture(temp.path())?;
        transition.apply_at(&live)?;
        let (_, rollback) = transition.paths(&live)?;
        fs::remove_file(rollback.join("save.dat"))?;
        transition.recover_at(&live, true)?;
        transition.recover_at(&live, true)?;
        assert_eq!(fs::read(live.join("save.dat"))?, b"other profile");
        Ok(())
    }

    // @variants: both
    #[test]
    fn external_save_edits_block_both_rollback_and_committed_cleanup() -> Result<()> {
        for committed in [false, true] {
            let temp = tempfile::tempdir()?;
            let (transition, live) = fixture(temp.path())?;
            transition.apply_at(&live)?;
            fs::write(live.join("save.dat"), b"external progress")?;
            assert!(transition.recover_at(&live, committed).is_err());
            assert_eq!(fs::read(live.join("save.dat"))?, b"external progress");
            let (_, rollback) = transition.paths(&live)?;
            assert_eq!(fs::read(rollback.join("save.dat"))?, b"current progress");
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn symbolic_links_and_changes_to_staged_saves_block_application() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (transition, live) = fixture(temp.path())?;
        let (staged, _) = transition.paths(&live)?;
        fs::remove_file(staged.join("save.dat"))?;
        std::os::unix::fs::symlink(live.join("save.dat"), staged.join("save.dat"))?;
        assert!(transition.apply_at(&live).is_err());
        assert_eq!(fs::read(live.join("save.dat"))?, b"current progress");
        Ok(())
    }
}
