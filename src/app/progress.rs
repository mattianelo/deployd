use std::sync::Mutex;
use std::time::{Duration, Instant};

use relm4::Sender;

use super::messages::AppMsg;
use super::state::InstallIdentity;

const MIN_PROGRESS_INTERVAL: Duration = Duration::from_millis(75);

struct ProgressState {
    last_emit: Option<Instant>,
}

impl ProgressState {
    fn should_emit(&mut self, now: Instant, done: usize, total: usize) -> bool {
        let first = self.last_emit.is_none();
        let final_update = total > 0 && done >= total;
        let elapsed = self
            .last_emit
            .map(|last| now.duration_since(last) >= MIN_PROGRESS_INTERVAL)
            .unwrap_or(true);
        if first || final_update || elapsed {
            self.last_emit = Some(now);
            true
        } else {
            false
        }
    }
}

pub(crate) fn throttled_install_progress(
    sender: Sender<AppMsg>,
    identity: InstallIdentity,
    label: &'static str,
) -> Box<dyn Fn(usize, usize) + Send> {
    let state = Mutex::new(ProgressState { last_emit: None });
    Box::new(move |done, total| {
        let now = Instant::now();
        let should_emit = {
            let mut state = match state.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            state.should_emit(now, done, total)
        };

        if should_emit {
            let fraction = if total == 0 {
                0.0
            } else {
                done as f64 / total as f64
            };
            let _ = sender.send(AppMsg::Install(
                crate::app::messages::InstallMsg::InstallProgress(
                    identity.clone(),
                    fraction.clamp(0.0, 1.0),
                    format!("{label} file {done}/{total}"),
                ),
            ));
        }
    })
}

pub(crate) fn throttled_download_install_progress(
    sender: Sender<AppMsg>,
    identity: InstallIdentity,
    download_id: String,
    phase_message: &'static str,
) -> Box<dyn Fn(usize, usize) + Send> {
    let state = Mutex::new(ProgressState { last_emit: None });
    Box::new(move |done, total| {
        let now = Instant::now();
        let should_emit = {
            let mut state = match state.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            state.should_emit(now, done, total)
        };

        if should_emit {
            let fraction = if total == 0 {
                0.0
            } else {
                done as f64 / total as f64
            };
            let detail = if total == 0 {
                phase_message.to_string()
            } else {
                format!("{phase_message} ({done}/{total})")
            };
            let _ = sender.send(AppMsg::Install(
                crate::app::messages::InstallMsg::InstallProgress(
                    identity.clone(),
                    fraction.clamp(0.0, 1.0),
                    detail.clone(),
                ),
            ));
            let _ = sender.send(AppMsg::Downloads(
                crate::app::messages::DownloadsMsg::DownloadProgress(
                    download_id.clone(),
                    fraction.clamp(0.0, 1.0),
                    detail,
                ),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_intermediate_progress_until_interval_elapses() {
        let start = Instant::now();
        let mut state = ProgressState { last_emit: None };

        assert!(state.should_emit(start, 1, 10));
        assert!(!state.should_emit(start + Duration::from_millis(74), 2, 10));
        assert!(state.should_emit(start + MIN_PROGRESS_INTERVAL, 3, 10));
    }

    #[test]
    fn final_progress_bypasses_the_interval() {
        let start = Instant::now();
        let mut state = ProgressState { last_emit: None };

        assert!(state.should_emit(start, 1, 10));
        assert!(state.should_emit(start + Duration::from_millis(1), 10, 10));
    }
}
