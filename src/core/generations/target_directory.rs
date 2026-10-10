use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, anyhow, ensure};

#[path = "directory_notifications.rs"]
mod notifications;

use notifications::Notifications;

const MAX_NAMES: usize = 262_144;
const MAX_DIRECTORIES: usize = 4096;

#[derive(PartialEq, Eq)]
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
    fn from(value: &Metadata) -> Self {
        Self {
            device: value.dev(),
            inode: value.ino(),
            size: value.size(),
            modified: (value.mtime(), value.mtime_nsec()),
            changed: (value.ctime(), value.ctime_nsec()),
            mode: value.mode(),
            links: value.nlink(),
        }
    }
}

#[derive(Default)]
struct Listing {
    names: BTreeMap<String, Vec<OsString>>,
    count: usize,
}

struct Cache {
    entries: BTreeMap<PathBuf, (Stamp, Arc<Listing>)>,
    order: VecDeque<PathBuf>,
    names: usize,
    notifications: Option<Notifications>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            names: 0,
            notifications: Notifications::new().ok(),
        }
    }
}

impl Cache {
    fn refresh(&mut self) {
        match self.notifications.as_mut().map(Notifications::changed) {
            Some(Ok(false)) | None => {}
            Some(Ok(true)) => *self = Self::default(),
            Some(Err(_)) => {
                *self = Self::default();
                self.notifications = None;
            }
        }
    }

    fn remove(&mut self, path: &Path) {
        if let Some((_, listing)) = self.entries.remove(path) {
            self.names -= listing.count;
            self.order.retain(|entry| entry != path);
        }
    }

    fn listing(&mut self, path: &Path, expected: Option<&str>) -> Result<Arc<Listing>> {
        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("Cannot inspect deployment parent '{}'", path.display()))?;
        ensure!(
            metadata.is_dir(),
            "Deployment parent is redirected or is not a directory: {}",
            path.display()
        );
        let stamp = Stamp::from(&metadata);
        self.refresh();
        if let Some((previous, listing)) = self.entries.get(path)
            && *previous == stamp
        {
            return Ok(Arc::clone(listing));
        }
        self.remove(path);
        if self
            .notifications
            .as_ref()
            .is_some_and(|notifications| notifications.len() >= MAX_DIRECTORIES)
        {
            *self = Self::default();
        }
        let watched = self
            .notifications
            .as_mut()
            .is_some_and(|notifications| notifications.watch(path).unwrap_or(false));
        let mut listing = Listing::default();
        for entry in fs::read_dir(path)
            .with_context(|| format!("Cannot inspect deployment directory '{}'", path.display()))?
        {
            let name = entry?.file_name();
            let key = name.to_string_lossy().to_lowercase();
            if watched || expected.is_none_or(|expected| expected == key) {
                listing.names.entry(key).or_default().push(name);
            }
            listing.count += 1;
        }
        let after = fs::symlink_metadata(path)?;
        ensure!(
            after.is_dir() && Stamp::from(&after) == stamp,
            "Deployment directory changed while resolving filenames: {}; retry deployment",
            path.display()
        );
        if watched
            && !self
                .notifications
                .as_mut()
                .is_some_and(|notifications| matches!(notifications.changed(), Ok(false)))
        {
            *self = Self::default();
            anyhow::bail!("Deployment folders changed while resolving filenames; retry deployment");
        }
        let listing = Arc::new(listing);
        if watched && listing.count <= MAX_NAMES {
            while self.names + listing.count > MAX_NAMES || self.entries.len() >= MAX_DIRECTORIES {
                let Some(oldest) = self.order.front().cloned() else {
                    break;
                };
                self.remove(&oldest);
            }
            self.names += listing.count;
            self.order.push_back(path.to_owned());
            self.entries
                .insert(path.to_owned(), (stamp, Arc::clone(&listing)));
        }
        Ok(listing)
    }
}

pub(super) fn matching(path: &Path, expected: &str) -> Result<Vec<PathBuf>> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let listing = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .map_err(|_| anyhow!("Deployment directory cache is unavailable"))?
        .listing(path, Some(expected))?;
    Ok(listing
        .names
        .get(expected)
        .into_iter()
        .flatten()
        .map(|name| path.join(name))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn reuses_unchanged_listings_and_refreshes_added_removed_and_renamed_names() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("Example.dds"), b"one")?;
        let mut cache = Cache::default();
        let first = cache.listing(temp.path(), None)?;
        assert!(Arc::ptr_eq(&first, &cache.listing(temp.path(), None)?));
        fs::write(temp.path().join("example.dds"), b"two")?;
        assert_eq!(
            cache.listing(temp.path(), None)?.names["example.dds"].len(),
            2
        );
        fs::remove_file(temp.path().join("Example.dds"))?;
        assert_eq!(
            cache.listing(temp.path(), None)?.names["example.dds"].len(),
            1
        );
        fs::rename(
            temp.path().join("example.dds"),
            temp.path().join("Other.dds"),
        )?;
        let renamed = cache.listing(temp.path(), None)?;
        assert!(!renamed.names.contains_key("example.dds"));
        assert!(renamed.names.contains_key("other.dds"));
        Ok(())
    }

    // @variants: both
    #[test]
    fn rejects_redirected_and_missing_cached_directories() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let parent = temp.path().join("parent");
        let moved = temp.path().join("moved");
        fs::create_dir(&parent)?;
        let mut cache = Cache::default();
        cache.listing(&parent, None)?;
        fs::rename(&parent, &moved)?;
        assert!(cache.listing(&parent, None).is_err());
        std::os::unix::fs::symlink(&moved, &parent)?;
        assert!(cache.listing(&parent, None).is_err());
        fs::remove_file(&parent)?;
        fs::create_dir(&parent)?;
        fs::write(parent.join("new"), b"new")?;
        assert!(cache.listing(&parent, None)?.names.contains_key("new"));
        Ok(())
    }

    // @variants: both
    #[test]
    fn bounds_cached_directories_and_reloads_evicted_entries() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut cache = Cache::default();
        let first = cache.listing(temp.path(), None)?;
        for index in 0..MAX_DIRECTORIES {
            let path = temp.path().join(index.to_string());
            fs::create_dir(&path)?;
            cache.listing(&path, None)?;
        }
        assert!(cache.entries.len() <= MAX_DIRECTORIES);
        assert!(cache.order.len() <= MAX_DIRECTORIES);
        assert!(!cache.entries.contains_key(temp.path()));
        let reloaded = cache.listing(temp.path(), None)?;
        assert!(!Arc::ptr_eq(&first, &reloaded));
        assert_eq!(reloaded.count, MAX_DIRECTORIES);
        assert_eq!(cache.names, MAX_DIRECTORIES);
        Ok(())
    }

    // @variants: both
    #[test]
    fn revoked_directory_permissions_prevent_cached_lookup() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("Example.dds"), b"texture")?;
        let mut cache = Cache::default();
        cache.listing(temp.path(), None)?;
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o000))?;
        let denied = cache.listing(temp.path(), None);
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
        assert!(denied.is_err());
        assert!(
            cache
                .listing(temp.path(), None)?
                .names
                .contains_key("example.dds")
        );
        Ok(())
    }

    // @variants: both
    #[test]
    fn scans_each_time_when_change_notifications_are_unavailable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("Old"), [])?;
        let mut cache = Cache {
            notifications: None,
            ..Cache::default()
        };
        let first = cache.listing(temp.path(), None)?;
        assert!(!Arc::ptr_eq(&first, &cache.listing(temp.path(), None)?));
        fs::rename(temp.path().join("Old"), temp.path().join("New"))?;
        let renamed = cache.listing(temp.path(), None)?;
        assert!(renamed.names.contains_key("new"));
        assert!(!renamed.names.contains_key("old"));
        assert!(cache.entries.is_empty());
        Ok(())
    }

    // @variants: both
    #[test]
    fn detects_rapid_renames_even_with_coarse_directory_timestamps() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("a"), [])?;
        let mut cache = Cache::default();
        cache.listing(temp.path(), None)?;
        for _ in 0..64 {
            for (before, after) in [("a", "b"), ("b", "a")] {
                fs::rename(temp.path().join(before), temp.path().join(after))?;
                let current = cache.listing(temp.path(), None)?;
                assert!(current.names.contains_key(after));
                assert!(!current.names.contains_key(before));
            }
        }
        Ok(())
    }

    // @variants: both
    #[test]
    #[ignore = "manual directory lookup benchmark"]
    fn measures_repeated_lookups_in_a_large_directory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        for index in 0..3100 {
            fs::write(temp.path().join(format!("Texture{index}.dds")), [])?;
        }
        let expected = "texture3099.dds";
        let expected_path = temp.path().join("Texture3099.dds");
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            let mut found = Vec::new();
            for entry in fs::read_dir(temp.path())? {
                let entry = entry?;
                if entry.file_name().to_string_lossy().to_lowercase() == expected {
                    found.push(entry.path());
                }
            }
            assert_eq!(found, vec![expected_path.clone()]);
        }
        let uncached = started.elapsed();
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            assert_eq!(
                matching(temp.path(), expected)?,
                vec![expected_path.clone()]
            );
        }
        eprintln!(
            "1000 lookups / 3100 entries: directory scans {uncached:?}, cached lookups {:?}",
            started.elapsed()
        );
        Ok(())
    }
}
