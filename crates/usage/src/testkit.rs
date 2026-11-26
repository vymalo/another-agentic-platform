//! The conformance suite of [`UsageStore`]: one `check_*` per rule, and [`run`] for all of them. Each returns the
//! rule it saw broken, so an adapter's test (and this crate's own, against stores broken on purpose) can say which.

use crate::{
    Inserted, PodMinuteEvent, SpendLine, SpendQuery, UsageError, UsageStore, Window, event_id,
};

/// An event of a valid shape: account `acct-a`, node type `std`, weight 1.
pub fn event(source: &str, pod: &str, minute: u64) -> PodMinuteEvent {
    PodMinuteEvent {
        source: source.into(),
        id: event_id(pod, minute),
        subject: "acct-a".into(),
        weight: 1,
        node_type: "std".into(),
        pod_kind: "agent".into(),
        agent_service: "coder".into(),
        run: None,
    }
}

fn q(from: u64, to: u64, account: Option<&str>) -> SpendQuery {
    SpendQuery {
        window: Window { from, to },
        account: account.map(String::from),
    }
}

fn line(account: &str, node_type: &str, weighted_minutes: u64) -> SpendLine {
    SpendLine {
        account: account.into(),
        node_type: node_type.into(),
        weighted_minutes,
    }
}

fn err<E: std::fmt::Debug>(rule: &str, e: E) -> String {
    format!("{rule}: the store failed: {e:?}")
}

/// Rule 1: the first insert is `New`, the same `(source, id)` again is `Duplicate`.
pub async fn check_idempotent<S: UsageStore>(s: &S) -> Result<(), String> {
    let e = event("m1", "pod-1", 10);
    let first = s
        .insert(e.clone(), 100)
        .await
        .map_err(|x| err("idempotent", x))?;
    let again = s.insert(e, 100).await.map_err(|x| err("idempotent", x))?;
    if first != Inserted::New || again != Inserted::Duplicate {
        return Err(format!(
            "idempotent: got {first:?} then {again:?}, want New then Duplicate"
        ));
    }
    Ok(())
}

/// Rule 2: a duplicate counts once, even when sent later with other data: the first copy and its time stand.
pub async fn check_first_copy_stands<S: UsageStore>(s: &S) -> Result<(), String> {
    let mut e = event("m1", "pod-1", 10);
    e.weight = 2;
    s.insert(e.clone(), 100)
        .await
        .map_err(|x| err("first copy", x))?;
    e.weight = 9;
    s.insert(e, 500).await.map_err(|x| err("first copy", x))?;
    let early = s
        .spend(q(0, 200, None))
        .await
        .map_err(|x| err("first copy", x))?;
    let late = s
        .spend(q(200, 1000, None))
        .await
        .map_err(|x| err("first copy", x))?;
    if early != vec![line("acct-a", "std", 2)] || !late.is_empty() {
        return Err(format!("first copy: early {early:?}, late {late:?}"));
    }
    Ok(())
}

/// Rule 3: the same id from another source is another event.
pub async fn check_source_scopes_id<S: UsageStore>(s: &S) -> Result<(), String> {
    let a = s
        .insert(event("m1", "pod-1", 10), 100)
        .await
        .map_err(|x| err("source", x))?;
    let b = s
        .insert(event("m2", "pod-1", 10), 100)
        .await
        .map_err(|x| err("source", x))?;
    let got = s
        .spend(q(0, 200, None))
        .await
        .map_err(|x| err("source", x))?;
    if a != Inserted::New || b != Inserted::New || got != vec![line("acct-a", "std", 2)] {
        return Err(format!("source: {a:?}, {b:?}, spend {got:?}"));
    }
    Ok(())
}

/// Rule 4: spend sums weights per account and node type, sorted by account then node type.
pub async fn check_sums_and_order<S: UsageStore>(s: &S) -> Result<(), String> {
    let put = |pod: &str, acct: &str, node: &str, w: u32| {
        let mut e = event("m1", pod, 1);
        e.subject = acct.into();
        e.node_type = node.into();
        e.weight = w;
        e
    };
    for e in [
        put("p1", "acct-b", "big", 4),
        put("p2", "acct-a", "std", 1),
        put("p3", "acct-a", "std", 2),
        put("p4", "acct-a", "big", 8),
    ] {
        s.insert(e, 50).await.map_err(|x| err("sums", x))?;
    }
    let got = s.spend(q(0, 100, None)).await.map_err(|x| err("sums", x))?;
    let want = vec![
        line("acct-a", "big", 8),
        line("acct-a", "std", 3),
        line("acct-b", "big", 4),
    ];
    if got != want {
        return Err(format!("sums: got {got:?}, want {want:?}"));
    }
    Ok(())
}

/// Rule 5: the window is half-open on ingestion time: `from` is in, `to` is out.
pub async fn check_window_half_open<S: UsageStore>(s: &S) -> Result<(), String> {
    for (pod, at) in [("p1", 99), ("p2", 100), ("p3", 199), ("p4", 200)] {
        s.insert(event("m1", pod, 1), at)
            .await
            .map_err(|x| err("window", x))?;
    }
    let got = s
        .spend(q(100, 200, None))
        .await
        .map_err(|x| err("window", x))?;
    if got != vec![line("acct-a", "std", 2)] {
        return Err(format!("window: [100, 200) got {got:?}, want 2 minutes"));
    }
    Ok(())
}

/// Rule 6: asking for one account returns that account alone.
pub async fn check_account_filter<S: UsageStore>(s: &S) -> Result<(), String> {
    let mut other = event("m1", "p2", 1);
    other.subject = "acct-b".into();
    s.insert(event("m1", "p1", 1), 50)
        .await
        .map_err(|x| err("filter", x))?;
    s.insert(other, 50).await.map_err(|x| err("filter", x))?;
    let got = s
        .spend(q(0, 100, Some("acct-b")))
        .await
        .map_err(|x| err("filter", x))?;
    if got != vec![line("acct-b", "std", 1)] {
        return Err(format!("filter: got {got:?}"));
    }
    let none = s
        .spend(q(0, 100, Some("acct-z")))
        .await
        .map_err(|x| err("filter", x))?;
    if !none.is_empty() {
        return Err(format!("filter: an account with no events got {none:?}"));
    }
    Ok(())
}

/// Rule 7: an event that breaks the contract is refused with the field's name and stores nothing.
pub async fn check_invalid_refused<S: UsageStore>(s: &S) -> Result<(), String> {
    let mut zero = event("m1", "p1", 1);
    zero.weight = 0;
    let mut spaced = event("m1", "p2", 1);
    spaced.subject = "someone@example.com".into();
    let mut empty = event("m1", "p3", 1);
    empty.node_type.clear();
    let mut long = event("m1", "p4", 1);
    long.agent_service = "a".repeat(129);
    for (bad, field) in [
        (zero, "weight"),
        (spaced, "subject"),
        (empty, "nodeType"),
        (long, "agentService"),
    ] {
        match s.insert(bad, 50).await {
            Err(UsageError::Invalid { field: f }) if f == field => {}
            other => return Err(format!("invalid: want Invalid({field}), got {other:?}")),
        }
    }
    let got = s
        .spend(q(0, 100, None))
        .await
        .map_err(|x| err("invalid", x))?;
    if !got.is_empty() {
        return Err(format!("invalid: a refused event was stored: {got:?}"));
    }
    Ok(())
}

/// Rule 8: an empty or reversed window is `EmptyWindow`, not an empty answer.
pub async fn check_empty_window<S: UsageStore>(s: &S) -> Result<(), String> {
    for (from, to) in [(5, 5), (6, 5)] {
        match s.spend(q(from, to, None)).await {
            Err(UsageError::EmptyWindow) => {}
            other => return Err(format!("empty window: [{from}, {to}) got {other:?}")),
        }
    }
    Ok(())
}

/// Rule 9: a batch answers one entry per event in order: a first copy is `New` (a copy stored by an earlier call
/// is `Duplicate`, and so is a repeat inside the batch), the first copy's time stands, and a batch with an invalid
/// event stores nothing.
pub async fn check_batch<S: UsageStore>(s: &S) -> Result<(), String> {
    s.insert(event("m1", "old", 1), 10)
        .await
        .map_err(|x| err("batch", x))?;
    let got = s
        .insert_many(
            vec![
                event("m1", "p1", 1),
                event("m1", "old", 1),
                event("m1", "p2", 1),
                event("m1", "p1", 1),
            ],
            100,
        )
        .await
        .map_err(|x| err("batch", x))?;
    let want = vec![
        Inserted::New,
        Inserted::Duplicate,
        Inserted::New,
        Inserted::Duplicate,
    ];
    if got != want {
        return Err(format!("batch: got {got:?}, want {want:?}"));
    }
    let at_100 = s
        .spend(q(100, 101, None))
        .await
        .map_err(|x| err("batch", x))?;
    if at_100 != vec![line("acct-a", "std", 2)] {
        return Err(format!("batch: ingested at 100 got {at_100:?}, want 2"));
    }
    let empty = s
        .insert_many(Vec::new(), 100)
        .await
        .map_err(|x| err("batch", x))?;
    if !empty.is_empty() {
        return Err(format!("batch: an empty batch answered {empty:?}"));
    }
    let mut bad = event("m1", "p3", 1);
    bad.weight = 0;
    match s.insert_many(vec![event("m1", "p4", 1), bad], 200).await {
        Err(UsageError::Invalid { field: "weight" }) => {}
        other => return Err(format!("batch: want Invalid(weight), got {other:?}")),
    }
    let after = s
        .spend(q(200, 201, None))
        .await
        .map_err(|x| err("batch", x))?;
    if !after.is_empty() {
        return Err(format!("batch: a refused batch stored {after:?}"));
    }
    Ok(())
}

/// Runs every rule, each on a store `make` returns fresh. The first broken rule is the error.
pub async fn run<S: UsageStore, F: Fn() -> S>(make: F) -> Result<(), String> {
    check_idempotent(&make()).await?;
    check_first_copy_stands(&make()).await?;
    check_source_scopes_id(&make()).await?;
    check_sums_and_order(&make()).await?;
    check_window_half_open(&make()).await?;
    check_account_filter(&make()).await?;
    check_invalid_refused(&make()).await?;
    check_empty_window(&make()).await?;
    check_batch(&make()).await
}
