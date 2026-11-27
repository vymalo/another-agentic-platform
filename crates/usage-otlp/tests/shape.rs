#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Our JSON is the shape of the example opentelemetry-proto publishes for logs, and AD-069's attributes.

use std::collections::BTreeSet;

use aap_usage::testkit::event;
use aap_usage::{PodKind, PodMinuteEvent};
use aap_usage_otlp::encode;
use aap_usage_otlp::testkit::{check_shape, decode_record, records};
use serde_json::Value;

fn vendored() -> Value {
    serde_json::from_str(include_str!("vendored/logs.json")).unwrap()
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

fn agent_event() -> PodMinuteEvent {
    let mut e = event("meter-1", "pod-2", 7);
    e.pod_kind = PodKind::Agent;
    e.run = None;
    e
}

#[test]
fn the_vendored_example_has_the_top_of_the_shape_the_collector_checks() {
    let v = vendored();
    assert!(v["resourceLogs"][0]["scopeLogs"][0]["logRecords"].is_array());
}

#[test]
fn our_encoding_has_the_shape_of_the_vendored_example() {
    let ours = encode(
        "aap-operator",
        &[event("meter-1", "pod-1", 10), agent_event()],
    );
    check_shape(&ours).unwrap();
    let theirs = vendored();
    // The same keys at each level that we write (we write a subset: no traceId, no severityText, no scope version).
    assert_eq!(keys(&theirs), keys(&ours));
    assert_eq!(
        keys(&theirs["resourceLogs"][0]),
        keys(&ours["resourceLogs"][0])
    );
    let (their_rec, our_rec) = (
        &theirs["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0],
        &ours["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0],
    );
    for k in our_rec.as_object().unwrap().keys() {
        assert!(
            their_rec.get(k).is_some(),
            "we write `{k}`, the example has no such field"
        );
        assert_eq!(
            std::mem::discriminant(&their_rec[k]),
            std::mem::discriminant(&our_rec[k]),
            "{k}"
        );
    }
    // The example's own attribute value kinds that we use: a string and an int-as-decimal-string.
    assert!(their_rec["attributes"][0]["value"]["stringValue"].is_string());
    assert!(their_rec["attributes"][2]["value"]["intValue"].is_string());
}

#[test]
fn the_record_is_ad_069_exactly() {
    let e = event("meter-1", "pod-1", 10);
    let body = encode("svc", std::slice::from_ref(&e));
    let r = &body["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0];
    let nanos = ((10 + 1) * 60 * 1_000_000_000u64).to_string();
    assert_eq!(r["timeUnixNano"], nanos.as_str());
    assert_eq!(r["observedTimeUnixNano"], nanos.as_str());
    assert_eq!(r["severityNumber"], 9);
    assert_eq!(r["body"]["stringValue"], "com.vymalo.agents.pod-minute.v1");
    let names: Vec<&str> = r["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "type",
            "event_id",
            "source",
            "subject",
            "weightQuarters",
            "nodeType",
            "podKind",
            "agentService",
            "run"
        ]
    );
    let by = |k: &str| {
        r["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|a| a["key"] == k)
            .unwrap()["value"]
            .clone()
    };
    assert_eq!(by("type")["stringValue"], "com.vymalo.agents.pod-minute.v1");
    assert_eq!(by("event_id")["stringValue"], e.event_id.as_str());
    assert_eq!(by("event_id")["stringValue"].as_str().unwrap().len(), 64);
    assert_eq!(by("source")["stringValue"], "meter-1");
    assert_eq!(by("subject")["stringValue"], "acct-a");
    assert_eq!(by("weightQuarters")["intValue"], "4");
    assert_eq!(by("nodeType")["stringValue"], "std");
    assert_eq!(by("podKind")["stringValue"], "run");
    assert_eq!(by("agentService")["stringValue"], "coder");
    assert_eq!(by("run")["stringValue"], "run-1");
    // The resource says the service name and nothing else.
    let res = body["resourceLogs"][0]["resource"]["attributes"]
        .as_array()
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0]["key"], "service.name");
    assert_eq!(res[0]["value"]["stringValue"], "svc");
    // And it decodes back to the event.
    assert_eq!(decode_record(records(&body)[0]).unwrap(), e);
}

#[test]
fn an_agent_pod_has_no_run_attribute() {
    let body = encode("svc", &[agent_event()]);
    let r = records(&body)[0];
    assert!(
        r["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["key"] != "run")
    );
    assert_eq!(decode_record(r).unwrap(), agent_event());
}

#[test]
fn the_shape_check_refuses_what_ad_069_does_not_say() {
    let good = encode("svc", &[event("m", "p", 1)]);
    check_shape(&good).unwrap();
    let mutate = |f: fn(&mut Value)| {
        let mut b = good.clone();
        f(&mut b["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]);
        check_shape(&b)
    };
    assert!(mutate(|r| r["severityNumber"] = 10.into()).is_err());
    assert!(mutate(|r| r["timeUnixNano"] = 5.into()).is_err());
    assert!(mutate(|r| r["attributes"][1]["key"] = "eventId".into()).is_err());
    assert!(mutate(|r| r["attributes"][4]["value"] = serde_json::json!({"intValue": 4})).is_err());
    assert!(
        mutate(|r| r["attributes"][4]["value"] = serde_json::json!({"stringValue": "4"})).is_err()
    );
    assert!(
        mutate(|r| r["attributes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"key":"email","value":{"stringValue":"a"}})))
        .is_err()
    );
}
