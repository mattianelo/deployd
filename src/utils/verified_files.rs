use std::collections::HashMap;
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use anyhow::{Result, anyhow, ensure};
use sha2::{Digest, Sha256};

const CAPACITY: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Stamp {
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    mode: u32,
    links: u64,
}

impl From<&Metadata> for Stamp {
    fn from(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            mode: metadata.mode(),
            links: metadata.nlink(),
        }
    }
}

#[derive(Default)]
struct Cache {
    epoch: u64,
    entries: HashMap<Stamp, String>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

static HASHED_BYTES: AtomicU64 = AtomicU64::new(0);
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static CLONED_BYTES: AtomicU64 = AtomicU64::new(0);
static COPIED_BYTES: AtomicU64 = AtomicU64::new(0);

pub(crate) fn record_copy(bytes: u64) {
    HASHED_BYTES.fetch_add(bytes, Ordering::Relaxed);
    COPIED_BYTES.fetch_add(bytes, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Metrics {
    pub(crate) hashed_bytes: u64,
    pub(crate) cache_hits: u64,
    pub(crate) copied_bytes: u64,
    pub(crate) cloned_bytes: u64,
}

pub(crate) fn metrics() -> Metrics {
    Metrics {
        hashed_bytes: HASHED_BYTES.load(Ordering::Relaxed),
        cache_hits: CACHE_HITS.load(Ordering::Relaxed),
        copied_bytes: COPIED_BYTES.load(Ordering::Relaxed),
        cloned_bytes: CLONED_BYTES.load(Ordering::Relaxed),
    }
}

pub(crate) fn invalidate() -> Result<()> {
    let mut cache = cache()
        .lock()
        .map_err(|_| anyhow!("File verification cache is unavailable"))?;
    cache.epoch = cache.epoch.wrapping_add(1);
    cache.entries.clear();
    Ok(())
}

pub(crate) fn hash(
    file: &mut File,
    mut check: impl FnMut() -> Result<()>,
    mut progress: impl FnMut(u64, u64),
) -> Result<String> {
    check()?;
    let before = file.metadata()?;
    ensure!(
        before.is_file(),
        "File verification requires a regular file"
    );
    let stamp = Stamp::from(&before);
    let (epoch, cached) = {
        let cache = cache()
            .lock()
            .map_err(|_| anyhow!("File verification cache is unavailable"))?;
        (cache.epoch, cache.entries.get(&stamp).cloned())
    };
    if let Some(hash) = cached {
        file.seek(SeekFrom::End(0))?;
        progress(stamp.size, stamp.size);
        check()?;
        ensure!(
            Stamp::from(&file.metadata()?) == stamp,
            "File changed during verification"
        );
        CACHE_HITS.fetch_add(1, Ordering::Relaxed);
        return Ok(hash);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        check()?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        HASHED_BYTES.fetch_add(count as u64, Ordering::Relaxed);
        ensure!(bytes <= stamp.size, "File grew during verification");
        hash.update(&buffer[..count]);
        progress(bytes, stamp.size);
    }
    check()?;
    ensure!(
        bytes == stamp.size && Stamp::from(&file.metadata()?) == stamp,
        "File changed during verification"
    );
    let hash = format!("{:x}", hash.finalize());
    let mut cache = cache()
        .lock()
        .map_err(|_| anyhow!("File verification cache is unavailable"))?;
    if cache.epoch == epoch {
        if cache.entries.len() >= CAPACITY {
            cache.entries.clear();
        }
        cache.entries.insert(stamp, hash.clone());
    }
    Ok(hash)
}

pub(crate) fn try_clone(source: &File, destination: &mut File) -> Result<bool> {
    let before = source.metadata()?;
    let target = destination.metadata()?;
    ensure!(
        before.is_file()
            && target.is_file()
            && target.len() == 0
            && target.nlink() == 1
            && (before.dev(), before.ino()) != (target.dev(), target.ino()),
        "Cloning requires an empty independent destination"
    );
    // FICLONE operates on these owned file descriptors; it does not share writable inodes.
    let result = unsafe { libc::ioctl(destination.as_raw_fd(), libc::FICLONE, source.as_raw_fd()) };
    if result == 0 {
        ensure!(
            Stamp::from(&source.metadata()?) == Stamp::from(&before),
            "Source changed while cloning"
        );
        CLONED_BYTES.fetch_add(before.len(), Ordering::Relaxed);
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(
            libc::EOPNOTSUPP
                | libc::EXDEV
                | libc::EINVAL
                | libc::ENOTTY
                | libc::ENOSYS
                | libc::EPERM
                | libc::EACCES
        )
    ) {
        destination.set_len(0)?;
        destination.seek(SeekFrom::Start(0))?;
        return Ok(false);
    }
    Err(error.into())
}

#[cfg(test)]
mod tests;
