//! A binary may link a second `jsonwebtoken` crypto backend (the dev-dependencies of this crate enable
//! `rust_crypto` beside `aws_lc_rs`, as another crate of the Platform API may). The library then cannot choose
//! between them, and panics on the first verification unless one is installed; the authenticator installs its
//! own. This is a test binary of its own: the choice is made once per process.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_auth_oidc::OidcAuthenticator;
use aap_auth_oidc::testkit::{ALGORITHMS, Signing, TEST_AUDIENCE, TestIdp};
use aap_ports::Authenticator;
use common::{assert_accepted, tuning};

#[tokio::test]
async fn two_backends_in_the_binary_and_two_authenticators_verify_without_a_panic() {
    let idp = TestIdp::start().await;
    let first = OidcAuthenticator::with_tuning(idp.config(), tuning()).unwrap();
    // The provider is installed once per process; building another authenticator is not an error.
    let second = OidcAuthenticator::with_tuning(idp.config(), tuning()).unwrap();
    for alg in ALGORITHMS {
        let token = idp.mint(&idp.claims(TEST_AUDIENCE, "alice"), Signing::Published(alg));
        for auth in [&first, &second] {
            assert_accepted(auth.authenticate(Some(&token)).await, &format!("{alg:?}"));
        }
    }
}
