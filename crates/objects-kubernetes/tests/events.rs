//! The events of an agent (K8, §60a *Events*) against the fake API server, on the hand-written `core/v1` fixture
//! (`tests/fixtures/events`, *unverified* against a real API server). The suite of `aap-ports` runs the cases that every
//! implementation passes (`tests/api.rs`); this is what is the Kubernetes backend's own: which object is which agent's.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use aap_ports::testkit::sample_agent;
use aap_ports::{AgentObjects, EventType, ObjectsError, Precondition};
use serde_json::{Value, json};
use support::harness::Harness;
use support::{Call, Fake};

const EVENT_LIST: &str = include_str!("fixtures/events/event-list.json");

fn setup() -> (Fake, Harness) {
    let fake = Fake::new();
    let h = Harness::on_fake(&fake);
    (fake, h)
}

fn asked(fake: &Fake) -> Vec<Call> {
    fake.calls()
        .into_iter()
        .filter(|c| c.who == "dashboard")
        .collect()
}

/// The fixture's events, with the `AgentService` event of `coder` made to name that agent's real uid (the fake makes its own).
fn put_events(fake: &Fake, service_uid: &str) {
    let list: Value = serde_json::from_str(EVENT_LIST).unwrap();
    for item in list["items"].as_array().unwrap() {
        let mut item = item.clone();
        if item["involvedObject"]["kind"] == "AgentService" {
            item["involvedObject"]["uid"] = json!(service_uid);
        }
        let name = item["metadata"]["name"].as_str().unwrap().to_owned();
        fake.put_raw("events", &name, item);
    }
}

fn event(name: &str, kind: &str, object: &str, uid: Option<&str>) -> Value {
    json!({
        "metadata": { "name": name, "namespace": "aap-test" },
        "involvedObject": { "kind": kind, "name": object, "uid": uid },
        "type": "Warning", "reason": "R", "message": "m", "count": 1,
        "lastTimestamp": "2026-11-14T10:00:00Z",
    })
}

#[tokio::test]
async fn the_events_fixture_is_attributed_to_the_agent_that_owns_each_object() {
    let (fake, h) = setup();
    for name in ["coder", "coder-ab"] {
        h.put_agent(&sample_agent(name), &Precondition::DoesNotExist)
            .await
            .unwrap();
    }
    let service_uid = h.get_agent("coder").await.unwrap().unwrap().service.uid;
    put_events(&fake, &service_uid);
    // An event of an agent that was deleted and made again under the name: it names the old uid.
    fake.put_raw(
        "events",
        "old-coder",
        event(
            "old-coder",
            "AgentService",
            "coder",
            Some("uid-of-the-old-coder"),
        ),
    );
    fake.clear_calls();

    let coder = h.agent_events("coder").await.unwrap();
    assert!(!coder.incomplete);
    let coder = coder.events;
    let seen: Vec<(&str, &str, &str)> = coder
        .iter()
        .map(|e| {
            (
                e.object.kind.as_str(),
                e.object.name.as_str(),
                e.last_seen.as_str(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        vec![
            (
                "Pod",
                "coder-egress-5f6d7c8b9-q2w4z",
                "2026-11-14T10:09:30Z"
            ),
            ("Pod", "coder-7d9f8b6c5-x2x4q", "2026-11-14T10:07:41Z"),
            ("Pod", "coder-7d9f8b6c5-x2x4q", "2026-11-14T10:07:40Z"),
            ("ReplicaSet", "coder-7d9f8b6c5", "2026-11-14T10:00:03Z"),
            ("Deployment", "coder", "2026-11-14T10:00:02Z"),
            ("AgentService", "coder", "2026-11-14T10:00:01Z"),
        ],
        "the events of `coder`, newest first, and not the other agent's pod, nor a Node's"
    );
    let failed = coder.iter().find(|e| e.reason == "Failed").unwrap();
    assert_eq!(failed.event_type, EventType::Warning);
    assert_eq!(failed.by, aap_ports::Attribution::Name);
    let service = coder.iter().find(|e| e.reason == "ConfigInvalid").unwrap();
    assert_eq!(
        service.by,
        aap_ports::Attribution::Uid,
        "the agent's own object, by uid"
    );
    assert_eq!(failed.count, 12);
    assert_eq!(failed.first_seen, "2026-11-14T10:00:04Z");
    assert!(
        failed.message.chars().all(|c| !c.is_control()),
        "{:?}",
        failed.message
    );
    let pulled = &coder[0];
    assert_eq!(pulled.count, 2, "a series' count");
    assert_eq!(
        pulled.first_seen, "2026-11-14T10:09:00Z",
        "eventTime, to the second"
    );

    let other = h.agent_events("coder-ab").await.unwrap().events;
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].reason, "FailedScheduling");

    // What it asked of the server: reads of services and Events, and nothing else.
    let calls: Vec<(String, String)> = asked(&fake)
        .iter()
        .map(|c| (c.method.clone(), c.plural().to_owned()))
        .collect();
    assert!(
        calls.iter().all(|(m, p)| m == "GET"
            && matches!(p.as_str(), "agentservices" | "agentconfigs" | "events")),
        "{calls:?}"
    );
    assert!(calls.iter().any(|(_, p)| p == "events"));
}

#[tokio::test]
async fn an_ambiguous_event_is_left_out_and_an_unknown_agent_is_not_found() {
    let (fake, h) = setup();
    // `a-front` is the front workload of `a` and the one workload of `a-front`.
    for name in ["a", "a-front"] {
        h.put_agent(&sample_agent(name), &Precondition::DoesNotExist)
            .await
            .unwrap();
    }
    fake.put_raw(
        "events",
        "ambiguous",
        json!({
            "metadata": { "name": "ambiguous", "namespace": "aap-test" },
            "involvedObject": { "kind": "Deployment", "name": "a-front" },
            "type": "Warning", "reason": "R", "message": "m", "count": 1,
            "lastTimestamp": "2026-11-14T10:00:00Z",
        }),
    );
    assert!(h.agent_events("a").await.unwrap().events.is_empty());
    assert!(h.agent_events("a-front").await.unwrap().events.is_empty());
    let error = h.agent_events("nobody").await.unwrap_err();
    assert!(matches!(error, ObjectsError::NotFound { .. }), "{error:?}");
}

#[tokio::test]
async fn a_server_that_refuses_the_events_is_unavailable() {
    let (fake, h) = setup();
    h.put_agent(&sample_agent("x"), &Precondition::DoesNotExist)
        .await
        .unwrap();
    fake.fail_with(Some(403));
    let error = h.agent_events("x").await.unwrap_err();
    assert!(matches!(error, ObjectsError::Unavailable(_)), "{error:?}");
}

fn busy(fake: &Fake, noise: usize, wanted_on: &str) {
    for i in 0..noise {
        let name = format!("noise-{i:05}");
        fake.put_raw("events", &name, event(&name, "Pod", "somebody-else", None));
    }
    // Sorts after every `noise-`: on the last page.
    fake.put_raw(
        "events",
        "zz-wanted",
        event("zz-wanted", "Deployment", wanted_on, None),
    );
}

fn event_lists(fake: &Fake) -> Vec<support::Call> {
    asked(fake)
        .into_iter()
        .filter(|c| c.plural() == "events" && c.method == "GET")
        .collect()
}

#[tokio::test]
async fn a_busy_namespace_is_read_page_by_page_and_the_event_on_the_second_page_is_found() {
    let (fake, h) = setup();
    h.put_agent(&sample_agent("x"), &Precondition::DoesNotExist)
        .await
        .unwrap();
    busy(&fake, 600, "x");
    fake.clear_calls();
    let got = h.agent_events("x").await.unwrap();
    assert_eq!(got.events.len(), 1, "the event that is on page 2");
    assert!(!got.incomplete);
    let lists = event_lists(&fake);
    assert_eq!(lists.len(), 2, "two pages of 500");
    assert!(
        lists
            .iter()
            .all(|c| c.query.get("limit").map(String::as_str) == Some("500"))
    );
    assert!(!lists[0].query.contains_key("continue"));
    assert!(lists[1].query.contains_key("continue"));
}

#[tokio::test]
async fn past_the_bound_the_answer_says_it_is_incomplete() {
    let (fake, h) = setup();
    h.put_agent(&sample_agent("x"), &Precondition::DoesNotExist)
        .await
        .unwrap();
    // 10 pages of 500 are read; the wanted event is on the eleventh.
    busy(&fake, 5200, "x");
    fake.clear_calls();
    let got = h.agent_events("x").await.unwrap();
    assert!(got.incomplete, "the bound was reached with events left");
    assert!(
        got.events.is_empty(),
        "the event of the eleventh page was not read"
    );
    assert_eq!(event_lists(&fake).len(), 10);
}

#[tokio::test]
async fn an_expired_continue_token_restarts_the_list_once_then_says_incomplete() {
    let (fake, h) = setup();
    h.put_agent(&sample_agent("x"), &Precondition::DoesNotExist)
        .await
        .unwrap();
    busy(&fake, 600, "x");
    fake.expire_continue_tokens(1);
    let got = h.agent_events("x").await.unwrap();
    assert_eq!(got.events.len(), 1, "the restart read everything");
    assert!(!got.incomplete);
    fake.expire_continue_tokens(2);
    let got = h.agent_events("x").await.unwrap();
    assert!(
        got.incomplete,
        "a second expiry answers what was read, and says so"
    );
    assert!(got.events.is_empty());
}

#[tokio::test]
async fn events_of_unrelated_objects_that_look_like_an_agents_are_not_shown() {
    let (fake, h) = setup();
    h.put_agent(&sample_agent("web"), &Precondition::DoesNotExist)
        .await
        .unwrap();
    for (i, (kind, name)) in [
        ("Pod", "web-migrate-x7k2p"),
        ("ReplicaSet", "web-redis"),
        ("Pod", "web-abcde-12345"),
        ("Pod", "web-6b7c8d9f4-x2x4q"),
    ]
    .into_iter()
    .enumerate()
    {
        let n = format!("e{i}");
        fake.put_raw("events", &n, event(&n, kind, name, None));
    }
    let got = h.agent_events("web").await.unwrap().events;
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].object.name, "web-6b7c8d9f4-x2x4q");
}

#[tokio::test]
async fn a_store_pod_is_its_own_agents_and_an_agent_called_a_db_contests_it() {
    let (fake, h) = setup();
    h.put_agent(&sample_agent("a"), &Precondition::DoesNotExist)
        .await
        .unwrap();
    fake.put_raw("events", "p", event("p", "Pod", "a-db-1", None));
    fake.put_raw("events", "c", event("c", "Cluster", "a-db", None));
    assert_eq!(
        h.agent_events("a").await.unwrap().events.len(),
        2,
        "the store's events are a's own"
    );
    h.put_agent(&sample_agent("a-db"), &Precondition::DoesNotExist)
        .await
        .unwrap();
    assert!(h.agent_events("a").await.unwrap().events.is_empty());
    assert!(h.agent_events("a-db").await.unwrap().events.is_empty());
}
