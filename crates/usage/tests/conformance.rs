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
