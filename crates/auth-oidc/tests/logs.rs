//! Nothing the authenticator logs holds a token, a `kid` or a claim.
//!
//! In a test binary of its own: a callsite that another test of the same process hit first, while no
//! subscriber was installed, is cached as uninteresting and logs nothing here (a mutant that logged the `kid`
//! passed this test when it shared a binary with the others).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use aap_auth_oidc::testkit::{Alg, Signing, TEST_AUDIENCE};
use aap_ports::Authenticator;
use common::{Rig, secrets_of, sentinel_token};
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

    let log = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
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
