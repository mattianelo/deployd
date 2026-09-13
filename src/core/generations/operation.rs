use std::future::Future;
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use tokio::sync::{Mutex, OwnedMutexGuard, OwnedRwLockReadGuard};
use tokio::task::JoinHandle;

pub(super) struct Lease {
    _history: OwnedMutexGuard<()>,
    _location: OwnedRwLockReadGuard<()>,
}

impl Lease {
    pub(super) async fn acquire() -> Result<Arc<Self>> {
        static OPERATIONS: OnceLock<Arc<Mutex<()>>> = OnceLock::new();
        let history = OPERATIONS
            .get_or_init(|| Arc::new(Mutex::new(())))
            .clone()
            .lock_owned()
            .await;
        let location = crate::core::location_recovery::activity_lock()
            .try_read_owned()
            .context("Folder access is being changed; retry deployment history when it finishes")?;
        Ok(Arc::new(Self {
            _history: history,
            _location: location,
        }))
    }

    pub(super) fn blocking<F, T>(self: &Arc<Self>, work: F) -> JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        let lease = self.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = lease;
            work()
        })
    }

    pub(super) fn participant<F>(self: &Arc<Self>, work: F) -> JoinHandle<F::Output>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let lease = self.clone();
        tokio::spawn(async move {
            let _lease = lease;
            work.await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex as StdMutex;
    use std::time::Duration;

    use super::*;
    use crate::core::generations::catalog::History;
    use crate::core::generations::content::{self, Control};
    use crate::core::location_recovery::activity_lock;
    use crate::core::tracker::Tracker;

    fn paused_copy(
        size: u64,
    ) -> (
        Control,
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
    ) {
        let (started, ready) = tokio::sync::oneshot::channel();
        let started = StdMutex::new(Some(started));
        let (release, wait) = std::sync::mpsc::channel();
        let wait = StdMutex::new(wait);
        let control = Control {
            progress: Arc::new(move |_, total| {
                if total != size {
                    return;
                }
                if let Some(started) = started.lock().unwrap().take() {
                    started.send(()).unwrap();
                    wait.lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5))
                        .unwrap();
                }
            }),
            ..Control::default()
        };
        (control, ready, release)
    }

    // @variants: both
    #[tokio::test]
    async fn location_recovery_blocks_history_before_storage_initialization() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let recovery = activity_lock().write_owned().await;
        assert!(
            History::open(&tracker, "game", temp.path(), true)
                .await
                .is_err()
        );
        assert_eq!(fs::read_dir(temp.path())?.count(), 0);
        let stores: i64 = sqlx::query_scalar("SELECT count(*) FROM generation_stores")
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(stores, 0);
        drop(recovery);
        drop(History::open(&tracker, "game", temp.path(), true).await?);
        assert!(activity_lock().try_write_owned().is_ok());
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn history_excludes_folder_recovery_until_it_is_closed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let history = History::open(&tracker, "game", temp.path(), true).await?;
        assert!(activity_lock().try_write_owned().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), Lease::acquire())
                .await
                .is_err()
        );
        drop(history);
        assert!(activity_lock().try_write_owned().is_ok());
        drop(Lease::acquire().await?);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn abandoned_retained_copy_keeps_both_leases_until_the_worker_finishes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("source");
        fs::write(&source, b"retained bytes")?;
        let identity = content::inspect(&source, &Control::default())?;
        let tracker = Tracker::open("sqlite::memory:").await?.tracker;
        let history = History::open(&tracker, "game", temp.path(), true).await?;
        let store = history.store.clone();
        let (control, ready, release) = paused_copy(identity.size);
        let task = tokio::spawn(async move { history.retain(source, control).await });
        ready.await?;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(activity_lock().try_write_owned().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), Lease::acquire())
                .await
                .is_err()
        );
        release.send(())?;
        let lease = tokio::time::timeout(Duration::from_secs(5), Lease::acquire()).await??;
        store.verify(&identity, &Control::default())?;
        let registered: i64 = sqlx::query_scalar("SELECT count(*) FROM generation_objects")
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(registered, 0);
        drop(lease);
        assert!(activity_lock().try_write_owned().is_ok());
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn abandoned_activation_finishes_under_lease_before_journal_recovery() -> Result<()> {
        use std::collections::BTreeMap;

        use crate::core::generations::journal::{Journal, Node};
        use crate::core::generations::target::Target;

        let temp = tempfile::tempdir()?;
        let (tracker, game, _) = super::super::tests::snapshot_fixture(temp.path()).await?;
        fs::create_dir_all(game.data_dir())?;
        let live = game.data_dir().join("file.txt");
        fs::write(&live, b"old")?;
        let history = History::open(&tracker, &game.id, temp.path(), true).await?;
        let content = history
            .retain(temp.path().join("winner/file.txt"), Control::default())
            .await?;
        let (control, ready, release) = paused_copy(content.size);
        let journal = Journal::prepare(
            &history,
            &game,
            vec![(
                Target::file(&game.engine, "file.txt")?,
                Node::File {
                    identity: content,
                    mode: 0o644,
                },
            )],
            Control::default(),
        )
        .await?;
        journal
            .persist(&history, &game, "deploy", BTreeMap::new())
            .await?;
        let applying_game = game.clone();
        let task = tokio::spawn(async move {
            journal
                .apply(&history, &applying_game, control)
                .await
                .map(|_| ())
        });
        ready.await?;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert!(activity_lock().try_write_owned().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), Lease::acquire())
                .await
                .is_err()
        );
        release.send(())?;
        let history = tokio::time::timeout(
            Duration::from_secs(5),
            History::open(&tracker, &game.id, temp.path(), false),
        )
        .await??;
        assert_eq!(fs::read(&live)?, b"winner");
        super::super::recovery::recover_journal(&history, &game).await?;
        assert_eq!(fs::read(live)?, b"old");
        let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM generation_journals")
            .fetch_one(&tracker.pool)
            .await?;
        assert_eq!(pending, 0);
        Ok(())
    }

    // @variants: both
    #[tokio::test]
    async fn abandoned_participant_retains_leases_for_its_nested_worker() -> Result<()> {
        let lease = Lease::acquire().await?;
        let (started, ready) = tokio::sync::oneshot::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let participant = lease.participant(async move {
            tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                Err::<(), _>(anyhow::anyhow!("injected participant failure"))
            })
            .await
        });
        ready.await?;
        drop(participant);
        drop(lease);
        assert!(activity_lock().try_write_owned().is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(20), Lease::acquire())
                .await
                .is_err()
        );
        release.send(())?;
        drop(tokio::time::timeout(Duration::from_secs(5), Lease::acquire()).await??);
        assert!(activity_lock().try_write_owned().is_ok());
        Ok(())
    }
}
