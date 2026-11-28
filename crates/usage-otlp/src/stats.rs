//! Counts only: no event, no subject, no id.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};

/// The adapter's counters; the operator renders them (ANR-1103).
#[derive(Debug, Default)]
pub struct Stats {
    pub(crate) queued: AtomicU64,
    pub(crate) sent: AtomicU64,
    pub(crate) dropped_queue_full: AtomicU64,
    pub(crate) refused: AtomicU64,
    pub(crate) retries: AtomicU64,
    pub(crate) unsent_at_shutdown: AtomicU64,
    pub(crate) depth: AtomicU64,
    pub(crate) outcomes_dropped: AtomicU64,
    /// Unix seconds at which the current episode of retries began; 0 when not retrying.
    pub(crate) retrying_since: AtomicU64,
}

/// What the counters said at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatsSnapshot {
    /// Events `send` took.
    pub queued: u64,
    /// Events a collector accepted (2xx).
    pub sent: u64,
    /// Events `send` refused because the queue was full (the caller keeps and re-sends them).
    pub dropped_queue_full: u64,
    /// Events dropped because the collector permanently refused their batch (a 4xx other than 408 and 429, or a
    /// `partialSuccess` rejection).
    pub refused: u64,
    /// POST attempts that failed transiently and were repeated.
    pub retries: u64,
    /// Events still unsent when a shutdown's deadline ran out.
    pub unsent_at_shutdown: u64,
    /// Events taken and not yet sent, refused or given up: the queue and the batch in flight.
    pub queue_depth: u64,
    /// Acknowledgements dropped because the meter did not drain them (never, while a meter runs).
    pub outcomes_dropped: u64,
    /// Seconds a batch has been retried without being taken (at least 1 while it is); 0 when not retrying. Alert on it.
    pub retrying_since_secs: u64,
}

impl Stats {
    /// The counters now.
    #[must_use]
    pub fn snapshot(&self) -> StatsSnapshot {
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        StatsSnapshot {
            queued: g(&self.queued),
            sent: g(&self.sent),
            dropped_queue_full: g(&self.dropped_queue_full),
            refused: g(&self.refused),
            retries: g(&self.retries),
            unsent_at_shutdown: g(&self.unsent_at_shutdown),
            queue_depth: g(&self.depth),
            outcomes_dropped: g(&self.outcomes_dropped),
            retrying_since_secs: match g(&self.retrying_since) {
                0 => 0,
                t => now_secs().saturating_sub(t).max(1),
            },
        }
    }

    pub(crate) fn add(a: &AtomicU64, n: u64) {
        a.fetch_add(n, Ordering::Relaxed);
    }

    pub(crate) fn resolve(&self, n: u64) {
        // Saturating: depth never wraps below zero.
        let _ = self
            .depth
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |d| {
                Some(d.saturating_sub(n))
            });
    }

    /// Records that a batch is being retried, keeping the first moment of the episode.
    pub(crate) fn mark_retrying(&self) {
        let _ = self.retrying_since.compare_exchange(
            0,
            now_secs(),
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_secs().max(1))
}

impl StatsSnapshot {
    /// The nine series of the sink in the Prometheus text format: counters and two gauges, with `# HELP` and `# TYPE`. The operator
    /// appends this to the controllers' text when the meter is wired into `operator run` (ANR-1194); with no meter, nothing of it
    /// appears. Counters start at 0 when the sink begins, so a fresh pod that has not queued a line yet shows zeros; the gauge of
    /// the current retrying episode is 0 when none is in flight.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        // Counters
        let counters: [(&str, &str, u64); 7] = [
            (
                "aap_meter_sink_queued_total",
                "Pod-minute events the sink took into its queue.",
                self.queued,
            ),
            (
                "aap_meter_sink_sent_total",
                "Pod-minute events the collector acknowledged (2xx).",
                self.sent,
            ),
            (
                "aap_meter_sink_dropped_queue_full_total",
                "Pod-minute events the sink refused because its queue was full; the meter holds the minute and sends it again.",
                self.dropped_queue_full,
            ),
            (
                "aap_meter_sink_refused_total",
                "Pod-minute events the collector refused for good (a 4xx, a partialSuccess rejection); never sent again.",
                self.refused,
            ),
            (
                "aap_meter_sink_retries_total",
                "POST attempts that failed transiently and were repeated.",
                self.retries,
            ),
            (
                "aap_meter_sink_unsent_at_shutdown_total",
                "Pod-minute events still unsent when the shutdown's deadline ran out.",
                self.unsent_at_shutdown,
            ),
            (
                "aap_meter_sink_outcomes_dropped_total",
                "Collector acknowledgements the sink dropped because the meter did not drain them (never while a meter runs).",
                self.outcomes_dropped,
            ),
        ];
        for (name, help, value) in counters {
            let _ = writeln!(
                out,
                "# HELP {name} {help}\n# TYPE {name} counter\n{name} {value}",
            );
        }
        // Gauges
        let _ = writeln!(
            out,
            "# HELP aap_meter_sink_queue_depth Pod-minute events still held by the sink (the queue and the batch in flight).\n\
             # TYPE aap_meter_sink_queue_depth gauge\n\
             aap_meter_sink_queue_depth {}",
            self.queue_depth
        );
        let _ = writeln!(
            out,
            "# HELP aap_meter_sink_retrying_secs Seconds the current retrying episode has been in flight (0 when none; alert on any rise).\n\
             # TYPE aap_meter_sink_retrying_secs gauge\n\
             aap_meter_sink_retrying_secs {}",
            self.retrying_since_secs
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text the operator's `/metrics` appends has every series of the nine, with `# HELP` and `# TYPE`, in the order the
    /// contract lists them, and the values it was given. Delete one branch and the assertion names it.
    #[test]
    fn render_emits_every_series_in_order() {
        let snap = StatsSnapshot {
            queued: 1,
            sent: 2,
            dropped_queue_full: 3,
            refused: 4,
            retries: 5,
            unsent_at_shutdown: 6,
            queue_depth: 7,
            outcomes_dropped: 8,
            retrying_since_secs: 9,
        };
        let text = snap.render();
        for (name, value) in [
            ("aap_meter_sink_queued_total", "1"),
            ("aap_meter_sink_sent_total", "2"),
            ("aap_meter_sink_dropped_queue_full_total", "3"),
            ("aap_meter_sink_refused_total", "4"),
            ("aap_meter_sink_retries_total", "5"),
            ("aap_meter_sink_unsent_at_shutdown_total", "6"),
            ("aap_meter_sink_outcomes_dropped_total", "8"),
            ("aap_meter_sink_queue_depth", "7"),
            ("aap_meter_sink_retrying_secs", "9"),
        ] {
            assert!(
                text.contains(&format!("# HELP {name} ")),
                "missing HELP for {name}: {text}"
            );
            assert!(
                text.contains(&format!(
                    "# TYPE {name} {}\n",
                    if name.contains("depth") || name.contains("retrying_secs") {
                        "gauge"
                    } else {
                        "counter"
                    }
                )),
                "missing TYPE for {name}: {text}"
            );
            assert!(
                text.contains(&format!("{name} {value}\n")),
                "missing value for {name}: {text}"
            );
        }
    }

    /// A snapshot of zeros renders as zeros, not as an empty string, so a fresh pod still has its nine series (counters at 0,
    /// gauges at 0) and a scraper sees the line keys.
    #[test]
    fn render_a_zero_snapshot_is_not_empty() {
        let text = StatsSnapshot::default().render();
        assert!(text.contains("aap_meter_sink_queued_total 0\n"), "{text}");
        assert!(text.contains("aap_meter_sink_sent_total 0\n"), "{text}");
        assert!(text.contains("aap_meter_sink_queue_depth 0\n"), "{text}");
        assert!(text.contains("aap_meter_sink_retrying_secs 0\n"), "{text}");
    }
}
