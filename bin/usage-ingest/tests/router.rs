#![allow(clippy::unwrap_used)]
use std::sync::Arc;

use aap_usage::MemoryUsageStore;
use aap_usage_ingest::{AuthFailure, Caller, CallerVerifier, IngestState, router};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

struct Tokens;
impl CallerVerifier for Tokens {
    async fn verify(&self, token: &str) -> Result<Caller, AuthFailure> {
        match token {
            "meter" => Ok(Caller::Meter),
            "ledger" => Ok(Caller::Ledger),
            "down" => Err(AuthFailure::Unavailable),
            _ => Err(AuthFailure::Refused),
        }
    }
}

const NOW: u64 = 10_000;
fn app() -> axum::Router {
    router(Arc::new(IngestState {
        store: MemoryUsageStore::new(),
        verifier: Tokens,
        now: Box::new(|| NOW),
        lag_secs: 300,
    }))
}

fn event(id: &str) -> Value {
    json!({"specversion":"1.0","type":"com.vymalo.agents.pod-minute.v1","source":"meter/0","id":id,
      "subject":"acct-1","time":"2026-11-25T10:31:00Z",
      "data":{"weight":2,"nodeType":"std","podKind":"run","agentService":"coder","run":"run-1"}})
}

async fn call(
    app: &axum::Router,
    path: &str,
    token: Option<&str>,
    ct: &str,
    body: Value,
) -> (StatusCode, Value) {
    let mut req = Request::post(path).header("content-type", ct);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

const CE: &str = "application/cloudevents+json";
const BATCH: &str = "application/cloudevents-batch+json";

#[tokio::test]
async fn accepted_then_duplicate_counts_once() {
    let app = app();
    assert_eq!(
        call(&app, "/v1/events", Some("meter"), CE, event("a"))
            .await
            .0,
        StatusCode::CREATED
    );
    assert_eq!(
        call(&app, "/v1/events", Some("meter"), CE, event("a"))
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn spend_matches_stored_events_and_respects_lag() {
    let store = MemoryUsageStore::new();
    let clock = Arc::new(std::sync::atomic::AtomicU64::new(1_000));
    let c = clock.clone();
    let app = router(Arc::new(IngestState {
        store,
        verifier: Tokens,
        now: Box::new(move || c.load(std::sync::atomic::Ordering::SeqCst)),
        lag_secs: 300,
    }));
    for id in ["a", "b", "a"] {
        call(&app, "/v1/events", Some("meter"), CE, event(id)).await;
    }
    clock.store(5_000, std::sync::atomic::Ordering::SeqCst);
    let (s, body) = call(
        &app,
        "/v1/spend",
        Some("ledger"),
        "application/json",
        json!({"from":0,"to":2000}),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        body,
        json!({"lines":[{"account":"acct-1","nodeType":"std","weightedMinutes":4}]})
    );
    // too recent
    let (s, _) = call(
        &app,
        "/v1/spend",
        Some("ledger"),
        "application/json",
        json!({"from":0,"to":4900}),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = call(
        &app,
        "/v1/spend",
        Some("ledger"),
        "application/json",
        json!({"from":5,"to":5}),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn identities_are_separate() {
    let app = app();
    assert_eq!(
        call(&app, "/v1/events", None, CE, event("a")).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "/v1/events", Some("stranger"), CE, event("a"))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "/v1/events", Some("ledger"), CE, event("a"))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "/v1/events", Some("down"), CE, event("a"))
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    let q = json!({"from":0,"to":1});
    assert_eq!(
        call(&app, "/v1/spend", Some("meter"), "application/json", q)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn malformed_events_are_refused_without_their_value() {
    let app = app();
    let mut bad = event("a");
    bad["subject"] = json!("Jane Doe <jane@example.com>");
    let (s, body) = call(&app, "/v1/events", Some("meter"), CE, bad).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(body, json!({"error":"invalid_event","field":"subject"}));
    assert!(!body.to_string().contains("Jane"));
    let mut zero = event("a");
    zero["data"]["weight"] = json!(0);
    assert_eq!(
        call(&app, "/v1/events", Some("meter"), CE, zero).await.1["field"],
        "weight"
    );
    let (s, _) = call(
        &app,
        "/v1/events",
        Some("meter"),
        "application/json",
        event("a"),
    )
    .await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn a_batch_is_all_or_nothing_and_counts() {
    let app = app();
    let mut bad = event("c");
    bad["data"]["nodeType"] = json!("no spaces");
    let (s, _) = call(
        &app,
        "/v1/events",
        Some("meter"),
        BATCH,
        json!([event("a"), bad]),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, body) = call(
        &app,
        "/v1/events",
        Some("meter"),
        BATCH,
        json!([event("a"), event("b")]),
    )
    .await;
    assert_eq!((s, body), (StatusCode::OK, json!({"new":2,"duplicate":0})));
    let (_, body) = call(
        &app,
        "/v1/events",
        Some("meter"),
        BATCH,
        json!([event("a"), event("z")]),
    )
    .await;
    assert_eq!(body, json!({"new":1,"duplicate":1}));
}
