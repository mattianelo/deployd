use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::super::operation::Control;
use super::catalog::Artifact;
use super::{Selection, artifacts};

pub(in crate::core::game::mass_effect) async fn seed(
    data: &Path,
    selections: &[Selection],
) -> Result<()> {
    let cache = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("out/mele-test-cache");
    for selection in selections {
        seed_artifact(&cache, data, selection.component.artifact()).await?;
    }
    Ok(())
}

async fn seed_artifact(cache: &Path, data: &Path, spec: Artifact) -> Result<()> {
    let lock = {
        let cache = cache.to_owned();
        tokio::task::spawn_blocking(move || -> Result<File> {
            std::fs::create_dir_all(&cache)?;
            let file = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(cache.join("download.lock"))?;
            file.lock()?;
            Ok(file)
        })
        .await??
    };
    let hash = spec.sha256;
    let bytes = artifacts::load(
        spec,
        cache.to_owned(),
        Control::recovery(),
        Arc::new(|_, _| {}),
    )
    .await?;
    drop(lock);
    let destination = data.join("mele-components/artifacts");
    tokio::task::spawn_blocking(move || {
        artifacts::write(&destination, hash, &bytes, &Control::recovery())
    })
    .await?
}

// @variants: both
#[tokio::test]
async fn cached_test_artifacts_survive_fixture_changes_and_reject_corruption() -> Result<()> {
    let cache = tempfile::tempdir()?;
    let first = tempfile::tempdir()?;
    let second = tempfile::tempdir()?;
    let hash = "d92c6a81b2ff50096bcda80885427d1f59a25b5f483f7055523504925d16ab23";
    let relative = format!("mele-components/artifacts/{hash}");
    let spec = || Artifact {
        url: "https://invalid.invalid/runtime",
        size: 7,
        sha256: hash,
    };
    artifacts::write(cache.path(), &relative, b"runtime", &Control::recovery())?;
    let (a, b) = tokio::join!(
        seed_artifact(cache.path(), first.path(), spec()),
        seed_artifact(cache.path(), second.path(), spec())
    );
    a?;
    b?;
    std::fs::write(first.path().join(&relative), b"changed")?;
    assert_eq!(std::fs::read(second.path().join(&relative))?, b"runtime");
    assert_eq!(std::fs::read(cache.path().join(&relative))?, b"runtime");
    drop(first);
    drop(second);
    let next = tempfile::tempdir()?;
    seed_artifact(cache.path(), next.path(), spec()).await?;
    assert_eq!(std::fs::read(next.path().join(&relative))?, b"runtime");
    std::fs::write(cache.path().join(&relative), b"corrupt")?;
    let rejected = tempfile::tempdir()?;
    assert!(
        seed_artifact(cache.path(), rejected.path(), spec())
            .await
            .is_err()
    );
    assert!(!rejected.path().join(&relative).exists());
    assert_eq!(std::fs::read(cache.path().join(relative))?, b"corrupt");
    Ok(())
}
