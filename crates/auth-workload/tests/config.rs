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
fn only_the_workload_permissions_are_grantable() {
    let refused = one(
        "spiffe://another/ns/test/sa/orchestrator",
        &["platform:agents.read"],
    );
    let err = WorkloadAuth::new(CA, &refused).unwrap_err();
    assert!(
        matches!(err, ConfigError::NotGrantable { index: 0, .. }),
        "{err}"
    );
    // Both workload permissions load: the orchestrator's, and the staff service's read-only one (AD-072).
    for granted in ["platform:agents.own", "platform:agents.read.owner"] {
        let allowed = one("spiffe://another/ns/test/sa/orchestrator", &[granted]);
        assert!(
            WorkloadAuth::new(CA, &allowed).is_ok(),
            "{granted} is not grantable"
        );
    }
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

#[test]
fn an_identity_that_is_not_a_spiffe_id_is_refused() {
    let err = WorkloadAuth::new(CA, &one("orchestrator", &["platform:agents.own"])).unwrap_err();
    assert!(
        matches!(err, ConfigError::NotAnIdentity { index: 0, .. }),
        "{err}"
    );
}

#[test]
fn the_wire_shape_is_camel_case_and_refuses_an_unknown_key() {
    let ok: WorkloadClients = serde_yaml::from_str(
        "clients:\n  - spiffeId: spiffe://another/ns/test/sa/orchestrator\n    permissions: [platform:agents.own]\n",
    )
    .expect("the documented shape");
    assert_eq!(ok.clients.len(), 1);
    assert_eq!(ok.clients[0].permissions, ["platform:agents.own"]);
    for text in [
        // a misspelt key inside an entry
        "clients:\n  - spiffeId: spiffe://x\n    permission: [platform:agents.own]\n",
        // the snake_case spelling
        "clients:\n  - spiffe_id: spiffe://x\n",
        // a key of the block
        "clients: []\nextra: true\n",
    ] {
        assert!(
            serde_yaml::from_str::<WorkloadClients>(text).is_err(),
            "{text:?} was accepted"
        );
    }
}

#[test]
fn a_comment_between_pem_blocks_is_skipped() {
    let table = one(
        "spiffe://another/ns/test/sa/orchestrator",
        &["platform:agents.own"],
    );
    let bundle = format!("# the agents CA\n{CA}\n# end\n");
    assert!(WorkloadAuth::new(&bundle, &table).is_ok());
}
