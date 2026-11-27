#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The sink against a test collector: the port's conformance suite, and the failure behaviour.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use aap_usage::testkit::event;
use aap_usage::{PodMinuteEvent, UsageError, UsageSink};
use aap_usage_otlp::testkit::{Behaviour, ObservedOtlp, TestCollector};
use aap_usage_otlp::{OtlpUsageSink, UsageOtlpSettings};

fn settings(c: &TestCollector) -> UsageOtlpSettings {
    let mut s = UsageOtlpSettings::new(c.base_url());
    s.interval = Duration::from_millis(40);
    s.timeout = Duration::from_millis(400);
    s.backoff_min = Duration::from_millis(15);
    s.backoff_max = Duration::from_millis(120);
    s
}

fn ids(events: &[PodMinuteEvent]) -> BTreeSet<String> {
    events.iter().map(|e| e.event_id.clone()).collect()
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

#[tokio::test(flavor = "multi_thread")]
async fn the_ports_conformance_suite_passes_against_a_collector() {
    aap_usage::testkit::run(|| {
        let collector = TestCollector::start();
        let sink = OtlpUsageSink::start(settings(&collector)).unwrap();
        ObservedOtlp { sink, collector }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_503_twice_then_200_delivers_every_event_with_the_same_bytes_and_times() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::Status(503));
    let sink = OtlpUsageSink::start(settings(&c)).unwrap();
    let sent: Vec<_> = (0..5).map(|m| event("m1", "pod-1", 100 + m)).collect();
    for e in &sent {
        sink.send(e.clone()).await.unwrap();
    }
    until("two refused attempts", || c.raw_requests().len() >= 2).await;
    c.set_behaviour(Behaviour::Accept);
    until("delivered", || c.events().len() >= 5).await;
    let got = c.events();
    assert_eq!(ids(&got), ids(&sent), "every event, none lost");
    assert_eq!(
        got.iter().map(|e| e.minute).collect::<Vec<_>>(),
        (100..105).collect::<Vec<_>>()
    );
    // The retried bytes are the first bytes: nothing was re-encoded, no time moved.
    let raw = c.raw_requests();
    assert!(raw.len() >= 3);
    assert!(
        raw.iter().all(|r| *r == raw[0]),
        "the same bytes every time"
    );
    let s = sink.stats().snapshot();
    assert!(s.retries >= 2, "{s:?}");
    assert_eq!((s.sent, s.refused, s.queue_depth), (5, 0, 0), "{s:?}");
}

#[tokio::test]
async fn timeouts_401_403_404_408_429_closed_connections_and_redirects_are_all_retried() {
    for behaviour in [
        Behaviour::Status(401),
        Behaviour::Status(403),
        Behaviour::Status(404),
        Behaviour::Status(408),
        Behaviour::Status(429),
        Behaviour::Status(500),
        Behaviour::Close,
        Behaviour::Hang,
        Behaviour::Redirect,
    ] {
        let c = TestCollector::start();
        c.set_behaviour(behaviour);
        let mut cfg = settings(&c);
        cfg.timeout = Duration::from_millis(100);
        let sink = OtlpUsageSink::start(cfg).unwrap();
        let e = event("m1", "pod-1", 5);
        sink.send(e.clone()).await.unwrap();
        until("a first attempt and a retry", || {
            c.raw_requests().len() >= 2
        })
        .await;
        c.set_behaviour(Behaviour::Accept);
        until("delivered", || !c.events().is_empty()).await;
        assert_eq!(c.events(), vec![e], "{behaviour:?}");
        assert_eq!(c.elsewhere_hits(), 0, "a redirect is never followed");
        assert_eq!(sink.stats().snapshot().refused, 0, "{behaviour:?}");
    }
}

#[tokio::test]
async fn a_lost_answer_gives_a_duplicate_and_no_loss() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::LoseAnswer);
    let sink = OtlpUsageSink::start(settings(&c)).unwrap();
    let e = event("m1", "pod-1", 5);
    sink.send(e.clone()).await.unwrap();
    until("seen twice", || c.events().len() >= 2).await;
    c.set_behaviour(Behaviour::Accept);
    assert!(sink.flush(Duration::from_secs(5)).await);
    let got = c.events();
    assert!(got.len() >= 3 || got.len() >= 2);
    assert!(
        got.iter().all(|x| *x == e),
        "the same event each time: the collector dedupes on event_id"
    );
}

#[tokio::test]
async fn a_collector_that_hangs_never_holds_send_and_the_queue_refuses_at_its_bound() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::Hang);
    let mut cfg = settings(&c);
    cfg.queue = 8;
    cfg.batch = 2;
    cfg.timeout = Duration::from_secs(30);
    let sink = OtlpUsageSink::start(cfg).unwrap();
    let started = Instant::now();
    let mut accepted = 0u64;
    let mut refused = 0u64;
    let all = tokio::time::timeout(Duration::from_secs(5), async {
        for m in 0..200u64 {
            match sink.send(event("m1", "pod-1", m)).await {
                Ok(()) => accepted += 1,
                Err(e) => {
                    assert_eq!(e, UsageError::Unavailable);
                    refused += 1;
                }
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(all.is_ok(), "send waited on the network");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(refused > 0, "the queue never refused");
    // The queue's 8, plus the one batch of 2 the task holds in flight.
    assert!(accepted <= 8 + 2 + 2, "accepted {accepted}");
    let s = sink.stats().snapshot();
    assert_eq!(s.dropped_queue_full, refused);
    assert_eq!(s.queued, accepted);
    assert_eq!(s.queue_depth, accepted);
    // Shutdown does not outlive its deadline, and says what it could not deliver.
    let t = Instant::now();
    let report = sink.shutdown(Duration::from_millis(300)).await;
    assert!(t.elapsed() < Duration::from_secs(3), "{:?}", t.elapsed());
    assert_eq!(report.unsent, accepted);
    assert_eq!(sink.stats().snapshot().queue_depth, 0);
}

#[tokio::test]
async fn a_permanent_400_drops_that_batch_counts_it_and_later_events_still_go() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::Status(400));
    let sink = OtlpUsageSink::start(settings(&c)).unwrap();
    sink.send(event("m1", "pod-1", 1)).await.unwrap();
    sink.send(event("m1", "pod-1", 2)).await.unwrap();
    assert!(sink.flush(Duration::from_secs(5)).await);
    let s = sink.stats().snapshot();
    assert_eq!((s.refused, s.sent, s.retries), (2, 0, 0), "{s:?}");
    assert_eq!(
        c.raw_requests().len(),
        1,
        "a refused batch is not sent again"
    );
    c.set_behaviour(Behaviour::Accept);
    let later = event("m1", "pod-1", 3);
    sink.send(later.clone()).await.unwrap();
    assert!(sink.flush(Duration::from_secs(5)).await);
    assert_eq!(c.events(), vec![later]);
    assert_eq!(sink.stats().snapshot().sent, 1);
}

#[tokio::test]
async fn records_the_collector_rejects_are_counted_and_not_retried() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::Reject(1));
    let sink = OtlpUsageSink::start(settings(&c)).unwrap();
    sink.send(event("m1", "pod-1", 1)).await.unwrap();
    sink.send(event("m1", "pod-1", 2)).await.unwrap();
    assert!(sink.flush(Duration::from_secs(5)).await);
    let s = sink.stats().snapshot();
    assert_eq!((s.sent, s.refused), (1, 1), "{s:?}");
    assert_eq!(c.raw_requests().len(), 1);
}

#[tokio::test]
async fn shutdown_flushes_what_is_queued_within_its_deadline() {
    let c = TestCollector::start();
    let mut cfg = settings(&c);
    cfg.interval = Duration::from_secs(3600);
    let sink = OtlpUsageSink::start(cfg).unwrap();
    let sent: Vec<_> = (0..7).map(|m| event("m1", "pod-1", m)).collect();
    for e in &sent {
        sink.send(e.clone()).await.unwrap();
    }
    assert!(
        c.events().is_empty(),
        "the interval is an hour: nothing went yet"
    );
    let report = sink.shutdown(Duration::from_secs(3)).await;
    assert_eq!(report.unsent, 0);
    assert_eq!(ids(&c.events()), ids(&sent));
    // After a shutdown the sink takes nothing, and says so.
    assert_eq!(
        sink.send(event("m1", "pod-1", 99)).await,
        Err(UsageError::Unavailable)
    );
    // A second shutdown is harmless.
    assert_eq!(sink.shutdown(Duration::from_millis(50)).await.unsent, 0);
}

#[tokio::test]
async fn shutdown_retries_a_failing_collector_until_its_deadline_then_counts_what_it_lost() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::Status(503));
    let mut cfg = settings(&c);
    cfg.interval = Duration::from_secs(3600);
    let sink = OtlpUsageSink::start(cfg).unwrap();
    for m in 0..3 {
        sink.send(event("m1", "pod-1", m)).await.unwrap();
    }
    let t = Instant::now();
    let report = sink.shutdown(Duration::from_millis(400)).await;
    assert!(t.elapsed() < Duration::from_secs(3));
    assert_eq!(report.unsent, 3);
    assert!(
        c.raw_requests().len() >= 2,
        "it kept trying until the deadline"
    );
}

#[tokio::test]
async fn an_invalid_event_is_refused_by_name_and_never_queued() {
    let c = TestCollector::start();
    let sink = OtlpUsageSink::start(settings(&c)).unwrap();
    let mut bad = event("m1", "pod-1", 1);
    bad.subject = "a@example.com".into();
    assert_eq!(
        sink.send(bad).await,
        Err(UsageError::Invalid { field: "subject" })
    );
    assert_eq!(sink.stats().snapshot().queued, 0);
}

#[test]
fn settings_are_checked() {
    let url = |s: &str| url::Url::parse(s).unwrap();
    let ok = UsageOtlpSettings::new(url("http://collector:4318"));
    ok.validate().unwrap();
    assert_eq!(ok.queue, 4096);
    assert_eq!(ok.batch, 512);
    assert_eq!(ok.interval, Duration::from_secs(5));
    assert_eq!(ok.service_name, "aap-operator");
    let bad = |f: &dyn Fn(&mut UsageOtlpSettings), setting: &str| {
        let mut s = ok.clone();
        f(&mut s);
        assert_eq!(s.validate().unwrap_err().setting, setting);
    };
    bad(&|s| s.endpoint = url("ftp://x"), "endpoint");
    bad(&|s| s.endpoint = url("http://u:p@x"), "endpoint");
    bad(&|s| s.endpoint = url("http://x/?a=b"), "endpoint");
    bad(&|s| s.queue = 0, "queue");
    bad(&|s| s.batch = 0, "batch");
    bad(&|s| s.batch = 10_001, "batch");
    bad(&|s| s.interval = Duration::ZERO, "interval");
    bad(&|s| s.timeout = Duration::from_secs(9999), "timeout");
    bad(&|s| s.warn_every = Duration::ZERO, "warnEvery");
    bad(&|s| s.backoff_max = Duration::from_millis(1), "backoff");
    bad(&|s| s.service_name = "a b".into(), "serviceName");
}

#[tokio::test]
async fn a_ca_file_that_is_missing_or_not_a_certificate_is_refused_at_start() {
    let url = url::Url::parse("https://collector:4318").unwrap();
    let mut s = UsageOtlpSettings::new(url);
    s.ca = Some("/nonexistent/ca.pem".into());
    assert_eq!(OtlpUsageSink::start(s.clone()).unwrap_err().setting, "ca");
    let dir = std::env::temp_dir();
    let file = dir.join(format!("aap-usage-otlp-notpem-{}", std::process::id()));
    std::fs::write(&file, "nothing").unwrap();
    s.ca = Some(file.clone());
    let e = OtlpUsageSink::start(s).unwrap_err();
    let _ = std::fs::remove_file(file);
    assert_eq!(e.setting, "ca");
}

#[tokio::test]
async fn only_400_415_and_422_are_permanent() {
    for status in [400u16, 415, 422] {
        let c = TestCollector::start();
        c.set_behaviour(Behaviour::Status(status));
        let sink = OtlpUsageSink::start(settings(&c)).unwrap();
        sink.send(event("m1", "pod-1", 1)).await.unwrap();
        sink.send(event("m1", "pod-1", 2)).await.unwrap();
        assert!(sink.flush(Duration::from_secs(5)).await, "{status}");
        let s = sink.stats().snapshot();
        assert_eq!((s.refused, s.sent, s.retries), (2, 0, 0), "{status}: {s:?}");
        assert_eq!(c.raw_requests().len(), 1, "{status}: never sent again");
    }
}

#[tokio::test]
async fn a_404_is_held_not_dropped_and_goes_when_the_path_is_fixed() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::Status(404));
    let sink = OtlpUsageSink::start(settings(&c)).unwrap();
    let sent: Vec<_> = (0..3).map(|m| event("m1", "pod-1", m)).collect();
    for e in &sent {
        sink.send(e.clone()).await.unwrap();
    }
    until("several attempts", || c.raw_requests().len() >= 4).await;
    let s = sink.stats().snapshot();
    assert_eq!(s.refused, 0, "{s:?}");
    assert!(s.retries >= 3 && s.queue_depth == 3, "{s:?}");
    c.set_behaviour(Behaviour::Accept);
    until("delivered", || c.events().len() >= 3).await;
    assert_eq!(ids(&c.events()), ids(&sent));
}

#[tokio::test]
async fn a_413_halves_the_batch_down_until_the_collector_takes_every_event() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::MaxRecords(2));
    let mut cfg = settings(&c);
    cfg.batch = 16;
    cfg.interval = Duration::from_secs(3600);
    let sink = OtlpUsageSink::start(cfg).unwrap();
    let sent: Vec<_> = (0..11).map(|m| event("m1", "pod-1", m)).collect();
    for e in &sent {
        sink.send(e.clone()).await.unwrap();
    }
    assert!(sink.flush(Duration::from_secs(10)).await);
    let got = c.events();
    assert_eq!(ids(&got), ids(&sent), "every event, none lost");
    assert_eq!(
        got.len(),
        11,
        "and none twice: a split never resends a half that was taken"
    );
    let s = sink.stats().snapshot();
    assert_eq!(
        (s.sent, s.refused, s.retries, s.queue_depth),
        (11, 0, 0, 0),
        "{s:?}"
    );
}

#[tokio::test]
async fn a_single_event_that_is_still_too_large_is_dropped_and_counted() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::MaxRecords(0));
    let mut cfg = settings(&c);
    cfg.interval = Duration::from_secs(3600);
    let sink = OtlpUsageSink::start(cfg).unwrap();
    for m in 0..4 {
        sink.send(event("m1", "pod-1", m)).await.unwrap();
    }
    assert!(sink.flush(Duration::from_secs(10)).await);
    let s = sink.stats().snapshot();
    assert_eq!(
        (s.refused, s.sent, s.retries, s.queue_depth),
        (4, 0, 0, 0),
        "{s:?}"
    );
    // 4 -> 2 + 2 -> four singles: 1 + 2 + 4 requests, each answered 413, none repeated.
    assert_eq!(c.raw_requests().len(), 7);
    // Later events still go.
    c.set_behaviour(Behaviour::Accept);
    let later = event("m1", "pod-1", 9);
    sink.send(later.clone()).await.unwrap();
    assert!(sink.flush(Duration::from_secs(5)).await);
    assert_eq!(c.events(), vec![later]);
}

#[tokio::test]
async fn retrying_since_says_how_long_a_batch_has_been_stuck_and_goes_back_to_zero() {
    let c = TestCollector::start();
    c.set_behaviour(Behaviour::Status(503));
    let sink = OtlpUsageSink::start(settings(&c)).unwrap();
    assert_eq!(sink.stats().snapshot().retrying_since_secs, 0);
    sink.send(event("m1", "pod-1", 1)).await.unwrap();
    until("stuck", || sink.stats().snapshot().retrying_since_secs >= 1).await;
    c.set_behaviour(Behaviour::Accept);
    until("delivered", || !c.events().is_empty()).await;
    until("episode over", || {
        sink.stats().snapshot().retrying_since_secs == 0
    })
    .await;
    assert_eq!(sink.stats().snapshot().sent, 1);
}
