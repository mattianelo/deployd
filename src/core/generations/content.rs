use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub(crate) size: u64,
    pub(crate) sha256: String,
}

impl Identity {
    pub(crate) fn validate(&self) -> Result<()> {
        ensure!(
            self.size <= i64::MAX as u64
                && self.sha256.len() == 64
                && self
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)),
            "Invalid retained content identity"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Control {
    pub(crate) cancelled: Arc<AtomicBool>,
    pub(crate) progress: Arc<dyn Fn(u64, u64) + Send + Sync>,
}

impl Default for Control {
    fn default() -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(|_, _| {}),
        }
    }
}

impl Control {
    pub(crate) fn check(&self) -> Result<()> {
        ensure!(
            !self.cancelled.load(Ordering::Acquire),
            "Deployment preparation cancelled"
        );
        Ok(())
    }
}

fn stamp(metadata: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}

pub(crate) fn inspect(path: &Path, control: &Control) -> Result<Identity> {
    transfer(path, &mut std::io::sink(), control)
}

pub(super) fn transfer(
    source: &Path,
    destination: &mut impl Write,
    control: &Control,
) -> Result<Identity> {
    control.check()?;
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(source)
        .with_context(|| format!("Cannot read '{}'", source.display()))?;
    let before = input.metadata()?;
    ensure!(before.is_file(), "Not a regular file: {}", source.display());
    let mut hasher = Sha256::new();
    let mut size = 0;
    let mut buffer = [0; 128 * 1024];
    loop {
        control.check()?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        destination.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
        size += count as u64;
        (control.progress)(size, before.len());
    }
    let current = fs::symlink_metadata(source)?;
    ensure!(
        current.is_file()
            && stamp(&before) == stamp(&input.metadata()?)
            && stamp(&before) == stamp(&current)
            && size == before.len(),
        "Source changed while preparing '{}'; close tools or updaters and retry",
        source.display()
    );
    let identity = Identity {
        size,
        sha256: format!("{:x}", hasher.finalize()),
    };
    identity.validate()?;
    Ok(identity)
}

pub(crate) fn copy(
    source: &Path,
    destination: &Path,
    expected: &Identity,
    mode: u32,
    control: &Control,
) -> Result<()> {
    expected.validate()?;
    let parent = destination
        .parent()
        .context("Destination has no parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    ensure!(
        &transfer(source, temporary.as_file_mut(), control)? == expected,
        "Source no longer matches prepared content: {}",
        source.display()
    );
    ensure!(mode & !0o777 == 0, "Invalid replacement file permissions");
    temporary
        .as_file()
        .set_permissions(fs::Permissions::from_mode(mode))?;
    temporary.as_file().sync_all()?;
    ensure!(
        &inspect(temporary.path(), control)? == expected,
        "Copied content failed verification"
    );
    control.check()?;
    temporary
        .persist(destination)
        .with_context(|| format!("Cannot publish '{}'", destination.display()))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
