use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use tokio::sync::{OwnedMutexGuard, OwnedRwLockReadGuard};

#[derive(Clone)]
pub(super) struct Control {
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) abandoned: Arc<AtomicBool>,
}

impl Control {
    pub(super) fn new(cancelled: Arc<AtomicBool>, abandoned: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            abandoned,
        }
    }

    pub(super) fn recovery() -> Self {
        Self::new(
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
    }

    pub(super) fn check(&self) -> Result<()> {
        ensure!(
            !self.cancelled.load(Ordering::Acquire) && !self.abandoned.load(Ordering::Acquire),
            "MELE operation cancelled"
        );
        Ok(())
    }

    pub(super) async fn stopped(&self) {
        while self.check().is_ok() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

pub(super) struct Lease {
    _mutation: OwnedMutexGuard<()>,
    _location: OwnedRwLockReadGuard<()>,
}

impl Lease {
    pub(super) async fn acquire(control: &Control) -> Result<Arc<Self>> {
        let mutation = tokio::select! {
            biased;
            _ = control.stopped() => bail!("MELE operation cancelled"),
            lease = super::mutation_lock().lock_owned() => lease,
        };
        let location = crate::core::location_recovery::activity_lock()
            .try_read_owned()
            .context("Folder access is being changed; retry the MELE operation when it finishes")?;
        control.check()?;
        Ok(Arc::new(Self {
            _mutation: mutation,
            _location: location,
        }))
    }
}
