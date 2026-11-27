#![allow(clippy::unwrap_used, clippy::expect_used)]
//! What the sink logs. A binary of its own with one test: `tracing` caches whether a call site is interesting the first
//! time any thread reaches it, so a test that reads the log must not share a process with tests that reach the same call
//! sites with no subscriber.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use aap_usage::UsageSink;
use aap_usage::testkit::event;
use aap_usage_otlp::testkit::{Behaviour, TestCollector};
use aap_usage_otlp::{OtlpUsageSink, UsageOtlpSettings};

fn settings(c: &TestCollector) -> UsageOtlpSettings {
    let mut s = UsageOtlpSettings::new(c.base_url());
    s.interval = Duration::from_millis(40);
    s.timeout = Duration::from_millis(400);
    s.backoff_min = Duration::from_millis(15);
    s.backoff_max = Duration::from_millis(120);
    s
}

async fn until(what: &str, mut f: impl FnMut() -> bool) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out: {what}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn the_logs_say_counts_and_statuses_and_never_a_subject_or_an_event_id() {
    let capture = Capture::default();
    let sink_for_logs = capture.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || sink_for_logs.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let c = TestCollector::start();
    let mut e = event("m1", "pod-secret-uid", 5);
    e.subject = "subject-SECRET-77".into();
    c.set_behaviour(Behaviour::Status(503));
    let mut cfg = settings(&c);
    cfg.warn_every = Duration::from_millis(60);
    cfg.backoff_max = Duration::from_millis(15);
    let sink = OtlpUsageSink::start(cfg).unwrap();
    sink.send(e.clone()).await.unwrap();
    until("a retry", || c.raw_requests().len() >= 2).await;
    tokio::time::sleep(Duration::from_millis(250)).await;
    c.set_behaviour(Behaviour::Status(400));
    until("a refusal", || sink.stats().snapshot().refused == 1).await;
    c.set_behaviour(Behaviour::Reject(1));
    sink.send(e.clone()).await.unwrap();
    assert!(sink.flush(Duration::from_secs(5)).await);

    let text = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(
        text.contains("status_class=\"5xx\""),
        "the class of the 503 was not logged: {text}"
    );
    assert!(
        !text.contains("status=503"),
        "a transient status is a class only: {text}"
    );
    // While the batch is stuck the warning comes again every `warn_every`, not on every attempt.
    let stuck = text
        .lines()
        .filter(|l| l.contains("is kept and sent again"))
        .count();
    assert!(stuck >= 2, "{text}");
    assert!(text.contains("attempts="), "{text}");
    assert!(
        text.contains("status=400"),
        "the 400 was not logged: {text}"
    );
    assert!(!text.contains("subject-SECRET-77"), "{text}");
    assert!(!text.contains(&e.event_id), "{text}");
    assert!(!text.contains("pod-secret-uid"), "{text}");
    // hyper's own TRACE lines (a dependency, off by default) name the peer; this crate's lines must not.
    let ours: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("aap_usage_otlp"))
        .collect();
    assert!(ours.len() >= 3, "{text}");
    assert!(
        ours.iter().all(|l| !l.contains("127.0.0.1")),
        "the endpoint was logged: {ours:?}"
    );
}
