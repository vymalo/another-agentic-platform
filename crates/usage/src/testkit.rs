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

/// The conformance suite of [`TombstoneStore`](crate::TombstoneStore): one `check_*` per rule, and [`run`](tombstones::run).
///
/// A store is tested through **a pair of handles on one backing store** (`make` returns them): the fourth rule needs
/// a second writer.
pub mod tombstones {
    use crate::{PodKind, Tombstone, TombstoneError, TombstoneStore};

    /// A tombstone of a run pod in `namespace`.
    pub fn tombstone(namespace: &str, uid: &str) -> Tombstone {
        Tombstone {
            namespace: namespace.into(),
            uid: uid.into(),
            service: "coder".into(),
            run: Some("run-1".into()),
            pod_kind: PodKind::Run,
            subject: Some("acct-a".into()),
            weight_quarters: 4,
            node_type: "std".into(),
            started_at: 600,
            ended_at: 900,
            first_minute: 10,
            written_at: 1_000,
        }
    }

    fn sorted(mut v: Vec<Tombstone>) -> Vec<Tombstone> {
        v.sort_by(|a, b| a.uid.cmp(&b.uid));
        v
    }

    async fn read<S: TombstoneStore>(
        s: &S,
        scope: &str,
        rule: &str,
    ) -> Result<Vec<Tombstone>, String> {
        s.load(scope)
            .await
            .map(|l| l.tombstones)
            .map_err(|e| format!("{rule}: {e}"))
    }

    /// Rule 1: nothing stored is an empty set, and what is saved reads back whole.
    pub async fn check_round_trip<S: TombstoneStore>(s: &S) -> Result<(), String> {
        if !read(s, "ns-a", "round-trip").await?.is_empty() {
            return Err("round-trip: a fresh store is not empty".into());
        }
        let mut b = tombstone("ns-a", "uid-b");
        b.subject = None;
        b.run = None;
        b.pod_kind = PodKind::Agent;
        let set = vec![tombstone("ns-a", "uid-a"), b];
        s.save("ns-a", &set)
            .await
            .map_err(|e| format!("round-trip: {e}"))?;
        if sorted(read(s, "ns-a", "round-trip").await?) != sorted(set) {
            return Err("round-trip: what was saved is not what was read".into());
        }
        Ok(())
    }

    /// Rule 2: a save **replaces** the set (a tombstone left out is gone), and an empty set clears it.
    pub async fn check_replaces<S: TombstoneStore>(s: &S) -> Result<(), String> {
        read(s, "ns-a", "replaces").await?;
        s.save(
            "ns-a",
            &[tombstone("ns-a", "uid-a"), tombstone("ns-a", "uid-b")],
        )
        .await
        .map_err(|e| format!("replaces: {e}"))?;
        s.save("ns-a", &[tombstone("ns-a", "uid-b")])
            .await
            .map_err(|e| format!("replaces: {e}"))?;
        let got = read(s, "ns-a", "replaces").await?;
        if got.len() != 1 || got[0].uid != "uid-b" {
            return Err("replaces: a tombstone left out of a save is still there".into());
        }
        s.save("ns-a", &[])
            .await
            .map_err(|e| format!("replaces: {e}"))?;
        if !read(s, "ns-a", "replaces").await?.is_empty() {
            return Err("replaces: an empty save did not clear the set".into());
        }
        Ok(())
    }

    /// Rule 3: namespaces are independent, and the empty scope reads every one.
    pub async fn check_namespaces<S: TombstoneStore>(s: &S) -> Result<(), String> {
        read(s, "", "namespaces").await?;
        s.save("ns-a", &[tombstone("ns-a", "uid-a")])
            .await
            .map_err(|e| format!("namespaces: {e}"))?;
        s.save("ns-b", &[tombstone("ns-b", "uid-b")])
            .await
            .map_err(|e| format!("namespaces: {e}"))?;
        s.save("ns-a", &[])
            .await
            .map_err(|e| format!("namespaces: {e}"))?;
        let b = read(s, "ns-b", "namespaces").await?;
        if b.len() != 1 || b[0].namespace != "ns-b" {
            return Err("namespaces: a save in one namespace changed another".into());
        }
        if read(s, "", "namespaces").await?.len() != 1 {
            return Err("namespaces: the empty scope does not read every namespace".into());
        }
        if !read(s, "ns-a", "namespaces").await?.is_empty() {
            return Err("namespaces: a load returned a tombstone of another namespace".into());
        }
        Ok(())
    }

    /// Rule 4: a write over a set that **changed since it was read** is refused as `Conflict` and changes nothing,
    /// and so is a write over a set that exists and was never read. After a read, the write is accepted.
    pub async fn check_conflict<S: TombstoneStore>(a: &S, b: &S) -> Result<(), String> {
        read(a, "ns-a", "conflict").await?;
        read(b, "ns-a", "conflict").await?;
        a.save("ns-a", &[tombstone("ns-a", "uid-a")])
            .await
            .map_err(|e| format!("conflict: {e}"))?;
        // B read the empty set, and A has written since.
        if b.save("ns-a", &[tombstone("ns-a", "uid-b")]).await != Err(TombstoneError::Conflict) {
            return Err(
                "conflict: a write over a set that changed since it was read was accepted".into(),
            );
        }
        let got = read(a, "ns-a", "conflict").await?;
        if got.len() != 1 || got[0].uid != "uid-a" {
            return Err("conflict: a refused write changed the set".into());
        }
        // A third handle that never read a set that exists is refused too.
        read(b, "ns-a", "conflict").await?;
        b.save(
            "ns-a",
            &[tombstone("ns-a", "uid-a"), tombstone("ns-a", "uid-b")],
        )
        .await
        .map_err(|e| format!("conflict: a write after a read was refused: {e}"))?;
        if a.save("ns-a", &[]).await != Err(TombstoneError::Conflict) {
            return Err("conflict: the first writer overwrote a set it had not re-read".into());
        }
        Ok(())
    }

    /// Runs every rule, each on a pair of handles `make` returns fresh. The first broken rule is the error.
    pub async fn run<S: TombstoneStore, F: Fn() -> (S, S)>(make: F) -> Result<(), String> {
        check_round_trip(&make().0).await?;
        check_replaces(&make().0).await?;
        check_namespaces(&make().0).await?;
        let (a, b) = make();
        check_conflict(&a, &b).await
    }
}

/// The conformance suite of [`AccountSource`](crate::AccountSource): one `check_*` per rule, and [`run`](accounts::run).
///
/// An adapter's test says what its source knows, one [`accounts::Case`] per pod: the account it must answer, no account
/// (`Ok(None)`, not an error), or unavailable (`Err(AccountUnavailable)`, not `Ok(None)` — the meter holds the minute).
/// A source whose answers change between calls (the broker against a stub, a fake made broken for one rule) calls the
/// checks with the source in the state each rule needs; [`run`](accounts::run) is for a source whose answers are fixed.
pub mod accounts {
    use crate::{AccountSource, AccountUnavailable, PodRef, is_line_text};

    /// What a source must answer for one pod.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Want {
        /// The account, exactly; it is line text and not empty.
        Account(&'static str),
        /// No account: `Ok(None)`, not an error and not an empty account.
        None,
        /// The source cannot answer now: `Err(AccountUnavailable)`, not `Ok(None)`.
        Unavailable,
    }

    /// One pod and what the source must answer for it.
    #[derive(Clone, Debug)]
    pub struct Case {
        /// The pod the source is asked about.
        pub pod: PodRef,
        /// The answer it must give.
        pub want: Want,
    }

    impl Case {
        /// A pod whose account is `account`.
        pub fn account(pod: PodRef, account: &'static str) -> Self {
            Self {
                pod,
                want: Want::Account(account),
            }
        }

        /// A pod with no account.
        pub fn none(pod: PodRef) -> Self {
            Self {
                pod,
                want: Want::None,
            }
        }

        /// A pod the source cannot answer for now.
        pub fn unavailable(pod: PodRef) -> Self {
            Self {
                pod,
                want: Want::Unavailable,
            }
        }
    }

    /// Rule 1: a pod the source knows gets exactly its account, and the account is line text and not empty.
    pub async fn check_accounts<S: AccountSource>(
        source: &S,
        cases: &[Case],
    ) -> Result<(), String> {
        for case in cases {
            let Want::Account(want) = case.want else {
                continue;
            };
            match source.account(&case.pod).await {
                Ok(Some(got)) => {
                    if got != want {
                        return Err(format!(
                            "accounts: {:?}: want {want:?}, got {got:?}",
                            case.pod
                        ));
                    }
                    if !is_line_text(&got) {
                        return Err(format!(
                            "accounts: {:?}: the account {got:?} is not line text",
                            case.pod
                        ));
                    }
                }
                Ok(None) => {
                    return Err(format!(
                        "accounts: {:?}: no account, want {want:?}",
                        case.pod
                    ));
                }
                Err(AccountUnavailable) => {
                    return Err(format!(
                        "accounts: {:?}: unavailable, want {want:?}",
                        case.pod
                    ));
                }
            }
        }
        Ok(())
    }

    /// Rule 2: a pod with no account is `Ok(None)`, never an error and never an empty account.
    pub async fn check_no_account<S: AccountSource>(
        source: &S,
        cases: &[Case],
    ) -> Result<(), String> {
        for case in cases {
            if case.want != Want::None {
                continue;
            }
            match source.account(&case.pod).await {
                Ok(None) => {}
                Ok(Some(got)) => {
                    return Err(format!(
                        "no-account: {:?}: got {got:?}, want none",
                        case.pod
                    ));
                }
                Err(AccountUnavailable) => {
                    return Err(format!(
                        "no-account: {:?}: unavailable, want none",
                        case.pod
                    ));
                }
            }
        }
        Ok(())
    }

    /// Rule 3: a source that cannot answer now is `Err(AccountUnavailable)`, never `Ok(None)` — a `None` here is a
    /// transient failure the meter would read as a real absence.
    pub async fn check_unavailable<S: AccountSource>(
        source: &S,
        cases: &[Case],
    ) -> Result<(), String> {
        for case in cases {
            if case.want != Want::Unavailable {
                continue;
            }
            match source.account(&case.pod).await {
                Err(AccountUnavailable) => {}
                Ok(None) => {
                    return Err(format!(
                        "unavailable: {:?}: none, want an error (the minute would be given up)",
                        case.pod
                    ));
                }
                Ok(Some(got)) => {
                    return Err(format!(
                        "unavailable: {:?}: got {got:?}, want an error",
                        case.pod
                    ));
                }
            }
        }
        Ok(())
    }

    /// Runs every rule on a source whose answers are fixed. The first broken rule is the error.
    pub async fn run<S: AccountSource>(source: &S, cases: &[Case]) -> Result<(), String> {
        check_accounts(source, cases).await?;
        check_no_account(source, cases).await?;
        check_unavailable(source, cases).await
    }
}
