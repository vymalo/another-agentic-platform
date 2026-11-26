#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
//! The OTLP/HTTP JSON exporter of the traces of the operator and the Platform API (K5, §48a).
//!
//! * [`TraceSettings::from_env`]: the standard `OTEL_*` variables, a strict subset, in one pure function.
//! * [`OtlpJson`]: a [`SpanExporter`](aap_trace::SpanExporter) on reqwest 0.12 (rustls with ring).
//! * [`encode`]: the hand-written OTLP/JSON encoding.
//! * The feature `testkit` adds `TestCollector`, an OTLP/JSON receiver on a local port.

mod encode;
mod exporter;
mod headers;
mod process;
mod settings;
#[cfg(feature = "testkit")]
pub mod testkit;

pub use encode::{encode, span_count};
pub use exporter::OtlpJson;
pub use headers::{Headers, MAX_HEADERS_FILE};
pub use process::{
    Started, check, start, start_section, start_with, tracer, tracer_from, tracer_with,
};
pub use settings::{
    DEFAULT_TIMEOUT, MAX_TIMEOUT, OtlpSection, SecretRef, SettingsError, TraceSettings, VARIABLES,
    any_set,
};
