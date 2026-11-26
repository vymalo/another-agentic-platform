//! Cases of the `ClientAuthenticator` suite (ANR-1010). The workload implementation
//! (`aap-auth-workload`) is held to these, and adds the cases only a certificate has (a chain with an
//! intermediate, a revoked leaf, a SAN of another shape).
//!
//! The certificates are **fixtures**, not minted at test time: a checked-in test CA and its leaves are
//! what the card asks for, and a suite that generated its own CA would not prove that a real CA bundle
//! loads. So an implementation hands the suite the certificates it has, with the identity each one names.

use super::*;
use crate::{Certificate, ClientAuthError, InvalidCertificate};

/// A certificate the authenticator accepts, and what it must make of it.
#[derive(Clone, Debug)]
pub struct Accepted {
    /// The certificate.
    pub certificate: Certificate,
    /// The URI SAN it carries, which the configuration names.
    pub identity: String,
    /// The permissions the configuration maps that identity to, in byte order.
    pub permissions: Vec<String>,
}

/// A [`ClientAuthenticator`](crate::ClientAuthenticator) the suite can ask for its fixtures.
///
/// The implementation is configured so that [`accepted`](Self::accepted)'s identity maps to exactly its
/// permissions; every other method hands a certificate that is valid in every respect but one, so that
/// each refusal has a single cause.
pub trait ClientAuthenticatorUnderTest: crate::ClientAuthenticator + 'static {
    /// A certificate the authenticator accepts, its URI SAN, and the permissions it maps to.
    fn accepted(&self) -> impl Future<Output = Accepted> + Send;

    /// A certificate with a valid shape and a URI SAN the configuration names, signed by **another** CA.
    fn another_ca(&self) -> impl Future<Output = Certificate> + Send;

    /// A certificate of the trusted CA whose extended key usage is server authentication, not client.
    fn wrong_use(&self) -> impl Future<Output = Certificate> + Send;

    /// A certificate of the trusted CA, for client authentication, past its `notAfter`.
    fn expired(&self) -> impl Future<Output = Certificate> + Send;

    /// A certificate of the trusted CA, for client authentication, whose URI SAN the configuration does
    /// not name, and that URI SAN.
    fn unknown_identity(&self) -> impl Future<Output = (Certificate, String)> + Send;
}

/// What an error shows: its message, its debug form and those of its whole chain of sources.
fn shown(error: &ClientAuthError) -> String {
    let mut text = format!("{error} {error:?}");
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        text.push_str(&format!(" {cause} {cause:?}"));
        source = cause.source();
    }
    text
}

/// Panics when `error` shows `identity` or the sentinel.
fn assert_hides(when: &str, error: &ClientAuthError, identity: &str) {
    let text = shown(error);
    for secret in [identity, SENTINEL] {
        assert!(
            !text.contains(secret),
            "{when}: the error holds the identity ({secret}): {text}"
        );
    }
}

/// Cases of the client-certificate suite, as plain async functions for harnesses that do not use the
/// macro.
#[allow(missing_docs)] // each case is named for what it asserts
pub mod cases {
    use super::*;

    pub async fn a_certificate_is_its_identity_and_permissions<A: ClientAuthenticatorUnderTest>(
        a: A,
    ) {
        let Accepted {
            certificate,
            identity,
            permissions,
        } = a.accepted().await;
        let principal = a.authenticate(Some(&certificate)).await.unwrap();
        assert_eq!(principal.subject(), identity);
        assert_eq!(
            principal.permissions().collect::<Vec<_>>(),
            permissions,
            "the permissions the configuration maps the identity to, in byte order"
        );
        assert!(principal.holds("platform:agents.own"));
        // It is not a person: nothing of the person's roles claim is here.
        assert!(!principal.holds("platform:agents.read"));
        assert_eq!(principal.with_prefix("platform:").count(), 1);
    }

    pub async fn no_certificate_is_missing<A: ClientAuthenticatorUnderTest>(a: A) {
        let err = a.authenticate(None).await.unwrap_err();
        assert!(matches!(err, ClientAuthError::Missing), "{err:?}");
        assert_eq!(err.class(), ErrorClass::Invalid);
    }

    pub async fn a_certificate_from_another_ca_is_invalid<A: ClientAuthenticatorUnderTest>(a: A) {
        let certificate = a.another_ca().await;
        let err = a.authenticate(Some(&certificate)).await.unwrap_err();
        assert!(
            matches!(err, ClientAuthError::Invalid(InvalidCertificate::BadChain)),
            "{err:?}"
        );
        assert_eq!(err.class(), ErrorClass::Invalid);
    }

    pub async fn a_certificate_for_another_use_is_invalid<A: ClientAuthenticatorUnderTest>(a: A) {
        let certificate = a.wrong_use().await;
        let err = a.authenticate(Some(&certificate)).await.unwrap_err();
        assert!(
            matches!(err, ClientAuthError::Invalid(InvalidCertificate::WrongUse)),
            "{err:?}"
        );
    }

    pub async fn an_expired_certificate_is_invalid<A: ClientAuthenticatorUnderTest>(a: A) {
        let certificate = a.expired().await;
        let err = a.authenticate(Some(&certificate)).await.unwrap_err();
        assert!(
            matches!(err, ClientAuthError::Invalid(InvalidCertificate::Expired)),
            "{err:?}"
        );
    }

    pub async fn an_unknown_identity_is_invalid<A: ClientAuthenticatorUnderTest>(a: A) {
        let (certificate, identity) = a.unknown_identity().await;
        let err = a.authenticate(Some(&certificate)).await.unwrap_err();
        assert!(
            matches!(
                err,
                ClientAuthError::Invalid(InvalidCertificate::UnknownIdentity)
            ),
            "{identity}: {err:?}"
        );
    }

    pub async fn garbage_is_malformed<A: ClientAuthenticatorUnderTest>(a: A) {
        let certificate = Certificate::new(b"not a certificate".to_vec());
        let err = a.authenticate(Some(&certificate)).await.unwrap_err();
        assert!(
            matches!(err, ClientAuthError::Invalid(InvalidCertificate::Malformed)),
            "{err:?}"
        );
    }

    pub async fn an_error_does_not_carry_the_certificate<A: ClientAuthenticatorUnderTest>(a: A) {
        // A valid certificate whose URI SAN is the sentinel, which the configuration does not name: the
        // implementation reads it, refuses it, and must not repeat it.
        let (certificate, identity) = a.unknown_identity().await;
        let err = a.authenticate(Some(&certificate)).await.unwrap_err();
        assert_hides("an unknown identity", &err, &identity);
        // The certificate itself has no printable form that holds its bytes.
        let debug = format!("{certificate:?}");
        assert!(
            !debug.contains(&identity),
            "a certificate's Debug shows its identity: {debug}"
        );
        assert!(
            !debug.contains(&hex(certificate.as_der())),
            "a certificate's Debug shows its bytes: {debug}"
        );
    }

    pub async fn only_the_configured_permissions_are_granted<A: ClientAuthenticatorUnderTest>(
        a: A,
    ) {
        let Accepted {
            certificate,
            permissions,
            ..
        } = a.accepted().await;
        let principal = a.authenticate(Some(&certificate)).await.unwrap();
        assert_eq!(
            principal.with_prefix("platform:").collect::<Vec<_>>(),
            permissions,
            "a workload never gets a permission the configuration did not name"
        );
        assert!(!principal.holds("platform:agents.write"));
        assert!(!principal.holds("secrets.pick"));
    }
}

/// The bytes of `der` as lowercase hex, for a test that looks for them in a message.
fn hex(der: &[u8]) -> String {
    let mut out = String::with_capacity(der.len() * 2);
    for byte in der {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Generate one `#[tokio::test]` per case of the client-certificate suite. `$make` is a path to an
/// `async fn() -> Option<A>` where `A: ClientAuthenticatorUnderTest`; `None` skips the suite, unless
/// `AAP_TEST_REQUIRE_BACKEND=1`, which fails it instead.
#[macro_export]
macro_rules! client_authenticator_conformance {
    ($make:path) => {
        $crate::client_authenticator_conformance!(@cases $make;
            a_certificate_is_its_identity_and_permissions,
            no_certificate_is_missing,
            a_certificate_from_another_ca_is_invalid,
            a_certificate_for_another_use_is_invalid,
            an_expired_certificate_is_invalid,
            an_unknown_identity_is_invalid,
            garbage_is_malformed,
            an_error_does_not_carry_the_certificate,
            only_the_configured_permissions_are_granted,
        );
    };
    (@cases $make:path; $($case:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $case() {
                let Some(authenticator) = $make().await else {
                    $crate::testkit::skipped(&format!("{}: client authenticator not configured", stringify!($case)));
                    return;
                };
                $crate::testkit::client_auth::cases::$case(authenticator).await;
            }
        )*
    };
}
