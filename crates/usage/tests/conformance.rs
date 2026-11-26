#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The fake passes the kit, and the kit fails a store that breaks each rule.

use std::sync::Mutex;

use aap_usage::testkit::{self, event};
use aap_usage::{
    Inserted, MemoryUsageStore, PodMinuteEvent, SpendLine, SpendQuery, UsageError, UsageStore,
    event_id,
};

#[tokio::test]
async fn the_fake_passes_every_rule() {
    testkit::run(MemoryUsageStore::new).await.unwrap();
}

/// Each variant breaks one rule of the kit.
#[derive(Clone, Copy)]
enum Fault {
    AlwaysNew,
    LastCopyStands,
    IdAloneIsTheKey,
    NoSumming,
    WindowClosedAtTheEnd,
    IgnoresAccount,
    AcceptsInvalid,
    EmptyWindowIsEmpty,
    BatchAllNew,
    BatchKeepsTheValidPart,
}

struct Broken {
    inner: MemoryUsageStore,
    fault: Fault,
    log: Mutex<Vec<(PodMinuteEvent, u64)>>,
}

impl Broken {
    fn new(fault: Fault) -> Self {
        Self {
            inner: MemoryUsageStore::new(),
            fault,
            log: Mutex::new(Vec::new()),
        }
    }
}

impl UsageStore for Broken {
    async fn insert(&self, mut e: PodMinuteEvent, at: u64) -> Result<Inserted, UsageError> {
        match self.fault {
            Fault::AlwaysNew => {
                self.inner.insert(e, at).await?;
                Ok(Inserted::New)
            }
            Fault::LastCopyStands => {
                // Stores the later copy under a key of its own, so it counts again at its own time.
                let first = self.inner.insert(e.clone(), at).await?;
                if first == Inserted::Duplicate {
                    e.source = format!("{}-late", e.source);
                    self.inner.insert(e, at).await?;
                }
                Ok(first)
            }
            Fault::IdAloneIsTheKey => {
                e.source = "same".into();
                self.inner.insert(e, at).await
            }
            Fault::AcceptsInvalid => {
                if e.validate().is_err() {
                    return Ok(Inserted::New);
                }
                self.inner.insert(e, at).await
            }
            _ => self.inner.insert(e, at).await,
        }
    }

    async fn insert_many(
        &self,
        events: Vec<PodMinuteEvent>,
        at: u64,
    ) -> Result<Vec<Inserted>, UsageError> {
        match self.fault {
            Fault::BatchAllNew => {
                let n = events.len();
                self.inner.insert_many(events, at).await?;
                Ok(vec![Inserted::New; n])
            }
            Fault::BatchKeepsTheValidPart => {
                let mut out = Vec::new();
                for e in events {
                    out.push(self.inner.insert(e, at).await?);
                }
                Ok(out)
            }
            _ => self.inner.insert_many(events, at).await,
        }
    }

    async fn spend(&self, mut q: SpendQuery) -> Result<Vec<SpendLine>, UsageError> {
        let _ = &self.log;
        match self.fault {
            Fault::NoSumming => {
                let mut lines = self.inner.spend(q).await?;
                for l in &mut lines {
                    l.weighted_minutes = l.weighted_minutes.min(1);
                }
                Ok(lines)
            }
            Fault::WindowClosedAtTheEnd => {
                q.window.to += 1;
                self.inner.spend(q).await
            }
            Fault::IgnoresAccount => {
                q.account = None;
                self.inner.spend(q).await
            }
            Fault::EmptyWindowIsEmpty => {
                if q.window.from >= q.window.to {
                    return Ok(Vec::new());
                }
                self.inner.spend(q).await
            }
            _ => self.inner.spend(q).await,
        }
    }
}

async fn named<F, Fut>(fault: Fault, check: F, rule: &str)
where
    F: Fn(Broken) -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let msg = check(Broken::new(fault))
        .await
        .expect_err("the rule must fail a store with this fault");
    assert!(msg.starts_with(rule), "wanted `{rule}`, got `{msg}`");
}

#[tokio::test]
async fn each_rule_is_broken_once_and_named() {
    named(
        Fault::AlwaysNew,
        |s| async move { testkit::check_idempotent(&s).await },
        "idempotent",
    )
    .await;
    named(
        Fault::LastCopyStands,
        |s| async move { testkit::check_first_copy_stands(&s).await },
        "first copy",
    )
    .await;
    named(
        Fault::IdAloneIsTheKey,
        |s| async move { testkit::check_source_scopes_id(&s).await },
        "source",
    )
    .await;
    named(
        Fault::NoSumming,
        |s| async move { testkit::check_sums_and_order(&s).await },
        "sums",
    )
    .await;
    named(
        Fault::WindowClosedAtTheEnd,
        |s| async move { testkit::check_window_half_open(&s).await },
        "window",
    )
    .await;
    named(
        Fault::IgnoresAccount,
        |s| async move { testkit::check_account_filter(&s).await },
        "filter",
    )
    .await;
    named(
        Fault::AcceptsInvalid,
        |s| async move { testkit::check_invalid_refused(&s).await },
        "invalid",
    )
    .await;
    named(
        Fault::EmptyWindowIsEmpty,
        |s| async move { testkit::check_empty_window(&s).await },
        "empty window",
    )
    .await;
    for fault in [Fault::BatchAllNew, Fault::BatchKeepsTheValidPart] {
        named(
            fault,
            |s| async move { testkit::check_batch(&s).await },
            "batch",
        )
        .await;
    }
}

#[tokio::test]
async fn the_whole_suite_fails_every_broken_store() {
    for fault in [
        Fault::AlwaysNew,
        Fault::LastCopyStands,
        Fault::IdAloneIsTheKey,
        Fault::NoSumming,
        Fault::WindowClosedAtTheEnd,
        Fault::IgnoresAccount,
        Fault::AcceptsInvalid,
        Fault::EmptyWindowIsEmpty,
        Fault::BatchAllNew,
        Fault::BatchKeepsTheValidPart,
    ] {
        assert!(testkit::run(|| Broken::new(fault)).await.is_err());
    }
}

#[test]
fn the_event_id_is_stable_and_separates_pod_and_minute() {
    assert_eq!(event_id("pod", 1), event_id("pod", 1));
    assert_ne!(event_id("pod", 1), event_id("pod", 2));
    // The separator keeps ("a1", 2) apart from ("a", 12).
    assert_ne!(event_id("a1", 2), event_id("a", 12));
    assert_eq!(event_id("pod", 1).len(), 64);
    assert_eq!(event("m", "pod", 1).id, event_id("pod", 1));
}
