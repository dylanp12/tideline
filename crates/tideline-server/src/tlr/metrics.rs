//! Record-layer counters.
//!
//! `/metrics` previously covered streams only, so an operator could watch token
//! throughput while the thing the product exists to protect — the record —
//! reported nothing at all.
//!
//! Process-global rather than threaded through state: Prometheus counters are
//! process-wide by nature, and the alternative is passing a handle into every
//! call site that might fail.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::OnceLock;

#[derive(Default)]
pub struct TlrMetrics {
    pub runs_created: AtomicU64,
    pub runs_sealed: AtomicU64,
    pub events_appended: AtomicU64,
    pub append_failures: AtomicU64,
    pub gates_opened: AtomicU64,
    pub gates_approved: AtomicU64,
    pub gates_rejected: AtomicU64,
    pub gates_expired: AtomicU64,
    pub redactions: AtomicU64,
    pub checkpoints_written: AtomicU64,
    /// Epoch milliseconds of the most recent checkpoint, 0 if none.
    pub last_checkpoint_ms: AtomicU64,
}

pub fn metrics() -> &'static TlrMetrics {
    static M: OnceLock<TlrMetrics> = OnceLock::new();
    M.get_or_init(TlrMetrics::default)
}

pub fn incr(counter: &AtomicU64) {
    counter.fetch_add(1, Relaxed);
}

/// Render the record counters in Prometheus text format.
pub fn render() -> String {
    let m = metrics();
    let g = |c: &AtomicU64| c.load(Relaxed);

    // Age is more actionable than a raw timestamp: a checkpoint that stopped
    // advancing means truncation protection has quietly lapsed.
    let last = g(&m.last_checkpoint_ms);
    let age_seconds = if last == 0 {
        -1.0
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(last);
        now.saturating_sub(last) as f64 / 1000.0
    };

    format!(
        "# HELP tideline_runs_created_total Runs opened since start\n\
         # TYPE tideline_runs_created_total counter\n\
         tideline_runs_created_total {}\n\
         # HELP tideline_runs_sealed_total Runs sealed since start\n\
         # TYPE tideline_runs_sealed_total counter\n\
         tideline_runs_sealed_total {}\n\
         # HELP tideline_events_appended_total Record events appended since start\n\
         # TYPE tideline_events_appended_total counter\n\
         tideline_events_appended_total {}\n\
         # HELP tideline_append_failures_total Record appends that failed\n\
         # TYPE tideline_append_failures_total counter\n\
         tideline_append_failures_total {}\n\
         # HELP tideline_gates_opened_total Approval gates opened since start\n\
         # TYPE tideline_gates_opened_total counter\n\
         tideline_gates_opened_total {}\n\
         # HELP tideline_gates_resolved_total Approval gates resolved, by decision\n\
         # TYPE tideline_gates_resolved_total counter\n\
         tideline_gates_resolved_total{{decision=\"approved\"}} {}\n\
         tideline_gates_resolved_total{{decision=\"rejected\"}} {}\n\
         tideline_gates_resolved_total{{decision=\"expired\"}} {}\n\
         # HELP tideline_redactions_total Redactions applied since start\n\
         # TYPE tideline_redactions_total counter\n\
         tideline_redactions_total {}\n\
         # HELP tideline_checkpoints_written_total Checkpoints signed since start\n\
         # TYPE tideline_checkpoints_written_total counter\n\
         tideline_checkpoints_written_total {}\n\
         # HELP tideline_checkpoint_age_seconds Seconds since the last checkpoint (-1 if none)\n\
         # TYPE tideline_checkpoint_age_seconds gauge\n\
         tideline_checkpoint_age_seconds {:.3}\n",
        g(&m.runs_created),
        g(&m.runs_sealed),
        g(&m.events_appended),
        g(&m.append_failures),
        g(&m.gates_opened),
        g(&m.gates_approved),
        g(&m.gates_rejected),
        g(&m.gates_expired),
        g(&m.redactions),
        g(&m.checkpoints_written),
        age_seconds,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_every_counter_it_declares() {
        let text = render();
        for name in [
            "tideline_runs_created_total",
            "tideline_runs_sealed_total",
            "tideline_events_appended_total",
            "tideline_append_failures_total",
            "tideline_gates_opened_total",
            "tideline_gates_resolved_total",
            "tideline_redactions_total",
            "tideline_checkpoints_written_total",
            "tideline_checkpoint_age_seconds",
        ] {
            assert!(text.contains(name), "{name} missing from /metrics");
        }
    }

    #[test]
    fn checkpoint_age_is_negative_until_one_is_written() {
        // A gauge of 0 would read as "just checkpointed", which is the opposite
        // of the truth when none exists.
        if metrics().last_checkpoint_ms.load(Relaxed) == 0 {
            assert!(render().contains("tideline_checkpoint_age_seconds -1.000"));
        }
    }

    #[test]
    fn counters_increment() {
        let before = metrics().events_appended.load(Relaxed);
        incr(&metrics().events_appended);
        assert_eq!(metrics().events_appended.load(Relaxed), before + 1);
    }
}
