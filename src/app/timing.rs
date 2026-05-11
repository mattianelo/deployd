use std::time::{Duration, Instant};

const SLOW_PHASE_THRESHOLD: Duration = Duration::from_millis(250);

pub(crate) fn log_phase(
    phase: &str,
    game_id: &str,
    started_at: Instant,
    item_count: Option<usize>,
) {
    let elapsed = started_at.elapsed();
    if elapsed < SLOW_PHASE_THRESHOLD {
        return;
    }

    let variant = if crate::core::game::is_snap() {
        "snap"
    } else {
        "appimage"
    };
    let count = item_count
        .map(|count| count.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    crate::dlog!(
        "deployd: timing phase={phase} elapsed_ms={} game_id={game_id} variant={variant} item_count={count}",
        elapsed.as_millis()
    );
}
