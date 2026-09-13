use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Phase {
    Discovering(usize),
    Hashing { bytes: u64, total: u64 },
    Checking(usize),
    Saving,
}

#[derive(Debug, Clone)]
pub(crate) struct Progress {
    pub(crate) game: String,
    pub(crate) index: usize,
    pub(crate) count: usize,
    pub(crate) phase: Phase,
}

pub(crate) type Callback = Arc<dyn Fn(Progress) + Send + Sync>;

pub(super) struct Reporter {
    pub(super) callback: Callback,
    pub(super) game: String,
    pub(super) index: usize,
    pub(super) count: usize,
    pub(super) last: Option<(Instant, Phase)>,
}

impl Reporter {
    pub(super) fn emit(&mut self, phase: Phase) {
        self.emit_at(phase, Instant::now());
    }

    fn emit_at(&mut self, phase: Phase, now: Instant) {
        if let Some((last, previous)) = &self.last
            && std::mem::discriminant(previous) == std::mem::discriminant(&phase)
            && now.duration_since(*last) < Duration::from_millis(100)
            && !matches!(phase, Phase::Hashing { bytes, total } if bytes == total)
        {
            return;
        }
        self.last = Some((now, phase.clone()));
        (self.callback)(Progress {
            game: self.game.clone(),
            index: self.index,
            count: self.count,
            phase,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @variants: both
    #[test]
    fn throttles_counts_but_keeps_phase_changes_and_final_bytes() {
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected = events.clone();
        let mut reporter = Reporter {
            callback: Arc::new(move |event| collected.lock().unwrap().push(event.phase)),
            game: "LE1".into(),
            index: 1,
            count: 3,
            last: None,
        };
        let now = Instant::now();
        reporter.emit_at(Phase::Discovering(0), now);
        reporter.emit_at(Phase::Discovering(1), now);
        reporter.emit_at(
            Phase::Hashing {
                bytes: 0,
                total: 100,
            },
            now,
        );
        reporter.emit_at(
            Phase::Hashing {
                bytes: 100,
                total: 100,
            },
            now,
        );
        reporter.emit_at(Phase::Checking(0), now);
        assert_eq!(
            *events.lock().unwrap(),
            vec![
                Phase::Discovering(0),
                Phase::Hashing {
                    bytes: 0,
                    total: 100
                },
                Phase::Hashing {
                    bytes: 100,
                    total: 100
                },
                Phase::Checking(0)
            ]
        );
    }
}
