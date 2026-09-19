use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::core::tracker::Tracker;

use super::catalog::durable;
use super::content::{Control, Identity};
use super::operation::Lease;
use super::store::Store;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    store_id: String,
    old_cache: PathBuf,
    new_cache: PathBuf,
}

pub(crate) struct Prepared {
    id: String,
    game: String,
    intent: Intent,
}

impl Prepared {
    pub(crate) fn journal(&self) -> &str {
        &self.id
    }

    pub(crate) fn new_cache(&self) -> &Path {
        &self.intent.new_cache
    }

    pub(crate) async fn abort(self, tracker: &Tracker) -> Result<()> {
        let destination =
            crate::utils::paths::generation_store_in(&self.intent.new_cache, &self.game)?;
        remove_store(destination).await?;
        delete_journal(tracker, &self.id, false).await
    }

    pub(crate) async fn finish(self, tracker: &Tracker) -> Result<()> {
        let source = crate::utils::paths::generation_store_in(&self.intent.old_cache, &self.game)?;
        remove_store(source).await?;
        delete_journal(tracker, &self.id, true).await
    }
}

pub(crate) async fn prepare(
    tracker: &Tracker,
    game: &str,
    old_cache: &Path,
    new_cache: &Path,
) -> Result<Option<Prepared>> {
    let binding: Option<(String, String)> =
        sqlx::query_as("SELECT store_id,cache_root FROM generation_stores WHERE game_id=?")
            .bind(game)
            .fetch_optional(&tracker.pool)
            .await?;
    let Some((store_id, bound)) = binding else {
        return Ok(None);
    };
    ensure!(
        Path::new(&bound) == old_cache,
        "Deployment history is bound to another cache; restore access before moving it"
    );
    let id = uuid::Uuid::new_v4().to_string();
    let intent = Intent {
        store_id: store_id.clone(),
        old_cache: old_cache.to_owned(),
        new_cache: new_cache.to_owned(),
    };
    let mut tx = durable(tracker).await?;
    sqlx::query("INSERT INTO generation_journals(id,game_id,kind,document_version,document) VALUES (?,?,'relocate',2,?)")
        .bind(&id)
        .bind(game)
        .bind(serde_json::to_string(&intent)?)
        .execute(&mut *tx)
        .await
        .context("Another operation must finish before moving deployment history")?;
    tx.commit().await?;

    let lease = Lease::acquire().await?;
    let source = crate::utils::paths::generation_store_in(old_cache, game)?;
    let destination = crate::utils::paths::generation_store_in(new_cache, game)?;
    let objects: Vec<(String, i64)> =
        sqlx::query_as("SELECT sha256,size FROM generation_objects WHERE game_id=?")
            .bind(game)
            .fetch_all(&tracker.pool)
            .await?;
    let old_cache = old_cache.to_owned();
    let new_cache = new_cache.to_owned();
    let game_id = game.to_owned();
    let copying = lease
        .blocking(move || -> Result<()> {
            ensure!(
                source.try_exists()?,
                "Deployment history storage is missing"
            );
            ensure!(
                !destination.try_exists()?,
                "The destination already contains deployment history for this game"
            );
            let parent = destination
                .parent()
                .context("History destination has no parent")?;
            fs::create_dir_all(parent)?;
            let temporary = tempfile::Builder::new()
                .prefix(".relocate-")
                .tempdir_in(parent)?;
            copy_directory(&source, temporary.path())?;
            fs::rename(temporary.path(), &destination)?;
            let store = Store::open(&new_cache, &game_id, &store_id)?;
            for (sha256, size) in objects {
                store.verify(
                    &Identity {
                        sha256,
                        size: size.try_into()?,
                    },
                    &Control::default(),
                )?;
            }
            Store::open(&old_cache, &game_id, &store_id)?;
            Ok(())
        })
        .await
        .context("History relocation worker stopped")?;
    if let Err(error) = copying {
        let prepared = Prepared {
            id,
            game: game.to_owned(),
            intent,
        };
        let cleanup = prepared.abort(tracker).await;
        return match cleanup {
            Ok(()) => Err(error),
            Err(cleanup) => {
                Err(error.context(format!("Relocation cleanup also failed: {cleanup:#}")))
            }
        };
    }
    Ok(Some(Prepared {
        id,
        game: game.to_owned(),
        intent,
    }))
}

pub(crate) async fn recover(tracker: &Tracker, game: &str) -> Result<()> {
    let pending: Option<(String, String, bool)> = sqlx::query_as(
        "SELECT id,document,committed FROM generation_journals WHERE game_id=? AND kind='relocate' AND document_version=2",
    )
    .bind(game)
    .fetch_optional(&tracker.pool)
    .await?;
    let Some((id, document, committed)) = pending else {
        return Ok(());
    };
    let intent: Intent = serde_json::from_str(&document)?;
    let obsolete = if committed {
        &intent.old_cache
    } else {
        &intent.new_cache
    };
    remove_store(crate::utils::paths::generation_store_in(obsolete, game)?).await?;
    delete_journal(tracker, &id, committed).await
}

async fn remove_store(path: PathBuf) -> Result<()> {
    Lease::acquire()
        .await?
        .blocking(move || match fs::remove_dir_all(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("Cannot remove obsolete deployment history storage"),
        })
        .await
        .context("History relocation cleanup worker stopped")?
}

async fn delete_journal(tracker: &Tracker, id: &str, committed: bool) -> Result<()> {
    let mut tx = durable(tracker).await?;
    let changed = sqlx::query("DELETE FROM generation_journals WHERE id=? AND committed=?")
        .bind(id)
        .bind(committed)
        .execute(&mut *tx)
        .await?;
    ensure!(
        changed.rows_affected() == 1,
        "History relocation decision changed"
    );
    tx.commit().await?;
    Ok(())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir(&target)?;
            copy_directory(&entry.path(), &target)?;
        } else {
            ensure!(
                kind.is_file(),
                "History storage contains an unsupported node"
            );
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::*;
    use crate::core::generations::catalog::History;

    // @variants: both
    #[tokio::test]
    async fn publishes_verified_history_with_the_cache_binding() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        fs::create_dir_all(&old)?;
        fs::create_dir_all(&new)?;
        let (tracker, game, _) = super::super::tests::snapshot_fixture(&old).await?;
        let history = History::open(&tracker, &game.id, &old, true).await?;
        let source = temp.path().join("source");
        fs::write(&source, b"retained")?;
        history.retain(source, Control::default()).await?;
        drop(history);

        let prepared = prepare(&tracker, &game.id, &old, &new)
            .await?
            .expect("history binding");
        assert!(crate::utils::paths::generation_store_in(&old, &game.id)?.exists());
        assert!(crate::utils::paths::generation_store_in(&new, &game.id)?.exists());
        tracker
            .commit_game_cache_move(
                &game.id,
                old.to_str().expect("old cache path"),
                new.to_str().expect("new cache path"),
                Some(&new),
                Some((prepared.journal(), prepared.new_cache())),
            )
            .await?;
        prepared.finish(&tracker).await?;

        assert!(!crate::utils::paths::generation_store_in(&old, &game.id)?.exists());
        History::open(&tracker, &game.id, &new, false).await?;
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn abort_keeps_the_original_history_binding() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        fs::create_dir_all(&old)?;
        fs::create_dir_all(&new)?;
        let (tracker, game, _) = super::super::tests::snapshot_fixture(&old).await?;
        History::open(&tracker, &game.id, &old, true).await?;

        prepare(&tracker, &game.id, &old, &new)
            .await?
            .expect("history binding")
            .abort(&tracker)
            .await?;

        assert!(!crate::utils::paths::generation_store_in(&new, &game.id)?.exists());
        History::open(&tracker, &game.id, &old, false).await?;
        Ok(())
    }
}
