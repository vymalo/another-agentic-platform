//! The `ClientAuthenticator` conformance suite, run against the workload authenticator with the
//! checked-in test CA (ANR-1010).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use aap_auth_workload::{WorkloadAuth, WorkloadClient, WorkloadClients};
use aap_ports::testkit::client_auth::{Accepted, ClientAuthenticatorUnderTest};
use aap_ports::{Certificate, ClientAuthError, ClientAuthenticator, Principal};

const CA: &str = include_str!("fixtures/ca.pem");
const OTHER_CA: &str = include_str!("fixtures/other-ca.pem");
const ACCEPTED: &str = include_str!("fixtures/accepted.pem");
const NO_PERMISSIONS: &str = include_str!("fixtures/no-permissions.pem");
const UNKNOWN: &str = include_str!("fixtures/unknown.pem");
const NO_EKU: &str = include_str!("fixtures/no-eku.pem");
const NO_IDENTITY: &str = include_str!("fixtures/no-identity.pem");
const TWO_SANS: &str = include_str!("fixtures/two-sans.pem");
const WRONG_USE: &str = include_str!("fixtures/wrong-use.pem");
const EXPIRED: &str = include_str!("fixtures/expired.pem");
const OTHER_CA_LEAF: &str = include_str!("fixtures/other-ca-leaf.pem");

const ORCHESTRATOR: &str = "spiffe://another/ns/test/sa/orchestrator";
const NO_PERMISSIONS_IDENTITY: &str = "spiffe://another/ns/test/sa/no-permissions";
const SENTINEL_IDENTITY: &str = "spiffe://another/ns/test/sa/aap-sentinel-never-a-value-8f3c1d";

/// The DER of the first certificate in `pem`.
fn der(pem: &str) -> Certificate {
    let (_, block) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).expect("a PEM certificate");
    Certificate::new(block.contents)
}

fn authenticator() -> WorkloadAuth {
    let clients = WorkloadClients {
        clients: vec![WorkloadClient {
            spiffe_id: ORCHESTRATOR.to_owned(),
            permissions: vec!["platform:agents.own".to_owned()],
        }],
    };
    WorkloadAuth::new(CA, &clients).expect("the test CA bundle and table")
}

struct Fixture(WorkloadAuth);

impl ClientAuthenticator for Fixture {
    async fn authenticate(
        &self,
        certificate: Option<&Certificate>,
    ) -> Result<Principal, ClientAuthError> {
        self.0.authenticate(certificate).await
    }
}

impl ClientAuthenticatorUnderTest for Fixture {
    async fn accepted(&self) -> Accepted {
        Accepted {
            certificate: der(ACCEPTED),
            identity: ORCHESTRATOR.to_owned(),
            permissions: vec!["platform:agents.own".to_owned()],
        }
    }

    async fn another_ca(&self) -> Certificate {
        // The leaf chains to OTHER_CA, which the authenticator does not trust.
        let _ = OTHER_CA;
        der(OTHER_CA_LEAF)
    }

    async fn wrong_use(&self) -> Certificate {
        der(WRONG_USE)
    }

    async fn expired(&self) -> Certificate {
        der(EXPIRED)
    }

    async fn unknown_identity(&self) -> (Certificate, String) {
        (der(UNKNOWN), SENTINEL_IDENTITY.to_owned())
    }

    async fn no_eku(&self) -> Certificate {
        der(NO_EKU)
    }

    async fn no_identity(&self) -> Certificate {
        der(NO_IDENTITY)
    }

    async fn two_identities(&self) -> Certificate {
        der(TWO_SANS)
    }
}

async fn make() -> Option<Fixture> {
    Some(Fixture(authenticator()))
}

aap_ports::client_authenticator_conformance!(make);

/// The table is read: an identity mapped to **no** permissions is a principal that holds nothing. An
/// implementation that handed every accepted identity `GRANTABLE` would pass every case of the suite
/// and fail this.
#[tokio::test]
async fn an_identity_mapped_to_no_permissions_holds_nothing() {
    let clients = WorkloadClients {
        clients: vec![
            WorkloadClient {
                spiffe_id: ORCHESTRATOR.to_owned(),
                permissions: vec!["platform:agents.own".to_owned()],
            },
            WorkloadClient {
                spiffe_id: NO_PERMISSIONS_IDENTITY.to_owned(),
                permissions: Vec::new(),
            },
        ],
    };
    let auth = WorkloadAuth::new(CA, &clients).expect("the test CA bundle and table");
    let principal = auth
        .authenticate(Some(&der(NO_PERMISSIONS)))
        .await
        .expect("a valid certificate of a named identity");
    assert_eq!(principal.subject(), NO_PERMISSIONS_IDENTITY);
    assert_eq!(principal.permissions().count(), 0);
    assert!(!principal.holds("platform:agents.own"));
}

/// The configured bundle decides: the accepted leaf is refused when the authenticator trusts the other
/// CA, so the chain check reads the bundle and not the leaf's shape.
#[tokio::test]
async fn the_configured_bundle_decides() {
    let clients = WorkloadClients {
        clients: vec![WorkloadClient {
            spiffe_id: ORCHESTRATOR.to_owned(),
            permissions: vec!["platform:agents.own".to_owned()],
        }],
    };
    let other = WorkloadAuth::new(OTHER_CA, &clients).expect("the other CA bundle and table");
    let err = other
        .authenticate(Some(&der(ACCEPTED)))
        .await
        .expect_err("a leaf of the trusted CA, not this one");
    assert!(matches!(
        err,
        aap_ports::ClientAuthError::Invalid(aap_ports::InvalidCertificate::BadChain)
    ));
}
