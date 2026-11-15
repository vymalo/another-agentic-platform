//! The spans of a request (K5): one per request, named by the route's template, with the ports as children, and nothing of
//! what the request or the answer held. Every route is asked with sentinels in the body, a header, the query string and the
//! token, and `assert_clean` reads what the tracer kept.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::time::Duration;

use aap_platform_api::ROUTES;
use aap_trace::testkit::{Memory, assert_clean};
use aap_trace::{Attr, FinishedSpan, Limits, Pipeline, Resource, Sampler, Tracer};
use aap_trace_otlp::testkit::{Behaviour, TestCollector};
use aap_trace_otlp::{OtlpJson, TraceSettings};
use axum::http::Method;
use common::*;
use serde_json::json;

const SENTINELS: [&str; 5] = [
    "SENTINEL-BODY",
    "SENTINEL-HEADER",
    "SENTINEL-QUERY",
    "SENTINEL-PATH",
    TOKEN_SENTINEL,
];

fn traced() -> (Api, Memory) {
    let (tracer, memory) = Tracer::in_memory(Sampler::AlwaysOn);
    (Api::traced(tracer), memory)
}

fn root<'a>(spans: &'a [FinishedSpan], name: &str) -> &'a FinishedSpan {
    spans
        .iter()
        .find(|s| s.parent_id.is_none() && s.name.as_str() == name)
        .unwrap_or_else(|| panic!("no root span {name} in {:?}", names(spans)))
}

fn names(spans: &[FinishedSpan]) -> Vec<&str> {
    spans.iter().map(|s| s.name.as_str()).collect()
}

fn has(span: &FinishedSpan, attr: &Attr) -> bool {
    span.attrs.contains(attr)
}

#[tokio::test]
async fn every_route_is_a_clean_span_named_by_its_template() {
    let (api, memory) = traced();
    let token = api.token(TOKEN_SENTINEL, &every_permission());
    for route in &ROUTES {
        let method = Method::from_bytes(route.verb.as_str().as_bytes()).unwrap();
        let path = route.path.replace("{name}", "x");
        let call = api
            .request(method, &format!("{path}?q=SENTINEL-QUERY"))
            .token(&token)
            .header("x-secret", "SENTINEL-HEADER")
            .json(&json!({ "name": "SENTINEL-BODY", "secret": "SENTINEL-BODY" }));
        let _ = call.send().await;
    }
    let spans = memory.spans();
    for route in &ROUTES {
        let name = format!("{} {}", route.verb.as_str(), route.path);
        let span = root(&spans, &name);
        assert!(has(span, &Attr::HttpRoute(route.path)), "{name}");
        assert!(
            span.attrs
                .iter()
                .any(|a| a.key() == "http.response.status_code"),
            "{name}"
        );
        assert_eq!(span.kind, aap_trace::SpanKind::Server);
    }
    assert_clean(&spans, &SENTINELS);
}

#[tokio::test]
async fn a_request_that_matched_nothing_is_named_by_its_method_and_holds_no_path() {
    let (api, memory) = traced();
    let token = api.admin();
    let r = api
        .get("/v1/SENTINEL-PATH/secret?q=SENTINEL-QUERY")
        .token(&token)
        .send()
        .await;
    assert_eq!(r.code(), 404);
    let spans = memory.spans();
    assert_eq!(names(&spans), ["GET"]);
    assert!(!spans[0].attrs.iter().any(|a| a.key() == "http.route"));
    assert!(has(&spans[0], &Attr::HttpStatus(404)));
    assert_clean(&spans, &SENTINELS);
}

#[tokio::test]
async fn a_method_outside_the_set_is_named_without_one() {
    let (api, memory) = traced();
    let r = api
        .request(Method::from_bytes(b"BREW").unwrap(), "/v1/SENTINEL-PATH")
        .send()
        .await;
    assert_eq!(r.code(), 404);
    assert_eq!(names(&memory.spans()), ["HTTP"]);
}

#[tokio::test]
async fn a_403_names_the_missing_permission_and_the_problem_slug() {
    let (api, memory) = traced();
    let token = api.token("reader", &[aap_api::permission::AGENTS_READ]);
    let r = api
        .put("/v1/models/x")
        .token(&token)
        .json(&model_body("x"))
        .create()
        .send()
        .await;
    assert_eq!(r.code(), 403);
    let spans = memory.spans();
    let span = root(&spans, "PUT /v1/models/{name}");
    assert!(has(
        span,
        &Attr::PermissionMissing(aap_api::permission::MODELS_WRITE)
    ));
    assert!(has(span, &Attr::ProblemType("forbidden")));
    assert!(has(span, &Attr::HttpStatus(403)));
    assert!(
        !span.error,
        "a 403 is an answer, not a failure of the server"
    );
    assert!(has(span, &Attr::AuthResult("ok")));
}

#[tokio::test]
async fn a_request_without_a_token_says_why_and_never_which_token() {
    let (api, memory) = traced();
    let r = api.get("/v1/agents").send().await;
    assert_eq!(r.code(), 401);
    let r = api.get("/v1/agents").token("SENTINEL-BODY").send().await;
    assert_eq!(r.code(), 401);
    let spans = memory.spans();
    let roots: Vec<_> = spans.iter().filter(|s| s.parent_id.is_none()).collect();
    assert!(has(roots[0], &Attr::AuthResult("missing")));
    assert!(has(roots[0], &Attr::ProblemType("missing-token")));
    assert!(has(roots[1], &Attr::ProblemType("invalid-token")));
    assert_clean(&spans, &SENTINELS);
}

#[tokio::test]
async fn the_ports_are_children_of_the_request() {
    let (api, memory) = traced();
    let token = api.admin();
    assert_eq!(api.get("/v1/agents").token(&token).send().await.code(), 200);
    let spans = memory.spans();
    let top = root(&spans, "GET /v1/agents");
    let children: Vec<&str> = spans
        .iter()
        .filter(|s| s.parent_id == Some(top.span_id))
        .map(|s| s.name.as_str())
        .collect();
    assert!(children.contains(&"authenticate"), "{children:?}");
    assert!(children.contains(&"objects.list_agents"), "{children:?}");
    assert!(spans.iter().all(|s| s.trace_id == top.trace_id));
}

#[tokio::test]
async fn a_refusal_of_the_store_is_a_slug_and_not_a_failure() {
    let (api, memory) = traced();
    let token = api.admin();
    let r = api.get("/v1/agents/missing").token(&token).send().await;
    assert_eq!(r.code(), 404);
    let spans = memory.spans();
    assert!(spans.iter().all(|s| !s.error));
    assert_clean(&spans, &["missing"]);
}

#[tokio::test]
async fn no_traceparent_is_read_and_every_request_is_a_root() {
    let (api, memory) = traced();
    let token = api.admin();
    let _ = api
        .get("/v1/me")
        .token(&token)
        .header(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .header("tracestate", "SENTINEL-HEADER=1")
        .send()
        .await;
    let spans = memory.spans();
    let top = root(&spans, "GET /v1/me");
    assert_ne!(
        top.trace_id
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>(),
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
    assert!(top.parent_id.is_none());
}

#[tokio::test]
async fn off_makes_no_span_and_changes_no_answer() {
    let off = Api::new();
    let (on, memory) = traced();
    for api in [&off, &on] {
        let token = api.admin();
        let r = api.get("/v1/agents").token(&token).send().await;
        assert_eq!(r.code(), 200);
        assert_eq!(r.json(), json!({ "agents": [] }));
    }
    assert!(!memory.spans().is_empty());
    assert!(!Tracer::off().enabled());
}

#[tokio::test]
async fn a_collector_that_is_down_changes_no_answer_and_costs_no_time() {
    let collector = TestCollector::start().await;
    collector.set_behaviour(Behaviour::Hang);
    let settings = TraceSettings::from_env("aap-api", |k| match k {
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT" => Some(collector.endpoint()),
        "OTEL_EXPORTER_OTLP_TIMEOUT" => Some("200".into()),
        _ => None,
    })
    .unwrap()
    .unwrap();
    let tracer = Pipeline::start(
        OtlpJson::new(&settings).unwrap(),
        Limits {
            batch: 1,
            ..Limits::default()
        },
        Resource::new("aap-api", "0"),
        Sampler::AlwaysOn,
    );
    let api = Api::traced(tracer.clone());
    let token = api.admin();
    let started = std::time::Instant::now();
    for _ in 0..50 {
        let r = api.get("/v1/me").token(&token).send().await;
        assert_eq!(r.code(), 200);
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    tracer.shutdown(Duration::from_millis(300)).await;
}

#[tokio::test]
async fn the_spans_reach_a_collector_with_the_apis_service_name() {
    let collector = TestCollector::start().await;
    let settings = TraceSettings::from_env("aap-api", |k| {
        (k == "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").then(|| collector.endpoint())
    })
    .unwrap()
    .unwrap();
    let tracer = Pipeline::start(
        OtlpJson::new(&settings).unwrap(),
        Limits::default(),
        Resource::new(settings.service_name.clone(), "0.1.0"),
        Sampler::AlwaysOn,
    );
    let api = Api::traced(tracer.clone());
    let token = api.admin();
    assert_eq!(api.get("/v1/me").token(&token).send().await.code(), 200);
    tracer.shutdown(Duration::from_secs(5)).await;
    assert_eq!(collector.service_names(), ["aap-api"]);
    assert!(collector.spans().iter().any(|s| s["name"] == "GET /v1/me"));
}
