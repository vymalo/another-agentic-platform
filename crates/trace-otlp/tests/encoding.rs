#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Our JSON is the shape of the example opentelemetry-proto publishes.

use std::collections::BTreeSet;

use aap_trace::testkit::sample_spans;
use aap_trace::{Attr, Method, Resource};
use aap_trace_otlp::encode;
use aap_trace_otlp::testkit::check_shape;
use serde_json::Value;

fn vendored() -> Value {
    serde_json::from_str(include_str!("vendored/trace.json")).unwrap()
}

fn keys(v: &Value) -> BTreeSet<String> {
    v.as_object().unwrap().keys().cloned().collect()
}

#[test]
fn the_vendored_example_passes_the_check_that_the_collector_applies() {
    check_shape(&vendored()).unwrap();
}

#[test]
fn our_encoding_has_the_shape_of_the_vendored_example() {
    let mut spans = sample_spans(2);
    spans[1].parent_id = Some([9; 8]);
    spans[1].attrs.push(Attr::HttpStatus(404));
    spans[0].attrs.push(Attr::HttpMethod(Method::Put));
    let ours = encode(&Resource::new("aap-api", "0.1.0"), &spans);
    check_shape(&ours).unwrap();

    let (theirs, ours_span) = (
        &vendored()["resourceSpans"][0]["scopeSpans"][0]["spans"][0].clone(),
        &ours["resourceSpans"][0]["scopeSpans"][0]["spans"][1],
    );
    // Every key of their span is a key of ours, with the same JSON type.
    for (k, v) in theirs.as_object().unwrap() {
        let mine = &ours_span[k];
        assert_eq!(
            std::mem::discriminant(v),
            std::mem::discriminant(mine),
            "{k}"
        );
    }
    // The shape of the top: same keys at each level.
    let t = vendored();
    assert_eq!(keys(&t), keys(&ours));
    assert_eq!(
        keys(&t["resourceSpans"][0]),
        keys(&ours["resourceSpans"][0])
    );
    // Hex ids, nanoseconds as strings, integer kind, intValue as a decimal string.
    assert_eq!(ours_span["traceId"].as_str().unwrap().len(), 32);
    assert!(ours_span["startTimeUnixNano"].is_string());
    assert!(ours_span["kind"].is_u64());
    let status = ours_span["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["key"] == "http.response.status_code")
        .unwrap();
    assert_eq!(status["value"]["intValue"], "404");
    // No events and no links, ever.
    for forbidden in ["events", "links", "droppedAttributesCount"] {
        assert!(ours_span.get(forbidden).is_none());
    }
}

#[test]
fn a_span_with_zero_ids_is_left_out() {
    let mut spans = sample_spans(2);
    spans[0].span_id = [0; 8];
    let body = encode(&Resource::new("x", "0"), &spans);
    assert_eq!(aap_trace_otlp::span_count(&body), 1);
}

#[test]
fn the_shape_check_refuses_what_the_spec_does_not_say() {
    let ok = encode(&Resource::new("x", "0"), &sample_spans(1));
    for mutate in [
        |v: &mut Value| {
            v["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["traceId"] = "AQID".into()
        },
        |v: &mut Value| {
            v["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["startTimeUnixNano"] = 5.into()
        },
        |v: &mut Value| {
            v["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["kind"] = "SPAN_KIND_SERVER".into()
        },
        |v: &mut Value| {
            v["resourceSpans"][0]["scopeSpans"][0]["spans"][0]["events"] =
                serde_json::json!([{"name":"e"}])
        },
    ] {
        let mut v = ok.clone();
        mutate(&mut v);
        assert!(check_shape(&v).is_err());
    }
}
