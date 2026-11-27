//! Counts only: no event, no subject, no id.

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
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(1, |d| d.as_secs().max(1))
}

impl Stats {
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
