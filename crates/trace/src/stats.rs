//! What the pipeline counts, and how `/metrics` says it.

use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};

/// The pipeline's counters.
#[derive(Debug, Default)]
pub struct Stats {
    pub(crate) exported: AtomicU64,
    pub(crate) queue_full: AtomicU64,
    pub(crate) export_failed: AtomicU64,
    pub(crate) rejected: AtomicU64,
}

/// A reading of [`Stats`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatsSnapshot {
    /// Spans an exporter accepted.
    pub exported: u64,
    /// Spans dropped because the queue was full.
    pub dropped_queue_full: u64,
    /// Spans dropped because an export failed.
    pub dropped_export_failed: u64,
    /// Spans the collector said it rejected.
    pub dropped_rejected: u64,
}

impl Stats {
    /// The counters now.
    #[must_use]
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            exported: self.exported.load(Ordering::Relaxed),
            dropped_queue_full: self.queue_full.load(Ordering::Relaxed),
            dropped_export_failed: self.export_failed.load(Ordering::Relaxed),
            dropped_rejected: self.rejected.load(Ordering::Relaxed),
        }
    }

    /// The counters in the Prometheus text format, to append to a `/metrics` answer when tracing is on.
    #[must_use]
    pub fn render(&self) -> String {
        let s = self.snapshot();
        let mut out = String::new();
        let _ = writeln!(
            out,
            "# HELP aap_trace_spans_exported_total Spans the trace exporter accepted.\n\
             # TYPE aap_trace_spans_exported_total counter\n\
             aap_trace_spans_exported_total {}",
            s.exported
        );
        let _ = writeln!(
            out,
            "# HELP aap_trace_spans_dropped_total Spans that were not exported, by reason.\n\
             # TYPE aap_trace_spans_dropped_total counter"
        );
        for (reason, n) in [
            ("queue_full", s.dropped_queue_full),
            ("export_failed", s.dropped_export_failed),
            ("rejected", s.dropped_rejected),
        ] {
            let _ = writeln!(
                out,
                "aap_trace_spans_dropped_total{{reason=\"{reason}\"}} {n}"
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_names_both_series() {
        let s = Stats::default();
        s.exported.store(3, Ordering::Relaxed);
        s.queue_full.store(2, Ordering::Relaxed);
        let text = s.render();
        assert!(text.contains("aap_trace_spans_exported_total 3\n"));
        assert!(text.contains("aap_trace_spans_dropped_total{reason=\"queue_full\"} 2\n"));
        assert!(text.contains("{reason=\"export_failed\"} 0\n"));
        assert!(text.contains("{reason=\"rejected\"} 0\n"));
    }
}
