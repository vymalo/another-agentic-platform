//! The bounded pipeline between `Span::end` and an exporter.
//!
//! `end` is a `try_send` on a queue of fixed size: it never waits, never allocates a second queue and never fails the
//! caller. One task takes spans off the queue, makes batches and calls the [`SpanExporter`]. A batch an exporter
//! fails is dropped and counted, never retried, and the task backs off (1 s, doubling, at most 60 s) before the next
//! export, so a collector that is down costs a bounded queue and one request in flight.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};

use crate::span::{FinishedSpan, Sampler, Tracer, queue};
use crate::stats::{Stats, StatsSnapshot};

/// Why an export did not deliver everything. There is no text: an error from a collector is its own business
/// and never reaches a log or a span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// Nothing was delivered: unreachable, refused, timed out, a status that is not a success.
    Failed,
    /// The collector took the request and said it rejected this many spans.
    Rejected(usize),
}

/// The attributes of the process that makes the spans, set once. The values come from the program and from
/// settings that a pattern validated; no request puts anything here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    /// `service.name`.
    pub service_name: String,
    /// `service.version`.
    pub service_version: String,
    /// `service.instance.id` and `k8s.pod.name`, from `POD_NAME`.
    pub pod_name: Option<String>,
    /// `k8s.namespace.name`, from `POD_NAMESPACE`.
    pub namespace: Option<String>,
    /// The three closed keys a deployment may add.
    pub extra: Vec<(ResourceKey, String)>,
}

/// The only keys a deployment may add to the resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResourceKey {
    /// `deployment.environment.name`.
    DeploymentEnvironmentName,
    /// `k8s.cluster.name`.
    K8sClusterName,
    /// `service.namespace`.
    ServiceNamespace,
}

impl ResourceKey {
    /// The key.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeploymentEnvironmentName => "deployment.environment.name",
            Self::K8sClusterName => "k8s.cluster.name",
            Self::ServiceNamespace => "service.namespace",
        }
    }

    /// The key named `s`.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        [
            Self::DeploymentEnvironmentName,
            Self::K8sClusterName,
            Self::ServiceNamespace,
        ]
        .into_iter()
        .find(|k| k.as_str() == s)
    }
}

impl Resource {
    /// A resource with a name and a version.
    #[must_use]
    pub fn new(service_name: impl Into<String>, service_version: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_version: service_version.into(),
            pod_name: None,
            namespace: None,
            extra: Vec::new(),
        }
    }
}

/// Where finished spans go. Implementations are separate crates (AD-020); [`exporter_conformance!`](crate::exporter_conformance)
/// is what one must pass.
pub trait SpanExporter: Send + Sync + 'static {
    /// Deliver a batch, once. An `Err` drops the batch (the pipeline counts it); a call that never returns is cut
    /// by the pipeline's own limit. Never panic.
    fn export(
        &self,
        resource: &Resource,
        batch: Vec<FinishedSpan>,
    ) -> impl Future<Output = Result<(), ExportError>> + Send;

    /// Called once when exports start failing after a success (or from the start): the first failure of an episode, not
    /// each one. An exporter may log one line here (a fixed reason and a host, nothing from an answer). Never panic.
    fn export_failing(&self) {}
}

/// The sizes of the pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Spans that wait for the exporter; the next one is dropped.
    pub queue: usize,
    /// Spans in one export.
    pub batch: usize,
    /// A batch goes at the latest this long after its first span.
    pub interval: Duration,
    /// The wait after the first failed export.
    pub backoff_min: Duration,
    /// The wait never grows past this.
    pub backoff_max: Duration,
    /// An export that takes this long is a failed one.
    pub export_timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            queue: 2048,
            batch: 512,
            interval: Duration::from_secs(5),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            export_timeout: Duration::from_secs(30),
        }
    }
}

pub(crate) enum Sink {
    Queue(mpsc::Sender<FinishedSpan>),
    #[cfg(feature = "testkit")]
    Memory(crate::testkit::Memory),
}

pub(crate) struct Control {
    stop: watch::Sender<bool>,
    handle: JoinHandle<()>,
}

pub(crate) struct Shared {
    pub(crate) sink: Sink,
    pub(crate) sampler: Sampler,
    pub(crate) stats: Arc<Stats>,
    /// Set when the shutdown begins: a span that ends after it is counted and dropped.
    pub(crate) stopping: AtomicBool,
    pub(crate) control: Mutex<Option<Control>>,
}

/// Starts the pipeline.
pub struct Pipeline;

impl Pipeline {
    /// Make a tracer whose spans reach `exporter`. Call it inside a Tokio runtime.
    pub fn start<E: SpanExporter>(
        exporter: E,
        limits: Limits,
        resource: Resource,
        sampler: Sampler,
    ) -> Tracer {
        let (tx, rx) = queue(limits.queue);
        let (stop_tx, stop_rx) = watch::channel(false);
        let stats = Arc::new(Stats::default());
        let handle = tokio::spawn(run(exporter, resource, limits, rx, stop_rx, stats.clone()));
        Tracer(
            Some(Arc::new(Shared {
                sink: Sink::Queue(tx),
                sampler,
                stats,
                stopping: AtomicBool::new(false),
                control: Mutex::new(Some(Control {
                    stop: stop_tx,
                    handle,
                })),
            })),
            true,
        )
    }
}

impl Tracer {
    /// Flush what is queued and stop the pipeline, for at most `deadline`. What an exporter did not take by then is
    /// dropped. Returns the counters. A tracer that is off, or already shut down, returns at once.
    pub async fn shutdown(&self, deadline: Duration) -> StatsSnapshot {
        let Some(shared) = &self.0 else {
            return StatsSnapshot::default();
        };
        shared.stopping.store(true, Ordering::Release);
        let control = shared
            .control
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(Control { stop, mut handle }) = control {
            let _ = stop.send(true);
            if timeout(deadline, &mut handle).await.is_err() {
                handle.abort();
                // What the queue took and no export accounted for (exported, failed, rejected) was in the worker's hands
                // or still queued: it is lost at the deadline, and counted.
                let s = &shared.stats;
                let done = s.exported.load(Ordering::Relaxed)
                    + s.rejected.load(Ordering::Relaxed)
                    + s.export_failed.load(Ordering::Relaxed);
                let lost = s.accepted.load(Ordering::Relaxed).saturating_sub(done);
                s.flush_deadline.fetch_add(lost, Ordering::Relaxed);
            }
        }
        shared.stats.snapshot()
    }
}

/// Resolves when a stop was asked for. The guard of `wait_for` is dropped inside, so the future is `Send`.
async fn stopped(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|stopped| *stopped).await;
}

struct Worker<E> {
    exporter: E,
    resource: Resource,
    limits: Limits,
    stats: Arc<Stats>,
    stop: watch::Receiver<bool>,
    backoff: Option<Duration>,
}

impl<E: SpanExporter> Worker<E> {
    async fn export(&self, batch: Vec<FinishedSpan>) -> bool {
        let n = batch.len();
        let n64 = n as u64;
        match timeout(
            self.limits.export_timeout,
            self.exporter.export(&self.resource, batch),
        )
        .await
        {
            Ok(Ok(())) => {
                self.stats.exported.fetch_add(n64, Ordering::Relaxed);
                true
            }
            Ok(Err(ExportError::Rejected(r))) => {
                let r = r.min(n);
                self.stats
                    .exported
                    .fetch_add((n - r) as u64, Ordering::Relaxed);
                self.stats.rejected.fetch_add(r as u64, Ordering::Relaxed);
                true
            }
            Ok(Err(ExportError::Failed)) | Err(_) => {
                self.stats.export_failed.fetch_add(n64, Ordering::Relaxed);
                if self.backoff.is_none() {
                    self.exporter.export_failing();
                }
                false
            }
        }
    }

    /// Export, and after a failure wait out the back-off (cut short by a stop).
    async fn flush(&mut self, buf: &mut Vec<FinishedSpan>) {
        if buf.is_empty() {
            return;
        }
        let batch = std::mem::take(buf);
        if self.export(batch).await {
            self.backoff = None;
            return;
        }
        let wait = self.backoff.map_or(self.limits.backoff_min, |b| {
            (b * 2).min(self.limits.backoff_max)
        });
        self.backoff = Some(wait);
        tokio::select! {
            () = sleep(wait) => {}
            () = stopped(&mut self.stop) => {}
        }
    }
}

async fn run<E: SpanExporter>(
    exporter: E,
    resource: Resource,
    limits: Limits,
    mut rx: mpsc::Receiver<FinishedSpan>,
    stop: watch::Receiver<bool>,
    stats: Arc<Stats>,
) {
    let mut w = Worker {
        exporter,
        resource,
        limits,
        stats,
        stop: stop.clone(),
        backoff: None,
    };
    let mut buf: Vec<FinishedSpan> = Vec::with_capacity(limits.batch);
    let mut tick = interval(limits.interval.max(Duration::from_millis(1)));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut stop_wait = stop;
    loop {
        tokio::select! {
            biased;
            () = stopped(&mut stop_wait) => break,
            _ = tick.tick() => w.flush(&mut buf).await,
            got = rx.recv() => match got {
                Some(span) => {
                    buf.push(span);
                    if buf.len() >= limits.batch {
                        w.flush(&mut buf).await;
                    }
                }
                None => break,
            },
        }
    }
    // The flush: what is queued goes out in batches, with no back-off, until a batch fails.
    rx.close();
    while let Ok(span) = rx.try_recv() {
        buf.push(span);
        if buf.len() >= limits.batch && !w.export(std::mem::take(&mut buf)).await {
            break;
        }
    }
    if !buf.is_empty() {
        let rest = std::mem::take(&mut buf);
        w.export(rest).await;
    }
    // A batch that failed during the flush leaves spans behind: they are dropped, and counted.
    let mut left = 0u64;
    while rx.try_recv().is_ok() {
        left += 1;
    }
    w.stats.export_failed.fetch_add(left, Ordering::Relaxed);
}
