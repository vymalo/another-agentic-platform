//! What the test files share: an issuer, an authenticator for it, and the claims of a good token.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::{Duration, Instant};

use aap_auth_oidc::testkit::{
    Signing, TEST_AUDIENCE, TEST_INTERVAL, TEST_ROLES_CLAIM, TestIdp, now,
};
use aap_auth_oidc::{OidcAuthenticator, OidcConfig, Tuning};
use aap_ports::testkit::SENTINEL;
use aap_ports::{AuthError, Authenticator, InvalidToken, Principal};
use serde_json::{Map, Value, json};

/// The subject of a good token.
pub const SUBJECT: &str = "subject-1";

/// The roles of a good token.
pub const ROLES: [&str; 2] = ["platform:agents.read", "user"];

/// An issuer and an authenticator for it, with the cache's intervals of the testkit.
pub struct Rig {
    pub idp: TestIdp,
    pub auth: OidcAuthenticator,
}

pub fn tuning() -> Tuning {
    Tuning {
        kid_refetch_interval: TEST_INTERVAL,
        retry_interval: TEST_INTERVAL,
        use_system_proxy: false,
        ..Tuning::default()
    }
}

impl Rig {
    pub async fn new() -> Self {
        Self::with(|config| config, |tuning| tuning).await
    }

    pub async fn with(
        config: impl FnOnce(OidcConfig) -> OidcConfig,
        adjust: impl FnOnce(Tuning) -> Tuning,
    ) -> Self {
        let idp = TestIdp::start().await;
        let auth = OidcAuthenticator::with_tuning(config(idp.config()), adjust(tuning())).unwrap();
        Self { idp, auth }
    }

    /// The claims of a good token: this issuer, this audience, valid now for five minutes, `SUBJECT`, `ROLES`.
    pub fn claims(&self) -> Map<String, Value> {
        let mut claims = self.idp.claims(TEST_AUDIENCE, SUBJECT);
        claims.insert(TEST_ROLES_CLAIM.to_owned(), json!(ROLES));
        claims
    }

    pub fn mint(&self, claims: &Map<String, Value>, signing: Signing) -> String {
        self.idp.mint(claims, signing)
    }

    pub async fn check(&self, token: &str) -> Result<Principal, AuthError> {
        self.auth.authenticate(Some(token)).await
    }

    /// The result for a token with `claims`, signed with a published RS256 key.
    pub async fn check_claims(&self, claims: &Map<String, Value>) -> Result<Principal, AuthError> {
        self.check(&self.mint(
            claims,
            Signing::Published(aap_auth_oidc::testkit::Alg::Rs256),
        ))
        .await
    }

    /// Waits until the cache may fetch again for an unknown `kid`, and after a failure.
    pub async fn wait_out_the_interval(&self) {
        tokio::time::sleep(TEST_INTERVAL + Duration::from_millis(100)).await;
    }
}

/// Panics unless `result` is the refusal `reason`.
#[track_caller]
pub fn assert_invalid(result: Result<Principal, AuthError>, reason: InvalidToken, what: &str) {
    match result {
        Err(AuthError::Invalid(got)) => assert_eq!(got, reason, "{what}"),
        other => panic!("{what}: expected Invalid({reason:?}), got {other:?}"),
    }
}

#[track_caller]
pub fn assert_accepted(result: Result<Principal, AuthError>, what: &str) -> Principal {
    match result {
        Ok(principal) => principal,
        Err(error) => panic!("{what}: expected the token to be accepted, got {error:?}"),
    }
}

pub fn unavailable<T: std::fmt::Debug>(result: &Result<T, AuthError>) -> bool {
    matches!(result, Err(AuthError::Unavailable(_)))
}

pub fn seconds_ago(seconds: u64) -> u64 {
    now() - seconds
}

/// A token that nobody issued, with the sentinel where a real one has something to leak.
pub fn sentinel_token(rig: &Rig) -> String {
    let mut claims = rig.claims();
    for claim in ["iss", "aud", "sub"] {
        claims.insert(claim.into(), json!(SENTINEL));
    }
    claims.insert("exp".into(), json!(seconds_ago(0) + 300));
    claims.insert("jti".into(), json!(SENTINEL));
    rig.idp.forge(
        &json!({"alg": "RS256", "typ": "JWT", "kid": SENTINEL}),
        &claims,
        SENTINEL.as_bytes(),
    )
}

pub fn secrets_of(token: &str) -> Vec<String> {
    let mut secrets = vec![token.to_owned(), SENTINEL.to_owned()];
    secrets.extend(token.split('.').map(str::to_owned));
    secrets
}

/// Polls `condition` every 10 ms until it holds, and fails after ten seconds.
pub async fn eventually(what: &str, mut condition: impl FnMut() -> bool) {
    let give_up = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < give_up, "gave up waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
