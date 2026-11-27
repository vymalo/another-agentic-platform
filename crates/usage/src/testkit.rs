//! The conformance suite of [`UsageSink`]: one `check_*` per rule, and [`run`] for all of them. Each returns the
//! rule it saw broken, so an adapter's test (and this crate's own, against sinks broken on purpose) can say which.
//!
//! A sink cannot be read back, so an adapter's test hands [`run`] a [`Observed`] wrapper that says what its sink
//! took (the OTLP adapter's: what its test collector received).

use crate::{PodKind, PodMinuteEvent, UsageError, UsageSink, event_id};

/// A [`UsageSink`] that can say what it has taken, in order.
pub trait Observed: UsageSink {
    /// Every event taken, in order, a repeated event each time.
    fn taken(&self) -> Vec<PodMinuteEvent>;
}

impl Observed for crate::MemoryUsageSink {
    fn taken(&self) -> Vec<PodMinuteEvent> {
        crate::MemoryUsageSink::taken(self)
    }
}

/// An event of a valid shape: account `acct-a`, node type `std`, weight 4 (one unit), a run pod's.
pub fn event(source: &str, pod: &str, minute: u64) -> PodMinuteEvent {
    PodMinuteEvent {
        source: source.into(),
        event_id: event_id(pod, minute),
        minute,
        subject: "acct-a".into(),
        weight_quarters: 4,
        node_type: "std".into(),
        pod_kind: PodKind::Run,
        agent_service: "coder".into(),
        run: Some("run-1".into()),
    }
}

fn err<E: std::fmt::Debug>(rule: &str, e: E) -> String {
    format!("{rule}: the sink failed: {e:?}")
}

/// Rule 1: a valid event is accepted, and is taken as it was sent.
pub async fn check_accepts_valid<S: Observed>(s: &S) -> Result<(), String> {
    let e = event("m1", "pod-1", 10);
    s.send(e.clone())
        .await
        .map_err(|x| err("accepts-valid", x))?;
    let mut agent = event("m1", "pod-2", 10);
    agent.pod_kind = PodKind::Agent;
    agent.run = None;
    s.send(agent.clone())
        .await
        .map_err(|x| err("accepts-valid", x))?;
    if s.taken() != vec![e, agent] {
        return Err("accepts-valid: the sink did not take what it was sent".into());
    }
    Ok(())
}

/// Rule 2: an invalid event is refused, naming the field, and nothing is taken.
pub async fn check_refuses_invalid<S: Observed>(s: &S) -> Result<(), String> {
    type Break = fn(&mut PodMinuteEvent);
    let cases: [(&str, Break); 8] = [
        ("subject", |e| e.subject = "a@example.com".into()),
        ("subject", |e| e.subject = String::new()),
        ("source", |e| e.source = "x".repeat(129)),
        ("nodeType", |e| e.node_type = "with space".into()),
        ("eventId", |e| e.event_id = "abc".into()),
        ("weightQuarters", |e| e.weight_quarters = 3),
        ("weightQuarters", |e| e.weight_quarters = 0),
        ("run", |e| e.run = None),
    ];
    for (field, make_bad) in cases {
        let mut bad = event("m1", "pod-1", 10);
        make_bad(&mut bad);
        match s.send(bad).await {
            Err(UsageError::Invalid { field: got }) if got == field => {}
            other => {
                return Err(format!(
                    "refuses-invalid: want Invalid({field}), got {other:?}"
                ));
            }
        }
    }
    let mut agent_with_run = event("m1", "pod-1", 10);
    agent_with_run.pod_kind = PodKind::Agent;
    match s.send(agent_with_run).await {
        Err(UsageError::Invalid { field: "run" }) => {}
        other => return Err(format!("refuses-invalid: want Invalid(run), got {other:?}")),
    }
    if !s.taken().is_empty() {
        return Err("refuses-invalid: a refused event was taken".into());
    }
    Ok(())
}

/// Rule 3: the same event twice is accepted both times (the collector dedupes, not the sink).
pub async fn check_same_event_twice<S: Observed>(s: &S) -> Result<(), String> {
    let e = event("m1", "pod-1", 10);
    s.send(e.clone()).await.map_err(|x| err("same-twice", x))?;
    s.send(e).await.map_err(|x| err("same-twice", x))?;
    if s.taken().len() != 2 {
        return Err(format!("same-twice: took {} of 2 sends", s.taken().len()));
    }
    Ok(())
}

/// Rule 4: events are taken in the order they were sent.
pub async fn check_order<S: Observed>(s: &S) -> Result<(), String> {
    let sent: Vec<PodMinuteEvent> = [5u64, 3, 9, 1, 7]
        .iter()
        .map(|m| event("m1", "pod-1", *m))
        .collect();
    for e in &sent {
        s.send(e.clone()).await.map_err(|x| err("order", x))?;
    }
    if s.taken() != sent {
        return Err("order: the sink took the events in another order".into());
    }
    Ok(())
}

/// How long rule 5 waits, in [`run`], for an adapter that reports from a task of its own.
pub const OUTCOMES_WITHIN: std::time::Duration = std::time::Duration::from_secs(5);

/// Rule 5, both halves of the contract of [`UsageSink::outcomes`]. A sink that answers `None` has nothing to check.
/// Otherwise, for events it accepted (a repeated one **twice**, as rule 3 says it is taken twice):
/// **it names no event that was not sent**, **no more outcomes than acceptances of an id**, and **every acceptance
/// gets a fate within `within`** (the adapter's own queue and batch may take that long).
pub async fn check_outcomes(s: &impl Observed, within: std::time::Duration) -> Result<(), String> {
    let mut sent: Vec<PodMinuteEvent> = (1u64..=3).map(|m| event("m1", "pod-1", m)).collect();
    sent.push(event("m1", "pod-1", 1)); // a repeat: two acceptances, two outcomes
    for e in &sent {
        s.send(e.clone()).await.map_err(|x| err("outcomes", x))?;
    }
    let mut want: std::collections::BTreeMap<String, usize> = Default::default();
    for e in &sent {
        *want.entry(e.event_id.clone()).or_default() += 1;
    }
    let mut got: std::collections::BTreeMap<String, usize> = Default::default();
    let deadline = std::time::Instant::now() + within;
    loop {
        let Some(batch) = s.outcomes() else {
            return Ok(());
        };
        for o in batch {
            let n = got.entry(o.event_id.clone()).or_default();
            *n += 1;
            match want.get(&o.event_id) {
                None => return Err("outcomes: an outcome names an event that was not sent".into()),
                Some(max) if *n > *max => {
                    return Err("outcomes: an event has more outcomes than acceptances".into());
                }
                Some(_) => {}
            }
        }
        if got == want {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("outcomes: an accepted event got no fate within the bound".into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
}

/// Runs every rule, each on a sink `make` returns fresh. The first broken rule is the error.
pub async fn run<S: Observed, F: Fn() -> S>(make: F) -> Result<(), String> {
    check_accepts_valid(&make()).await?;
    check_refuses_invalid(&make()).await?;
    check_same_event_twice(&make()).await?;
    check_order(&make()).await?;
    check_outcomes(&make(), OUTCOMES_WITHIN).await
}
