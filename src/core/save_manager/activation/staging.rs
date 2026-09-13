use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::core::save_manager::SaveSetId;

use super::tree::{self, Tree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Purpose {
    Snapshot,
    Bank,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Staging {
    version: u32,
    id: String,
    purpose: Purpose,
    save_set: SaveSetId,
    pub(super) inventory: Tree,
}

fn record(root: &Path) -> PathBuf {
    root.with_extension("preparation.json")
}

fn sync(parent: &Path) -> Result<()> {
    File::open(parent)?.sync_all()?;
    Ok(())
}

impl Staging {
    pub(super) fn owner(&self) -> &SaveSetId {
        &self.save_set
    }

    pub(super) fn purpose(&self) -> Purpose {
        self.purpose
    }

    pub(super) fn new(save_set: &SaveSetId, purpose: Purpose, inventory: Tree) -> Self {
        Self {
            version: 1,
            id: uuid::Uuid::new_v4().to_string(),
            purpose,
            save_set: save_set.clone(),
            inventory,
        }
    }

    pub(super) fn path(&self, parent: &Path) -> Result<PathBuf> {
        ensure!(
            self.version == 1 && uuid::Uuid::parse_str(&self.id).is_ok(),
            "Unsupported save staging record"
        );
        tree::validate(&self.inventory)?;
        let prefix = match self.purpose {
            Purpose::Snapshot => ".deployd-save-snapshot-",
            Purpose::Bank => ".generation-bank-",
        };
        Ok(parent.join(format!("{prefix}{}", self.id)))
    }

    pub(super) fn persist(&self, parent: &Path) -> Result<PathBuf> {
        ensure!(
            fs::symlink_metadata(parent)?.is_dir(),
            "Save staging parent is unavailable"
        );
        let root = self.path(parent)?;
        ensure!(
            fs::symlink_metadata(&root)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
            "Save staging location is occupied or inaccessible"
        );
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(&serde_json::to_vec(self)?)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(record(&root))
            .context("Save staging already requires recovery")?;
        sync(parent)?;
        Ok(root)
    }

    pub(super) fn discard(&self, parent: &Path) -> Result<()> {
        if self.purpose == Purpose::Bank {
            let bank = self.save_set.profile_id().unwrap_or("global");
            super::super::validate_component("save bank", bank)?;
            let publication = super::bank::journal(&parent.join(bank))?;
            ensure!(
                fs::symlink_metadata(publication)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
                "Save bank publication requires recovery before staging cleanup"
            );
        }
        let root = self.path(parent)?;
        let stored =
            read(&root)?.context("Save staging ownership is missing; contents were preserved")?;
        ensure!(
            stored == *self,
            "Save staging ownership changed; contents were preserved"
        );
        tree::remove_preparation(&root, &self.inventory)?;
        fs::remove_file(record(&root))?;
        sync(parent)
    }
}

pub(super) fn read(root: &Path) -> Result<Option<Staging>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(record(root))
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot read save staging ownership"),
    };
    ensure!(
        file.metadata()?.is_file(),
        "Save staging record is not a regular file"
    );
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let staging: Staging = serde_json::from_slice(&bytes).context("Invalid save staging record")?;
    ensure!(
        staging.path(root.parent().context("Save staging parent is missing")?)? == root,
        "Save staging identity differs from its record location"
    );
    Ok(Some(staging))
}

pub(super) fn adopt_bank(root: &Path, save_set: &SaveSetId, inventory: &Tree) -> Result<()> {
    if let Some(staging) = read(root)? {
        ensure!(
            staging.purpose == Purpose::Bank
                && &staging.save_set == save_set
                && &staging.inventory == inventory,
            "Save staging differs from its bank publication; recovery was preserved"
        );
        fs::remove_file(record(root))?;
        sync(root.parent().context("Save staging parent is missing")?)?;
    }
    Ok(())
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

    // @variants: both
    #[test]
    fn pending_bank_publication_protects_its_staging_record() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let staging = Staging::new(&owner(), Purpose::Bank, tree::empty());
        let root = staging.persist(temp.path())?;
        fs::write(
            super::super::bank::journal(&temp.path().join("global"))?,
            b"pending publication",
        )?;
        assert!(staging.discard(temp.path()).is_err());
        assert_eq!(read(&root)?, Some(staging));
        Ok(())
    }

    // @variants: both
    #[test]
    fn staging_ownership_precedes_creation_and_survives_parent_relocation() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let parent = temp.path().join("original");
        fs::create_dir(&parent)?;
        let staging = Staging::new(&owner(), Purpose::Snapshot, tree::empty());
        let root = staging.persist(&parent)?;
        assert!(!root.exists());
        assert_eq!(read(&root)?, Some(staging.clone()));
        let relocated = temp.path().join("relocated");
        fs::rename(&parent, &relocated)?;
        let root = staging.path(&relocated)?;
        let recovered = read(&root)?.context("Missing ownership record")?;
        recovered.discard(&relocated)?;
        assert!(read(&root)?.is_none());
        Ok(())
    }

    // @variants: both
    #[test]
    fn bank_adoption_rejects_another_owner_and_snapshot_intents() -> Result<()> {
        let temp = tempfile::tempdir()?;
        for purpose in [Purpose::Bank, Purpose::Snapshot] {
            let staging = Staging::new(&owner(), purpose, tree::empty());
            let root = staging.persist(temp.path())?;
            let other = SaveSetId::Profile {
                game_id: "game".into(),
                profile_id: "profile".into(),
            };
            assert!(adopt_bank(&root, &other, &staging.inventory).is_err());
            assert!(read(&root)?.is_some());
            if purpose == Purpose::Snapshot {
                assert!(adopt_bank(&root, &owner(), &staging.inventory).is_err());
                staging.discard(temp.path())?;
            } else {
                adopt_bank(&root, &owner(), &staging.inventory)?;
                assert!(read(&root)?.is_none());
            }
        }
        Ok(())
    }

    // @variants: both
    #[test]
    fn changed_or_missing_staging_records_never_authorize_deletion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source");
        fs::create_dir(&source)?;
        fs::write(source.join("save.dat"), b"progress")?;
        let inventory = tree::scan(&source)?.context("Missing source inventory")?;
        let staging = Staging::new(&owner(), Purpose::Snapshot, inventory);
        let root = staging.persist(temp.path())?;
        tree::copy(&source, &root, &staging.inventory, &Control::default())?;
        let mut changed = staging.clone();
        changed.save_set = SaveSetId::Global {
            game_id: "other".into(),
        };
        fs::write(record(&root), serde_json::to_vec(&changed)?)?;
        assert!(staging.discard(temp.path()).is_err());
        fs::remove_file(record(&root))?;
        assert!(staging.discard(temp.path()).is_err());
        assert_eq!(fs::read(root.join("save.dat"))?, b"progress");
        Ok(())
    }
}
