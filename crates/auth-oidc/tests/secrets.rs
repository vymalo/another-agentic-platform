//! Nothing of a token is in an error, in the chain of its sources, in its debug form or in a log: the
//! sentinel is every place a real token has something to leak (the `kid`, the claims, the signature).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_auth_oidc::testkit::{Alg, Signing, TEST_AUDIENCE};
use aap_ports::{AuthError, Authenticator};
use common::{Rig, secrets_of, sentinel_token};

/// What an error shows: its message, its debug form and those of its whole chain of sources.
fn shown(error: &AuthError) -> String {
    let mut text = format!("{error} {error:?}");
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(&format!(" {cause} {cause:?}"));
        source = cause.source();
    }
    text
}

#[tokio::test]
async fn unavailable_keys_say_nothing_of_the_token_the_issuer_or_its_address() {
    let rig = Rig::new().await;
    rig.idp.set_jwks_down(true);
    let kid = rig.idp.published_kid(Alg::Rs256);
    let issued = rig.mint(&rig.claims(), Signing::Published(Alg::Rs256));
    for token in [sentinel_token(&rig), issued] {
        let error = rig.auth.authenticate(Some(&token)).await.unwrap_err();
        assert!(matches!(error, AuthError::Unavailable(_)), "{error:?}");
        let text = shown(&error);
        for secret in secrets_of(&token) {
            assert!(
                !text.contains(&secret),
                "the error shows `{secret}`: {text}"
            );
        }
        for claim in [
            kid.as_str(),
            TEST_AUDIENCE,
            common::SUBJECT,
            "platform:agents.read",
            "127.0.0.1",
        ] {
            assert!(!text.contains(claim), "the error shows `{claim}`: {text}");
        }
        // It does say what the operator needs.
        assert!(text.contains("503"), "{text}");
    }
}

#[tokio::test]
async fn a_refusal_is_a_word_of_a_closed_set() {
    let rig = Rig::new().await;
    let token = sentinel_token(&rig);
    let error = rig.auth.authenticate(Some(&token)).await.unwrap_err();
    assert!(matches!(error, AuthError::Invalid(_)), "{error:?}");
    for secret in secrets_of(&token) {
        assert!(!shown(&error).contains(&secret), "{}", shown(&error));
    }
}
