use std::sync::Mutex;
use std::time::{Duration, Instant};

use relm4::Sender;

use super::messages::AppMsg;

const MIN_PROGRESS_INTERVAL: Duration = Duration::from_millis(75);

struct ProgressState {
    last_emit: Option<Instant>,
}

pub(crate) fn throttled_install_progress(
    sender: Sender<AppMsg>,
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
            let first = state.last_emit.is_none();
            let final_update = total > 0 && done >= total;
            let elapsed = state
                .last_emit
                .map(|last| now.duration_since(last) >= MIN_PROGRESS_INTERVAL)
                .unwrap_or(true);
            if first || final_update || elapsed {
                state.last_emit = Some(now);
                true
            } else {
                false
            }
        };

        if should_emit {
            let fraction = if total == 0 {
                0.0
            } else {
                done as f64 / total as f64
            };
            let _ = sender.send(AppMsg::InstallProgress(
                fraction.clamp(0.0, 1.0),
                format!("{label} file {done}/{total}"),
            ));
        }
    })
}

pub(crate) fn throttled_download_install_progress(
    sender: Sender<AppMsg>,
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
            let first = state.last_emit.is_none();
            let final_update = total > 0 && done >= total;
            let elapsed = state
                .last_emit
                .map(|last| now.duration_since(last) >= MIN_PROGRESS_INTERVAL)
                .unwrap_or(true);
            if first || final_update || elapsed {
                state.last_emit = Some(now);
                true
            } else {
                false
            }
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
            let _ = sender.send(AppMsg::InstallProgress(
                fraction.clamp(0.0, 1.0),
                detail.clone(),
            ));
            let _ = sender.send(AppMsg::DownloadProgress(
                download_id.clone(),
                fraction.clamp(0.0, 1.0),
                detail,
            ));
        }
    })
}
