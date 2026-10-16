//! The `Authenticator` conformance suite of `aap-ports` against the real authenticator, over real HTTP,
//! against an issuer on a local port: RSA, EC and Ed25519 keys made in the test process, published by
//! discovery.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use aap_auth_oidc::testkit::TestAuthenticator;

async fn make() -> Option<TestAuthenticator> {
    Some(TestAuthenticator::start().await)
}

aap_ports::authenticator_conformance!(make);
