//! What the API logs holds a method, a route template, a status, a latency and a subject: never a token, a body, a name from
//! the path, or a secret key.
//!
//! In a test binary of its own, as `aap-auth-oidc`'s `tests/logs.rs` is: `tracing` caches a callsite that another test of the
//! same process hit first while no subscriber was installed, and a log test would pass because nothing is logged.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::*;
use serde_json::json;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

const SENTINEL_TOKEN: &str = "token-sentinel-8c41e7a2";
const SENTINEL_SECRET: &str = "secret-sentinel-name-5d19";
const SENTINEL_KEY: &str = "key-sentinel-0b73";
const SENTINEL_VALUE: &str = "value-sentinel-e2f6";
const SENTINEL_PATH: &str = "path-sentinel-91ac";
const SENTINEL_TITLE: &str = "title-sentinel-4477";

#[tokio::test]
async fn nothing_the_api_logs_holds_a_token_a_body_a_path_or_a_secret_key() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let (api, admin) = Api::seeded().await;
    // The sentinel is the token of a person, and the token of nobody.
    api.auth.grant(
        SENTINEL_TOKEN,
        "alice",
        ["platform:agents.write", "platform:agents.read"],
    );
    api.objects
        .offer(SENTINEL_SECRET, SENTINEL_KEY, "sentinel-es");

    let mut coder = coder_body("coder-me");
    coder["title"] = json!(SENTINEL_TITLE);
    coder["agent"]["github"]["privateKey"] =
        json!({ "secret": SENTINEL_SECRET, "key": SENTINEL_KEY });

    // 201: the person can pick no key (403), the administrator can (201).
    let r = api
        .put("/v1/agents/coder-me")
        .token(SENTINEL_TOKEN)
        .create()
        .json(&coder)
        .send()
        .await;
    assert_eq!(r.code(), 403, "{}", r.text());
    let r = api
        .put("/v1/agents/coder-me")
        .token(&admin)
        .create()
        .json(&coder)
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
    // 422: a key that is not offered, named with the sentinel.
    let mut unoffered = coder.clone();
    unoffered["name"] = json!("other");
    unoffered["agent"]["github"]["privateKey"] =
        json!({ "secret": SENTINEL_SECRET, "key": "not-offered" });
    let r = api
        .put("/v1/agents/other")
        .token(&admin)
        .create()
        .json(&unoffered)
        .send()
        .await;
    assert_eq!(r.code(), 422, "{}", r.text());
    // 400: a value where none belongs.
    let mut valued = coder.clone();
    valued["agent"]["github"]["privateKey"]["value"] = json!(SENTINEL_VALUE);
    let r = api
        .put("/v1/agents/coder-me")
        .token(&admin)
        .create()
        .json(&valued)
        .send()
        .await;
    assert_eq!(r.code(), 400, "{}", r.text());
    // 401 with the sentinel as a token nobody has, and with a scheme that is not Bearer.
    for header in [
        format!("Bearer {SENTINEL_VALUE}"),
        format!("Basic {SENTINEL_VALUE}"),
    ] {
        let r = api
            .get("/v1/me")
            .header("authorization", &header)
            .send()
            .await;
        assert_eq!(r.code(), 401);
    }
    // 404: a name in the path, and a path that is no route.
    assert_eq!(
        api.get(&format!("/v1/agents/{SENTINEL_PATH}"))
            .token(&admin)
            .send()
            .await
            .code(),
        404
    );
    assert_eq!(
        api.get(&format!("/{SENTINEL_PATH}"))
            .token(&admin)
            .send()
            .await
            .code(),
        404
    );
    // 412 and 428 and a 503.
    let r = api
        .put("/v1/agents/coder-me")
        .token(&admin)
        .replace("\"stale\"")
        .json(&coder)
        .send()
        .await;
    assert_eq!(r.code(), 412);
    api.objects.set_unavailable(true);
    assert_eq!(api.get("/v1/agents").token(&admin).send().await.code(), 503);
    api.objects.set_unavailable(false);
    api.auth.set_unavailable(true);
    assert_eq!(
        api.get("/v1/me").token(SENTINEL_TOKEN).send().await.code(),
        503
    );

    let log = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();

    // The log is not empty, and says what it should.
    assert!(log.contains("route=/v1/agents/{name}"), "{log}");
    assert!(log.contains("method=PUT"), "{log}");
    assert!(
        log.contains("status=201") && log.contains("status=422") && log.contains("status=503"),
        "{log}"
    );
    assert!(log.contains("latency_ms="), "{log}");
    assert!(
        log.contains("subject=\"alice\"") || log.contains("subject=alice"),
        "the subject may be logged: {log}"
    );
    // The template, not the path.
    assert!(
        log.contains("route=-"),
        "a path that is no route has no template: {log}"
    );

    // And holds none of these.
    for (what, sentinel) in [
        ("the token", SENTINEL_TOKEN),
        ("a token nobody has", SENTINEL_VALUE),
        ("a secret's name", SENTINEL_SECRET),
        ("a secret's key", SENTINEL_KEY),
        ("a name from the path", SENTINEL_PATH),
        ("a title from the body", SENTINEL_TITLE),
    ] {
        assert!(!log.contains(sentinel), "{what} is in the log:\n{log}");
    }
    assert!(
        !log.to_ascii_lowercase().contains("authorization"),
        "no header is logged:\n{log}"
    );
}
