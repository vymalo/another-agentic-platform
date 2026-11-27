//! The pod-minute meter's pure part (ANR-673, AD-069): what a pod's facts say about the minutes it ran, with
//! no clock of its own, no Kubernetes client and no I/O. [`crate::meter`] reads pods and sends; this decides.
//!
//! **Counting.** A minute counts if the pod was Running at **any instant** in it: idle Running counts, Pending
//! never does. A running pod's minute is counted once it is over (its end is not after `now`); a pod that is
//! over (finished, or being deleted) counts through the minute of its end, at once. A pod that finished while the
//! operator was down ends at its last container's `finishedAt`; one deleted with no status ends at its deletion
//! time. Minutes older than [`BACKFILL_MINUTES`] are skipped, and counted.
//!
//! **Determinism.** Everything here is a function of the facts and `now`, so a restart and a second replica
//! derive the same minutes, and (with `aap_usage::event_id`) the same ids and end-of-minute times.

use std::collections::{BTreeMap, BTreeSet};

use aap_usage::WEIGHT_GRID;
use serde_json::Value;

/// How far back a minute is still sent: 24 h. An older one is skipped and counted
/// (`aap_meter_backfill_skipped_total`).
pub const BACKFILL_MINUTES: u64 = 24 * 60;

/// `app.kubernetes.io/managed-by` of a run pod (`aap_domain::contract::run_pods::MANAGED_BY`).
pub const RUN_MANAGED_BY: &str = "adam-coder";
/// `app.kubernetes.io/managed-by` of an agent's own pod (`aap_runtime_kubernetes::names::MANAGED_BY_VALUE`).
pub const AGENT_MANAGED_BY: &str = "aap-operator";
/// `app.kubernetes.io/managed-by`.
pub const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
/// `app.kubernetes.io/component`.
pub const COMPONENT_LABEL: &str = "app.kubernetes.io/component";
/// `app.kubernetes.io/instance`: the agent service.
pub const INSTANCE_LABEL: &str = "app.kubernetes.io/instance";
/// `app.kubernetes.io/name`: a run pod's is `<service>-run`.
pub const NAME_LABEL: &str = "app.kubernetes.io/name";
/// The component of a run pod.
pub const RUN_COMPONENT: &str = "run";
/// The label the coder puts on a run pod: the run id (`aap_domain::contract::run_pods::RUN_LABEL`).
pub const RUN_LABEL: &str = "adam.vymalo.com/run";
/// The annotation of the size weight (`aap_domain::contract::run_pods::SIZE_WEIGHT_ANNOTATION`).
pub const SIZE_WEIGHT_ANNOTATION: &str = "agents.vymalo.com/size-weight";
/// The components of an agent's own workload (`aap_runtime_kubernetes::names::component`). The egress proxy's
/// (`egress-proxy`) is deliberately not among them: it is no agent's work.
pub const AGENT_COMPONENTS: [&str; 3] = ["agent", "worker", "front"];

/// Why a pod's minutes make no line. A closed set: it is the `reason` label of `aap_meter_unattributed_total`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unattributed {
    /// No app container ever started although the pod is not Pending (a crash loop whose container never ran): there
    /// is no start to bill from. Counted once per pod.
    NoStart,
    /// A pod that finished (Succeeded or Failed) and says when nowhere: neither a container's `finishedAt` nor a
    /// deletion time. Counted once per pod.
    NoEnd,
    /// The pod has no size weight (an agent pod has none yet, AD-069; a run pod whose annotation the coder did
    /// not carry over, or that is off the grid).
    NoWeight,
    /// The node type cannot be told: no label key is configured, or the pod's required affinity does not admit
    /// exactly one value.
    UnknownNodeType,
    /// The account source does not know the pod.
    UnknownAccount,
    /// A run pod without its run label, or a pod without its agent service.
    NoIdentity,
}

impl Unattributed {
    /// The label value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoEnd => "no-end",
            Self::NoStart => "no-start",
            Self::NoWeight => "no-weight",
            Self::UnknownNodeType => "unknown-node-type",
            Self::UnknownAccount => "unknown-account",
            Self::NoIdentity => "no-identity",
        }
    }
}

/// What the meter reads of a pod. Built from the Kubernetes object by [`crate::meter`]; tests build it by hand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PodFacts {
    /// `metadata.uid`.
    pub uid: String,
    /// `metadata.labels`.
    pub labels: BTreeMap<String, String>,
    /// `metadata.annotations`.
    pub annotations: BTreeMap<String, String>,
    /// `status.phase`.
    pub phase: String,
    /// The earliest of the **app** containers' (`spec.containers`) `state.running.startedAt`,
    /// `state.terminated.startedAt` and `lastState.terminated.startedAt`, in seconds since the Unix epoch. Init
    /// containers and native sidecars never set it, and neither does `status.startTime` (the kubelet's, which
    /// includes Pending and the image pull). Times have **second resolution**.
    pub started_at: Option<i64>,
    /// Whether the pod reports any container status at all.
    pub has_container_status: bool,
    /// The latest `state.terminated.finishedAt`, when **every** container has terminated. The end is
    /// **exclusive**: a pod that finished at 12:03:00 did not run in the minute 12:03.
    pub finished_at: Option<i64>,
    /// `metadata.deletionTimestamp`.
    pub deleted_at: Option<i64>,
    /// `spec.affinity.nodeAffinity`, as written.
    pub node_affinity: Option<Value>,
}

/// What a pod is, for the meter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Not metered (another component, another manager, the egress proxy).
    Skip,
    /// A run pod: its agent service and its run, when it has them.
    Run {
        /// The agent service, if known.
        service: Option<String>,
        /// The run id, if the pod carries it.
        run: Option<String>,
    },
    /// An agent's own pod: its agent service, if known.
    Agent {
        /// The agent service.
        service: Option<String>,
    },
}

/// Tells what a pod is from its labels: a run pod is `managed-by=adam-coder` and `component=run`; an agent pod is
/// `managed-by=aap-operator` and a workload component. Everything else, the egress proxy included, is skipped.
pub fn classify(p: &PodFacts) -> Kind {
    let label = |k: &str| p.labels.get(k).map(String::as_str);
    match (label(MANAGED_BY_LABEL), label(COMPONENT_LABEL)) {
        (Some(RUN_MANAGED_BY), Some(RUN_COMPONENT)) => Kind::Run {
            // A run pod's own template labels carry only its name (`<service>-run`); the instance is the
            // service when the coder adds it.
            service: label(INSTANCE_LABEL)
                .map(str::to_owned)
                .or_else(|| {
                    label(NAME_LABEL)
                        .and_then(|n| n.strip_suffix("-run"))
                        .map(str::to_owned)
                })
                .filter(|s| !s.is_empty()),
            run: label(RUN_LABEL)
                .filter(|r| !r.is_empty())
                .map(str::to_owned),
        },
        (Some(AGENT_MANAGED_BY), Some(c)) if AGENT_COMPONENTS.contains(&c) => Kind::Agent {
            service: label(INSTANCE_LABEL)
                .filter(|s| !s.is_empty())
                .map(str::to_owned),
        },
        _ => Kind::Skip,
    }
}

/// The weight in quarter-units from the annotation `agents.vymalo.com/size-weight`, which `SizeWeight`'s `Display`
/// writes as the decimal grid value (`0.25`, `0.5`, `1`, `2`, `4`, `8`). Anything else, or a value off the grid,
/// is `None`.
pub fn weight_quarters(annotations: &BTreeMap<String, String>) -> Option<u32> {
    let text = annotations.get(SIZE_WEIGHT_ANNOTATION)?;
    let quarters = match text.as_str() {
        "0.25" => 1,
        "0.5" => 2,
        other => other.parse::<u32>().ok()?.checked_mul(4)?,
    };
    WEIGHT_GRID.contains(&quarters).then_some(quarters)
}

/// The node type from the required node affinity the operator injects (`labelKey In admitted`, AD-066): the one
/// value every required term admits for `label_key`. `None` when no key is configured, when a term has no `In`
/// on the key (it would admit other node types), or when the terms together admit more than one value. Nothing
/// is read from a Node: no `ClusterRole`.
pub fn node_type(affinity: Option<&Value>, label_key: &str) -> Option<String> {
    if label_key.is_empty() {
        return None;
    }
    let terms = affinity?
        .pointer("/requiredDuringSchedulingIgnoredDuringExecution/nodeSelectorTerms")?
        .as_array()?;
    if terms.is_empty() {
        return None;
    }
    let mut admitted: BTreeSet<&str> = BTreeSet::new();
    for term in terms {
        let mut here: Option<BTreeSet<&str>> = None;
        for expr in term.get("matchExpressions")?.as_array()? {
            if expr["key"] != label_key || expr["operator"] != "In" {
                continue;
            }
            let values: BTreeSet<&str> = expr["values"]
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .collect();
            here = Some(match here {
                Some(prev) => prev.intersection(&values).copied().collect(),
                None => values,
            });
        }
        admitted.extend(here?);
    }
    let mut it = admitted.into_iter();
    match (it.next(), it.next()) {
        (Some(one), None) => Some(one.to_owned()),
        _ => None,
    }
}

/// The minutes a pod ran in, and whether that is all of them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    /// The half-open range of minutes since the Unix epoch that are over (see [`minutes`]).
    pub range: std::ops::Range<u64>,
    /// Whether the range reaches the pod's end: the pod is over and so is its last minute. A pod that is still
    /// running, or whose last minute is not over yet, is not complete.
    pub complete: bool,
}

/// Why a pod makes no minutes although it is not Pending.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unmetered {
    /// It finished (Succeeded or Failed) and says when nowhere.
    NoEnd,
    /// No app container ever started.
    NoStart,
}

/// The whole minutes a pod ran in, or `None` when it is Pending, or [`Unmetered`].
///
/// **A minute counts if the pod was Running at any instant in it, and is sent only when it is over**: the newest
/// minute is the one that ended at or before `now - skew` (`skew` covers the difference between this clock and the
/// API server's). The end is **exclusive** and times have second resolution: a pod that finished at 12:03:00 did not
/// run in 12:03. A pod ends at `finished_at` (the last `finishedAt`, once every app container has finished) and, when
/// that is absent, at `deleted_at` (`deletionTimestamp` is the *end* of the grace period, so it is usually in the
/// future, and a pod whose end is after `now - skew` is counted only up to `now - skew`). **Under-billing is the rule, never over-billing.**
pub fn minutes(p: &PodFacts, now: i64, skew: i64) -> Result<Option<Ran>, Unmetered> {
    if p.phase == "Pending" || p.phase.is_empty() {
        return Ok(None);
    }
    let Some(start) = p.started_at else {
        return Err(Unmetered::NoStart);
    };
    let end = p.finished_at.or(p.deleted_at);
    if matches!(p.phase.as_str(), "Succeeded" | "Failed") && end.is_none() {
        return Err(Unmetered::NoEnd);
    }
    let end = end.map(|e| e.max(start));
    let first = to_minute(start);
    let over = to_minute(now.saturating_sub(skew));
    let closed_at = match end {
        Some(e) if e <= now.saturating_sub(skew) => {
            Some(to_minute(e.saturating_sub(1)).max(first) + 1)
        }
        _ => None,
    };
    let end_exclusive = closed_at.map_or(over, |c| c.min(over));
    let complete = closed_at.is_some_and(|c| c <= over);
    let range = if first < end_exclusive {
        first..end_exclusive
    } else {
        first..first
    };
    Ok(Some(Ran { range, complete }))
}

fn to_minute(secs: i64) -> u64 {
    u64::try_from(secs.div_euclid(60)).unwrap_or(0)
}

/// What is due of a pod's minutes: `skipped` older than the backfill bound, and `send` the ones to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Due {
    /// The minutes to send, in order.
    pub send: std::ops::Range<u64>,
    /// How many were too old to send.
    pub skipped: u64,
}

/// What of `ran` is still to do, given the pod's `cursor` (the first minute not yet sent, if any) at `now`.
pub fn due(ran: &std::ops::Range<u64>, cursor: Option<u64>, now: i64) -> Due {
    let from = cursor.map_or(ran.start, |c| c.max(ran.start));
    let oldest = to_minute(now).saturating_sub(BACKFILL_MINUTES);
    let start = from.max(oldest).min(ran.end);
    Due {
        skipped: start.saturating_sub(from),
        send: start..ran.end.max(start),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn labels(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn running(start: i64) -> PodFacts {
        PodFacts {
            uid: "u".into(),
            phase: "Running".into(),
            started_at: Some(start),
            ..PodFacts::default()
        }
    }

    #[test]
    fn a_run_pod_an_agent_pod_and_the_egress_proxy() {
        let run = PodFacts {
            labels: labels(&[
                (MANAGED_BY_LABEL, "adam-coder"),
                (COMPONENT_LABEL, "run"),
                (NAME_LABEL, "coder-run"),
                (RUN_LABEL, "r1"),
            ]),
            ..PodFacts::default()
        };
        assert_eq!(
            classify(&run),
            Kind::Run {
                service: Some("coder".into()),
                run: Some("r1".into())
            }
        );
        let agent = PodFacts {
            labels: labels(&[
                (MANAGED_BY_LABEL, "aap-operator"),
                (COMPONENT_LABEL, "agent"),
                (INSTANCE_LABEL, "coder"),
            ]),
            ..PodFacts::default()
        };
        assert_eq!(
            classify(&agent),
            Kind::Agent {
                service: Some("coder".into())
            }
        );
        let proxy = PodFacts {
            labels: labels(&[
                (MANAGED_BY_LABEL, "aap-operator"),
                (COMPONENT_LABEL, "egress-proxy"),
                (INSTANCE_LABEL, "coder"),
            ]),
            ..PodFacts::default()
        };
        assert_eq!(classify(&proxy), Kind::Skip);
        // The coder's own manager with another component is not a run pod either.
        let other = PodFacts {
            labels: labels(&[(MANAGED_BY_LABEL, "adam-coder"), (COMPONENT_LABEL, "x")]),
            ..PodFacts::default()
        };
        assert_eq!(classify(&other), Kind::Skip);
    }

    #[test]
    fn the_annotation_is_the_decimal_grid_value() {
        let w = |v: &str| weight_quarters(&labels(&[(SIZE_WEIGHT_ANNOTATION, v)]));
        assert_eq!(w("0.25"), Some(1));
        assert_eq!(w("0.5"), Some(2));
        assert_eq!(w("1"), Some(4));
        assert_eq!(w("8"), Some(32));
        assert_eq!(w("3"), None);
        assert_eq!(w("0"), None);
        assert_eq!(w("x"), None);
        assert_eq!(weight_quarters(&BTreeMap::new()), None);
    }

    #[test]
    fn the_weights_are_the_domain_grid() {
        let domain: Vec<u32> = aap_domain::SizeWeight::GRID
            .iter()
            .map(|q| u32::from(*q))
            .collect();
        assert_eq!(domain, WEIGHT_GRID.to_vec());
        // Every grid value's Display round-trips through the annotation reader. `SizeWeight` has no public
        // constructor from a quarter count, so the decimal texts are the ones its `Display` documents.
        for (text, quarters) in [
            ("0.25", 1),
            ("0.5", 2),
            ("1", 4),
            ("2", 8),
            ("4", 16),
            ("8", 32),
        ] {
            assert_eq!(
                weight_quarters(&labels(&[(SIZE_WEIGHT_ANNOTATION, text)])),
                Some(quarters)
            );
        }
    }

    fn affinity(terms: Value) -> Value {
        json!({ "requiredDuringSchedulingIgnoredDuringExecution": { "nodeSelectorTerms": terms } })
    }

    #[test]
    fn one_admitted_value_is_the_node_type() {
        let a = affinity(json!([{ "matchExpressions": [
            { "key": "other", "operator": "In", "values": ["x", "y"] },
            { "key": "node-type", "operator": "In", "values": ["big"] }
        ]}]));
        assert_eq!(node_type(Some(&a), "node-type").as_deref(), Some("big"));
    }

    #[test]
    fn several_values_none_or_a_term_without_the_key_is_unknown() {
        let two = affinity(json!([{ "matchExpressions": [
            { "key": "node-type", "operator": "In", "values": ["big", "std"] }
        ]}]));
        assert_eq!(node_type(Some(&two), "node-type"), None);
        let terms_disagree = affinity(json!([
            { "matchExpressions": [{ "key": "node-type", "operator": "In", "values": ["big"] }] },
            { "matchExpressions": [{ "key": "node-type", "operator": "In", "values": ["std"] }] }
        ]));
        assert_eq!(node_type(Some(&terms_disagree), "node-type"), None);
        let one_term_open = affinity(json!([
            { "matchExpressions": [{ "key": "node-type", "operator": "In", "values": ["big"] }] },
            { "matchExpressions": [{ "key": "zone", "operator": "In", "values": ["a"] }] }
        ]));
        assert_eq!(node_type(Some(&one_term_open), "node-type"), None);
        // Terms are alternatives: a second term that admits `std` lets the pod land on `std`.
        let alternative = affinity(json!([
            { "matchExpressions": [{ "key": "node-type", "operator": "In", "values": ["big"] }] },
            { "matchExpressions": [{ "key": "node-type", "operator": "In", "values": ["big", "std"] }] }
        ]));
        assert_eq!(node_type(Some(&alternative), "node-type"), None);
        // Several `In` on the key within one term are ANDed: the intersection is what is admitted.
        let agree = affinity(json!([
            { "matchExpressions": [{ "key": "node-type", "operator": "In", "values": ["big"] }] },
            { "matchExpressions": [
                { "key": "node-type", "operator": "In", "values": ["big", "std"] },
                { "key": "node-type", "operator": "In", "values": ["big"] },
                { "key": "zone", "operator": "In", "values": ["a"] }
            ] }
        ]));
        assert_eq!(node_type(Some(&agree), "node-type").as_deref(), Some("big"));
        let not_in = affinity(json!([{ "matchExpressions": [
            { "key": "node-type", "operator": "NotIn", "values": ["big"] }
        ]}]));
        assert_eq!(node_type(Some(&not_in), "node-type"), None);
        assert_eq!(node_type(None, "node-type"), None);
        assert_eq!(node_type(Some(&agree), ""), None);
        assert_eq!(node_type(Some(&json!({})), "node-type"), None);
    }

    fn mins(p: &PodFacts, now: i64) -> Option<std::ops::Range<u64>> {
        minutes(p, now, 0).unwrap().map(|r| r.range)
    }

    #[test]
    fn pending_never_counts_and_idle_running_does() {
        // A Pending pod whose container reports a start (a pod that is still being set up) still never counts.
        let mut p = running(120);
        p.phase = "Pending".into();
        assert_eq!(mins(&p, 1_000), None);
        let mut never = running(120);
        never.started_at = None;
        assert_eq!(minutes(&never, 1_000, 0), Err(Unmetered::NoStart));
        // Started at 02:00, now 02:00:30: the minute is not over.
        assert_eq!(mins(&running(120), 150).map(|r| r.is_empty()), Some(true));
        // Now 03:00: minute 2 is over.
        assert_eq!(mins(&running(120), 180), Some(2..3));
        // Started at 02:59 (second 179), now 03:05: minute 2 was run in, for a second.
        assert_eq!(mins(&running(179), 185), Some(2..3));
    }

    #[test]
    fn a_minute_that_is_not_over_is_never_sent_and_the_skew_holds_the_edge() {
        let p = running(120);
        // Now 03:00:03 with a 5 s skew: minute 2 ended 3 s ago on this clock, which may be ahead: not yet.
        assert_eq!(minutes(&p, 183, 5).unwrap().unwrap().range, 2..2);
        assert_eq!(minutes(&p, 185, 5).unwrap().unwrap().range, 2..3);
        // A finished pod waits for its last minute to be over too, and is then complete.
        let mut done = running(120);
        done.phase = "Succeeded".into();
        done.finished_at = Some(130);
        let early = minutes(&done, 150, 5).unwrap().unwrap();
        assert_eq!((early.range, early.complete), (2..2, false));
        let late = minutes(&done, 185, 5).unwrap().unwrap();
        assert_eq!((late.range, late.complete), (2..3, true));
    }

    #[test]
    fn a_terminating_pod_is_not_billed_into_the_minute_its_grace_ends_in() {
        // Deleted: the grace ends at 04:30 (second 270), in the future. Now is 03:20 (200).
        let mut p = running(120);
        p.has_container_status = true;
        p.deleted_at = Some(270);
        let r = minutes(&p, 200, 5).unwrap().unwrap();
        assert_eq!(
            r.range,
            2..3,
            "minute 3 is not over, and the grace end is not the end"
        );
        assert!(!r.complete);
        // Once the grace end is in the past it is the end: minutes 2, 3 and 4 (second 270 is in minute 4).
        let r = minutes(&p, 400, 5).unwrap().unwrap();
        assert_eq!(r.range, 2..5);
        assert!(r.complete);
        // Every container finished: the last finishedAt wins over the deletion time.
        p.finished_at = Some(190);
        let r = minutes(&p, 400, 5).unwrap().unwrap();
        assert_eq!(r.range, 2..4);
    }

    #[test]
    fn a_finished_pod_ends_at_its_last_finish_and_the_end_is_exclusive() {
        let mut p = running(120);
        p.phase = "Succeeded".into();
        p.finished_at = Some(300);
        // 02:00 to 05:00 exactly: minutes 2, 3 and 4; not 5.
        assert_eq!(mins(&p, 10_000), Some(2..5));
        p.finished_at = Some(301);
        assert_eq!(mins(&p, 10_000), Some(2..6));
        // Finished in the minute it started: that minute counts.
        p.finished_at = Some(125);
        assert_eq!(mins(&p, 10_000), Some(2..3));
        // No finish: the deletion time.
        p.finished_at = None;
        p.deleted_at = Some(250);
        assert_eq!(mins(&p, 10_000), Some(2..5));
    }

    #[test]
    fn a_finished_pod_that_says_when_nowhere_has_no_end() {
        let mut p = running(120);
        p.phase = "Failed".into();
        assert_eq!(minutes(&p, 10_000, 0), Err(Unmetered::NoEnd));
        p.phase = "Succeeded".into();
        assert_eq!(minutes(&p, 10_000, 0), Err(Unmetered::NoEnd));
    }

    #[test]
    fn a_deleted_pod_with_no_status_ends_at_its_deletion() {
        let mut p = running(120);
        p.deleted_at = Some(250);
        assert_eq!(mins(&p, 10_000), Some(2..5));
        p.finished_at = Some(200);
        assert_eq!(mins(&p, 10_000), Some(2..4));
    }

    #[test]
    fn a_pod_whose_app_containers_never_started_has_no_start_and_borrows_none() {
        let mut p = running(0);
        p.started_at = None;
        assert_eq!(minutes(&p, 10_000, 0), Err(Unmetered::NoStart));
        // Pending is Pending, not a missing start.
        p.phase = "Pending".into();
        assert_eq!(minutes(&p, 10_000, 0), Ok(None));
    }

    #[test]
    fn backfill_is_bounded_and_counted() {
        let now = 100_000 * 60;
        let ran = 0..100_000u64;
        let d = due(&ran, None, now);
        assert_eq!(d.send.start, 100_000 - BACKFILL_MINUTES);
        assert_eq!(d.skipped, 100_000 - BACKFILL_MINUTES);
        assert_eq!(d.send.end, 100_000);
        // A cursor inside the window is kept, and nothing is skipped.
        let d = due(&ran, Some(99_990), now);
        assert_eq!((d.send, d.skipped), (99_990..100_000, 0));
        // Everything sent: nothing due.
        let d = due(&ran, Some(100_000), now);
        assert!(d.send.is_empty());
        assert_eq!(d.skipped, 0);
    }

    #[test]
    fn the_same_facts_give_the_same_minutes() {
        let p = running(1_234_567);
        assert_eq!(minutes(&p, 2_000_000, 5), minutes(&p.clone(), 2_000_000, 5));
    }
}
