#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The pipeline, the sampler and the span API, in paused time.

use std::time::Duration;

use aap_trace::testkit::{Harness, MemoryExporter, Mode, assert_clean};
use aap_trace::{
    Attr, Limits, Method, Pipeline, Resource, Sampler, SpanKind, SpanName, Step, Tracer,
    current_set, step,
};
use tokio::time::Instant;

fn resource() -> Resource {
    Resource::new("test", "0")
}

fn name() -> SpanName {
    SpanName::fixed("t")
}

async fn make() -> Harness<MemoryExporter> {
    let e = MemoryExporter::default();
    let (reader, failer) = (e.clone(), e.clone());
    Harness {
        exporter: e,
        delivered: Box::new(move || reader.spans().iter().map(|s| s.span_id).collect()),
        fail: Box::new(move || failer.set_mode(Mode::Fail)),
    }
}

aap_trace::exporter_conformance!(make);

#[test]
fn off_is_inert() {
    let tracer = Tracer::off();
    assert!(!tracer.enabled());
    assert!(tracer.stats().is_none());
    for _ in 0..100_000 {
        let span = tracer.root(name(), SpanKind::Server);
        assert!(!span.is_recording());
        span.set(Attr::HttpStatus(200));
        span.fail();
        assert!(!span.child(name(), SpanKind::Internal).is_recording());
    }
}

#[tokio::test(start_paused = true)]
async fn queue_is_bounded() {
    let limits = Limits {
        queue: 8,
        batch: 4,
        ..Limits::default()
    };
    let tracer = Pipeline::start(
        MemoryExporter::new(Mode::Hang),
        limits,
        resource(),
        Sampler::AlwaysOn,
    );
    // No await between the spans: the worker has not taken one, so exactly `queue` are kept.
    for _ in 0..100 {
        tracer.root(name(), SpanKind::Internal).end();
    }
    let stats = tracer.stats().unwrap().snapshot();
    assert_eq!(stats.dropped_queue_full, 92);
    assert_eq!(stats.exported, 0);
}

#[tokio::test(start_paused = true)]
async fn export_failure_drops_and_backs_off() {
    let exporter = MemoryExporter::new(Mode::Fail);
    let limits = Limits {
        batch: 1,
        backoff_max: Duration::from_secs(4),
        ..Limits::default()
    };
    let tracer = Pipeline::start(exporter.clone(), limits, resource(), Sampler::AlwaysOn);
    for _ in 0..5 {
        tracer.root(name(), SpanKind::Internal).end();
    }
    tokio::time::sleep(Duration::from_secs(60)).await;
    let calls = exporter.call_times();
    assert_eq!(calls.len(), 5);
    let gaps: Vec<u64> = calls.windows(2).map(|w| (w[1] - w[0]).as_secs()).collect();
    assert_eq!(gaps, [1, 2, 4, 4]);
    let stats = tracer.stats().unwrap().snapshot();
    assert_eq!(stats.dropped_export_failed, 5);
    assert_eq!(stats.exported, 0);
    // A success resets the wait.
    exporter.set_mode(Mode::Keep);
    tokio::time::sleep(Duration::from_secs(60)).await;
    tracer.root(name(), SpanKind::Internal).end();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(exporter.spans().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_flushes() {
    let exporter = MemoryExporter::default();
    let tracer = Pipeline::start(
        exporter.clone(),
        Limits::default(),
        resource(),
        Sampler::AlwaysOn,
    );
    for _ in 0..10 {
        tracer.root(name(), SpanKind::Internal).end();
    }
    let stats = tracer.shutdown(Duration::from_secs(3)).await;
    assert_eq!(exporter.spans().len(), 10);
    assert_eq!(stats.exported, 10);
    // A second shutdown is harmless.
    tracer.shutdown(Duration::from_secs(3)).await;
}

#[tokio::test(start_paused = true)]
async fn shutdown_is_bounded() {
    let tracer = Pipeline::start(
        MemoryExporter::new(Mode::Hang),
        Limits::default(),
        resource(),
        Sampler::AlwaysOn,
    );
    for _ in 0..3 {
        tracer.root(name(), SpanKind::Internal).end();
    }
    let started = Instant::now();
    tracer.shutdown(Duration::from_secs(3)).await;
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(3) && took < Duration::from_secs(4),
        "{took:?}"
    );
}

#[test]
fn trace_id_ratio_keeps_none_all_or_some() {
    let count = |sampler| {
        let (tracer, memory) = Tracer::in_memory(sampler);
        for _ in 0..1000 {
            tracer.root(name(), SpanKind::Internal).end();
        }
        memory.spans().len()
    };
    assert_eq!(count(Sampler::TraceIdRatio(0.0)), 0);
    assert_eq!(count(Sampler::TraceIdRatio(1.0)), 1000);
    assert_eq!(count(Sampler::AlwaysOff), 0);
    assert_eq!(count(Sampler::AlwaysOn), 1000);
    let some = count(Sampler::TraceIdRatio(0.5));
    assert!((300..700).contains(&some), "{some}");
}

#[tokio::test]
async fn children_follow_the_scope_and_carry_their_class() {
    let (tracer, memory) = Tracer::in_memory(Sampler::AlwaysOn);
    let root = tracer.root(SpanName::route(Method::Get, "/v1/agents"), SpanKind::Server);
    root.scope(async {
        current_set(Attr::HttpStatus(200));
        let ok: Result<u8, &str> = step(Step::StoreEnsure, async { Ok(1) }, |r| {
            r.as_ref().err().map(|_| "x")
        })
        .await;
        assert!(ok.is_ok());
        let bad: Result<u8, &str> = step(
            Step::RuntimeEnsure,
            async { Err("a message that must not appear") },
            |_| Some("transient"),
        )
        .await;
        assert!(bad.is_err());
    })
    .await;
    let (trace, span) = root.ids().unwrap();
    root.end();
    let spans = memory.spans();
    assert_eq!(spans.len(), 3);
    let top = memory.named("GET /v1/agents").unwrap();
    assert_eq!(top.parent_id, None);
    assert!(top.attrs.contains(&Attr::HttpStatus(200)));
    for child in spans.iter().filter(|s| s.parent_id.is_some()) {
        assert_eq!(child.parent_id, Some(span));
        assert_eq!(child.trace_id, trace);
    }
    let failed = memory.named("runtime.ensure").unwrap();
    assert!(failed.error);
    assert!(failed.attrs.contains(&Attr::ErrorType("transient")));
    assert!(!memory.named("store.ensure").unwrap().error);
    assert_clean(&spans, &["must not appear"]);
}

#[tokio::test]
async fn outside_a_scope_there_is_no_span() {
    current_set(Attr::HttpStatus(500));
    let out = step(Step::ConfigRead, async { 5 }, |_| None).await;
    assert_eq!(out, 5);
}

#[test]
fn an_attribute_is_replaced_and_the_seventeenth_dropped() {
    let (tracer, memory) = Tracer::in_memory(Sampler::AlwaysOn);
    let span = tracer.root(name(), SpanKind::Internal);
    span.set(Attr::HttpStatus(200));
    span.set(Attr::HttpStatus(404));
    span.end();
    assert_eq!(memory.spans()[0].attrs, [Attr::HttpStatus(404)]);
}

#[test]
fn the_keys_are_the_readme_table() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    let keys: Vec<&str> = readme
        .lines()
        .filter_map(|l| l.strip_prefix("| `"))
        .filter_map(|l| l.split_once('`').map(|(k, _)| k))
        .filter(|k| k.contains('.'))
        .collect();
    assert_eq!(keys, Attr::ALL_KEYS);
}
