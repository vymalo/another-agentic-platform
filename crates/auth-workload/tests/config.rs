//! The load-time refusals of `workloadClients` (ANR-1010): a permission a workload may not carry, a
//! duplicated identity, an empty one, and a bundle that is not a CA.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use aap_auth_workload::{ConfigError, WorkloadAuth, WorkloadClient, WorkloadClients};

const CA: &str = include_str!("fixtures/ca.pem");
const NOT_A_CA: &str = include_str!("fixtures/accepted.pem");

fn clients(clients: Vec<WorkloadClient>) -> WorkloadClients {
    WorkloadClients { clients }
}

fn one(identity: &str, permissions: &[&str]) -> WorkloadClients {
    clients(vec![WorkloadClient {
        spiffe_id: identity.to_owned(),
        permissions: permissions.iter().map(|p| (*p).to_owned()).collect(),
    }])
}

#[test]
fn only_platform_agents_own_is_grantable() {
    let refused = one(
        "spiffe://another/ns/test/sa/orchestrator",
        &["platform:agents.read"],
    );
    let err = WorkloadAuth::new(CA, &refused).unwrap_err();
    assert!(
        matches!(err, ConfigError::NotGrantable { index: 0, .. }),
        "{err}"
    );
    // The one that is grantable loads.
    let allowed = one(
        "spiffe://another/ns/test/sa/orchestrator",
        &["platform:agents.own"],
    );
    assert!(WorkloadAuth::new(CA, &allowed).is_ok());
}

#[test]
fn an_identity_named_twice_is_refused() {
    let twice = clients(vec![
        WorkloadClient {
            spiffe_id: "spiffe://another/ns/test/sa/a".to_owned(),
            permissions: vec!["platform:agents.own".to_owned()],
        },
        WorkloadClient {
            spiffe_id: "spiffe://another/ns/test/sa/a".to_owned(),
            permissions: vec!["platform:agents.own".to_owned()],
        },
    ]);
    let err = WorkloadAuth::new(CA, &twice).unwrap_err();
    assert!(
        matches!(err, ConfigError::Duplicate { index: 1, .. }),
        "{err}"
    );
}

#[test]
fn an_empty_identity_is_refused() {
    let err = WorkloadAuth::new(CA, &one("  ", &["platform:agents.own"])).unwrap_err();
    assert!(
        matches!(err, ConfigError::EmptyIdentity { index: 0 }),
        "{err}"
    );
}

#[test]
fn a_bundle_that_is_not_a_ca_is_refused_at_load() {
    let table = one(
        "spiffe://another/ns/test/sa/orchestrator",
        &["platform:agents.own"],
    );
    let err = WorkloadAuth::new(NOT_A_CA, &table).unwrap_err();
    assert!(matches!(err, ConfigError::Bundle(_)), "{err}");
    let err = WorkloadAuth::new("", &table).unwrap_err();
    assert!(matches!(err, ConfigError::Bundle(_)), "{err}");
}
