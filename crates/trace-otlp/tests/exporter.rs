#![allow(clippy::unwrap_used, clippy::expect_used)]
//! The exporter against the test collector: the conformance suite, the wire, and every way a collector can be wrong.

use std::sync::Arc;
use std::time::Duration;

use aap_trace::testkit::{Harness, assert_clean, sample_spans};
use aap_trace::{
    Controller, ExportError, Limits, Method, Pipeline, Resource, ResourceKey, Sampler,
    SpanExporter, SpanKind, SpanName, Step,
};
use aap_trace_otlp::testkit::{Behaviour, TestCollector};
use aap_trace_otlp::{OtlpJson, TraceSettings};

fn settings(endpoint: &str, extra: &[(&str, &str)]) -> TraceSettings {
    let endpoint = endpoint.to_owned();
    let extra: Vec<(String, String)> = extra
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    TraceSettings::from_env("test", move |k| {
        if k == "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT" {
            return Some(endpoint.clone());
        }
        extra.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone())
    })
    .unwrap()
    .unwrap()
}

/// Settings for a plain-http test collector with a headers file: `from_env` refuses that pair (headers are never sent in
/// clear), so the file is set on the value, which is what the exporter reads.
fn settings_with_headers_file(endpoint: &str, file: &str) -> TraceSettings {
    let mut s = settings(endpoint, &[]);
    s.headers_file = Some(file.into());
    s
}

fn resource() -> Resource {
    let mut r = Resource::new("aap-test", "1.2.3");
    r.pod_name = Some("pod-0".into());
    r.namespace = Some("ns".into());
    r.extra
        .push((ResourceKey::K8sClusterName, "kind-aap".into()));
    r
}

async fn make() -> Harness<OtlpJson> {
    let collector = Arc::new(TestCollector::start().await);
    let exporter = OtlpJson::new(&settings(&collector.endpoint(), &[])).unwrap();
    let (reader, failer) = (collector.clone(), collector);
    Harness {
        exporter,
        delivered: Box::new(move || reader.span_ids()),
        fail: Box::new(move || failer.set_behaviour(Behaviour::Status(503))),
    }
}

aap_trace::exporter_conformance!(make);

#[tokio::test]
async fn the_wire_carries_the_resource_and_only_listed_keys() {
    let collector = TestCollector::start().await;
    let exporter = OtlpJson::new(&settings(&collector.endpoint(), &[])).unwrap();
    exporter.export(&resource(), sample_spans(2)).await.unwrap();
    assert_eq!(collector.service_names(), ["aap-test"]);
    let body = &collector.requests()[0];
    let attrs: Vec<(&str, &str)> = body["resourceSpans"][0]["resource"]["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["key"].as_str().unwrap(),
                a["value"]["stringValue"].as_str().unwrap(),
            )
        })
        .collect();
    for want in [
        ("service.name", "aap-test"),
        ("service.version", "1.2.3"),
        ("service.instance.id", "pod-0"),
        ("k8s.pod.name", "pod-0"),
        ("k8s.namespace.name", "ns"),
        ("k8s.cluster.name", "kind-aap"),
        ("telemetry.sdk.name", "aap-trace"),
        ("telemetry.sdk.language", "rust"),
    ] {
        assert!(attrs.contains(&want), "{want:?} in {attrs:?}");
    }
    for span in collector.spans() {
        for a in span["attributes"].as_array().unwrap() {
            assert!(aap_trace::Attr::ALL_KEYS.contains(&a["key"].as_str().unwrap()));
        }
    }
}

#[tokio::test]
async fn spans_made_through_the_pipeline_reach_the_collector() {
    let collector = TestCollector::start().await;
    let exporter = OtlpJson::new(&settings(&collector.endpoint(), &[])).unwrap();
    let tracer = Pipeline::start(exporter, Limits::default(), resource(), Sampler::AlwaysOn);
    let root = tracer.root(
        SpanName::reconcile(Controller::AgentService),
        SpanKind::Internal,
    );
    root.child(SpanName::step(Step::StoreEnsure), SpanKind::Internal)
        .end();
    root.fail();
    root.end();
    let stats = tracer.shutdown(Duration::from_secs(5)).await;
    assert_eq!(stats.exported, 2);
    let spans = collector.spans();
    assert_eq!(spans.len(), 2);
    let top = spans
        .iter()
        .find(|s| s["name"] == "reconcile agentservice")
        .unwrap();
    assert_eq!(top["status"]["code"], 2);
    assert_eq!(top["kind"], 1);
    assert!(top.get("parentSpanId").is_none());
    let child = spans.iter().find(|s| s["name"] == "store.ensure").unwrap();
    assert_eq!(child["parentSpanId"], top["spanId"]);
    assert_eq!(child["traceId"], top["traceId"]);
    assert!(child.get("status").is_none());
    assert_clean(&sample_spans(1), &[]);
}

#[tokio::test]
async fn a_collector_that_is_down_is_an_err_within_the_timeout() {
    // Refused: bind a port, close it.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/v1/traces", l.local_addr().unwrap())
    };
    let exporter =
        OtlpJson::new(&settings(&closed, &[("OTEL_EXPORTER_OTLP_TIMEOUT", "500")])).unwrap();
    assert_eq!(
        exporter.export(&resource(), sample_spans(1)).await,
        Err(ExportError::Failed)
    );

    let collector = TestCollector::start().await;
    let exporter = OtlpJson::new(&settings(
        &collector.endpoint(),
        &[("OTEL_EXPORTER_OTLP_TIMEOUT", "300")],
    ))
    .unwrap();
    collector.set_behaviour(Behaviour::Hang);
    let started = std::time::Instant::now();
    assert_eq!(
        exporter.export(&resource(), sample_spans(1)).await,
        Err(ExportError::Failed)
    );
    assert!(started.elapsed() < Duration::from_secs(5));

    collector.set_behaviour(Behaviour::Status(503));
    assert_eq!(
        exporter.export(&resource(), sample_spans(1)).await,
        Err(ExportError::Failed)
    );
    collector.set_behaviour(Behaviour::Status(404));
    assert_eq!(
        exporter.export(&resource(), sample_spans(1)).await,
        Err(ExportError::Failed)
    );
}

#[tokio::test]
async fn a_failing_collector_makes_the_pipeline_count_drops() {
    let collector = TestCollector::start().await;
    collector.set_behaviour(Behaviour::Status(503));
    let exporter = OtlpJson::new(&settings(&collector.endpoint(), &[])).unwrap();
    let tracer = Pipeline::start(exporter, Limits::default(), resource(), Sampler::AlwaysOn);
    for _ in 0..4 {
        tracer
            .root(SpanName::method(Method::Get), SpanKind::Internal)
            .end();
    }
    let stats = tracer.shutdown(Duration::from_secs(5)).await;
    assert_eq!(stats.exported, 0);
    assert_eq!(stats.dropped_export_failed, 4);
}

#[tokio::test]
async fn a_partial_success_counts_the_rejected_spans() {
    let collector = TestCollector::start().await;
    collector.set_behaviour(Behaviour::Reject(2));
    let exporter = OtlpJson::new(&settings(&collector.endpoint(), &[])).unwrap();
    let tracer = Pipeline::start(exporter, Limits::default(), resource(), Sampler::AlwaysOn);
    for _ in 0..5 {
        tracer
            .root(SpanName::method(Method::Get), SpanKind::Internal)
            .end();
    }
    let stats = tracer.shutdown(Duration::from_secs(5)).await;
    assert_eq!(stats.dropped_rejected, 2);
    assert_eq!(stats.exported, 3);
}

#[tokio::test]
async fn a_redirect_is_a_failure_and_is_not_followed() {
    let collector = TestCollector::start().await;
    collector.set_behaviour(Behaviour::Redirect);
    let exporter = OtlpJson::new(&settings(&collector.endpoint(), &[])).unwrap();
    assert_eq!(
        exporter.export(&resource(), sample_spans(1)).await,
        Err(ExportError::Failed)
    );
    assert_eq!(collector.elsewhere_hits(), 0);
}

#[tokio::test]
async fn headers_come_from_the_file_and_the_exporter_owns_its_own() {
    let dir = tempdir("headers");
    let file = dir.join("headers");
    std::fs::write(&file, "authorization=Bearer file-token-1,x-team=blue\n").unwrap();
    let collector = TestCollector::start().await;
    let exporter = OtlpJson::new(&settings_with_headers_file(
        &collector.endpoint(),
        file.to_str().unwrap(),
    ))
    .unwrap();
    exporter.export(&resource(), sample_spans(1)).await.unwrap();
    let seen = &collector.request_headers()[0];
    assert_eq!(seen["authorization"], "Bearer file-token-1");
    assert_eq!(seen["x-team"], "blue");
    assert_eq!(seen["content-type"], "application/json");
    assert!(seen.get("traceparent").is_none());
}

#[test]
fn a_bad_headers_file_stops_the_start_and_names_only_the_variable() {
    let dir = tempdir("badheaders");
    let big = dir.join("big");
    std::fs::write(&big, format!("a={}", "b".repeat(5000))).unwrap();
    let forbidden = dir.join("forbidden");
    std::fs::write(&forbidden, "host=SECRET-HOST").unwrap();
    for (path, what) in [
        (big, "4 KiB"),
        (forbidden, "sets itself"),
        (dir.join("missing"), "cannot be read"),
    ] {
        let e = OtlpJson::new(&settings_with_headers_file(
            "http://127.0.0.1:1/v1/traces",
            path.to_str().unwrap(),
        ))
        .unwrap_err();
        assert_eq!(e.variable, "AAP_OTEL_HEADERS_FILE");
        assert!(e.to_string().contains(what), "{e}");
        assert!(!e.to_string().contains("SECRET"));
    }
}

#[test]
fn the_certificate_must_be_a_pem_bundle() {
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ca.pem");
    OtlpJson::new(&settings(
        "https://collector.example:4318/v1/traces",
        &[("OTEL_EXPORTER_OTLP_CERTIFICATE", fixture)],
    ))
    .unwrap();
    let dir = tempdir("cert");
    let junk = dir.join("junk");
    std::fs::write(&junk, "not a certificate").unwrap();
    for path in [junk, dir.join("missing")] {
        let e = OtlpJson::new(&settings(
            "https://collector.example:4318/v1/traces",
            &[("OTEL_EXPORTER_OTLP_CERTIFICATE", path.to_str().unwrap())],
        ))
        .unwrap_err();
        assert_eq!(e.variable, "OTEL_EXPORTER_OTLP_CERTIFICATE");
    }
}

fn tempdir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aap-trace-otlp-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
