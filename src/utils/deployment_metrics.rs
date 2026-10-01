use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Result, ensure};
use serde::Serialize;

use super::verified_files;

static HELPERS: AtomicU64 = AtomicU64::new(0);
static RESULTS: AtomicU64 = AtomicU64::new(0);
static CHANGED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn helper_run() {
    HELPERS.fetch_add(1, Ordering::Relaxed);
}
pub(crate) fn cache_hit() {
    RESULTS.fetch_add(1, Ordering::Relaxed);
}
pub(crate) fn changed_file() {
    CHANGED.fetch_add(1, Ordering::Relaxed);
}

pub(crate) struct Recorder {
    started: Instant,
    phase_started: Instant,
    phase: &'static str,
    phases: BTreeMap<&'static str, u128>,
    before: verified_files::Metrics,
    helpers: u64,
    results: u64,
    changed: u64,
    finished: bool,
}

#[derive(Serialize)]
struct Report {
    outcome: &'static str,
    version: u32,
    elapsed_ms: u128,
    confirmation_ms: u128,
    work_ms: u128,
    phases_ms: BTreeMap<&'static str, u128>,
    hashed_bytes: u64,
    copied_bytes: u64,
    cloned_bytes: u64,
    reused_hashes: u64,
    helper_executions: u64,
    result_cache_hits: u64,
    changed_files: u64,
}

impl Recorder {
    pub(crate) fn start() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            started: Instant::now(),
            phase_started: Instant::now(),
            phase: "preparation",
            phases: BTreeMap::new(),
            before: verified_files::metrics(),
            helpers: HELPERS.load(Ordering::Relaxed),
            results: RESULTS.load(Ordering::Relaxed),
            changed: CHANGED.load(Ordering::Relaxed),
            finished: false,
        }))
    }

    pub(crate) fn phase(&mut self, phase: &str) {
        if self.finished {
            return;
        }
        let next = classify(phase);
        if next == self.phase {
            return;
        }
        self.tick();
        self.phase = next;
    }

    fn tick(&mut self) {
        *self.phases.entry(self.phase).or_default() += self.phase_started.elapsed().as_millis();
        self.phase_started = Instant::now();
    }

    fn report(&mut self) -> Option<Report> {
        if self.finished {
            return None;
        }
        self.tick();
        self.finished = true;
        let after = verified_files::metrics();
        let elapsed_ms = self.started.elapsed().as_millis();
        let confirmation_ms = self.phases.get("confirmation").copied().unwrap_or(0);
        Some(Report {
            outcome: "unknown",
            version: 1,
            elapsed_ms,
            confirmation_ms,
            work_ms: elapsed_ms.saturating_sub(confirmation_ms),
            phases_ms: self.phases.clone(),
            hashed_bytes: after.hashed_bytes.saturating_sub(self.before.hashed_bytes),
            copied_bytes: after.copied_bytes.saturating_sub(self.before.copied_bytes),
            cloned_bytes: after.cloned_bytes.saturating_sub(self.before.cloned_bytes),
            reused_hashes: after.cache_hits.saturating_sub(self.before.cache_hits),
            helper_executions: HELPERS.load(Ordering::Relaxed).saturating_sub(self.helpers),
            result_cache_hits: RESULTS.load(Ordering::Relaxed).saturating_sub(self.results),
            changed_files: CHANGED.load(Ordering::Relaxed).saturating_sub(self.changed),
        })
    }

    pub(crate) fn finish(&mut self, heading: &str) {
        let Some(mut report) = self.report() else {
            return;
        };
        report.outcome = match heading {
            "Deployment complete" | "Purge complete" | "Integrity check complete" => "complete",
            "Deployment cancelled" => "cancelled",
            _ => "stopped",
        };
        // Timing is reconstructible diagnostic data; it must not delay or invalidate deployment.
        std::thread::spawn(move || {
            if save(&report).is_err() {
                eprintln!("deployd: deployment timing could not be saved");
            }
        });
    }
}

fn classify(phase: &str) -> &'static str {
    match phase {
        "preview" | "Checking deployment…" => "preview",
        "confirmation" => "confirmation",
        "Activating files and saves…" => "activation",
        "Committing deployment…" => "commit",
        "Finishing deployment…" => "completion",
        "Recovering deployment…" | "Recovering interrupted deployment…" => "recovery",
        "Preparing saves…" => "saves",
        "Retaining prepared MELE outputs…" | "Retaining MELE mod sources…" => "retention",
        "Verifying prepared deployment…" | "Verifying retained MELE mod sources…" => {
            "verification"
        }
        _ => "preparation",
    }
}

fn save(report: &Report) -> Result<()> {
    let root = super::paths::deployd_data_dir()?;
    fs::create_dir_all(&root)?;
    let path = root.join("deployment-timing.jsonl");
    write_report(&path, report)
}

fn write_report(path: &std::path::Path, report: &Report) -> Result<()> {
    let mut file = File::options()
        .append(true)
        .create(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .mode(0o600)
        .open(path)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1,
        "Invalid timing log"
    );
    if metadata.len() >= 256 * 1024 {
        file.set_len(0)?;
    }
    let mut bytes = serde_json::to_vec(report)?;
    bytes.push(b'\n');
    file.write_all(&bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests;
