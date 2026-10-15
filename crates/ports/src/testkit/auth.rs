//! Cases of the `Authenticator` suite. The OIDC implementation (D6) is held to these, and adds the cases
//! only a token with a signature has (algorithms, `aud`, `exp`, an unknown `kid`).

use super::*;
use crate::AuthError;

/// Cases of the authenticator suite, as plain async functions for harnesses that do not use the macro.
#[allow(missing_docs)] // each case is named for what it asserts
pub mod cases {
    use super::*;

    pub async fn a_token_is_its_persons_subject_and_permissions<A: AuthenticatorUnderTest>(a: A) {
        let subject = unique("alice");
        let claim = ["platform:agents.read", "agent.use:chat", "user"];
        let token = a.issue(&subject, &claim).await;
        let principal = a.authenticate(Some(&token)).await.unwrap();
        assert_eq!(principal.subject(), subject);
        // The claim whole, the values the dashboard ignores included: it is the router that picks.
        assert_eq!(
            principal.permissions().collect::<Vec<_>>(),
            ["agent.use:chat", "platform:agents.read", "user"],
            "the values of the roles claim, in byte order"
        );
        assert!(principal.holds("platform:agents.read"));
        assert!(principal.holds("agent.use:chat"));
        assert!(!principal.holds("platform:agents.write"));
    }

    pub async fn no_token_is_missing_whatever_the_state_of_the_keys<A: AuthenticatorUnderTest>(
        a: A,
    ) {
        for token in [None, Some("")] {
            let err = a.authenticate(token).await.unwrap_err();
            assert!(matches!(err, AuthError::Missing), "{token:?}: {err:?}");
            assert_eq!(err.class(), ErrorClass::Invalid);
        }
        if let Some((without_keys, _)) = a.without_keys().await {
            let err = without_keys.authenticate(None).await.unwrap_err();
            assert!(
                matches!(err, AuthError::Missing),
                "a request with no token needs no keys: {err:?}"
            );
        }
    }

    pub async fn a_token_nobody_issued_is_invalid<A: AuthenticatorUnderTest>(a: A) {
        for token in ["not-a-token", "a.b.c", "Bearer x", "0"] {
            let err = a.authenticate(Some(token)).await.unwrap_err();
            assert!(matches!(err, AuthError::Invalid(_)), "{token:?}: {err:?}");
            assert_eq!(err.class(), ErrorClass::Invalid);
        }
    }

    pub async fn a_person_who_holds_nothing_is_still_a_person<A: AuthenticatorUnderTest>(a: A) {
        let subject = unique("bob");
        let token = a.issue(&subject, &[]).await;
        let principal = a.authenticate(Some(&token)).await.unwrap();
        assert_eq!(principal.subject(), subject);
        assert_eq!(principal.permissions().count(), 0);
        assert_eq!(principal.with_prefix("platform:").count(), 0);
        assert!(!principal.holds("platform:agents.read"));
    }

    pub async fn permissions_are_a_set_compared_whole<A: AuthenticatorUnderTest>(a: A) {
        let token = a
            .issue(
                &unique("carol"),
                &[
                    "platform:agents.write",
                    "platform:agents.read",
                    "platform:agents.read",
                    "agent.use:x",
                ],
            )
            .await;
        let principal = a.authenticate(Some(&token)).await.unwrap();
        assert_eq!(
            principal.with_prefix("platform:").collect::<Vec<_>>(),
            ["platform:agents.read", "platform:agents.write"],
            "what `GET /v1/me` lists: a set, in byte order, only the prefix"
        );
        // Byte for byte: a longer name, a shorter one and another case are not the permission.
        assert!(principal.holds("platform:agents.read"));
        assert!(!principal.holds("platform:agents"));
        assert!(!principal.holds("platform:agents.read.more"));
        assert!(!principal.holds("PLATFORM:AGENTS.READ"));
        assert!(!principal.holds(""));
    }

    pub async fn two_tokens_are_two_people<A: AuthenticatorUnderTest>(a: A) {
        let (x, y) = (unique("x"), unique("y"));
        let tx = a.issue(&x, &["platform:agents.read"]).await;
        let ty = a.issue(&y, &["platform:agents.write"]).await;
        assert_ne!(tx, ty);
        let px = a.authenticate(Some(&tx)).await.unwrap();
        let py = a.authenticate(Some(&ty)).await.unwrap();
        assert_eq!((px.subject(), py.subject()), (x.as_str(), y.as_str()));
        assert!(px.holds("platform:agents.read") && !px.holds("platform:agents.write"));
        assert!(py.holds("platform:agents.write") && !py.holds("platform:agents.read"));
        // The same token is the same person, every time.
        assert_eq!(a.authenticate(Some(&tx)).await.unwrap(), px);
    }

    pub async fn keys_that_cannot_be_fetched_are_unavailable_not_invalid<
        A: AuthenticatorUnderTest,
    >(
        a: A,
    ) {
        let Some((without_keys, token)) = a.without_keys().await else {
            skipped("this authenticator cannot be made to lose its keys");
            return;
        };
        let err = without_keys.authenticate(Some(&token)).await.unwrap_err();
        assert!(matches!(err, AuthError::Unavailable(_)), "{err:?}");
        assert_eq!(err.class(), ErrorClass::Transient);
    }

    pub async fn an_error_does_not_carry_the_token<A: AuthenticatorUnderTest>(a: A) {
        let sentinel = format!("{SENTINEL}.credential");
        let err = a.authenticate(Some(&sentinel)).await.unwrap_err();
        let shown = format!("{err} {err:?}");
        assert!(
            !shown.contains(&sentinel),
            "the error holds the token: {shown}"
        );
        if let Some((without_keys, token)) = a.without_keys().await {
            let err = without_keys.authenticate(Some(&token)).await.unwrap_err();
            let shown = format!("{err} {err:?}");
            assert!(
                !shown.contains(&token),
                "the error holds the token: {shown}"
            );
        }
    }
}

/// Generate one `#[tokio::test]` per case of the authenticator suite. `$make` is a path to an
/// `async fn() -> Option<A>` where `A: AuthenticatorUnderTest`; `None` skips the suite, unless
/// `AAP_TEST_REQUIRE_BACKEND=1`, which fails it instead.
#[macro_export]
macro_rules! authenticator_conformance {
    ($make:path) => {
        $crate::authenticator_conformance!(@cases $make;
            a_token_is_its_persons_subject_and_permissions,
            no_token_is_missing_whatever_the_state_of_the_keys,
            a_token_nobody_issued_is_invalid,
            a_person_who_holds_nothing_is_still_a_person,
            permissions_are_a_set_compared_whole,
            two_tokens_are_two_people,
            keys_that_cannot_be_fetched_are_unavailable_not_invalid,
            an_error_does_not_carry_the_token,
        );
    };
    (@cases $make:path; $($case:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $case() {
                let Some(authenticator) = $make().await else {
                    $crate::testkit::skipped(&format!("{}: authenticator not configured", stringify!($case)));
                    return;
                };
                $crate::testkit::auth::cases::$case(authenticator).await;
            }
        )*
    };
}
