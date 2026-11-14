//! The Events of an agent's objects (K8, §60a *Events*): which events of the namespace are about which agent, and how a core/v1
//! `Event` becomes an [`AgentEvent`].
//!
//! **The API group is `core/v1`** (`events`, group `""`): it is what `kubectl get events` and `kubectl describe` read, what the
//! kubelet, the deployment controller and the scheduler's recorders write through the API server's compatibility layer, and
//! its `involvedObject` carries the kind and the name this selects by. `events.k8s.io/v1` is the same store with the
//! `regarding` spelling and `eventTime`; reading both would double the Role and the code for the same rows, and a core/v1
//! `Event` is served whichever of the two a recorder wrote (*unverified* here against a real API server: the shapes below are
//! from the Kubernetes API reference, and `tests/fixtures/events` is hand-written from it).
//!
//! # Which event is about which agent
//!
//! A pod's name is made by a controller, and the API server cannot select by a prefix, so the events of the namespace are
//! listed and each is attributed by the **kind and the name of its `involvedObject`**, by the names this operator gives
//! what it makes (`aap-runtime-kubernetes` `names.rs`: the workloads are `<agent>` and, split, `<agent>-front`, the egress
//! proxy is `<agent>-egress`):
//!
//! | Kind | Name |
//! |---|---|
//! | `AgentService`, `AgentConfig` | `<agent>` |
//! | `Deployment`, `StatefulSet`, `Service`, `PodDisruptionBudget` | a workload name `W` |
//! | `ReplicaSet` | `W-<hash>` |
//! | `Pod` | `W-<ordinal>` (a StatefulSet's) or `W-<hash>-<suffix>` (a Deployment's) |
//!
//! where `W` is `<agent>`, `<agent>-front` or `<agent>-egress`. **A name that also fits another agent of the namespace is
//! attributed to neither** (an agent `a` and an agent `a-front` both have a workload `a-front`): an event is never shown on the
//! wrong agent, at the price of leaving out an ambiguous one. Persistent volume claims and the pods a coder makes for its runs
//! are not selected (their names are not this operator's to say).

use aap_ports::{AgentEvent, EventType};
use serde_json::Value;

/// The workload names an agent's objects carry.
fn workloads(agent: &str) -> [String; 3] {
    [
        agent.to_owned(),
        format!("{agent}-front"),
        format!("{agent}-egress"),
    ]
}

fn is_hash(text: &str) -> bool {
    (5..=10).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn is_ordinal(text: &str) -> bool {
    !text.is_empty() && text.len() <= 6 && text.bytes().all(|b| b.is_ascii_digit())
}

/// Whether an object of `kind` named `name` is one of `agent`'s.
pub(crate) fn is_about(agent: &str, kind: &str, name: &str) -> bool {
    if matches!(kind, "AgentService" | "AgentConfig") {
        return name == agent;
    }
    workloads(agent).iter().any(|w| {
        if name == w {
            return matches!(
                kind,
                "Deployment" | "StatefulSet" | "Service" | "PodDisruptionBudget"
            );
        }
        let Some(rest) = name
            .strip_prefix(w.as_str())
            .and_then(|r| r.strip_prefix('-'))
        else {
            return false;
        };
        match kind {
            "ReplicaSet" => is_hash(rest),
            "Pod" => {
                is_ordinal(rest)
                    || rest.split_once('-').is_some_and(|(hash, suffix)| {
                        is_hash(hash) && is_hash(suffix) && suffix.len() == 5
                    })
            }
            _ => false,
        }
    })
}

/// Whether the event is about `agent` and about no other of `agents`.
pub(crate) fn is_only_about(agent: &str, agents: &[String], kind: &str, name: &str) -> bool {
    is_about(agent, kind, name)
        && !agents
            .iter()
            .any(|other| other != agent && is_about(other, kind, name))
}

/// `YYYY-MM-DDTHH:MM:SSZ` from an RFC 3339 time of the API server (UTC, with or without a fraction), or `None`.
fn second(time: &str) -> Option<String> {
    let bytes = time.as_bytes();
    let shaped = bytes.len() >= 20
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && time.ends_with('Z')
        && time[..19]
            .bytes()
            .enumerate()
            .all(|(i, b)| matches!(i, 4 | 7 | 10 | 13 | 16) || b.is_ascii_digit());
    shaped.then(|| format!("{}Z", &time[..19]))
}

/// What a time that cannot be read is: the oldest there is, so it sorts last.
const EPOCH: &str = "1970-01-01T00:00:00Z";

fn text<'a>(event: &'a Value, pointer: &str) -> Option<&'a str> {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// The first of these times that is there and readable.
fn first_time(event: &Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .filter_map(|p| text(event, p))
        .find_map(second)
}

/// The kind and the name an event is about.
pub(crate) fn involved(event: &Value) -> Option<(&str, &str)> {
    Some((
        text(event, "/involvedObject/kind")?,
        text(event, "/involvedObject/name")?,
    ))
}

/// A core/v1 `Event` as an [`AgentEvent`], or `None` when it names no object.
///
/// `count` is the series' when there is one (`series.count`), then the event's own, and 1 when neither is there; the last time
/// is `series.lastObservedTime`, `lastTimestamp`, `eventTime`, the first time, the creation time; the first time is
/// `firstTimestamp`, `eventTime`, the creation time.
pub(crate) fn read(event: &Value) -> Option<AgentEvent> {
    let (kind, name) = involved(event)?;
    let first = first_time(
        event,
        &[
            "/firstTimestamp",
            "/eventTime",
            "/metadata/creationTimestamp",
        ],
    );
    let last = first_time(
        event,
        &[
            "/series/lastObservedTime",
            "/lastTimestamp",
            "/eventTime",
            "/firstTimestamp",
            "/metadata/creationTimestamp",
        ],
    );
    let (first, last) = match (first, last) {
        (Some(f), Some(l)) => (f, l),
        (Some(t), None) | (None, Some(t)) => (t.clone(), t),
        (None, None) => (EPOCH.to_owned(), EPOCH.to_owned()),
    };
    let count = event
        .pointer("/series/count")
        .or_else(|| event.pointer("/count"))
        .and_then(Value::as_u64)
        .map_or(1, |c| u32::try_from(c).unwrap_or(u32::MAX));
    Some(AgentEvent::new(
        EventType::parse(text(event, "/type").unwrap_or("Warning")),
        text(event, "/reason").unwrap_or(""),
        text(event, "/message").unwrap_or(""),
        (kind, name),
        count,
        &first,
        &last,
    ))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn what_an_agent_owns_is_named_by_the_operator() {
        for (kind, name) in [
            ("AgentService", "a"),
            ("AgentConfig", "a"),
            ("Deployment", "a"),
            ("StatefulSet", "a-front"),
            ("Service", "a-egress"),
            ("PodDisruptionBudget", "a"),
            ("ReplicaSet", "a-7d9f8b6c5"),
            ("Pod", "a-0"),
            ("Pod", "a-front-12"),
            ("Pod", "a-7d9f8b6c5-x2x4q"),
            ("Pod", "a-egress-6b7c8d9f4-abcde"),
        ] {
            assert!(is_about("a", kind, name), "{kind} {name}");
        }
        for (kind, name) in [
            ("AgentService", "a-front"),
            ("AgentConfig", "b"),
            ("Deployment", "b"),
            ("Node", "a"),
            ("Secret", "a"),
            ("ConfigMap", "a"),
            ("Pod", "a-run-1"),
            ("Pod", "a"),
            ("Pod", "ab-0"),
            ("ReplicaSet", "a-x"),
            ("PersistentVolumeClaim", "a-work"),
            ("Pod", "a-7d9f8b6c5-x2x"),
        ] {
            assert!(!is_about("a", kind, name), "{kind} {name}");
        }
    }

    #[test]
    fn a_name_that_fits_two_agents_is_neither() {
        let agents = vec!["a".to_owned(), "a-front".to_owned()];
        // `a-front` is `a`'s front workload and `a-front`'s own service workload.
        assert!(!is_only_about("a", &agents, "Deployment", "a-front"));
        assert!(!is_only_about("a-front", &agents, "Deployment", "a-front"));
        // Exact names of the kinds that are one agent's alone stay.
        assert!(is_only_about("a", &agents, "AgentService", "a"));
        assert!(is_only_about("a-front", &agents, "AgentService", "a-front"));
        assert!(is_only_about("a", &agents, "Deployment", "a"));
    }

    #[test]
    fn a_core_event_is_read_whole() {
        let event = json!({
            "metadata": { "name": "a.1", "creationTimestamp": "2026-11-14T10:00:00Z" },
            "involvedObject": { "kind": "Pod", "name": "a-0" },
            "type": "Warning", "reason": "BackOff", "message": "Back-off pulling image \"x\"",
            "count": 7,
            "firstTimestamp": "2026-11-14T10:00:01Z", "lastTimestamp": "2026-11-14T10:05:09Z",
        });
        let got = read(&event).unwrap();
        assert_eq!(got.event_type, EventType::Warning);
        assert_eq!(got.reason, "BackOff");
        assert_eq!(got.count, 7);
        assert_eq!(got.first_seen, "2026-11-14T10:00:01Z");
        assert_eq!(got.last_seen, "2026-11-14T10:05:09Z");
        assert_eq!(
            (got.object.kind.as_str(), got.object.name.as_str()),
            ("Pod", "a-0")
        );
    }

    #[test]
    fn a_series_and_an_event_time_are_read_when_the_old_fields_are_not_there() {
        let event = json!({
            "metadata": { "creationTimestamp": "2026-11-14T10:00:00Z" },
            "involvedObject": { "kind": "Pod", "name": "a-0" },
            "type": "Normal", "reason": "Pulled", "message": "ok",
            "eventTime": "2026-11-14T10:01:02.123456Z",
            "series": { "count": 3, "lastObservedTime": "2026-11-14T10:09:00.000000Z" },
        });
        let got = read(&event).unwrap();
        assert_eq!(got.count, 3);
        assert_eq!(got.first_seen, "2026-11-14T10:01:02Z");
        assert_eq!(got.last_seen, "2026-11-14T10:09:00Z");
        assert_eq!(got.event_type, EventType::Normal);
    }

    #[test]
    fn what_is_missing_is_not_made_up() {
        assert!(read(&json!({ "involvedObject": { "kind": "Pod" } })).is_none());
        let bare = read(&json!({ "involvedObject": { "kind": "Pod", "name": "p" } })).unwrap();
        assert_eq!(bare.count, 1);
        assert_eq!(bare.last_seen, EPOCH);
        assert_eq!(
            bare.event_type,
            EventType::Warning,
            "no type is shown, not hidden"
        );
        let odd = read(&json!({ "involvedObject": { "kind": "Pod", "name": "p" },
                                 "lastTimestamp": "yesterday" }))
        .unwrap();
        assert_eq!(odd.last_seen, EPOCH);
    }
}
