//! `/readyz`'s word while a namespace's caches have not listed (K6b), in a binary of its own: the check reads the
//! operator's **warnings**, and a tracing subscriber set for one thread (`tracing::subscriber::set_default`) is not
//! safe beside other tests in one process — they rebuild the process-wide callsite interest cache from their own
//! threads, and a callsite can be cached as "never" for this test's subscriber (measured 2026-11-18: the same test
//! passes alone and fails, at random, in `tests/operator.rs`). A binary of one test has no such neighbour.
//!
//! What a real API server and a real `Role` make of it is `deploy/operator/tests/e2e/namespaces-e2e.sh`'s.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use aap_controller::{Operator, Options, OwnerOf, Resync};
use aap_ports::OwnerHandle;
use aap_ports::memory::MemoryStore;
use support::Fake;

/// Every warning logged while it is the default subscriber, as `field=value` text.
#[derive(Clone, Default)]
struct Warnings(Arc<std::sync::Mutex<Vec<String>>>);

impl Warnings {
    fn lines(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// The subscriber that keeps them: only the fields, and only warnings.
struct Capture(Arc<std::sync::Mutex<Vec<String>>>);

struct Fields(String);

impl tracing::field::Visit for Fields {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if *event.metadata().level() == tracing::Level::WARN {
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            self.0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(fields.0);
        }
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// K6b: while a namespace's caches have not listed, the operator says which ones every 30 s. Without it the only
/// symptom of a missing `Role` in one namespace of many is a failing readiness probe, and nothing says where.
#[tokio::test(start_paused = true)]
async fn the_namespaces_whose_caches_have_not_listed_are_named_every_thirty_seconds() {
    let warnings = Warnings::default();
    let _guard = tracing::subscriber::set_default(Capture(warnings.0.clone()));

    let fake = Fake::new();
    // The whole namespace is refused: its services' and its configs' caches never list.
    fake.fail_path(None, "/namespaces/blocked/", 403);
    let owner: OwnerOf = Arc::new(|_| OwnerHandle::none());
    let operator = Operator::new(
        fake.client(),
        aap_ports::memory::MemoryRuntime::new(),
        MemoryStore::new(),
        owner,
        Options {
            watch_namespaces: Some(vec!["team".to_owned(), "blocked".to_owned()]),
            resync: Resync {
                settled: Duration::from_secs(3600),
                pending: Duration::from_secs(3600),
                not_installed: Duration::from_secs(3600),
            },
            ..Options::default()
        },
    );
    let readiness = operator.readiness();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let done = tokio::spawn(operator.run(async move {
        let _ = stopped.await;
    }));
    // The readable namespace's watches are open; the refused one never opens any.
    for _ in 0..500 {
        if fake.watchers_in("agentservices", "team") >= 1
            && fake.watchers_in("agentconfigs", "team") >= 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        fake.watchers_in("agentservices", "team") >= 1,
        "the readable namespace's watches never opened"
    );
    assert!(!readiness.is_ready());

    // The interval's first tick is immediate; the next is 30 s later. Advance past it and let the task run, rather
    // than polling: a poll loop leaves the clock to the runtime's auto-advance, and what is asserted is the tick.
    tokio::time::advance(Duration::from_secs(31)).await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }

    let line = warnings
        .lines()
        .into_iter()
        .find(|line| line.contains("have not listed"))
        .unwrap_or_else(|| {
            panic!(
                "the waiting namespace was not named within 31 s; the warnings were {:?}",
                warnings.lines()
            )
        });
    assert!(
        line.contains("blocked"),
        "the namespace whose caches never listed is named: {line}"
    );
    assert!(
        !line.contains("team"),
        "the namespace whose caches did list is not named: {line}"
    );
    assert!(
        !readiness.is_ready(),
        "the operator is still not ready while a cache has not listed"
    );

    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done)
        .await
        .expect("the operator stops when told to")
        .unwrap();
}
