//! The counters of the API (K4): the route label is the template of the table, a path that matched nothing is one fixed
//! label, and nothing a caller sent (a token, a path, a name) is a label or a value.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::sync::Arc;

use aap_platform_api::{ApiMetrics, ApiState, metric, router};
use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
use aap_ports::{AuthError, Authenticator, InvalidToken, Principal};
use axum::body::Body;
use axum::http::{Method, Request};
use common::*;
use tower::ServiceExt;

/// An API over the `Memory` ports that counts into `metrics`, and a token that holds every permission.
fn api() -> (Api, Arc<ApiMetrics>, String) {
    let metrics = Arc::new(ApiMetrics::new());
    let objects = MemoryObjects::in_namespace(NS);
    let auth = MemoryAuthenticator::new();
    let state = ApiState::new(objects.clone(), auth.clone(), defaults())
        .unwrap()
        .with_metrics(metrics.clone());
    let api = Api {
        router: router(state),
        objects,
        auth,
    };
    let token = api.admin();
    (api, metrics, token)
}

#[tokio::test]
async fn metrics_label_is_route_template() {
    let (api, metrics, token) = api();
    // Three different names, one template: one series.
    for name in ["alpha", "beta", "gamma-9"] {
        let reply = api
            .get(&format!("/v1/agents/{name}"))
            .token(&token)
            .send()
            .await;
        assert_eq!(reply.code(), 404);
    }
    api.get("/v1/me").token(&token).send().await;

    assert_eq!(
        metrics.get(
            metric::REQUESTS,
            &[
                ("route", "/v1/agents/{name}"),
                ("method", "GET"),
                ("status", "404")
            ]
        ),
        3
    );
    assert_eq!(
        metrics.get(
            metric::REQUESTS,
            &[("route", "/v1/me"), ("method", "GET"), ("status", "200")]
        ),
        1
    );
    let text = metrics.render();
    for name in ["alpha", "beta", "gamma-9"] {
        assert!(
            !text.contains(name),
            "a name of the path is in the metrics: {text}"
        );
    }
}

#[tokio::test]
async fn unknown_path_counts_as_unmatched() {
    let (api, metrics, token) = api();
    for path in [
        "/",
        "/v1/nothing",
        "/v1/agents/a/b/c",
        "/secret-path-sentinel-77",
    ] {
        api.get(path).token(&token).send().await;
    }
    // A method nobody knows is one label, too.
    let odd = Request::builder()
        .method(Method::from_bytes(b"BREW").unwrap())
        .uri("/v1/anything-sentinel-12")
        .body(Body::empty())
        .unwrap();
    api.router.clone().oneshot(odd).await.unwrap();

    assert_eq!(
        metrics.get(
            metric::REQUESTS,
            &[("route", "unmatched"), ("method", "GET"), ("status", "404")]
        ),
        4
    );
    assert_eq!(
        metrics.get(
            metric::REQUESTS,
            &[
                ("route", "unmatched"),
                ("method", "other"),
                ("status", "404")
            ]
        ),
        1
    );
    let text = metrics.render();
    assert!(!text.contains("sentinel"), "{text}");
    assert!(!text.contains("BREW"), "{text}");
    // A method the table does not have for a path it has: the template, the method, 405.
    let reply = api
        .request(Method::POST, "/v1/me")
        .token(&token)
        .send()
        .await;
    assert_eq!(reply.code(), 405);
    assert_eq!(
        metrics.get(
            metric::REQUESTS,
            &[("route", "/v1/me"), ("method", "POST"), ("status", "405")]
        ),
        1
    );
}

#[tokio::test]
async fn auth_failures_are_counted_by_closed_reason() {
    let (api, metrics, _) = api();
    api.get("/v1/me").send().await;
    api.get("/v1/me").token("not-a-token").send().await;
    api.get("/v1/me").token("not-a-token-either").send().await;
    assert_eq!(
        metrics.get(metric::AUTH_FAILURES, &[("reason", "missing")]),
        1
    );
    // The memory authenticator refuses an unknown token as a bad signature.
    assert_eq!(
        metrics.get(metric::AUTH_FAILURES, &[("reason", "bad_signature")]),
        2
    );
}

#[tokio::test]
async fn no_token_in_metrics() {
    let (api, metrics, token) = api();
    let sentinel = "token-sentinel-metrics-5be1";
    api.auth
        .grant(sentinel, "carol-sentinel", ["platform:agents.read"]);
    api.get("/v1/me").token(sentinel).send().await;
    api.get("/v1/secret-keys").token(sentinel).send().await; // 403
    api.get("/v1/me")
        .token("wrong-sentinel-token-90d2")
        .send()
        .await; // 401
    api.get("/v1/agents/path-sentinel-4a")
        .token(&token)
        .send()
        .await;

    let text = metrics.render();
    for needle in [
        sentinel,
        "carol-sentinel",
        "wrong-sentinel",
        "path-sentinel",
        token.as_str(),
        "Bearer",
    ] {
        assert!(
            !text.contains(needle),
            "{needle} is in the metrics:\n{text}"
        );
    }
    // What is there is the closed vocabulary and numbers.
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        assert!(
            line.starts_with("aap_api_requests_total{route=\"")
                || line.starts_with("aap_api_auth_failures_total{reason=\""),
            "{line}"
        );
    }
}

#[tokio::test]
async fn the_text_is_prometheus_format_with_help_and_type() {
    let (api, metrics, token) = api();
    api.get("/v1/me").token(&token).send().await;
    let text = metrics.render();
    assert!(text.contains("# HELP aap_api_requests_total"), "{text}");
    assert!(
        text.contains("# TYPE aap_api_requests_total counter"),
        "{text}"
    );
    assert!(
        text.contains("aap_api_requests_total{route=\"/v1/me\",method=\"GET\",status=\"200\"} 1"),
        "{text}"
    );
}

#[tokio::test]
async fn refusals_are_counted() {
    let (api, metrics, _) = api();
    let reader = api.token("reader", &["platform:models.write"]);
    api.get("/v1/info").send().await; // no token
    api.get("/v1/info").token("not-a-token").send().await; // a token nobody knows
    let lacking = api.get("/v1/info").token(&reader).send().await; // a token without the permission
    assert_eq!(lacking.code(), 403);

    let series = |status| {
        metrics.get(
            metric::REQUESTS,
            &[("route", "/v1/info"), ("method", "GET"), ("status", status)],
        )
    };
    assert_eq!(series("401"), 2);
    assert_eq!(series("403"), 1);
    assert_eq!(series("200"), 0);
}

/// An authenticator that answers with the error its token names.
struct Scripted;

const SOURCE_SENTINEL: &str = "source-sentinel-keys-at-https-issuer-7f3a";

impl Authenticator for Scripted {
    async fn authenticate(&self, token: Option<&str>) -> Result<Principal, AuthError> {
        match token {
            None | Some("") => Err(AuthError::Missing),
            Some("unavailable") => Err(AuthError::Unavailable(SOURCE_SENTINEL.into())),
            Some(name) => InvalidToken::ALL
                .iter()
                .find(|r| format!("{r:?}") == name)
                .map_or_else(
                    || panic!("no reason named {name}"),
                    |r| Err(AuthError::Invalid(*r)),
                ),
        }
    }
}

#[tokio::test]
async fn every_auth_reason_has_its_word() {
    let metrics = Arc::new(ApiMetrics::new());
    let objects = MemoryObjects::in_namespace(NS);
    let state = ApiState::new(objects.clone(), Scripted, defaults())
        .unwrap()
        .with_metrics(metrics.clone());
    let api = Api::from_router(router(state), objects);

    api.get("/v1/me").send().await;
    for reason in InvalidToken::ALL {
        api.get("/v1/me").token(&format!("{reason:?}")).send().await;
    }
    let down = api.get("/v1/me").token("unavailable").send().await;
    assert_eq!(down.code(), 503);

    let words = [
        "missing",
        "malformed",
        "bad_signature",
        "unknown_key",
        "expired",
        "wrong_issuer",
        "wrong_audience",
        "missing_claim",
        "unavailable",
    ];
    for word in words {
        assert_eq!(
            metrics.get(metric::AUTH_FAILURES, &[("reason", word)]),
            1,
            "{word}: {}",
            metrics.render()
        );
    }
    let text = metrics.render();
    assert!(!text.contains(SOURCE_SENTINEL), "{text}");
    assert_eq!(
        text.lines()
            .filter(|l| l.starts_with("aap_api_auth_failures_total{"))
            .count(),
        words.len(),
        "{text}"
    );
}
