use std::fs::{self, FileTimes};
use std::os::unix::fs::{PermissionsExt, symlink};

use super::*;
use crate::core::generations::content::{self, Control};

// @variants: both
#[test]
fn unchanged_files_avoid_reads_but_a_forty_megabyte_edit_is_rehashed() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let unchanged = temp.path().join("large");
    let edited = temp.path().join("edited");
    fs::write(&unchanged, vec![1; 64 * 1024 * 1024])?;
    fs::write(&edited, vec![2; 40 * 1024 * 1024])?;
    let control = Control::default();
    content::inspect(&unchanged, &control)?;
    let original = content::inspect(&edited, &control)?;
    let before = metrics();
    for _ in 0..7 {
        content::inspect(&unchanged, &control)?;
        assert_eq!(content::inspect(&edited, &control)?, original);
    }
    assert_eq!(metrics().hashed_bytes - before.hashed_bytes, 0);
    assert_eq!(metrics().cache_hits - before.cache_hits, 14);
    let modified = fs::metadata(&edited)?.modified()?;
    fs::write(&edited, vec![3; 40 * 1024 * 1024])?;
    File::options()
        .write(true)
        .open(&edited)?
        .set_times(FileTimes::new().set_modified(modified))?;
    let before = metrics();
    content::inspect(&unchanged, &control)?;
    assert_ne!(content::inspect(&edited, &control)?, original);
    assert_eq!(
        metrics().hashed_bytes - before.hashed_bytes,
        40 * 1024 * 1024
    );
    Ok(())
}

// @variants: both
#[test]
fn replacement_links_permissions_and_cancellation_do_not_bypass_verification() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("file");
    fs::write(&path, b"first")?;
    let control = Control::default();
    let first = content::inspect(&path, &control)?;
    let replacement = temp.path().join("replacement");
    fs::write(&replacement, b"other")?;
    fs::rename(&replacement, &path)?;
    assert_ne!(content::inspect(&path, &control)?, first);
    let before = metrics();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400))?;
    content::inspect(&path, &control)?;
    assert_eq!(metrics().hashed_bytes - before.hashed_bytes, 5);
    control.cancelled.store(true, Ordering::Release);
    assert!(content::inspect(&path, &control).is_err());
    control.cancelled.store(false, Ordering::Release);
    let link = temp.path().join("link");
    symlink(&path, &link)?;
    assert!(content::inspect(&link, &control).is_err());
    fs::remove_file(&path)?;
    assert!(content::inspect(&path, &control).is_err());
    Ok(())
}

// @variants: both
#[test]
fn full_verification_discards_previous_hashes_and_rejects_changes_during_reads() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("file");
    fs::write(&path, vec![1; 256 * 1024])?;
    let control = Control::default();
    content::inspect(&path, &control)?;
    let stamp = Stamp::from(&fs::metadata(&path)?);
    cache()
        .lock()
        .unwrap()
        .entries
        .insert(stamp, "0".repeat(64));
    invalidate()?;
    let before = metrics();
    assert_ne!(content::inspect(&path, &control)?.sha256, "0".repeat(64));
    assert_eq!(metrics().hashed_bytes - before.hashed_bytes, 256 * 1024);
    invalidate()?;
    let mut changed = false;
    let result = hash(
        &mut File::open(&path)?,
        || Ok(()),
        |_, _| {
            if !changed {
                fs::write(&path, b"changed").unwrap();
                changed = true;
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(content::inspect(&path, &control)?.size, 7);
    Ok(())
}

// @variants: both
#[test]
fn independent_copy_preserves_source_and_existing_destination_on_failure() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    let target = temp.path().join("target");
    fs::write(&source, vec![7; 256 * 1024])?;
    let control = Control::default();
    let identity = content::inspect(&source, &control)?;
    content::copy(&source, &target, &identity, 0o600, &control)?;
    assert_ne!(fs::metadata(&source)?.ino(), fs::metadata(&target)?.ino());
    assert_eq!(fs::metadata(&target)?.nlink(), 1);
    fs::write(&target, b"independent")?;
    assert_eq!(content::inspect(&source, &control)?, identity);
    fs::write(&source, b"changed")?;
    assert!(content::copy(&source, &target, &identity, 0o600, &control).is_err());
    assert_eq!(fs::read(&target)?, b"independent");
    control.cancelled.store(true, Ordering::Release);
    assert!(content::copy(&source, &target, &identity, 0o600, &control).is_err());
    assert_eq!(fs::read(&target)?, b"independent");
    Ok(())
}

// @variants: both
#[test]
fn unsupported_and_cross_device_clones_fall_back_to_verified_copies() -> Result<()> {
    let memory = tempfile::tempdir_in("/dev/shm")?;
    let disk = tempfile::tempdir()?;
    let source = memory.path().join("source");
    fs::write(&source, b"independent content")?;
    let control = Control::default();
    let expected = content::inspect(&source, &control)?;
    for root in [memory.path(), disk.path()] {
        let target = root.join("target");
        let before = metrics();
        content::copy(&source, &target, &expected, 0o640, &control)?;
        assert_eq!(content::inspect(&target, &control)?, expected);
        assert_eq!(metrics().copied_bytes - before.copied_bytes, expected.size);
        assert_eq!(metrics().cloned_bytes, before.cloned_bytes);
        assert_eq!(fs::metadata(&target)?.permissions().mode() & 0o777, 0o640);
        fs::write(&target, b"changed")?;
        assert_eq!(content::inspect(&source, &control)?, expected);
    }
    Ok(())
}
