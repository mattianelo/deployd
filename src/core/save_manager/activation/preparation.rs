use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use super::{Transition, tree};

fn path(live: &Path, operation: &str) -> Result<PathBuf> {
    uuid::Uuid::parse_str(operation).context("Invalid save preparation identity")?;
    Ok(live
        .parent()
        .context("Save parent is missing")?
        .join(format!(".deployd-saves-{operation}-preparation.json")))
}

fn sync(live: &Path) -> Result<()> {
    File::open(live.parent().context("Save parent is missing")?)?.sync_all()?;
    Ok(())
}

pub(super) fn read(live: &Path, operation: &str) -> Result<Option<Transition>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path(live, operation)?)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Save preparation record is unavailable"),
    };
    ensure!(
        file.metadata()?.is_file(),
        "Save preparation record is not a regular file"
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let transition: Transition =
        serde_json::from_slice(&bytes).context("Invalid save preparation record")?;
    ensure!(
        transition.operation == operation,
        "Save preparation identity changed"
    );
    transition.paths(live)?;
    Ok(Some(transition))
}

pub(super) fn persist(live: &Path, transition: &Transition) -> Result<()> {
    transition.paths(live)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(live.parent().context("Save parent is missing")?)?;
    temporary.write_all(&serde_json::to_vec(transition)?)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path(live, &transition.operation)?)
        .context("Another save preparation requires recovery")?;
    sync(live)
}

pub(super) fn adopt(live: &Path, transition: &Transition) -> Result<()> {
    if let Some(stored) = read(live, &transition.operation)? {
        ensure!(
            &stored == transition,
            "Save preparation differs from its activation journal; recovery is blocked"
        );
        fs::remove_file(path(live, &transition.operation)?)?;
        sync(live)?;
    }
    Ok(())
}

pub(super) fn discard(live: &Path, operation: &str, game: &str) -> Result<()> {
    let Some(transition) = read(live, operation)? else {
        return Ok(());
    };
    ensure!(
        transition.source.game_id() == game,
        "Save preparation belongs to another game"
    );
    let (staged, rollback) = transition.paths(live)?;
    ensure!(
        tree::scan(&rollback)?.is_none(),
        "Applied saves require activation recovery; preparation was preserved"
    );
    tree::remove_preparation(&staged, &transition.after)?;
    fs::remove_file(path(live, operation)?)?;
    sync(live)
}

#[cfg(test)]
mod tests {
    use crate::core::generations::content::Control;
    use crate::core::save_manager::SaveSetId;

    use super::*;

    fn fixture(root: &Path) -> Result<(PathBuf, Transition)> {
        let live = root.join("live");
        let bank = root.join("bank");
        fs::create_dir(&live)?;
        fs::create_dir(&bank)?;
        fs::write(live.join("save.dat"), b"current")?;
        fs::write(bank.join("save.dat"), b"target")?;
        let transition = super::super::stage(
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
            &Control::default(),
        )?;
        Ok((live, transition))
    }

    // @variants: both
    #[test]
    fn abandoned_preparation_cleanup_preserves_new_live_saves_after_relocation() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let original = temp.path().join("original");
        fs::create_dir(&original)?;
        let (_, transition) = fixture(&original)?;
        let relocated = temp.path().join("relocated");
        fs::rename(&original, &relocated)?;
        let live = relocated.join("live");
        fs::write(live.join("save.dat"), b"new live progress")?;
        discard(&live, &transition.operation, "game")?;
        discard(&live, &transition.operation, "game")?;
        assert_eq!(fs::read(live.join("save.dat"))?, b"new live progress");
        assert!(!transition.paths(&live)?.0.exists());
        assert!(!path(&live, &transition.operation)?.exists());
        Ok(())
    }

    // @variants: both
    #[test]
    fn cleanup_resumes_for_a_partially_copied_recorded_inventory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (live, transition) = fixture(temp.path())?;
        let (staged, _) = transition.paths(&live)?;
        fs::remove_file(staged.join("save.dat"))?;
        discard(&live, &transition.operation, "game")?;
        assert!(!staged.exists());
        assert_eq!(fs::read(live.join("save.dat"))?, b"current");
        Ok(())
    }

    // @variants: both
    #[test]
    fn unknown_staging_contents_block_cleanup_and_preserve_the_record() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (live, transition) = fixture(temp.path())?;
        let (staged, _) = transition.paths(&live)?;
        fs::write(staged.join("unrecognized.tmp"), b"unverified bytes")?;
        assert!(discard(&live, &transition.operation, "game").is_err());
        assert!(path(&live, &transition.operation)?.exists());
        assert_eq!(fs::read(staged.join("save.dat"))?, b"target");
        assert_eq!(
            fs::read(staged.join("unrecognized.tmp"))?,
            b"unverified bytes"
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn activation_adopts_only_its_matching_preparation_without_deleting_staging() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (live, transition) = fixture(temp.path())?;
        let mut other = transition.clone();
        other.target = SaveSetId::Profile {
            game_id: "game".into(),
            profile_id: "other".into(),
        };
        assert!(adopt(&live, &other).is_err());
        assert!(path(&live, &transition.operation)?.exists());
        adopt(&live, &transition)?;
        adopt(&live, &transition)?;
        assert!(!path(&live, &transition.operation)?.exists());
        assert_eq!(
            fs::read(transition.paths(&live)?.0.join("save.dat"))?,
            b"target"
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn a_missing_preparation_record_never_authorizes_staging_deletion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (live, transition) = fixture(temp.path())?;
        fs::remove_file(path(&live, &transition.operation)?)?;
        discard(&live, &transition.operation, "game")?;
        assert_eq!(
            fs::read(transition.paths(&live)?.0.join("save.dat"))?,
            b"target"
        );
        Ok(())
    }
}
