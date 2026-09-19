use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};

use super::{Journal, Node, Target, inspect};
use crate::core::generations::catalog::History;
use crate::core::generations::content::Control;
use crate::core::generations::target::relative;
use crate::models::game::Game;

impl Journal {
    pub(in crate::core::generations) async fn bind_cache(
        &mut self,
        history: &History,
        game: &Game,
        sources: BTreeMap<Target, String>,
        control: Control,
    ) -> Result<()> {
        let mut prepared = self.clone();
        prepared.version = prepared.version.max(5);
        prepared.cache_sources = sources.into_iter().collect();
        prepared.validate(game)?;
        let verifying = prepared.clone();
        let cache = history.cache.clone();
        let store = history.store.clone();
        history
            .lease
            .blocking(move || verifying.verify_cache(&cache, &store, &control))
            .await
            .context("Cache source verification worker stopped")??;
        *self = prepared;
        Ok(())
    }

    pub(super) fn validate_cache(&self) -> Result<()> {
        ensure!(
            self.cache_sources.is_empty() || self.version >= 5,
            "Cache sources require their versioned activation journal"
        );
        let mut seen = std::collections::BTreeSet::new();
        for (target, source) in &self.cache_sources {
            ensure!(seen.insert(target), "Duplicate cache source target");
            let path = relative(source)?;
            ensure!(
                path.components().count() >= 2 && !source.starts_with("deployd-history/"),
                "Activation cache source must belong to a writable mod directory"
            );
            ensure!(
                self.changes
                    .iter()
                    .any(|change| &change.target == target
                        && matches!(change.after, Node::File { .. })),
                "Cache binding does not identify a prepared file"
            );
        }
        Ok(())
    }

    pub(super) fn verify_cache(
        &self,
        cache: &Path,
        store: &crate::core::generations::store::Store,
        control: &Control,
    ) -> Result<()> {
        for (target, source) in &self.cache_sources {
            let expected = &self
                .changes
                .iter()
                .find(|change| &change.target == target)
                .context("Cache binding lost its prepared file")?
                .after;
            verified_source(cache, source, expected, store, control)?;
        }
        Ok(())
    }

    pub(super) fn apply_cache(
        &self,
        cache: &Path,
        store: &crate::core::generations::store::Store,
        target: &Target,
        destination: &Path,
        expected: &Node,
        control: &Control,
    ) -> Result<bool> {
        if !matches!(expected, Node::File { .. }) {
            return Ok(false);
        }
        let Some(source) = self
            .cache_sources
            .iter()
            .find_map(|(bound, source)| (bound == target).then_some(source))
        else {
            return Ok(false);
        };
        let source = verified_source(cache, source, expected, store, control)?;
        let parent = destination
            .parent()
            .context("Deployment file has no parent")?;
        let temporary = tempfile::Builder::new()
            .prefix(".deployd-link-")
            .tempdir_in(parent)?;
        let staged = temporary.path().join("content");
        match fs::hard_link(&source, &staged) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(libc::EXDEV) => return Ok(false),
            Err(error) => return Err(error).context("Cannot stage a cache hardlink"),
        }
        ensure!(
            inspect(&staged, control)? == *expected,
            "Cache source changed while staging deployment; prepare again"
        );
        fs::File::open(&staged)?.sync_all()?;
        fs::rename(&staged, destination)?;
        fs::File::open(parent)?.sync_all()?;
        Ok(true)
    }
}

fn verified_source(
    cache: &Path,
    source: &str,
    expected: &Node,
    store: &crate::core::generations::store::Store,
    control: &Control,
) -> Result<PathBuf> {
    let relative = relative(source)?;
    let mut path = cache.to_owned();
    ensure!(
        fs::symlink_metadata(&path)?.is_dir(),
        "Configured cache is unavailable"
    );
    for component in relative.components() {
        path.push(component);
        ensure!(
            !fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "Cache source contains a symbolic link; restore its writable mod copy"
        );
    }
    ensure!(
        inspect(&path, control)? == *expected,
        "Cache content changed after preparation; prepare deployment again"
    );
    let Node::File { identity, .. } = expected else {
        anyhow::bail!("Cache binding is not a file");
    };
    store.verify(identity, control)?;
    let retained = fs::metadata(store.source(identity)?)?;
    let writable = fs::metadata(&path)?;
    ensure!(
        (retained.dev(), retained.ino()) != (writable.dev(), writable.ino()),
        "Retained history cannot be linked into writable deployment paths"
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[tokio::test]
    async fn cache_hardlinks_preserve_history_and_recover_previous_bytes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (tracker, game, _) =
            crate::core::generations::tests::snapshot_fixture(temp.path()).await?;
        fs::create_dir_all(game.data_dir())?;
        let live = game.data_dir().join("File.txt");
        fs::write(&live, b"original")?;
        let cache = temp.path().join("winner/file.txt");
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let identity = history.retain(cache.clone(), Control::default()).await?;
        let target = Target::file(&game.engine, "File.txt")?;
        let mut journal = Journal::prepare(
            &history,
            &game,
            vec![(target.clone(), inspect(&cache, &Control::default())?)],
            Control::default(),
        )
        .await?;
        journal
            .bind_cache(
                &history,
                &game,
                BTreeMap::from([(target, "winner/file.txt".into())]),
                Control::default(),
            )
            .await?;
        let serialized = serde_json::to_string(&journal)?;
        let journal = Journal::from_record(5, &serialized)?;
        journal
            .persist(&history, &game, "deploy", Default::default())
            .await?;
        journal.apply(&history, &game, Control::default()).await?;
        assert_eq!(fs::metadata(&cache)?.ino(), fs::metadata(&live)?.ino());
        assert_ne!(
            fs::metadata(history.store.source(&identity)?)?.ino(),
            fs::metadata(&live)?.ino()
        );
        journal.recover(&history, &game, false).await?;
        assert_eq!(fs::read(&live)?, b"original");
        fs::write(&cache, b"edited cache")?;
        history.store.verify(&identity, &Control::default())?;
        assert_eq!(fs::read(&live)?, b"original");
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn changed_cache_source_blocks_activation_before_live_changes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (tracker, game, _) =
            crate::core::generations::tests::snapshot_fixture(temp.path()).await?;
        fs::create_dir_all(game.data_dir())?;
        let live = game.data_dir().join("File.txt");
        fs::write(&live, b"original")?;
        let cache = temp.path().join("winner/file.txt");
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        history.retain(cache.clone(), Control::default()).await?;
        let target = Target::file(&game.engine, "File.txt")?;
        let mut journal = Journal::prepare(
            &history,
            &game,
            vec![(target.clone(), inspect(&cache, &Control::default())?)],
            Control::default(),
        )
        .await?;
        journal
            .bind_cache(
                &history,
                &game,
                BTreeMap::from([(target, "winner/file.txt".into())]),
                Control::default(),
            )
            .await?;
        fs::write(&cache, b"changed")?;
        assert!(
            journal
                .verify_prepared(&history, &game, Vec::new(), Control::default())
                .await
                .is_err()
        );
        assert_eq!(fs::read(&live)?, b"original");
        Ok(())
    }
}
