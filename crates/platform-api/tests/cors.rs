//! CORS: the deployment's origins, exactly, with no wildcard and no credentials.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_platform_api::{CorsError, CorsOrigins};
use axum::http::Method;
use common::*;

const WEB: &str = "https://admin.example.com";
const TAURI: &str = "tauri://localhost";
const STRANGER: &str = "https://evil.example.com";

fn api() -> Api {
    Api::with(CorsOrigins::new([WEB, TAURI]).unwrap())
}

async fn preflight(api: &Api, origin: &str, path: &str, method: &str, headers: &str) -> Reply {
    api.request(Method::OPTIONS, path)
        .header("origin", origin)
        .header("access-control-request-method", method)
        .header("access-control-request-headers", headers)
        .send()
        .await
}

#[tokio::test]
async fn a_preflight_from_a_listed_origin_gets_the_headers_and_needs_no_token() {
    let api = api();
    for origin in [WEB, TAURI] {
        let r = preflight(
            &api,
            origin,
            "/v1/agents/chat",
            "PUT",
            "authorization, content-type, if-match, if-none-match",
        )
        .await;
        assert!(r.status.is_success(), "{origin}: {} {}", r.code(), r.text());
        assert_eq!(r.header("access-control-allow-origin"), Some(origin));
        let methods = r
            .header("access-control-allow-methods")
            .unwrap()
            .to_ascii_uppercase();
        for m in ["GET", "PUT", "DELETE", "OPTIONS"] {
            assert!(methods.contains(m), "{methods}");
        }
        assert!(
            !methods.contains("POST") && !methods.contains("PATCH"),
            "{methods}"
        );
        let allowed = r
            .header("access-control-allow-headers")
            .unwrap()
            .to_ascii_lowercase();
        for h in ["authorization", "content-type", "if-match", "if-none-match"] {
            assert!(allowed.contains(h), "{allowed}");
        }
        assert_eq!(r.header("access-control-max-age"), Some("600"));
        assert!(
            r.header("access-control-allow-credentials").is_none(),
            "no credentials mode: the token is in a header"
        );
        assert_ne!(r.header("access-control-allow-origin"), Some("*"));
        assert!(
            r.header("vary")
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains("origin")
        );
    }
}

#[tokio::test]
async fn every_answer_carries_the_hardening_headers_a_preflight_and_a_refusal_included() {
    // The CORS layer answers a preflight itself; the headers that make an answer not cached and not sniffed must be
    // outside it, or the preflight, which is an answer, would go without.
    let api = api();
    let preflight_ok = preflight(&api, WEB, "/v1/agents/chat", "PUT", "authorization").await;
    assert!(preflight_ok.status.is_success(), "{}", preflight_ok.text());
    let preflight_stranger =
        preflight(&api, STRANGER, "/v1/agents/chat", "PUT", "authorization").await;
    let preflight_nowhere = preflight(&api, WEB, "/v1/nowhere", "GET", "authorization").await;
    let not_found = api.get("/v1/nowhere").header("origin", WEB).send().await;
    let not_allowed = api
        .request(Method::POST, "/v1/agents")
        .header("origin", WEB)
        .send()
        .await;
    let unauthorized = api.get("/v1/me").header("origin", WEB).send().await;
    for (what, r) in [
        ("a preflight", &preflight_ok),
        ("a preflight from a stranger", &preflight_stranger),
        ("a preflight of a path that is no route", &preflight_nowhere),
        ("a 404", &not_found),
        ("a 405", &not_allowed),
        ("a 401", &unauthorized),
    ] {
        assert_eq!(r.header("cache-control"), Some("no-store"), "{what}");
        assert_eq!(
            r.header("x-content-type-options"),
            Some("nosniff"),
            "{what}"
        );
    }
    // And they still carry the CORS headers: reordering the layers must not cost the page its error bodies.
    assert_eq!(not_found.header("access-control-allow-origin"), Some(WEB));
    assert_eq!(
        unauthorized.header("access-control-allow-origin"),
        Some(WEB)
    );
}

#[tokio::test]
async fn a_preflight_from_another_origin_gets_no_allow_origin() {
    let api = api();
    for origin in [
        STRANGER,
        "http://admin.example.com",
        "https://admin.example.com:8443",
        "https://sub.admin.example.com",
        "https://admin.example.com.evil.example",
        "null",
        "tauri://localhost.evil",
    ] {
        let r = preflight(&api, origin, "/v1/agents/chat", "PUT", "authorization").await;
        assert!(
            r.header("access-control-allow-origin").is_none(),
            "{origin} was allowed: {:?}",
            r.headers
        );
        assert!(r.header("access-control-allow-credentials").is_none());
    }
}

#[tokio::test]
async fn an_answer_to_a_listed_origin_exposes_the_etag_and_another_origin_gets_no_header() {
    let api = api();
    let token = api.admin();
    let ok = api
        .get("/v1/me")
        .token(&token)
        .header("origin", WEB)
        .send()
        .await;
    assert_eq!(ok.code(), 200);
    assert_eq!(ok.header("access-control-allow-origin"), Some(WEB));
    assert!(
        ok.header("access-control-expose-headers")
            .unwrap()
            .to_ascii_lowercase()
            .contains("etag")
    );
    assert!(ok.header("access-control-allow-credentials").is_none());

    let other = api
        .get("/v1/me")
        .token(&token)
        .header("origin", STRANGER)
        .send()
        .await;
    // tower-http adds the static headers whatever the origin; without `Allow-Origin` a browser reads none of them.
    assert!(other.header("access-control-allow-origin").is_none());
    assert!(other.header("access-control-allow-credentials").is_none());

    // A refusal is readable by the page too: a 401 or a 403 is something the web shows.
    let refused = api.get("/v1/agents").header("origin", WEB).send().await;
    assert_eq!(refused.code(), 401);
    assert_eq!(refused.header("access-control-allow-origin"), Some(WEB));
    let missing = api.get("/v1/nothing").header("origin", WEB).send().await;
    assert_eq!(missing.code(), 404);
    assert_eq!(missing.header("access-control-allow-origin"), Some(WEB));
}

#[tokio::test]
async fn with_no_origin_configured_there_is_no_cors_at_all() {
    let api = Api::new();
    let token = api.admin();
    let r = api
        .get("/v1/me")
        .token(&token)
        .header("origin", WEB)
        .send()
        .await;
    assert_eq!(r.code(), 200);
    assert!(r.header("access-control-allow-origin").is_none());
    let r = preflight(&api, WEB, "/v1/agents/chat", "PUT", "authorization").await;
    assert!(r.header("access-control-allow-origin").is_none());
    assert_eq!(r.code(), 405, "an OPTIONS that nothing answers is a 405");
}

#[test]
fn a_wildcard_is_refused_when_the_origins_are_read() {
    assert_eq!(CorsOrigins::new(["*"]).unwrap_err(), CorsError::Wildcard);
    assert_eq!(
        CorsOrigins::new([WEB, "https://*.example.com"]).unwrap_err(),
        CorsError::Wildcard
    );
    assert_eq!(CorsOrigins::new(["null"]).unwrap_err(), CorsError::Null);
    assert!(CorsOrigins::new(Vec::<String>::new()).unwrap().is_empty());
}
