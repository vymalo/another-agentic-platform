//! What the pipeline counts, and how `/metrics` says it.

use std::fmt::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::span::Tracer;

/// The pipeline's counters.
#[derive(Debug, Default)]
pub struct Stats {
    pub(crate) exported: AtomicU64,
    pub(crate) queue_full: AtomicU64,
    pub(crate) export_failed: AtomicU64,
    pub(crate) rejected: AtomicU64,
    /// Spans that ended after the shutdown began.
    pub(crate) shutdown: AtomicU64,
    /// Spans a flush that ran out of time left behind.
    pub(crate) flush_deadline: AtomicU64,
    /// Spans the queue took: what is not exported, failed, rejected by the end is lost at the deadline.
    pub(crate) accepted: AtomicU64,
}

/// What a `/metrics` answer says about tracing, and when. With no tracing variable at all it says nothing, so the text is what
/// it was before tracing existed; with one set, `aap_trace_enabled` says whether the tracer runs (a setting that was refused
/// leaves it off, 0); with the tracer on, the counters follow.
#[derive(Clone, Debug, Default)]
pub struct TraceMetrics {
    stats: Option<Arc<Stats>>,
    asked: bool,
}

impl TraceMetrics {
    /// No tracing variable was set: nothing is said.
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// What the process's `tracer` says: the counters when it is on, the gauge alone when tracing was asked for
    /// ([`Tracer::asked`]) and is off, nothing otherwise.
    #[must_use]
    pub fn of(tracer: &Tracer) -> Self {
        Self {
            stats: tracer.stats(),
            asked: tracer.asked(),
        }
    }

    pub(crate) fn gauge(enabled: bool) -> String {
        format!(
            "# HELP aap_trace_enabled Whether tracing runs (1) or a tracing setting was given and tracing is off (0).\n\
             # TYPE aap_trace_enabled gauge\n\
             aap_trace_enabled {}\n",
            u8::from(enabled)
        )
    }

    /// The text to append to the process's own metrics.
    #[must_use]
    pub fn render(&self) -> String {
        match &self.stats {
            Some(stats) => stats.render(),
            None if self.asked => Self::gauge(false),
            None => String::new(),
        }
    }
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
    /// Spans that ended after the shutdown began.
    pub dropped_shutdown: u64,
    /// Spans that were queued or in a batch when the flush ran out of time.
    pub dropped_flush_deadline: u64,
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
            dropped_shutdown: self.shutdown.load(Ordering::Relaxed),
            dropped_flush_deadline: self.flush_deadline.load(Ordering::Relaxed),
        }
    }

    /// The gauge and the counters in the Prometheus text format, to append to a `/metrics` answer when tracing is on.
    #[must_use]
    pub fn render(&self) -> String {
        let s = self.snapshot();
        let mut out = TraceMetrics::gauge(true);
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
            ("shutdown", s.dropped_shutdown),
            ("flush_deadline", s.dropped_flush_deadline),
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
        assert!(text.contains("{reason=\"shutdown\"} 0\n"));
        assert!(text.contains("{reason=\"flush_deadline\"} 0\n"));
        assert!(text.starts_with("# HELP aap_trace_enabled"));
        assert!(text.contains("aap_trace_enabled 1\n"));
    }

    #[test]
    fn the_gauge_is_there_only_when_a_setting_was_given() {
        assert_eq!(TraceMetrics::none().render(), "");
        let off = TraceMetrics::of(&Tracer::off());
        assert_eq!(off.render(), "");
        let refused = TraceMetrics::of(&Tracer::off_but_asked());
        let text = refused.render();
        assert!(text.contains("aap_trace_enabled 0\n"), "{text}");
        assert!(!text.contains("aap_trace_spans"), "{text}");
    }
}
