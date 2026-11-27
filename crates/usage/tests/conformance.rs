#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The fake passes the kit, and the kit fails a sink that breaks each rule.

use std::sync::Mutex;

use aap_usage::testkit::{self, Observed, event};
use aap_usage::{AccountSource, PodRef};
use aap_usage::{MemoryAccounts, MemoryUsageSink, PodMinuteEvent, UsageError, UsageSink};

#[tokio::test]
async fn the_fake_passes_every_rule() {
    testkit::run(MemoryUsageSink::new).await.unwrap();
}

/// Each variant breaks one rule of the kit.
#[derive(Clone, Copy)]
enum Fault {
    AcceptsInvalid,
    NamesTheWrongField,
    DropsEverything,
    RefusesTheRepeat,
    Reverses,
}

struct Broken {
    taken: Mutex<Vec<PodMinuteEvent>>,
    fault: Fault,
}

impl Broken {
    fn new(fault: Fault) -> Self {
        Self {
            taken: Mutex::new(Vec::new()),
            fault,
        }
    }
}

impl UsageSink for Broken {
    async fn send(&self, e: PodMinuteEvent) -> Result<(), UsageError> {
        let mut taken = self.taken.lock().unwrap();
        match self.fault {
            Fault::AcceptsInvalid => {
                taken.push(e);
                Ok(())
            }
            Fault::NamesTheWrongField => {
                e.validate()
                    .map_err(|_| UsageError::Invalid { field: "source" })?;
                taken.push(e);
                Ok(())
            }
            Fault::DropsEverything => {
                e.validate()?;
                Ok(())
            }
            Fault::RefusesTheRepeat => {
                e.validate()?;
                if taken.iter().any(|t| t.event_id == e.event_id) {
                    return Err(UsageError::Unavailable);
                }
                taken.push(e);
                Ok(())
            }
            Fault::Reverses => {
                e.validate()?;
                // Newest minute first (a stable sort: events of one minute keep their order).
                taken.push(e);
                taken.sort_by(|a, b| b.minute.cmp(&a.minute));
                Ok(())
            }
        }
    }
}

impl Observed for Broken {
    fn taken(&self) -> Vec<PodMinuteEvent> {
        self.taken.lock().unwrap().clone()
    }
}

async fn names(fault: Fault, rule: &str) {
    let msg = testkit::run(|| Broken::new(fault))
        .await
        .expect_err("the kit must fail a sink with this fault");
    assert!(msg.starts_with(rule), "wanted `{rule}`, got `{msg}`");
}

#[tokio::test]
async fn a_sink_that_takes_an_invalid_event_fails_the_kit() {
    names(Fault::AcceptsInvalid, "refuses-invalid").await;
}

#[tokio::test]
async fn a_sink_that_names_the_wrong_field_fails_the_kit() {
    names(Fault::NamesTheWrongField, "refuses-invalid").await;
}

#[tokio::test]
async fn a_sink_that_takes_nothing_fails_the_kit() {
    names(Fault::DropsEverything, "accepts-valid").await;
}

#[tokio::test]
async fn a_sink_that_refuses_a_repeat_fails_the_kit() {
    names(Fault::RefusesTheRepeat, "same-twice").await;
}

#[tokio::test]
async fn a_sink_that_reorders_fails_the_kit() {
    names(Fault::Reverses, "order").await;
}

#[test]
fn an_email_is_not_an_account() {
    let mut e = event("m1", "p", 1);
    e.subject = "someone@example.com".into();
    assert_eq!(e.validate(), Err(UsageError::Invalid { field: "subject" }));
    e.subject = "acct:7f3a-opaque/01".into();
    assert_eq!(e.validate(), Ok(()));
}

#[test]
fn the_time_is_the_end_of_the_minute() {
    let e = event("m1", "p", 3);
    assert_eq!(e.end_unix_nanos(), 4 * 60 * 1_000_000_000);
}

#[tokio::test]
async fn a_down_sink_is_unavailable_and_takes_nothing() {
    let s = MemoryUsageSink::new();
    s.set_down(true);
    assert_eq!(
        s.send(event("m", "p", 1)).await,
        Err(UsageError::Unavailable)
    );
    assert!(s.taken().is_empty());
}

#[tokio::test]
async fn the_account_fake_answers_by_run_and_by_service() {
    let a = MemoryAccounts::new();
    a.set_run("coder", "r1", "acct-1");
    a.set_agent("coder", "acct-2");
    let run = PodRef::Run {
        service: "coder".into(),
        run: "r1".into(),
    };
    let agent = PodRef::Agent {
        service: "coder".into(),
    };
    assert_eq!(a.account(&run).await.unwrap().as_deref(), Some("acct-1"));
    assert_eq!(a.account(&agent).await.unwrap().as_deref(), Some("acct-2"));
    assert_eq!(
        a.account(&PodRef::Agent {
            service: "other".into()
        })
        .await
        .unwrap(),
        None
    );
    a.set_broken(true);
    assert!(a.account(&run).await.is_err());
}

// ---- rule 5: the sink's outcomes

#[derive(Clone, Copy)]
enum OutcomeFault {
    NamesAnEventNeverSent,
    ReportsTooMany,
    NeverReports,
}

/// A sink that is right about everything but its outcomes.
struct WrongOutcomes {
    inner: MemoryUsageSink,
    fault: OutcomeFault,
}

impl UsageSink for WrongOutcomes {
    async fn send(&self, e: PodMinuteEvent) -> Result<(), UsageError> {
        self.inner.send(e).await
    }
    fn outcomes(&self) -> Option<Vec<aap_usage::Outcome>> {
        let mut real = self.inner.outcomes()?;
        match self.fault {
            OutcomeFault::NamesAnEventNeverSent => real.push(aap_usage::Outcome {
                event_id: "f".repeat(64),
                fate: aap_usage::Fate::Delivered,
            }),
            OutcomeFault::ReportsTooMany => {
                let again = real.clone();
                real.extend(again);
            }
            OutcomeFault::NeverReports => real.clear(),
        }
        Some(real)
    }
}

impl Observed for WrongOutcomes {
    fn taken(&self) -> Vec<PodMinuteEvent> {
        self.inner.taken()
    }
}

async fn wrong(fault: OutcomeFault) -> String {
    let s = WrongOutcomes {
        inner: MemoryUsageSink::new(),
        fault,
    };
    testkit::check_outcomes(&s, std::time::Duration::from_millis(60))
        .await
        .expect_err("rule 5 must fail this sink")
}

#[tokio::test]
async fn rule_five_fails_a_sink_that_names_an_event_it_was_not_sent() {
    assert!(
        wrong(OutcomeFault::NamesAnEventNeverSent)
            .await
            .contains("not sent")
    );
}

#[tokio::test]
async fn rule_five_fails_a_sink_that_reports_more_than_it_accepted() {
    assert!(
        wrong(OutcomeFault::ReportsTooMany)
            .await
            .contains("more outcomes")
    );
}

#[tokio::test]
async fn rule_five_fails_a_sink_whose_accepted_events_never_get_a_fate() {
    assert!(wrong(OutcomeFault::NeverReports).await.contains("no fate"));
}

#[tokio::test]
async fn a_repeated_event_gets_two_outcomes_from_the_fake() {
    testkit::check_outcomes(
        &MemoryUsageSink::new(),
        std::time::Duration::from_millis(60),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn the_tombstone_fake_passes_every_rule() {
    aap_usage::testkit::tombstones::run(|| {
        let a = aap_usage::MemoryTombstones::new();
        let b = a.another();
        (a, b)
    })
    .await
    .unwrap();
}

/// A store broken on purpose: each variant breaks one rule of the tombstone kit.
mod broken_tombstones {
    use std::sync::{Arc, Mutex};

    use aap_usage::testkit::tombstones;
    use aap_usage::{Loaded, Tombstone, TombstoneError, TombstoneStore};

    #[derive(Clone, Copy)]
    enum Fault {
        MergesInsteadOfReplacing,
        IgnoresTheNamespace,
        LosesTheSubject,
        NeverConflicts,
    }

    struct Broken {
        held: Arc<Mutex<Vec<Tombstone>>>,
        fault: Fault,
    }

    impl TombstoneStore for Broken {
        async fn load(&self, scope: &str) -> Result<Loaded, TombstoneError> {
            let held = self.held.lock().unwrap().clone();
            let tombstones = match self.fault {
                Fault::IgnoresTheNamespace => held,
                _ => held
                    .into_iter()
                    .filter(|t| scope.is_empty() || t.namespace == scope)
                    .collect(),
            };
            Ok(Loaded {
                tombstones,
                ..Loaded::default()
            })
        }
        async fn save(&self, namespace: &str, set: &[Tombstone]) -> Result<(), TombstoneError> {
            let mut held = self.held.lock().unwrap();
            if !matches!(self.fault, Fault::MergesInsteadOfReplacing) {
                held.retain(|t| t.namespace != namespace);
            }
            for t in set {
                let mut t = t.clone();
                if matches!(self.fault, Fault::LosesTheSubject) {
                    t.subject = None;
                }
                held.retain(|h| h.uid != t.uid);
                held.push(t);
            }
            Ok(())
        }
    }

    async fn fails(fault: Fault, rule: &str) {
        let err = tombstones::run(|| {
            let held = Arc::new(Mutex::new(Vec::new()));
            (
                Broken {
                    held: held.clone(),
                    fault,
                },
                Broken { held, fault },
            )
        })
        .await
        .expect_err("the kit must fail a broken store");
        assert!(err.starts_with(rule), "{err}");
    }

    #[tokio::test]
    async fn a_store_that_merges_fails_the_replace_rule() {
        fails(Fault::MergesInsteadOfReplacing, "replaces").await;
    }

    #[tokio::test]
    async fn a_store_that_ignores_the_namespace_fails_the_namespace_rule() {
        fails(Fault::IgnoresTheNamespace, "namespaces").await;
    }

    #[tokio::test]
    async fn a_store_that_loses_a_field_fails_the_round_trip() {
        fails(Fault::LosesTheSubject, "round-trip").await;
    }

    #[tokio::test]
    async fn a_store_that_never_conflicts_fails_the_conflict_rule() {
        fails(Fault::NeverConflicts, "conflict").await;
    }
}
