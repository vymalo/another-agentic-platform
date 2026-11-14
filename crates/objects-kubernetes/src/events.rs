//! The Events of an agent's objects (K8, §60a *Events*): which events of the namespace are about which agent, and how a core/v1
//! `Event` becomes an [`AgentEvent`].
//!
//! **The API group is `core/v1`** (`events`, group `""`): it is what `kubectl get events` and `kubectl describe` read, what the
//! kubelet, the deployment controller and the scheduler's recorders write through the API server's compatibility layer, and
//! its `involvedObject` carries the kind, the name and the uid this selects by. `events.k8s.io/v1` is the same store with the
//! `regarding` spelling and `eventTime`; reading both would double the Role and the code for the same rows (*unverified* here
//! against a real API server: the shapes below are from the Kubernetes API reference, and `tests/fixtures/events` is
//! hand-written from it).
//!
//! # Which event is about which agent
//!
//! **By the names the operator gives an agent's objects, and for the agent's own `AgentService` and `AgentConfig` by uid.** The
//! Role has no right on pods or workloads, so no owner reference can be read: an unrelated object of the namespace whose name
//! has the shape of one of an agent's (a pod `web-<hash>-<suffix>` of something that is not the agent's Deployment, if
//! it also has the controllers' alphabet and lengths) **can appear**; an event of another *agent's* object never does.
//!
//! | Kind | Name | `by` |
//! |---|---|---|
//! | `AgentService`, `AgentConfig` | exactly `<agent>`; if the event names a `uid`, it is the object's now (the one this call read: an agent made again under the name does not inherit the old one's events) | `uid`, or `name` when the event names none |
//! | `Deployment`, `StatefulSet`, `Service`, `PodDisruptionBudget` | exactly `W` | `name` |
//! | `ReplicaSet` | `W-<hash>` | `name` |
//! | `Pod` | `W-<ordinal>` (a StatefulSet's) or `W-<hash>-<suffix>` (a Deployment's) | `name` |
//! | `Cluster` (CloudNativePG) | exactly `<agent>-db` | `name` |
//! | `Service` | `<agent>-db-rw`, `-r`, `-ro` | `name` |
//! | `Job` | `<agent>-db-<n>-initdb` or `-join` | `name` |
//! | `Pod` | `<agent>-db-<n>` or `<agent>-db-<n>-(initdb|join)-<suffix>` | `name` |
//!
//! `W` is `<agent>`, `<agent>-front` or `<agent>-egress` (`aap-runtime-kubernetes` `names.rs`; the store's `<agent>-db` is
//! `aap-store-cnpg`'s `cluster_name`). A **hash** is the pod-template-hash a Deployment adds to its ReplicaSet and pods: the
//! decimal digits of a 32-bit FNV hash, each mapped to the alphabet `bcdfghjklmnpqrstvwxz2456789` (no vowel, no `0`, no `1`,
//! no `3`), so 1 to 10 characters; **5 to 10 are matched** (a shorter one has a probability of about 2e-6). A **suffix** is
//! the 5 characters a `generateName` adds, from the same alphabet. *Both are unverified here* (from `k8s.io/apimachinery/pkg/util/rand`
//! `SafeEncodeString`, `k8s.io/apiserver/pkg/storage/names` and `k8s.io/kubernetes/pkg/controller/controller_utils.go`, read from
//! memory and not fetched in this session). CloudNativePG's names are also *unverified* (from its documentation, from memory).
//! The names are matched exactly with a `-` between the parts: `a7` and `abcdefg` are not `a`'s.
//!
//! **An ambiguous name is attributed to neither agent.** An event is dropped when another agent of the namespace could own the
//! object by the same table, **and** when another agent's name is one of the bases this agent's names are built on
//! (`<agent>-front`, `<agent>-egress`, `<agent>-db`) and the object's name is that base or starts with it and a `-`: agents `a`
//! and `a-db` both claim `a-db-1`, and `a`'s own `Cluster` `a-db` is dropped while an agent `a-db` exists. Persistent volume
//! claims and the pods a coder makes for its runs are not selected (their names are not this operator's to say).

use aap_ports::{AgentEvent, Attribution, EventType};
use serde_json::Value;

/// The alphabet of the pod-template-hash and of the 5 random characters a `generateName` adds (*unverified*, see above).
const SAFE: &str = "bcdfghjklmnpqrstvwxz2456789";

fn safe(text: &str) -> bool {
    text.chars().all(|c| SAFE.contains(c))
}

fn is_hash(text: &str) -> bool {
    (5..=10).contains(&text.len()) && safe(text)
}

fn is_suffix(text: &str) -> bool {
    text.len() == 5 && safe(text)
}

fn is_ordinal(text: &str) -> bool {
    !text.is_empty() && text.len() <= 6 && text.bytes().all(|b| b.is_ascii_digit())
}

/// The workload names an agent's objects carry.
fn workloads(agent: &str) -> [String; 3] {
    [
        agent.to_owned(),
        format!("{agent}-front"),
        format!("{agent}-egress"),
    ]
}

/// `name` with the prefix `prefix` and a `-` taken off, or `None`: the `-` is required.
fn after<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    name.strip_prefix(prefix)?.strip_prefix('-')
}

/// Whether an object of `kind` named `name` has the name and the shape of one of `agent`'s workloads, or of its store.
pub(crate) fn is_about(agent: &str, kind: &str, name: &str) -> bool {
    let db = format!("{agent}-db");
    let store = match kind {
        "Cluster" => name == db,
        "Service" => after(name, &db).is_some_and(|r| matches!(r, "rw" | "r" | "ro")),
        "Job" => after(name, &db).is_some_and(|r| {
            r.split_once('-')
                .is_some_and(|(n, job)| is_ordinal(n) && matches!(job, "initdb" | "join"))
        }),
        "Pod" => after(name, &db).is_some_and(|r| match r.split_once('-') {
            None => is_ordinal(r),
            Some((n, rest)) => {
                is_ordinal(n)
                    && rest.rsplit_once('-').is_some_and(|(job, suffix)| {
                        matches!(job, "initdb" | "join") && is_suffix(suffix)
                    })
            }
        }),
        _ => false,
    };
    store
        || workloads(agent).iter().any(|w| {
            if name == w {
                return matches!(
                    kind,
                    "Deployment" | "StatefulSet" | "Service" | "PodDisruptionBudget"
                );
            }
            let Some(rest) = after(name, w) else {
                return false;
            };
            match kind {
                "ReplicaSet" => is_hash(rest),
                "Pod" => {
                    is_ordinal(rest)
                        || rest
                            .split_once('-')
                            .is_some_and(|(hash, suffix)| is_hash(hash) && is_suffix(suffix))
                }
                _ => false,
            }
        })
}

/// The bases another agent's name could be equal to, so that the objects under them are not this agent's.
fn bases(agent: &str) -> [String; 3] {
    [
        format!("{agent}-front"),
        format!("{agent}-egress"),
        format!("{agent}-db"),
    ]
}

/// The objects of the agent whose own uids are known: what an event naming a `uid` is compared with.
pub(crate) struct Own<'a> {
    pub(crate) agent: &'a str,
    /// Every agent of the namespace, this one included.
    pub(crate) agents: &'a [String],
    pub(crate) service_uid: &'a str,
    pub(crate) config_uid: Option<&'a str>,
}

impl Own<'_> {
    /// How the event of the object `kind` `name` (with `uid`, if the event names one) is this agent's, or `None` when it is not
    /// or when it could be another agent's.
    pub(crate) fn attribute(
        &self,
        kind: &str,
        name: &str,
        uid: Option<&str>,
    ) -> Option<Attribution> {
        if matches!(kind, "AgentService" | "AgentConfig") {
            if name != self.agent {
                return None;
            }
            let now = if kind == "AgentService" {
                Some(self.service_uid)
            } else {
                self.config_uid
            };
            return match (uid, now) {
                (Some(seen), Some(now)) if !now.is_empty() => {
                    (seen == now).then_some(Attribution::Uid)
                }
                _ => Some(Attribution::Name),
            };
        }
        if !is_about(self.agent, kind, name) {
            return None;
        }
        let others = || self.agents.iter().filter(|a| a.as_str() != self.agent);
        let contested = others().any(|other| is_about(other, kind, name))
            || bases(self.agent).iter().any(|base| {
                self.agents.contains(base) && (name == base || after(name, base).is_some())
            });
        (!contested).then_some(Attribution::Name)
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ` from an RFC 3339 time of the API server (UTC, with or without a fraction), or `None` when it is not
/// one. Never indexes by bytes: a multi-byte character anywhere makes it `None`.
fn second(time: &str) -> Option<String> {
    let head = time.get(..19)?;
    let b = head.as_bytes();
    let shaped = time.len() >= 20
        && time.ends_with('Z')
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            _ => c.is_ascii_digit(),
        });
    shaped.then(|| format!("{head}Z"))
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
pub(crate) fn involved(event: &Value) -> Option<(&str, &str, Option<&str>)> {
    Some((
        text(event, "/involvedObject/kind")?,
        text(event, "/involvedObject/name")?,
        text(event, "/involvedObject/uid"),
    ))
}

/// A core/v1 `Event` as an [`AgentEvent`], or `None` when it names no object.
///
/// `count` is the series' when there is one (`series.count`), then the event's own, and 1 when neither is there; the last time
/// is `series.lastObservedTime`, `lastTimestamp`, `eventTime`, the first time, the creation time; the first time is
/// `firstTimestamp`, `eventTime`, the creation time.
pub(crate) fn read(event: &Value) -> Option<AgentEvent> {
    let (kind, name, _) = involved(event)?;
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

    fn own<'a>(agent: &'a str, agents: &'a [String]) -> Own<'a> {
        Own {
            agent,
            agents,
            service_uid: "uid-service",
            config_uid: Some("uid-config"),
        }
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn what_an_agent_owns_is_named_by_the_operator() {
        for (kind, name) in [
            ("Deployment", "a"),
            ("StatefulSet", "a-front"),
            ("Service", "a-egress"),
            ("PodDisruptionBudget", "a"),
            ("ReplicaSet", "a-6b7c8d9f4"),
            ("Pod", "a-0"),
            ("Pod", "a-front-12"),
            ("Pod", "a-6b7c8d9f4-x2x4q"),
            ("Pod", "a-egress-6b7c8d9f4-bcdfg"),
            ("Cluster", "a-db"),
            ("Service", "a-db-rw"),
            ("Pod", "a-db-1"),
            ("Pod", "a-db-2-initdb-x2x4q"),
            ("Job", "a-db-1-initdb"),
        ] {
            assert!(is_about("a", kind, name), "{kind} {name}");
        }
        for (kind, name) in [
            ("Deployment", "b"),
            ("Deployment", "a7"),
            ("Node", "a"),
            ("Secret", "a"),
            ("ConfigMap", "a"),
            ("Pod", "a-run-1"),
            ("Pod", "a"),
            ("Pod", "ab-0"),
            ("Pod", "a7"),
            ("ReplicaSet", "abcdefg"),
            ("ReplicaSet", "a-x"),
            ("ReplicaSet", "a-redis"),
            ("PersistentVolumeClaim", "a-work"),
            ("Pod", "a-6b7c8d9f4-x2x"),
            ("Pod", "a-abcde-12345"),
            ("Pod", "a-migrate-x7k2p"),
            ("Pod", "a-db"),
            ("Cluster", "a"),
            ("Cluster", "a-db-1"),
        ] {
            assert!(!is_about("a", kind, name), "{kind} {name}");
        }
    }

    #[test]
    fn the_reviewers_inputs_are_not_the_agents() {
        let agents = names(&["web"]);
        let w = own("web", &agents);
        for (kind, name) in [
            ("Pod", "web-migrate-x7k2p"),
            ("ReplicaSet", "web-redis"),
            ("Pod", "web-abcde-12345"),
        ] {
            assert_eq!(w.attribute(kind, name, None), None, "{kind} {name}");
        }
        assert_eq!(
            w.attribute("Pod", "web-6b7c8d9f4-x2x4q", None),
            Some(Attribution::Name)
        );
    }

    #[test]
    fn the_store_is_its_own_agents_and_an_agent_named_like_it_contests_it_both_ways() {
        let alone = names(&["a"]);
        assert_eq!(
            own("a", &alone).attribute("Pod", "a-db-1", None),
            Some(Attribution::Name),
            "the store's pod for its own agent"
        );
        assert_eq!(
            own("a", &alone).attribute("Cluster", "a-db", None),
            Some(Attribution::Name)
        );
        let both = names(&["a", "a-db"]);
        // `a-db-1` is the store's pod of `a` and a StatefulSet pod of `a-db`: neither.
        assert_eq!(own("a", &both).attribute("Pod", "a-db-1", None), None);
        assert_eq!(own("a-db", &both).attribute("Pod", "a-db-1", None), None);
        // `a`'s own Cluster `a-db` is under the base of the other agent: neither shows it.
        assert_eq!(own("a", &both).attribute("Cluster", "a-db", None), None);
        assert_eq!(own("a-db", &both).attribute("Cluster", "a-db", None), None);
        // The same for the other bases.
        for other in ["a-front", "a-egress"] {
            let pair = names(&["a", other]);
            assert_eq!(
                own("a", &pair).attribute("Deployment", other, None),
                None,
                "{other}"
            );
            assert_eq!(
                own("a", &pair).attribute("Deployment", "a", None),
                Some(Attribution::Name)
            );
        }
    }

    #[test]
    fn an_object_made_again_does_not_inherit_the_events_of_the_old_one() {
        let agents = names(&["a"]);
        let a = own("a", &agents);
        assert_eq!(
            a.attribute("AgentService", "a", Some("uid-service")),
            Some(Attribution::Uid)
        );
        assert_eq!(
            a.attribute("AgentService", "a", Some("uid-of-the-old-one")),
            None
        );
        assert_eq!(
            a.attribute("AgentConfig", "a", Some("uid-config")),
            Some(Attribution::Uid)
        );
        assert_eq!(a.attribute("AgentConfig", "a", Some("old")), None);
        assert_eq!(
            a.attribute("AgentService", "a", None),
            Some(Attribution::Name)
        );
        assert_eq!(a.attribute("AgentService", "b", Some("uid-service")), None);
    }

    #[test]
    fn a_core_event_is_read_whole() {
        let event = json!({
            "metadata": { "name": "a.1", "creationTimestamp": "2026-11-14T10:00:00Z" },
            "involvedObject": { "kind": "Pod", "name": "a-0", "uid": "u" },
            "type": "Warning", "reason": "BackOff", "message": "Back-off pulling image \"x\"",
            "count": 7,
            "firstTimestamp": "2026-11-14T10:00:01Z", "lastTimestamp": "2026-11-14T10:05:09Z",
        });
        assert_eq!(involved(&event), Some(("Pod", "a-0", Some("u"))));
        let got = read(&event).unwrap();
        assert_eq!(got.event_type, EventType::Warning);
        assert_eq!(got.reason, "BackOff");
        assert_eq!(got.count, 7);
        assert_eq!(got.first_seen, "2026-11-14T10:00:01Z");
        assert_eq!(got.last_seen, "2026-11-14T10:05:09Z");
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
    fn what_is_missing_is_not_made_up_and_a_time_never_panics() {
        assert!(read(&json!({ "involvedObject": { "kind": "Pod" } })).is_none());
        let bare = read(&json!({ "involvedObject": { "kind": "Pod", "name": "p" } })).unwrap();
        assert_eq!(bare.count, 1);
        assert_eq!(bare.last_seen, EPOCH);
        assert_eq!(bare.event_type, EventType::Warning);
        for odd in [
            "yesterday",
            "2026-11-14T10:00:0\u{e9}Z",
            "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
            "2026-11-14T10:00:00+02:00",
        ] {
            let got = read(
                &json!({ "involvedObject": { "kind": "Pod", "name": "p" }, "lastTimestamp": odd }),
            )
            .unwrap();
            assert_eq!(got.last_seen, EPOCH, "{odd}");
        }
    }
}
