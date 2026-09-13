use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::core::generations::content::Control;
use crate::core::save_manager::{SAVE_SCHEMA_VERSION, SaveBankManifest, SaveSetId};

use super::staging::{self, Purpose, Staging};
use super::tree::{self, Tree};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    version: u32,
    save_set: SaveSetId,
    staged: String,
    before: Option<Tree>,
    after: Tree,
}

pub(super) fn journal(root: &Path) -> Result<PathBuf> {
    let name = root
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid save bank path")?;
    Ok(root.with_file_name(format!(".{name}.generation-bank.json")))
}

fn sync_parent(path: &Path) -> Result<()> {
    File::open(path.parent().context("Save bank has no parent directory")?)?.sync_all()?;
    Ok(())
}

fn directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(metadata.is_dir(), "Save-bank storage is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            directory(path.parent().context("Save-bank storage has no parent")?)?;
            fs::create_dir(path)?;
            sync_parent(path)?;
        }
        Err(error) => return Err(error).context("Save-bank storage is unavailable"),
    }
    Ok(())
}

impl Publication {
    fn paths(&self, root: &Path, save_set: &SaveSetId) -> Result<(PathBuf, PathBuf)> {
        ensure!(
            self.version == 1 && &self.save_set == save_set,
            "Incompatible save-bank recovery record"
        );
        ensure!(
            self.staged.starts_with(".generation-bank-")
                && self.staged.len() < 100
                && self.staged[1..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "Invalid save-bank staging identity"
        );
        tree::validate(&self.after)?;
        if let Some(before) = &self.before {
            tree::validate(before)?;
        }
        let parent = root.parent().context("Save bank has no parent directory")?;
        ensure!(
            fs::symlink_metadata(parent)?.is_dir(),
            "Save-bank storage is unavailable"
        );
        Ok((
            parent.join(&self.staged),
            parent.join(format!("{}.old", self.staged)),
        ))
    }

    fn finish(&self, root: &Path, save_set: &SaveSetId) -> Result<()> {
        let (staged, old) = self.paths(root, save_set)?;
        staging::adopt_bank(&staged, save_set, &self.after)?;
        let current = tree::scan(root)?;
        if current.as_ref() != Some(&self.after) {
            ensure!(
                tree::scan(&staged)?.as_ref() == Some(&self.after),
                "Prepared save bank changed; recovery data was preserved"
            );
            let preserved = tree::scan(&old)?;
            if current.is_some() {
                ensure!(
                    current == self.before && preserved.is_none(),
                    "External save-bank changes were preserved; recovery is blocked"
                );
                fs::rename(root, &old)?;
                sync_parent(root)?;
            } else {
                ensure!(
                    preserved == self.before,
                    "Original save bank is missing or changed; recovery is blocked"
                );
            }
            fs::rename(&staged, root)?;
            sync_parent(root)?;
        }
        ensure!(
            tree::scan(root)?.as_ref() == Some(&self.after),
            "Save-bank publication failed verification"
        );
        if let Some(before) = &self.before {
            tree::remove(&old, before)?;
        } else {
            ensure!(
                tree::scan(&old)?.is_none(),
                "Unexpected save-bank rollback data was preserved"
            );
        }
        tree::remove(&staged, &self.after)?;
        ensure!(
            tree::scan(root)?.as_ref() == Some(&self.after),
            "Save bank changed during cleanup; recovery record was preserved"
        );
        fs::remove_file(journal(root)?)?;
        sync_parent(root)
    }
}

pub(super) fn recover(root: &Path, save_set: &SaveSetId) -> Result<()> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(journal(root)?)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Cannot read save-bank recovery information"),
    };
    ensure!(
        file.metadata()?.is_file(),
        "Save-bank recovery record is not a regular file"
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let publication: Publication =
        serde_json::from_slice(&bytes).context("Invalid save-bank recovery record")?;
    publication.finish(root, save_set)
}

fn prepare(
    root: &Path,
    save_set: &SaveSetId,
    source: &Path,
    control: &Control,
) -> Result<Publication> {
    control.check()?;
    recover(root, save_set)?;
    let parent = root.parent().context("Save bank has no parent directory")?;
    directory(parent)?;
    ensure!(
        fs::symlink_metadata(parent)?.is_dir(),
        "Save-bank storage is unavailable"
    );
    let before = tree::scan_with_control(root, control)?;
    let inventory = tree::scan_with_control(source, control)?
        .context("Prepared live-save snapshot is missing")?;
    let manifest = SaveBankManifest {
        schema_version: SAVE_SCHEMA_VERSION,
        save_set: save_set.clone(),
        captured_at: chrono::Utc::now().to_rfc3339(),
        files: tree::files(&inventory)?,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    let ownership = Staging::new(
        save_set,
        Purpose::Bank,
        tree::bank_inventory(&inventory, &bytes)?,
    );
    control.check()?;
    let staged = ownership.persist(parent)?;
    let publication = Publication {
        version: 1,
        save_set: save_set.clone(),
        staged: staged
            .file_name()
            .and_then(|name| name.to_str())
            .context("Invalid bank staging path")?
            .to_owned(),
        before,
        after: ownership.inventory.clone(),
    };
    let result = (|| -> Result<()> {
        fs::create_dir(&staged)?;
        fs::set_permissions(&staged, fs::Permissions::from_mode(0o700))?;
        tree::copy(source, &staged.join("data"), &inventory, control)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staged.join("manifest.json"))?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&bytes)?;
        file.set_modified(std::time::UNIX_EPOCH)?;
        file.sync_all()?;
        File::open(&staged)?.sync_all()?;
        sync_parent(&staged)?;
        ensure!(
            tree::scan_with_control(&staged, control)?.as_ref() == Some(&publication.after),
            "Prepared save bank failed verification"
        );
        ensure!(
            tree::scan_with_control(root, control)? == publication.before,
            "Save bank changed during preparation"
        );
        let mut record = tempfile::NamedTempFile::new_in(parent)?;
        record.write_all(&serde_json::to_vec(&publication)?)?;
        record.as_file().sync_all()?;
        control.check()?;
        record
            .persist_noclobber(journal(root)?)
            .context("Cannot publish save-bank recovery record; prepared data was preserved")?;
        sync_parent(root)
    })();
    if let Err(error) = result {
        if !journal(root)?.try_exists()? {
            ownership
                .discard(parent)
                .with_context(|| format!("{error:#}; bank staging cleanup also failed"))?;
        }
        return Err(error);
    }
    Ok(publication)
}

pub(super) fn replace(
    root: &Path,
    save_set: &SaveSetId,
    source: &Path,
    control: &Control,
) -> Result<()> {
    prepare(root, save_set, source, control)?.finish(root, save_set)
}

#[cfg(test)]
pub(super) fn leave_pending(root: &Path, save_set: &SaveSetId, source: &Path) -> Result<()> {
    prepare(root, save_set, source, &Control::default()).map(|_| ())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn set() -> SaveSetId {
        SaveSetId::Profile {
            game_id: "game".into(),
            profile_id: "profile".into(),
        }
    }

    fn fixture(root: &Path) -> Result<(PathBuf, PathBuf)> {
        let bank = root.join("profile");
        let source = root.join("snapshot");
        fs::create_dir(&source)?;
        fs::write(source.join("save.dat"), b"old progress")?;
        replace(&bank, &set(), &source, &Control::default())?;
        fs::write(source.join("save.dat"), b"new progress")?;
        Ok((bank, source))
    }

    // @variants: both
    #[test]
    fn first_bank_publication_recovers_after_storage_root_changes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("snapshot");
        fs::create_dir(&source)?;
        let original = temp.path().join("original");
        let root = original.join("game/profiles/profile");
        prepare(&root, &set(), &source, &Control::default())?;
        let restored = temp.path().join("restored");
        fs::rename(&original, &restored)?;
        let root = restored.join("game/profiles/profile");
        recover(&root, &set())?;
        let manifest: SaveBankManifest =
            serde_json::from_slice(&fs::read(root.join("manifest.json"))?)?;
        assert!(manifest.files.is_empty());
        assert!(root.join("data").is_dir());
        assert!(!journal(&root)?.exists());
        Ok(())
    }

    // @variants: both
    #[test]
    fn interrupted_bank_publication_finishes_at_every_rename_boundary() -> Result<()> {
        for boundary in 0..3 {
            let temp = tempfile::tempdir()?;
            let (bank, source) = fixture(temp.path())?;
            let publication = prepare(&bank, &set(), &source, &Control::default())?;
            let (staged, old) = publication.paths(&bank, &set())?;
            if boundary >= 1 {
                fs::rename(&bank, &old)?;
            }
            if boundary >= 2 {
                fs::rename(&staged, &bank)?;
            }
            recover(&bank, &set())?;
            recover(&bank, &set())?;
            assert_eq!(fs::read(bank.join("data/save.dat"))?, b"new progress");
            assert!(!old.exists() && !staged.exists() && !journal(&bank)?.exists());
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn interrupted_old_bank_cleanup_resumes_without_losing_the_new_bank() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (bank, source) = fixture(temp.path())?;
        let publication = prepare(&bank, &set(), &source, &Control::default())?;
        let (staged, old) = publication.paths(&bank, &set())?;
        fs::rename(&bank, &old)?;
        fs::rename(&staged, &bank)?;
        fs::remove_file(old.join("data/save.dat"))?;
        recover(&bank, &set())?;
        assert_eq!(fs::read(bank.join("data/save.dat"))?, b"new progress");
        assert!(!old.exists());
        Ok(())
    }

    // @variants: both
    #[test]
    fn external_bank_edits_block_recovery_and_preserve_all_copies() -> Result<()> {
        for published in [false, true] {
            let temp = tempfile::tempdir()?;
            let (bank, source) = fixture(temp.path())?;
            let publication = prepare(&bank, &set(), &source, &Control::default())?;
            let (staged, old) = publication.paths(&bank, &set())?;
            if published {
                fs::rename(&bank, &old)?;
                fs::rename(&staged, &bank)?;
            }
            fs::write(bank.join("data/save.dat"), b"external progress")?;
            assert!(recover(&bank, &set()).is_err());
            assert_eq!(fs::read(bank.join("data/save.dat"))?, b"external progress");
            assert!(journal(&bank)?.exists());
            assert!(if published {
                old.exists()
            } else {
                staged.exists()
            });
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn corrupt_staging_and_wrong_save_owners_block_publication() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (bank, source) = fixture(temp.path())?;
        let publication = prepare(&bank, &set(), &source, &Control::default())?;
        let (staged, _) = publication.paths(&bank, &set())?;
        assert!(
            recover(
                &bank,
                &SaveSetId::Global {
                    game_id: "game".into()
                }
            )
            .is_err()
        );
        fs::write(staged.join("data/save.dat"), b"corrupt")?;
        assert!(recover(&bank, &set()).is_err());
        assert_eq!(fs::read(bank.join("data/save.dat"))?, b"old progress");
        assert!(journal(&bank)?.exists());
        Ok(())
    }

    // @variants: both
    #[test]
    fn cancellation_before_publication_preserves_the_existing_bank() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (bank, source) = fixture(temp.path())?;
        let mut control = Control::default();
        let cancelled = control.cancelled.clone();
        let calls = AtomicUsize::new(0);
        control.progress = Arc::new(move |_, _| {
            if calls.fetch_add(1, Ordering::AcqRel) == 3 {
                cancelled.store(true, Ordering::Release);
            }
        });
        assert!(replace(&bank, &set(), &source, &control).is_err());
        assert_eq!(fs::read(bank.join("data/save.dat"))?, b"old progress");
        assert!(!journal(&bank)?.exists());
        assert_eq!(fs::read_dir(temp.path())?.count(), 2);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn published_banks_preserve_read_only_saves_and_legacy_manifest_compatibility()
    -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (bank, source) = fixture(temp.path())?;
        fs::set_permissions(source.join("save.dat"), fs::Permissions::from_mode(0o400))?;
        replace(&bank, &set(), &source, &Control::default())?;
        let manifest: SaveBankManifest =
            serde_json::from_slice(&fs::read(bank.join("manifest.json"))?)?;
        super::super::super::verify_tree(&bank.join("data"), &manifest.files).await?;
        assert_eq!(
            fs::metadata(bank.join("data/save.dat"))?
                .permissions()
                .mode()
                & 0o777,
            0o400
        );
        assert_eq!(manifest.save_set, set());
        Ok(())
    }
}
