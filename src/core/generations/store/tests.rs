use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;

use super::Store;
use crate::core::generations::content::{self, Control};

// @variants: both
#[test]
fn reuses_verified_content_without_creating_a_staged_copy() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    fs::write(&source, vec![5; 300_000])?;
    let store = Store::create(temp.path(), "game", "store")?;
    let identity = store.retain(&source, &Control::default())?;
    let object = store.source(&identity)?;
    let before = fs::metadata(&object)?;
    let staging = store.root.join("staging");
    let staged = Arc::new(AtomicBool::new(false));
    let observed = staged.clone();
    let control = Control {
        progress: Arc::new(move |_, _| {
            if fs::read_dir(&staging)
                .expect("read staging")
                .next()
                .is_some()
            {
                observed.store(true, Ordering::Release);
            }
        }),
        ..Control::default()
    };
    assert_eq!(
        store.retain_expected(&source, &identity, &control)?,
        identity
    );
    assert!(!staged.load(Ordering::Acquire));
    assert_eq!(fs::metadata(&object)?.ino(), before.ino());
    assert_eq!(fs::metadata(&object)?.nlink(), 1);
    fs::write(&source, b"edited source")?;
    store.verify(&identity, &Control::default())?;
    Ok(())
}

// @variants: both
#[test]
fn rejects_changed_source_even_when_expected_content_is_retained() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    fs::write(&source, b"original")?;
    let store = Store::create(temp.path(), "game", "store")?;
    let control = Control::default();
    let identity = store.retain(&source, &control)?;
    fs::write(&source, b"modified")?;
    let error = store
        .retain_expected(&source, &identity, &control)
        .unwrap_err();
    assert!(error.to_string().contains("Source no longer matches"));
    store.verify(&identity, &control)?;
    assert_eq!(fs::read_dir(store.root.join("objects"))?.count(), 1);
    Ok(())
}

// @variants: both
#[test]
fn rejects_corrupt_retained_content_instead_of_reusing_or_overwriting_it() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    fs::write(&source, b"original")?;
    let store = Store::create(temp.path(), "game", "store")?;
    let control = Control::default();
    let identity = store.retain(&source, &control)?;
    let object = store.source(&identity)?;
    fs::set_permissions(&object, fs::Permissions::from_mode(0o600))?;
    fs::write(&object, b"modified")?;
    let error = store
        .retain_expected(&source, &identity, &control)
        .unwrap_err();
    assert!(error.to_string().contains("damaged"));
    assert_eq!(fs::read(&object)?, b"modified");
    assert_eq!(fs::read_dir(store.root.join("staging"))?.count(), 0);
    Ok(())
}

// @variants: both
#[test]
fn publishes_only_matching_new_content() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    fs::write(&source, b"original")?;
    let store = Store::create(temp.path(), "game", "store")?;
    let control = Control::default();
    let identity = content::inspect(&source, &control)?;
    fs::write(&source, b"modified")?;
    assert!(store.retain_expected(&source, &identity, &control).is_err());
    assert_eq!(fs::read_dir(store.root.join("objects"))?.count(), 0);
    assert_eq!(fs::read_dir(store.root.join("staging"))?.count(), 0);
    fs::write(&source, b"original")?;
    assert_eq!(
        store.retain_expected(&source, &identity, &control)?,
        identity
    );
    store.verify(&identity, &control)?;
    Ok(())
}

// @variants: both
#[test]
fn rejects_cancellation_during_retained_content_verification() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source");
    fs::write(&source, vec![5; 300_000])?;
    let store = Store::create(temp.path(), "game", "store")?;
    let identity = store.retain(&source, &Control::default())?;
    let cancelled = Arc::new(AtomicBool::new(false));
    let signal = cancelled.clone();
    let control = Control {
        cancelled,
        progress: Arc::new(move |_, _| signal.store(true, Ordering::Release)),
        ..Control::default()
    };
    assert!(store.retain_expected(&source, &identity, &control).is_err());
    store.verify(&identity, &Control::default())?;
    assert_eq!(fs::read_dir(store.root.join("staging"))?.count(), 0);
    Ok(())
}
