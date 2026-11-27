//! OTLP/JSON logs, by hand: `ExportLogsServiceRequest` of opentelemetry-proto, JSON mapping. `*UnixNano` are decimal
//! strings, `severityNumber` an integer, `intValue` a decimal string, names lowerCamelCase. The attribute names are
//! AD-069's, exactly.

use aap_usage::{EVENT_TYPE, PodMinuteEvent};
use serde_json::{Value as Json, json};

fn string(key: &str, value: &str) -> Json {
    json!({ "key": key, "value": { "stringValue": value } })
}

fn record(e: &PodMinuteEvent) -> Json {
    let nanos = e.end_unix_nanos().to_string();
    let mut attributes = vec![
        string("type", EVENT_TYPE),
        string("event_id", &e.event_id),
        string("source", &e.source),
        string("subject", &e.subject),
        json!({ "key": "weightQuarters", "value": { "intValue": e.weight_quarters.to_string() } }),
        string("nodeType", &e.node_type),
        string("podKind", e.pod_kind.as_str()),
        string("agentService", &e.agent_service),
    ];
    if let Some(run) = &e.run {
        attributes.push(string("run", run));
    }
    json!({
        "timeUnixNano": nanos,
        "observedTimeUnixNano": nanos,
        "severityNumber": 9,
        "body": { "stringValue": EVENT_TYPE },
        "attributes": attributes,
    })
}

/// The request body for `events`: one resource (`service.name` only, nothing free-form), one scope, one record each.
#[must_use]
pub fn encode(service_name: &str, events: &[PodMinuteEvent]) -> Json {
    json!({
        "resourceLogs": [{
            "resource": { "attributes": [string("service.name", service_name)] },
            "scopeLogs": [{
                "scope": { "name": "aap-usage-otlp" },
                "logRecords": events.iter().map(record).collect::<Vec<_>>(),
            }],
        }],
    })
}

/// [`encode`], as the bytes that are POSTed (and, on a retry, POSTed again unchanged).
#[must_use]
pub fn encode_bytes(service_name: &str, events: &[PodMinuteEvent]) -> Vec<u8> {
    // A `Value` always serialises.
    serde_json::to_vec(&encode(service_name, events)).unwrap_or_default()
}
