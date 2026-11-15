//! OTLP/JSON, written by hand (the encoding is `ExportTraceServiceRequest` of opentelemetry-proto, JSON mapping):
//! ids are hex, `*UnixNano` are decimal strings, `kind` and `status.code` are integers, field names are lowerCamelCase,
//! values are `{"stringValue"}`, `{"intValue":"<decimal>"}` or `{"boolValue"}`.

use std::fmt::Write;

use aap_trace::{FinishedSpan, Resource, Value};
use serde_json::{Value as Json, json};

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn string(key: &str, value: &str) -> Json {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn resource_attributes(r: &Resource) -> Vec<Json> {
    let mut out = vec![
        string("service.name", &r.service_name),
        string("service.version", &r.service_version),
        string("telemetry.sdk.name", "aap-trace"),
        string("telemetry.sdk.language", "rust"),
    ];
    if let Some(pod) = &r.pod_name {
        out.push(string("service.instance.id", pod));
        out.push(string("k8s.pod.name", pod));
    }
    if let Some(ns) = &r.namespace {
        out.push(string("k8s.namespace.name", ns));
    }
    for (key, value) in &r.extra {
        out.push(string(key.as_str(), value));
    }
    out
}

fn span(s: &FinishedSpan) -> Json {
    let attributes: Vec<Json> = s
        .attrs
        .iter()
        .map(|a| match a.value() {
            Value::Str(v) => string(a.key(), v),
            Value::Int(n) => json!({ "key": a.key(), "value": { "intValue": n.to_string() } }),
        })
        .collect();
    let mut o = json!({
        "traceId": hex(&s.trace_id),
        "spanId": hex(&s.span_id),
        "name": s.name.as_str(),
        "kind": s.kind.otlp(),
        "startTimeUnixNano": s.start_unix_nanos.to_string(),
        "endTimeUnixNano": s.end_unix_nanos.to_string(),
        "attributes": attributes,
    });
    if let (Some(parent), Some(map)) = (s.parent_id, o.as_object_mut()) {
        map.insert("parentSpanId".into(), Json::String(hex(&parent)));
    }
    if s.error
        && let Some(map) = o.as_object_mut()
    {
        // A status has a code and no message: what went wrong is a class in `error.type`.
        map.insert("status".into(), json!({ "code": 2 }));
    }
    o
}

/// The request body for `batch`. A span whose ids are all zeros (a draw of randomness that failed) is left out, as
/// OTLP calls such an id invalid.
#[must_use]
pub fn encode(resource: &Resource, batch: &[FinishedSpan]) -> Json {
    let spans: Vec<Json> = batch
        .iter()
        .filter(|s| s.trace_id != [0; 16] && s.span_id != [0; 8])
        .map(span)
        .collect();
    json!({
        "resourceSpans": [{
            "resource": { "attributes": resource_attributes(resource) },
            "scopeSpans": [{
                "scope": { "name": resource.service_name, "version": resource.service_version },
                "spans": spans,
            }],
        }],
    })
}

/// The number of spans in an encoded request.
#[must_use]
pub fn span_count(body: &Json) -> usize {
    body["resourceSpans"][0]["scopeSpans"][0]["spans"]
        .as_array()
        .map_or(0, Vec::len)
}
