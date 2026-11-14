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

fn put_events(fake: &Fake) {
    let list: Value = serde_json::from_str(EVENT_LIST).unwrap();
    for item in list["items"].as_array().unwrap() {
        fake.put_raw(
            "events",
            item["metadata"]["name"].as_str().unwrap(),
            item.clone(),
        );
    }
}

#[tokio::test]
async fn the_events_fixture_is_attributed_to_the_agent_that_owns_each_object() {
    let (fake, h) = setup();
    for name in ["coder", "coder-ab"] {
        h.put_agent(&sample_agent(name), &Precondition::DoesNotExist)
            .await
            .unwrap();
    }
    put_events(&fake);
    fake.clear_calls();

    let coder = h.agent_events("coder").await.unwrap();
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
                "coder-egress-5f6d7c8b9-q1w2e",
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

    let other = h.agent_events("coder-ab").await.unwrap();
    assert_eq!(other.len(), 1);
    assert_eq!(other[0].reason, "FailedScheduling");

    // What it asked of the server: reads of services and Events, and nothing else.
    let calls: Vec<(String, String)> = asked(&fake)
        .iter()
        .map(|c| (c.method.clone(), c.plural().to_owned()))
        .collect();
    assert!(
        calls
            .iter()
            .all(|(m, p)| m == "GET" && matches!(p.as_str(), "agentservices" | "events")),
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
    assert!(h.agent_events("a").await.unwrap().is_empty());
    assert!(h.agent_events("a-front").await.unwrap().is_empty());
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
