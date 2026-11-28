//! Spans, the tracer that makes them and the scope that finds the current one.

use std::borrow::Cow;
use std::future::Future;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;

use crate::attr::{Attr, Controller, Method, RouteTemplate, Step};
use crate::pipeline::{Shared, Sink};
use crate::stats::Stats;

/// A span has at most this many attributes: the 17th is dropped.
pub const MAX_ATTRS: usize = 16;

/// The kind of a span. Only these two are made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpanKind {
    /// Work inside the process (OTLP `SPAN_KIND_INTERNAL`, 1).
    Internal,
    /// Work for a request (OTLP `SPAN_KIND_SERVER`, 2).
    Server,
}

impl SpanKind {
    /// The integer OTLP says.
    #[must_use]
    pub fn otlp(self) -> u8 {
        match self {
            Self::Internal => 1,
            Self::Server => 2,
        }
    }
}

/// The name of a span: built only from a [`Method`], a [`RouteTemplate`], a [`Step`], a [`Controller`], or a static name of the
/// alphabet `[a-z0-9_.: -]` (at most 64 characters, so a word or two of the program's own, in lower case). There is no
/// constructor from an unchecked string, and none takes a request's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpanName(Cow<'static, str>);

impl SpanName {
    /// The longest name made by [`SpanName::fixed`].
    pub const MAX_LEN: usize = 64;

    /// A name the program wrote: `Some` when it is non-empty, at most 64 characters, holds only `[a-z0-9_.: -]` and starts
    /// with a letter or a digit.
    #[must_use]
    pub fn fixed(name: &'static str) -> Option<Self> {
        let ok = !name.is_empty()
            && name.len() <= Self::MAX_LEN
            && name.as_bytes()[0].is_ascii_alphanumeric()
            && name.bytes().all(|b| {
                b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || matches!(b, b'_' | b'.' | b':' | b' ' | b'-')
            });
        ok.then_some(Self(Cow::Borrowed(name)))
    }

    /// The name of the root span of a pass of a controller this build does not know by name.
    pub const RECONCILE: Self = Self(Cow::Borrowed("reconcile"));

    /// The name of a step's span: its label.
    #[must_use]
    pub fn step(step: Step) -> Self {
        Self(Cow::Borrowed(step.as_str()))
    }

    /// The name of the root span of one pass of a controller.
    #[must_use]
    pub fn reconcile(controller: Controller) -> Self {
        Self(Cow::Borrowed(controller.span_name()))
    }

    /// `{METHOD} {template}` of a matched route.
    #[must_use]
    pub fn route(method: Method, template: RouteTemplate) -> Self {
        Self(Cow::Owned(format!(
            "{} {}",
            method.as_str(),
            template.as_str()
        )))
    }

    /// `{METHOD}` of a request that matched no route: the path is never part of a name.
    #[must_use]
    pub fn method(method: Method) -> Self {
        Self(Cow::Borrowed(match method {
            Method::Get => "GET",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
            Method::Post => "POST",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
            Method::Other => "HTTP",
        }))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A span that ended: what the pipeline queues and an exporter gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinishedSpan {
    /// The 16 bytes of the trace id.
    pub trace_id: [u8; 16],
    /// The 8 bytes of the span id.
    pub span_id: [u8; 8],
    /// The parent's span id, none for a root.
    pub parent_id: Option<[u8; 8]>,
    /// The name.
    pub name: SpanName,
    /// The kind.
    pub kind: SpanKind,
    /// Nanoseconds since the Unix epoch.
    pub start_unix_nanos: u64,
    /// Nanoseconds since the Unix epoch, never before the start.
    pub end_unix_nanos: u64,
    /// The attributes, each key at most once.
    pub attrs: Vec<Attr>,
    /// Whether the span failed: OTLP status `ERROR`, with no message. A span has no events and no links.
    pub error: bool,
}

/// How a root span is kept or dropped. The decision is a function of the trace id, so it is the same for the whole
/// trace and needs no state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sampler {
    /// Every trace.
    AlwaysOn,
    /// None.
    AlwaysOff,
    /// This fraction of traces, between 0 and 1.
    TraceIdRatio(f64),
}

impl Sampler {
    /// The decision that needs no id: `Some(true)` keep, `Some(false)` drop, `None` when the trace id decides
    /// (`TraceIdRatio` strictly between 0 and 1). `AlwaysOff` and a ratio of 0 draw nothing.
    fn without_id(self) -> Option<bool> {
        match self {
            Self::AlwaysOn => Some(true),
            Self::AlwaysOff => Some(false),
            Self::TraceIdRatio(ratio) => {
                if ratio >= 1.0 {
                    Some(true)
                } else if ratio <= 0.0 || ratio.is_nan() {
                    Some(false)
                } else {
                    None
                }
            }
        }
    }

    fn keeps(self, trace_id: &[u8; 16]) -> bool {
        match self.without_id() {
            Some(keep) => keep,
            None => {
                let Self::TraceIdRatio(ratio) = self else {
                    return false;
                };
                let mut low = [0u8; 8];
                low.copy_from_slice(&trace_id[8..]);
                let n = u64::from_be_bytes(low) >> 1;
                // 2^63 as the denominator: the arithmetic stays exact enough for a sampling ratio.
                #[allow(clippy::cast_precision_loss)]
                let position = n as f64 / (1u64 << 63) as f64;
                position < ratio
            }
        }
    }
}

struct Data {
    attrs: Vec<Attr>,
    error: bool,
}

struct Cell {
    tracer: Arc<Shared>,
    trace_id: [u8; 16],
    span_id: [u8; 8],
    parent_id: Option<[u8; 8]>,
    name: SpanName,
    kind: SpanKind,
    start: u64,
    data: Mutex<Option<Data>>,
}

fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

impl Cell {
    fn with(&self, f: impl FnOnce(&mut Data)) {
        let mut guard = self.data.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(data) = guard.as_mut() {
            f(data);
        }
    }

    fn set(&self, attr: Attr) {
        self.with(|d| {
            if let Some(slot) = d.attrs.iter_mut().find(|a| a.key() == attr.key()) {
                *slot = attr;
            } else if d.attrs.len() < MAX_ATTRS {
                d.attrs.push(attr);
            }
        });
    }

    fn finish(&self) {
        let data = self
            .data
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(data) = data else { return };
        let end = now_nanos().max(self.start);
        self.tracer.accept(FinishedSpan {
            trace_id: self.trace_id,
            span_id: self.span_id,
            parent_id: self.parent_id,
            name: self.name.clone(),
            kind: self.kind,
            start_unix_nanos: self.start,
            end_unix_nanos: end,
            attrs: data.attrs,
            error: data.error,
        });
    }
}

/// A span that is being made. Dropping it ends it. A span of a tracer that is off, or that the sampler dropped, is
/// inert: it holds nothing and every call on it does nothing.
#[must_use = "a span ends when it is dropped"]
pub struct Span(Option<Arc<Cell>>);

impl Span {
    const fn inert() -> Self {
        Self(None)
    }

    /// Whether this span records anything.
    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.0.is_some()
    }

    /// Set an attribute; the same key again replaces it.
    ///
    /// The argument is an [`Attr`], so no key or value of any other shape can be given:
    ///
    /// ```compile_fail
    /// # let span = aap_trace::Tracer::off().root(aap_trace::SpanName::method(aap_trace::Method::Get), aap_trace::SpanKind::Internal);
    /// span.set("user.email", "someone@example.com");
    /// ```
    pub fn set(&self, attr: Attr) {
        if let Some(c) = &self.0 {
            c.set(attr);
        }
    }

    /// Mark the span as failed. There is no message: the class goes in `error.type`.
    pub fn fail(&self) {
        if let Some(c) = &self.0 {
            c.with(|d| d.error = true);
        }
    }

    /// A child of this span.
    pub fn child(&self, name: SpanName, kind: SpanKind) -> Span {
        match &self.0 {
            Some(c) => Span(Some(Arc::new(Cell {
                tracer: c.tracer.clone(),
                trace_id: c.trace_id,
                span_id: random_span_id(),
                parent_id: Some(c.span_id),
                name,
                kind,
                start: now_nanos(),
                data: Mutex::new(Some(Data {
                    attrs: Vec::new(),
                    error: false,
                })),
            }))),
            None => Span::inert(),
        }
    }

    /// End the span now.
    pub fn end(self) {
        drop(self);
    }

    /// Run `fut` with this span as the current one: [`current_set`], [`current_fail`] and [`step`] inside it
    /// find this span. Nothing inside is linked to it unless it runs in the same task (a `tokio::spawn` starts
    /// a task with no span).
    ///
    /// `fut` is boxed before anything else, recording or not: a wrapper that holds a large future by value is
    /// copied onto the stack at every level of an unoptimised build, and a reconcile pass wrapped that way
    /// overflowed a worker's 2 MiB.
    pub fn scope<F: Future>(&self, fut: F) -> impl Future<Output = F::Output> + use<F> {
        let fut = Box::pin(fut);
        let cell = self.0.clone();
        async move {
            match cell {
                Some(c) => CURRENT.scope(c, fut).await,
                None => fut.await,
            }
        }
    }

    /// The ids, for a test.
    #[doc(hidden)]
    #[must_use]
    pub fn ids(&self) -> Option<([u8; 16], [u8; 8])> {
        self.0.as_ref().map(|c| (c.trace_id, c.span_id))
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(c) = self.0.take() {
            c.finish();
        }
    }
}

tokio::task_local! {
    static CURRENT: Arc<Cell>;
}

/// Set an attribute on the current span, if there is one.
pub fn current_set(attr: Attr) {
    let _ = CURRENT.try_with(|c| c.set(attr));
}

/// Mark the current span as failed, if there is one.
pub fn current_fail() {
    let _ = CURRENT.try_with(|c| c.with(|d| d.error = true));
}

fn current_child(name: SpanName) -> Span {
    CURRENT
        .try_with(|c| {
            Span(Some(Arc::new(Cell {
                tracer: c.tracer.clone(),
                trace_id: c.trace_id,
                span_id: random_span_id(),
                parent_id: Some(c.span_id),
                name,
                kind: SpanKind::Internal,
                start: now_nanos(),
                data: Mutex::new(Some(Data {
                    attrs: Vec::new(),
                    error: false,
                })),
            })))
        })
        .unwrap_or_else(|_| Span::inert())
}

/// A child of the current span with a name the program wrote ([`SpanName::fixed`]), or an inert span when there is no current
/// span or the name is not one.
pub fn child(name: &'static str) -> Span {
    let name = SpanName::fixed(name);
    debug_assert!(name.is_some(), "not a span name");
    match name {
        Some(name) => current_child(name),
        None => Span::inert(),
    }
}

/// Ends a step span as a timeout when the future of the step is dropped before it finished: a pass cut by its time limit
/// drops the step it was in, which must not look successful.
struct Unfinished(Option<Span>);

impl Drop for Unfinished {
    fn drop(&mut self) {
        if let Some(span) = self.0.take() {
            if let Some(a) = Attr::error_type("timeout") {
                span.set(a);
            }
            span.fail();
        }
    }
}

/// Run `fut` as a child span of the current span named by `step`, with `aap.step` set. `class` reads the output: when
/// it says `Some(class)` the span fails and carries `error.type = class`, and nothing else of the output. A step whose future
/// is dropped before it finishes (a pass cut by its time limit) ends failed, with `error.type = timeout`.
pub fn step<T, F>(
    step: Step,
    fut: F,
    class: impl FnOnce(&T) -> Option<&'static str>,
) -> impl Future<Output = T>
where
    F: Future<Output = T>,
{
    // Boxed here for the reason [`Span::scope`] gives: what this returns is a few words, whatever `fut` is.
    let fut = Box::pin(fut);
    async move {
        let mut guard = Unfinished(Some(current_child(SpanName::step(step))));
        let out = match &guard.0 {
            Some(span) => {
                span.set(Attr::Step(step));
                span.scope(fut).await
            }
            None => fut.await,
        };
        if let Some(span) = guard.0.take()
            && let Some(class) = class(&out)
            && let Some(a) = Attr::error_type(class)
        {
            span.set(a);
            span.fail();
        }
        out
    }
}

fn random_span_id() -> [u8; 8] {
    let mut id = [0u8; 8];
    // A failed draw leaves zeros, which OTLP calls invalid: the exporter skips such a span.
    let _ = getrandom::getrandom(&mut id);
    id
}

/// The handle both binaries pass around. Cheap to clone; off by default. The second field is whether tracing was asked for
/// (a tracing variable was set) while the tracer is off: a setting was refused, or no endpoint was given.
#[derive(Clone, Default)]
pub struct Tracer(pub(crate) Option<Arc<Shared>>, pub(crate) bool);

impl std::fmt::Debug for Tracer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Tracer(on)"
        } else {
            "Tracer(off)"
        })
    }
}

impl Tracer {
    /// A tracer that makes no span: no task, no queue, no random draw.
    #[must_use]
    pub const fn off() -> Self {
        Self(None, false)
    }

    /// A tracer that makes no span, for a process that was asked for tracing and could not give it (a setting was refused): it
    /// is [`asked`](Self::asked), so `/metrics` says `aap_trace_enabled 0`.
    #[must_use]
    pub const fn off_but_asked() -> Self {
        Self(None, true)
    }

    /// Whether tracing was asked for: the tracer is on, or it is off in spite of a tracing variable.
    #[must_use]
    pub fn asked(&self) -> bool {
        self.0.is_some() || self.1
    }

    /// Whether spans are made.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.0.is_some()
    }

    /// A tracer that keeps its spans in memory, for the tests of a crate that makes spans.
    #[cfg(feature = "testkit")]
    #[must_use]
    pub fn in_memory(sampler: Sampler) -> (Self, crate::testkit::Memory) {
        let memory = crate::testkit::Memory::default();
        let shared = Shared {
            sink: Sink::Memory(memory.clone()),
            sampler,
            stats: Arc::new(Stats::default()),
            stopping: std::sync::atomic::AtomicBool::new(false),
            control: Mutex::new(None),
        };
        (Self(Some(Arc::new(shared)), true), memory)
    }

    /// A new root span (a new trace), if the tracer is on and the sampler keeps it. `AlwaysOff`, a ratio of 0 and a ratio of
    /// 1 are decided before any random draw; a ratio between them needs the trace id (the decision is a function of it, so it
    /// is the same for the whole trace), so the id is drawn first and the span id only for a trace that is kept.
    pub fn root(&self, name: SpanName, kind: SpanKind) -> Span {
        let Some(shared) = &self.0 else {
            return Span::inert();
        };
        if shared.sampler.without_id() == Some(false) {
            return Span::inert();
        }
        let mut trace_id = [0u8; 16];
        let mut span_id = [0u8; 8];
        if getrandom::getrandom(&mut trace_id).is_err()
            || trace_id == [0u8; 16]
            || !shared.sampler.keeps(&trace_id)
            || getrandom::getrandom(&mut span_id).is_err()
            || span_id == [0u8; 8]
        {
            return Span::inert();
        }
        Span(Some(Arc::new(Cell {
            tracer: shared.clone(),
            trace_id,
            span_id,
            parent_id: None,
            name,
            kind,
            start: now_nanos(),
            data: Mutex::new(Some(Data {
                attrs: Vec::new(),
                error: false,
            })),
        })))
    }

    /// The counters, for `/metrics`: none when tracing is off.
    #[must_use]
    pub fn stats(&self) -> Option<Arc<Stats>> {
        self.0.as_ref().map(|s| s.stats.clone())
    }
}

impl Shared {
    pub(crate) fn accept(&self, span: FinishedSpan) {
        match &self.sink {
            Sink::Queue(tx) => {
                // A span that ends after the shutdown began has nowhere to go: it is counted, under its own reason.
                if self.stopping.load(Ordering::Acquire) {
                    self.stats.shutdown.fetch_add(1, Ordering::Relaxed);
                } else if tx.try_send(span).is_ok() {
                    self.stats.accepted.fetch_add(1, Ordering::Relaxed);
                } else {
                    self.stats.queue_full.fetch_add(1, Ordering::Relaxed);
                }
            }
            #[cfg(feature = "testkit")]
            Sink::Memory(m) => m.push(span),
        }
    }
}

pub(crate) fn queue(capacity: usize) -> (mpsc::Sender<FinishedSpan>, mpsc::Receiver<FinishedSpan>) {
    mpsc::channel(capacity.max(1))
}
