#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! Traces of the operator and the Platform API (K5, §48a).
//!
//! * [`Attr`] is the closed list of what a span can say; there is no string key anywhere.
//! * [`Tracer`] makes spans (`off()` makes none), [`Span`] ends on `drop` with a `try_send`, and
//!   [`step`], [`current_set`] and [`Span::scope`] find the current span of a task.
//! * [`Pipeline`] queues, batches and hands spans to a [`SpanExporter`]; it never blocks and never crashes the caller.
//! * The feature `testkit` adds `MemoryExporter`, `Tracer::in_memory`, `exporter_conformance!` and `assert_clean`.
//!
//! This crate has no HTTP and no OpenTelemetry library; `aap-trace-otlp` is the exporter.

mod attr;
mod pipeline;
mod span;
mod stats;
#[cfg(feature = "testkit")]
pub mod testkit;

pub use attr::{Attr, Controller, K8sName, Method, ObjectKind, Outcome, ServiceState, Step, Value};
pub use pipeline::{ExportError, Limits, Pipeline, Resource, ResourceKey, SpanExporter};
pub use span::{
    FinishedSpan, MAX_ATTRS, Sampler, Span, SpanKind, SpanName, Tracer, child, current_fail,
    current_set, step,
};
pub use stats::{Stats, StatsSnapshot};
