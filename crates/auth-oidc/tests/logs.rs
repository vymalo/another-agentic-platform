//! Nothing the authenticator logs holds a token, a `kid` or a claim.
//!
//! In a test binary of its own: a callsite that another test of the same process hit first, while no
//! subscriber was installed, is cached as uninteresting and logs nothing here (a mutant that logged the `kid`
//! passed this test when it shared a binary with the others).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use aap_auth_oidc::OidcAuthenticator;
use aap_auth_oidc::testkit::{Alg, Signing, TEST_AUDIENCE};
use aap_ports::Authenticator;
use axum::Router;
use axum::routing::get;
use common::{Rig, secrets_of, sentinel_token, tuning};
use serde_json::{Value, json};
use tracing_subscriber::fmt::MakeWriter;

/// The `kid` of a key the build cannot use: it is counted in the log and never named.
const SKIPPED_KID: &str = "skipped-kid-7c1e9d";

/// The URL of a server that serves `keys` as its key set.
async fn serve_keys(keys: Vec<Value>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/jwks", listener.local_addr().unwrap());
    let body = json!({ "keys": keys });
    let router = Router::new().route(
        "/jwks",
        get(move || {
            let body = body.clone();
            async move { axum::Json(body) }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    url
}

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

#[tokio::test]
async fn nothing_the_authenticator_logs_holds_a_token_or_a_claim() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let rig = Rig::new().await;
    let sentinel = sentinel_token(&rig);
    let kid = rig.idp.published_kid(Alg::Rs256);
    // Every path that logs: the keys cannot be fetched (first, and again), they come back, a token is refused.
    rig.idp.set_jwks_down(true);
    let issued = rig.mint(&rig.claims(), Signing::Published(Alg::Rs256));
    for token in [&sentinel, &issued, &sentinel] {
        let _ = rig.auth.authenticate(Some(token)).await;
        rig.wait_out_the_interval().await;
    }
    rig.idp.set_jwks_down(false);
    rig.wait_out_the_interval().await;
    let _ = rig.auth.authenticate(Some(&issued)).await.unwrap();
    for token in [sentinel.as_str(), "garbage", "a.b.c"] {
        let _ = rig.auth.authenticate(Some(token)).await;
    }

    // Keys the build cannot use: the log counts them, beside good ones and when there is no good one, and never
    // names one.
    let real: Value = serde_json::from_str(
        &reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(rig.idp.jwks_url())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    let mut mixed = real["keys"].as_array().unwrap().clone();
    mixed.insert(
        0,
        json!({"kty": "oct", "kid": SKIPPED_KID, "k": "c2VjcmV0c2VjcmV0"}),
    );
    mixed.insert(0, json!({"kty": "RSA", "kid": SKIPPED_KID}));
    let only_unusable = vec![json!({"kty": "oct", "kid": SKIPPED_KID, "k": "c2VjcmV0c2VjcmV0"})];
    let (mixed, only_unusable) = (serve_keys(mixed).await, serve_keys(only_unusable).await);
    let before = capture.0.lock().unwrap().len();
    for url in [mixed, only_unusable] {
        let auth =
            OidcAuthenticator::with_tuning(rig.idp.config().with_jwks_url(url), tuning()).unwrap();
        let _ = auth.ready().await;
    }
    let counted = String::from_utf8(capture.0.lock().unwrap()[before..].to_vec()).unwrap();
    assert!(
        counted.contains("skipped=2") && counted.contains("usable=3"),
        "the skipped keys beside good ones are counted: {counted}"
    );
    assert!(
        counted.contains("skipped=1") && counted.contains("usable=0"),
        "the skipped keys of a set with no good key are counted: {counted}"
    );

    let log = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
    assert!(!log.contains(SKIPPED_KID), "the log names a key: {log}");
    assert!(
        log.contains("cannot be fetched") && log.contains("can be fetched again"),
        "the capture works, and the paths that log were taken: {log}"
    );
    for secret in secrets_of(&sentinel).into_iter().chain(secrets_of(&issued)) {
        assert!(!log.contains(&secret), "the log shows `{secret}`: {log}");
    }
    for claim in [
        kid.as_str(),
        TEST_AUDIENCE,
        common::SUBJECT,
        "platform:agents.read",
    ] {
        assert!(!log.contains(claim), "the log shows `{claim}`");
    }
}
