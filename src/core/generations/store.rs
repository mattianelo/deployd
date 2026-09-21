use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::content::{self, Control, Identity};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u32,
    game: String,
    id: String,
}

pub(super) struct Store {
    root: PathBuf,
    directory: File,
    marker: Marker,
}

fn directory(path: &Path) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| {
            format!(
                "History storage '{}' is unavailable; restore access to the configured cache",
                path.display()
            )
        })
}

fn marker(root: &Path) -> Result<Marker> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(root.join("store.json"))?;
    ensure!(
        file.metadata()?.is_file() && file.metadata()?.len() <= 4096,
        "Invalid history store marker"
    );
    serde_json::from_reader(file.take(4097)).context("Cannot read history store identity")
}

impl Store {
    pub(super) fn create(cache: &Path, game: &str, id: &str) -> Result<Self> {
        let root = crate::utils::paths::generation_store_in(cache, game)?;
        let cache_directory = directory(cache)?;
        let parent = root.parent().context("History store has no parent")?;
        match fs::create_dir(parent) {
            Ok(()) => cache_directory.sync_all()?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error).context("Cannot create deployment history directory"),
        }
        let parent_directory = directory(parent)?;
        if root.try_exists()? {
            return Self::open(cache, game, id);
        }
        let temporary = tempfile::Builder::new()
            .prefix(".initialize-")
            .tempdir_in(parent)?;
        fs::set_permissions(temporary.path(), fs::Permissions::from_mode(0o700))?;
        fs::create_dir(temporary.path().join("objects"))?;
        fs::create_dir(temporary.path().join("staging"))?;
        let marker = Marker {
            version: 1,
            game: game.to_owned(),
            id: id.to_owned(),
        };
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary.path().join("store.json"))?;
        file.write_all(&serde_json::to_vec(&marker)?)?;
        file.sync_all()?;
        directory(&temporary.path().join("objects"))?.sync_all()?;
        directory(&temporary.path().join("staging"))?.sync_all()?;
        directory(temporary.path())?.sync_all()?;
        fs::rename(temporary.path(), &root).context("Cannot publish history store")?;
        parent_directory.sync_all()?;
        Self::open(cache, game, id)
    }

    pub(super) fn open(cache: &Path, game: &str, id: &str) -> Result<Self> {
        let root = crate::utils::paths::generation_store_in(cache, game)?;
        directory(cache)?;
        directory(root.parent().context("History store has no parent")?)?;
        let dir = directory(&root)?;
        let marker = marker(&root)?;
        ensure!(
            marker.version == 1 && marker.game == game && marker.id == id,
            "This cache contains a different history store; restore access to the original store"
        );
        directory(&root.join("objects"))?;
        directory(&root.join("staging"))?;
        Ok(Self {
            root,
            directory: dir,
            marker,
        })
    }

    fn check(&self) -> Result<()> {
        let original = self.directory.metadata()?;
        let current = directory(&self.root)?.metadata()?;
        ensure!(
            original.dev() == current.dev() && original.ino() == current.ino(),
            "History location changed during the operation"
        );
        ensure!(
            marker(&self.root)? == self.marker,
            "History store identity changed during the operation"
        );
        Ok(())
    }

    pub(super) fn source(&self, identity: &Identity) -> Result<PathBuf> {
        identity.validate()?;
        self.check()?;
        directory(&self.root.join("objects"))?;
        let object = self.root.join("objects").join(&identity.sha256);
        directory(&object)?;
        Ok(object.join("content"))
    }

    pub(super) fn verify(&self, identity: &Identity, control: &Control) -> Result<()> {
        ensure!(
            &content::inspect(&self.source(identity)?, control)? == identity,
            "Retained content {} is damaged; restoration is unavailable",
            identity.sha256
        );
        self.check()
    }

    pub(super) fn contains(&self, identity: &Identity) -> Result<bool> {
        identity.validate()?;
        self.check()?;
        directory(&self.root.join("objects"))?;
        let object = self.root.join("objects").join(&identity.sha256);
        let metadata = match fs::symlink_metadata(&object) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        ensure!(metadata.is_dir(), "Invalid retained content directory");
        let content = object.join("content");
        let metadata = match fs::symlink_metadata(content) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        Ok(metadata.is_file() && metadata.len() == identity.size)
    }

    pub(super) fn retain(&self, source: &Path, control: &Control) -> Result<Identity> {
        self.retain_checked(source, None, control)
    }

    pub(super) fn retain_expected(
        &self,
        source: &Path,
        expected: &Identity,
        control: &Control,
    ) -> Result<Identity> {
        control.check()?;
        expected.validate()?;
        if self.contains(expected)? {
            ensure!(
                content::inspect(source, control)? == *expected,
                "Source no longer matches prepared content: {}",
                source.display()
            );
            self.verify(expected, control)?;
            control.check()?;
            return Ok(expected.clone());
        }
        self.retain_checked(source, Some(expected), control)
    }

    fn retain_checked(
        &self,
        source: &Path,
        expected: Option<&Identity>,
        control: &Control,
    ) -> Result<Identity> {
        self.check()?;
        let staging = self.root.join("staging");
        directory(&staging)?;
        let temporary = tempfile::Builder::new()
            .prefix(".prepare-")
            .tempdir_in(staging)?;
        let path = temporary.path().join("content");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let identity = content::transfer(source, &mut file, control)?;
        ensure!(
            expected.is_none_or(|expected| *expected == identity),
            "Source no longer matches prepared content: {}",
            source.display()
        );
        self.publish(temporary, file, identity, control)
    }

    pub(super) fn retain_generated(&self, bytes: &[u8], control: &Control) -> Result<Identity> {
        self.check()?;
        control.check()?;
        let staging = self.root.join("staging");
        directory(&staging)?;
        let temporary = tempfile::Builder::new()
            .prefix(".prepare-")
            .tempdir_in(staging)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temporary.path().join("content"))?;
        let mut hash = Sha256::new();
        let mut size = 0;
        for chunk in bytes.chunks(128 * 1024) {
            control.check()?;
            file.write_all(chunk)?;
            hash.update(chunk);
            size += chunk.len() as u64;
            (control.progress)(size, bytes.len() as u64);
        }
        control.check()?;
        let identity = Identity {
            size,
            sha256: format!("{:x}", hash.finalize()),
        };
        identity.validate()?;
        self.publish(temporary, file, identity, control)
    }

    fn publish(
        &self,
        temporary: tempfile::TempDir,
        file: File,
        identity: Identity,
        control: &Control,
    ) -> Result<Identity> {
        let path = temporary.path().join("content");
        file.set_permissions(fs::Permissions::from_mode(0o400))?;
        file.sync_all()?;
        ensure!(
            content::inspect(&path, control)? == identity,
            "Retained copy failed verification"
        );
        directory(temporary.path())?.sync_all()?;
        let objects = self.root.join("objects");
        let objects_directory = directory(&objects)?;
        let destination = objects.join(&identity.sha256);
        if destination.try_exists()? {
            self.verify(&identity, control)?;
            return Ok(identity);
        }
        self.check()?;
        control.check()?;
        match fs::rename(temporary.path(), &destination) {
            Ok(()) => objects_directory.sync_all()?,
            Err(error)
                if error.kind() == std::io::ErrorKind::DirectoryNotEmpty
                    || error.kind() == std::io::ErrorKind::AlreadyExists =>
            {
                self.verify(&identity, control)?
            }
            Err(error) => return Err(error).context("Cannot publish retained content"),
        }
        self.verify(&identity, control)?;
        Ok(identity)
    }

    pub(super) fn materialize(
        &self,
        identity: &Identity,
        destination: &Path,
        mode: u32,
        control: &Control,
    ) -> Result<()> {
        content::copy(
            &self.source(identity)?,
            destination,
            identity,
            mode,
            control,
        )?;
        self.check()
    }

    pub(super) fn remove(&self, identity: &Identity) -> Result<()> {
        identity.validate()?;
        self.check()?;
        let root = self.root.join("objects").join(&identity.sha256);
        if !root.try_exists()? {
            return Ok(());
        }
        directory(&root)?;
        let content = root.join("content");
        if content.try_exists()? {
            self.verify(identity, &Control::default())?;
            fs::remove_file(content)?;
            directory(&root)?.sync_all()?;
        }
        fs::remove_dir(&root).context("Unexpected files in retained object; deletion stopped")?;
        directory(&self.root.join("objects"))?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
