//! `aap-usage-otlp` (ANR-1102, AD-069): the [`aap_usage::UsageSink`] that sends pod-minute events to the OpenTelemetry
//! Collector as OTLP/HTTP JSON **logs**, written by hand on the workspace's reqwest 0.12 (no SDK, no new package).
//!
//! Unlike the trace exporter, **this one never drops a batch the collector might take later**: a failed POST keeps the
//! batch and sends the same bytes again, because a lost minute is money. Duplicates are safe, the collector dedupes on
//! `event_id`. See the crate's README for the wire and the failure behaviour.

mod client;
mod encode;
mod settings;
mod sink;
mod stats;
#[cfg(feature = "testkit")]
pub mod testkit;

pub use encode::{encode, encode_bytes};
pub use settings::{SettingsError, UsageOtlpSettings};
pub use sink::{OtlpUsageSink, ShutdownReport};
pub use stats::{Stats, StatsSnapshot};
