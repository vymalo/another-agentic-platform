#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The pipeline, the sampler and the span API, in paused time.

use std::time::Duration;

use aap_trace::testkit::{Harness, MemoryExporter, Mode, assert_clean};
use aap_trace::{
    Attr, Limits, Method, Pipeline, Resource, RouteTemplate, Sampler, SpanKind, SpanName, Step,
    Tracer, current_set, step,
};
use tokio::time::Instant;

fn resource() -> Resource {
    Resource::new("test", "0")
}

fn name() -> SpanName {
    SpanName::fixed("t").unwrap()
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
    // Five failed exports are one episode: the exporter was told once.
    assert_eq!(exporter.failing_episodes(), 1);
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
    let root = tracer.root(
        SpanName::route(Method::Get, RouteTemplate::new("/v1/agents").unwrap()),
        SpanKind::Server,
    );
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
    assert!(
        failed
            .attrs
            .contains(&Attr::error_type("transient").unwrap())
    );
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
fn a_second_set_of_a_key_replaces_the_first() {
    let (tracer, memory) = Tracer::in_memory(Sampler::AlwaysOn);
    let span = tracer.root(name(), SpanKind::Internal);
    // The cap of `MAX_ATTRS` (16) is a defence that cannot be reached today: there are 15 keys, and a key set twice is
    // replaced, so no span can hold a seventeenth. A test cannot drive it; adding a 17th key would need one.
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

#[test]
fn a_span_name_refuses_free_text() {
    for ok in ["reconcile agentservice", "store.ensure", "t", "a-b_c:d.e 1"] {
        assert!(SpanName::fixed(ok).is_some(), "{ok}");
    }
    for bad in [
        "",
        " leading",
        "Upper",
        "a@b",
        "a?b",
        "a=b",
        "a/b",
        "GET /v1/agents/my-agent",
        "tab\there",
    ] {
        assert!(SpanName::fixed(bad).is_none(), "{bad:?}");
    }
    let long: &'static str = "a".repeat(65).leak();
    assert!(SpanName::fixed(long).is_none());
}

#[tokio::test]
async fn a_step_dropped_before_it_finishes_ends_as_a_timeout() {
    let (tracer, memory) = Tracer::in_memory(Sampler::AlwaysOn);
    let root = tracer.root(name(), SpanKind::Internal);
    root.scope(async {
        // The pass is cut by its time limit: the step's future is dropped while it waits.
        let cut = tokio::time::timeout(
            Duration::from_millis(20),
            step(Step::RuntimeEnsure, std::future::pending::<u8>(), |_| None),
        )
        .await;
        assert!(cut.is_err());
        // A step that finishes is not touched by the guard.
        let ok = step(Step::StoreEnsure, async { 1u8 }, |_| None).await;
        assert_eq!(ok, 1);
    })
    .await;
    root.end();
    let cut = memory.named("runtime.ensure").unwrap();
    assert!(cut.error);
    assert!(cut.attrs.contains(&Attr::error_type("timeout").unwrap()));
    let ok = memory.named("store.ensure").unwrap();
    assert!(!ok.error);
    assert!(!ok.attrs.iter().any(|a| a.key() == "error.type"));
}

#[tokio::test(start_paused = true)]
async fn a_span_that_ends_after_the_shutdown_began_is_counted_under_its_own_reason() {
    let tracer = Pipeline::start(
        MemoryExporter::default(),
        Limits::default(),
        resource(),
        Sampler::AlwaysOn,
    );
    let late = tracer.root(name(), SpanKind::Internal);
    let stats = tracer.shutdown(Duration::from_secs(3)).await;
    assert_eq!(stats.dropped_shutdown, 0);
    late.end();
    assert_eq!(tracer.stats().unwrap().snapshot().dropped_shutdown, 1);
    assert_eq!(tracer.stats().unwrap().snapshot().dropped_queue_full, 0);
}

#[tokio::test(start_paused = true)]
async fn a_batch_lost_at_the_flush_deadline_is_counted() {
    let tracer = Pipeline::start(
        MemoryExporter::new(Mode::Hang),
        Limits::default(),
        resource(),
        Sampler::AlwaysOn,
    );
    for _ in 0..3 {
        tracer.root(name(), SpanKind::Internal).end();
    }
    let stats = tracer.shutdown(Duration::from_secs(3)).await;
    assert_eq!(stats.dropped_flush_deadline, 3);
    assert_eq!(stats.exported, 0);
}

#[test]
fn a_sampler_that_drops_everything_makes_no_span_at_all() {
    for sampler in [Sampler::AlwaysOff, Sampler::TraceIdRatio(0.0)] {
        let (tracer, memory) = Tracer::in_memory(sampler);
        let span = tracer.root(name(), SpanKind::Internal);
        assert!(!span.is_recording());
        assert!(span.ids().is_none());
        span.end();
        assert!(memory.spans().is_empty());
    }
}

/// A future that holds 64 KiB across an await.
async fn big() -> u8 {
    let a = [0u8; 65_536];
    tokio::task::yield_now().await;
    std::hint::black_box(&a);
    1
}

#[test]
fn a_scope_does_not_hold_its_future_by_value() {
    assert!(size_of_val(&big()) >= 65_536, "the control is large");
    for tracer in [Tracer::off(), Tracer::in_memory(Sampler::AlwaysOn).0] {
        let span = tracer.root(name(), SpanKind::Internal);
        let size = size_of_val(&span.scope(big()));
        assert!(size < 1024, "{size} bytes");
    }
}

#[test]
fn a_step_does_not_hold_its_future_by_value() {
    let fut = step(Step::StoreEnsure, big(), |_| None);
    let size = size_of_val(&fut);
    assert!(size < 1024, "{size} bytes");
}

#[test]
fn the_defaults_are_the_numbers_of_the_operations_section() {
    assert_eq!(
        Limits::default(),
        Limits {
            queue: 2048,
            batch: 512,
            interval: Duration::from_secs(5),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            export_timeout: Duration::from_secs(30),
        }
    );
}

#[tokio::test(start_paused = true)]
async fn a_success_resets_the_backoff_and_the_episode() {
    let exporter = MemoryExporter::new(Mode::Fail);
    let limits = Limits {
        batch: 1,
        ..Limits::default()
    };
    let tracer = Pipeline::start(exporter.clone(), limits, resource(), Sampler::AlwaysOn);
    tracer.root(name(), SpanKind::Internal).end();
    tokio::time::sleep(Duration::from_millis(10)).await;
    exporter.set_mode(Mode::Keep);
    tracer.root(name(), SpanKind::Internal).end();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(exporter.spans().len(), 1, "the success after the back-off");
    exporter.set_mode(Mode::Fail);
    tracer.root(name(), SpanKind::Internal).end();
    tokio::time::sleep(Duration::from_millis(10)).await;
    tracer.root(name(), SpanKind::Internal).end();
    tokio::time::sleep(Duration::from_secs(3)).await;
    let calls = exporter.call_times();
    assert_eq!(calls.len(), 4);
    // The wait after the second episode's first failure is the first wait again, not the doubled one.
    assert_eq!(calls[3] - calls[2], Duration::from_secs(1));
    // Two episodes: the exporter was told twice.
    assert_eq!(exporter.failing_episodes(), 2);
}

#[tokio::test(start_paused = true)]
async fn the_final_flush_stops_at_a_failed_batch_and_counts_what_is_left() {
    let exporter = MemoryExporter::new(Mode::Fail);
    let limits = Limits {
        batch: 2,
        ..Limits::default()
    };
    let tracer = Pipeline::start(exporter.clone(), limits, resource(), Sampler::AlwaysOn);
    for _ in 0..5 {
        tracer.root(name(), SpanKind::Internal).end();
    }
    let stats = tracer.shutdown(Duration::from_secs(3)).await;
    // One batch of 2 failed; the flush did not go on, and the 3 left are counted, not lost.
    assert_eq!(exporter.call_times().len(), 1);
    assert_eq!(stats.dropped_export_failed, 5);
    assert_eq!(stats.exported, 0);
}

#[tokio::test(start_paused = true)]
async fn a_stop_cuts_the_backoff_short() {
    let exporter = MemoryExporter::new(Mode::Fail);
    let limits = Limits {
        batch: 1,
        backoff_min: Duration::from_secs(100),
        backoff_max: Duration::from_secs(100),
        ..Limits::default()
    };
    let tracer = Pipeline::start(exporter, limits, resource(), Sampler::AlwaysOn);
    tracer.root(name(), SpanKind::Internal).end();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let started = Instant::now();
    let stats = tracer.shutdown(Duration::from_secs(3)).await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(stats.dropped_flush_deadline, 0);
}
