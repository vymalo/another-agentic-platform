//! The sink: `send` is a `try_send` on a bounded queue; one task batches, POSTs and retries.

use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aap_usage::{PodMinuteEvent, UsageError, UsageSink};
use reqwest::Client;
use reqwest::header::CONTENT_TYPE;
use serde_json::Value as Json;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior, interval, sleep, timeout};

use crate::client;
use crate::encode::encode_bytes;
use crate::settings::{SettingsError, UsageOtlpSettings};
use crate::stats::Stats;

/// The most of an answer that is read.
const MAX_ANSWER: usize = 64 * 1024;
/// How long a sink that is dropped without `shutdown` still tries to deliver what it holds.
const DROP_GRACE: Duration = Duration::from_secs(5);

struct Flush(oneshot::Sender<()>);

/// What `shutdown` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    /// Events that were still not delivered when the deadline ran out (also in `Stats::unsent_at_shutdown`).
    pub unsent: u64,
}

/// The sink. Build it with [`start`](Self::start) inside a Tokio runtime.
pub struct OtlpUsageSink {
    tx: mpsc::Sender<PodMinuteEvent>,
    ctl: mpsc::Sender<Flush>,
    closing: watch::Sender<Option<Instant>>,
    stats: Arc<Stats>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for OtlpUsageSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // No endpoint, no event.
        f.write_str("OtlpUsageSink")
    }
}

impl OtlpUsageSink {
    /// Validates the settings, builds the client (no network call) and spawns the one background task.
    ///
    /// # Errors
    ///
    /// A setting out of bounds, or a CA file that cannot be read or holds no certificate.
    ///
    /// # Panics
    ///
    /// Outside a Tokio runtime.
    pub fn start(settings: UsageOtlpSettings) -> Result<Self, SettingsError> {
        settings.validate()?;
        let client = client::build(&settings)?;
        let (tx, rx) = mpsc::channel(settings.queue);
        let (ctl, ctl_rx) = mpsc::channel(8);
        let (closing, closing_rx) = watch::channel(None);
        let stats = Arc::new(Stats::default());
        let worker = Worker {
            rx,
            ctl_rx,
            closing: closing_rx,
            client,
            url: settings.logs_url(),
            service: settings.service_name.clone(),
            batch: settings.batch,
            interval: settings.interval,
            backoff_min: settings.backoff_min,
            backoff_max: settings.backoff_max,
            warn_every: settings.warn_every,
            stats: stats.clone(),
        };
        let task = tokio::spawn(worker.run());
        Ok(Self {
            tx,
            ctl,
            closing,
            stats,
            task: Mutex::new(Some(task)),
        })
    }

    /// The counters, for the operator to render.
    #[must_use]
    pub fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }

    /// Sends what is queued now, without waiting for the interval; true when it was delivered (or permanently refused)
    /// within `within`. For tests and for a caller that wants a barrier; `send` never needs it.
    pub async fn flush(&self, within: Duration) -> bool {
        let (reply, done) = oneshot::channel();
        if self.ctl.try_send(Flush(reply)).is_err() {
            return false;
        }
        timeout(within, done).await.is_ok_and(|r| r.is_ok())
    }

    /// Stops taking events, delivers what it holds within `deadline` and ends the task. After it `send` is
    /// [`UsageError::Unavailable`]. Safe to call twice.
    pub async fn shutdown(&self, deadline: Duration) -> ShutdownReport {
        let handle = self
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut handle) = handle {
            self.closing.send_replace(Some(Instant::now() + deadline));
            if timeout(deadline + Duration::from_millis(500), &mut handle)
                .await
                .is_err()
            {
                // The task did not end in time: what it still held is lost with it, and counted.
                handle.abort();
                let left = self.stats.depth.load(Ordering::Relaxed);
                Stats::add(&self.stats.unsent_at_shutdown, left);
                self.stats.resolve(left);
            }
        }
        ShutdownReport {
            unsent: self.stats.unsent_at_shutdown.load(Ordering::Relaxed),
        }
    }
}

impl UsageSink for OtlpUsageSink {
    async fn send(&self, event: PodMinuteEvent) -> Result<(), UsageError> {
        event.validate()?;
        // `try_send`, never `send().await`: the meter's pass must not wait on a collector. A full queue refuses and
        // counts; the meter does not advance its cursor on an error, so the minute is sent again next pass.
        match self.tx.try_send(event) {
            Ok(()) => {
                Stats::add(&self.stats.queued, 1);
                Stats::add(&self.stats.depth, 1);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                Stats::add(&self.stats.dropped_queue_full, 1);
                Err(UsageError::Unavailable)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(UsageError::Unavailable),
        }
    }
}

struct Worker {
    rx: mpsc::Receiver<PodMinuteEvent>,
    ctl_rx: mpsc::Receiver<Flush>,
    closing: watch::Receiver<Option<Instant>>,
    client: Client,
    url: String,
    service: String,
    batch: usize,
    interval: Duration,
    backoff_min: Duration,
    backoff_max: Duration,
    warn_every: Duration,
    stats: Arc<Stats>,
}

enum Post {
    /// 2xx; the collector said it rejected this many records (usually 0).
    Accepted { rejected: u64 },
    /// 400, 415 or 422: the request itself is refused and will never succeed.
    Permanent(u16),
    /// 413: the batch is too large for the collector; it is split.
    TooLarge,
    /// Anything else; the status, or 0 for no answer.
    Transient(u16),
}

impl Worker {
    async fn run(mut self) {
        let mut buf: Vec<PodMinuteEvent> = Vec::with_capacity(self.batch);
        let mut tick = interval(self.interval);
        tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            if self.closing.borrow().is_some() {
                break;
            }
            tokio::select! {
                event = self.rx.recv() => match event {
                    Some(e) => {
                        buf.push(e);
                        if buf.len() >= self.batch {
                            self.send_buf(&mut buf, None).await;
                        }
                    }
                    None => break,
                },
                _ = tick.tick() => {
                    if !buf.is_empty() {
                        self.send_buf(&mut buf, None).await;
                    }
                }
                Some(Flush(reply)) = self.ctl_rx.recv() => {
                    while let Ok(e) = self.rx.try_recv() {
                        buf.push(e);
                        if buf.len() >= self.batch {
                            self.send_buf(&mut buf, None).await;
                        }
                    }
                    self.send_buf(&mut buf, None).await;
                    let _ = reply.send(());
                }
                changed = self.closing.changed() => {
                    if changed.is_err() {
                        break;
                    }
                }
            }
        }
        // Shutdown, or the sink was dropped: stop taking events and deliver what is held, within the deadline.
        self.rx.close();
        while let Ok(e) = self.rx.try_recv() {
            buf.push(e);
        }
        let fallback = Some(Instant::now() + DROP_GRACE);
        while !buf.is_empty() {
            self.send_buf(&mut buf, fallback).await;
        }
    }

    /// Sends up to one batch of `buf`, leaving the rest. Events not delivered by the deadline are counted.
    async fn send_buf(&mut self, buf: &mut Vec<PodMinuteEvent>, fallback: Option<Instant>) {
        if buf.is_empty() {
            return;
        }
        let take = buf.len().min(self.batch);
        let chunk: Vec<PodMinuteEvent> = buf.drain(..take).collect();
        let unsent = self.deliver(chunk, fallback).await;
        if unsent > 0 {
            // The deadline ran out: what is unsent and everything still held is given up, counted.
            // Events still in the channel are given up with the rest, and counted with it: none is dropped uncounted.
            let mut rest = buf.len() as u64;
            buf.clear();
            while self.rx.try_recv().is_ok() {
                rest += 1;
            }
            Stats::add(&self.stats.unsent_at_shutdown, unsent + rest);
            self.stats.resolve(unsent + rest);
        }
    }

    /// POSTs `events` until the collector takes them, or refuses them for good. **A transient failure keeps the
    /// batch** and repeats the same bytes after a capped, jittered, doubling wait; 413 halves it and sends each half.
    /// Only the shutdown's deadline ends the loop otherwise. Returns how many events it did not deliver (0 unless the
    /// deadline ran out).
    fn deliver(
        &mut self,
        events: Vec<PodMinuteEvent>,
        fallback: Option<Instant>,
    ) -> Pin<Box<dyn Future<Output = u64> + Send + '_>> {
        Box::pin(async move {
            let n = events.len() as u64;
            let bytes = encode_bytes(&self.service, &events);
            let mut backoff = self.backoff_min;
            let mut attempts = 0u64;
            let mut last_warn: Option<Instant> = None;
            let unsent = loop {
                let deadline = (*self.closing.borrow()).or(fallback);
                let limit = match deadline {
                    Some(d) => {
                        let left = d.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            break n;
                        }
                        Some(left)
                    }
                    None => None,
                };
                match self.post(&bytes, limit).await {
                    Post::Accepted { rejected } => {
                        let rejected = rejected.min(n);
                        Stats::add(&self.stats.sent, n - rejected);
                        Stats::add(&self.stats.refused, rejected);
                        self.stats.resolve(n);
                        if rejected > 0 {
                            tracing::warn!(
                                rejected,
                                events = n,
                                "the usage collector rejected records of a batch; they are not retried"
                            );
                        }
                        if attempts > 0 {
                            tracing::info!(attempts, "the usage collector is taking batches again");
                        }
                        break 0;
                    }
                    Post::Permanent(status) => {
                        // 400, 415 and 422 say the request itself is wrong and will never succeed; holding it would
                        // block every later minute behind it. Dropped and counted: the status is all that is said.
                        Stats::add(&self.stats.refused, n);
                        self.stats.resolve(n);
                        tracing::warn!(
                            status,
                            events = n,
                            "the usage collector refused a batch for good; it is dropped"
                        );
                        break 0;
                    }
                    Post::TooLarge => {
                        if events.len() == 1 {
                            Stats::add(&self.stats.refused, 1);
                            self.stats.resolve(1);
                            tracing::warn!(
                                status = 413u16,
                                events = 1,
                                "the usage collector refused a single event as too large; it is dropped"
                            );
                            break 0;
                        }
                        let mut first = events;
                        let second = first.split_off(first.len() / 2);
                        self.stats.retrying_since.store(0, Ordering::Relaxed);
                        let left = self.deliver(first, fallback).await;
                        if left > 0 {
                            break left + second.len() as u64;
                        }
                        return self.deliver(second, fallback).await;
                    }
                    Post::Transient(status) => {
                        Stats::add(&self.stats.retries, 1);
                        attempts += 1;
                        self.stats.mark_retrying();
                        if last_warn.is_none_or(|t| t.elapsed() >= self.warn_every) {
                            last_warn = Some(Instant::now());
                            tracing::warn!(
                                status_class = status_class(status),
                                attempts,
                                events = n,
                                "the usage collector did not take a batch; it is kept and sent again"
                            );
                        }
                    }
                }
                let mut nap_for = jitter(backoff);
                if let Some(d) = deadline {
                    nap_for = nap_for.min(d.saturating_duration_since(Instant::now()));
                }
                let mut nap = pin!(sleep(nap_for));
                tokio::select! {
                    () = &mut nap => {}
                    changed = self.closing.changed() => {
                        if changed.is_err() {
                            // The sink is gone: nothing will wake us, so just wait the backoff out.
                            nap.await;
                        }
                    }
                }
                backoff = (backoff * 2).min(self.backoff_max);
            };
            self.stats.retrying_since.store(0, Ordering::Relaxed);
            unsent
        })
    }

    async fn post(&self, bytes: &[u8], limit: Option<Duration>) -> Post {
        let request = self
            .client
            .post(&self.url)
            .header(CONTENT_TYPE, "application/json")
            .body(bytes.to_vec())
            .send();
        let response = match limit {
            Some(l) => match timeout(l, request).await {
                Ok(r) => r,
                Err(_) => return Post::Transient(0),
            },
            None => request.await,
        };
        let Ok(mut response) = response else {
            return Post::Transient(0);
        };
        let status = response.status().as_u16();
        match status {
            200..=299 => {
                let mut answer = Vec::new();
                while let Ok(Some(chunk)) = response.chunk().await {
                    if answer.len() + chunk.len() > MAX_ANSWER {
                        answer.clear();
                        break;
                    }
                    answer.extend_from_slice(&chunk);
                }
                Post::Accepted {
                    rejected: rejected(&answer),
                }
            }
            400 | 415 | 422 => Post::Permanent(status),
            413 => Post::TooLarge,
            // 401, 403 and 404 (an auth rollout, a wrong path: a person fixes it and holding loses nothing), 408, 429,
            // every other 4xx, 5xx and 3xx: the batch is kept.
            _ => Post::Transient(status),
        }
    }
}

/// The class of a status, for the log: never the status itself of a transient failure, never an answer.
fn status_class(status: u16) -> &'static str {
    match status {
        0 => "none",
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

/// `partialSuccess.rejectedLogRecords`, which JSON mapping writes as a string or a number.
fn rejected(body: &[u8]) -> u64 {
    let Ok(json) = serde_json::from_slice::<Json>(body) else {
        return 0;
    };
    match &json["partialSuccess"]["rejectedLogRecords"] {
        Json::String(s) => s.parse().unwrap_or(0),
        Json::Number(n) => n.as_u64().unwrap_or(0),
        _ => 0,
    }
}

/// `base` less up to a quarter of it, so replicas that fail together do not retry together. The cap stays `base`.
fn jitter(base: Duration) -> Duration {
    static COUNTER: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::from(d.subsec_nanos()));
    let mix = (nanos ^ COUNTER.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed))
        .wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let quarter = (mix >> 40) % 256; // 0..=255
    base - base.mul_f64(quarter as f64 / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_reads_a_string_or_a_number_and_nothing_else() {
        assert_eq!(
            rejected(br#"{"partialSuccess":{"rejectedLogRecords":"3"}}"#),
            3
        );
        assert_eq!(
            rejected(br#"{"partialSuccess":{"rejectedLogRecords":2}}"#),
            2
        );
        assert_eq!(rejected(b"{}"), 0);
        assert_eq!(rejected(b"not json"), 0);
    }

    #[test]
    fn jitter_stays_between_three_quarters_and_all_of_the_base() {
        let base = Duration::from_secs(8);
        for _ in 0..200 {
            let j = jitter(base);
            assert!(j <= base && j >= base.mul_f64(0.75), "{j:?}");
        }
    }
}
