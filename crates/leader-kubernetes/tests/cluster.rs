//! The election against a real API server: the conformance suite of `aap-ports`, with the default timings (a
//! lease of 15 s, a renewal of 10, a retry of 2), so the takeover of a killed holder takes the 15 seconds a
//! cluster takes and not a fake's three.
//!
//! **Opt in.** The cases make leases named `aap-leader-<hex>` in the namespace `aap-test`; they run only when
//! `AAP_TEST_KUBECONFIG` names a kubeconfig file (never the default context: a developer's cluster is not a
//! fixture by accident), and the user it names needs `create`, `get` and `update` on `leases` there. Without it
//! they skip, and with `AAP_TEST_REQUIRE_CLUSTER=1` (CI) skipping is a failure. A throwaway cluster is the
//! point: nothing is cleaned up but what a case deletes itself, and a lease that is left expires.
//!
//! ```sh
//! kind create cluster --name aap
//! kubectl create namespace aap-test
//! AAP_TEST_KUBECONFIG=$HOME/.kube/config cargo test -p aap-leader-kubernetes --test cluster
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use aap_leader_kubernetes::LeaseElection;
use aap_ports::CandidateId;
use aap_ports::testkit::unique;
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};

const KUBECONFIG_VAR: &str = "AAP_TEST_KUBECONFIG";
const REQUIRE_VAR: &str = "AAP_TEST_REQUIRE_CLUSTER";

/// The namespace the cases live in.
const NAMESPACE: &str = "aap-test";

fn required() -> bool {
    matches!(std::env::var(REQUIRE_VAR).as_deref(), Ok("1" | "true"))
}

/// The client of the throwaway cluster, or `None` (with a note, or a panic under `AAP_TEST_REQUIRE_CLUSTER=1`).
async fn client() -> Option<Client> {
    let Some(path) = std::env::var(KUBECONFIG_VAR)
        .ok()
        .filter(|path| !path.is_empty())
    else {
        assert!(
            !required(),
            "{KUBECONFIG_VAR} is not set, and {REQUIRE_VAR}=1 forbids skipping"
        );
        eprintln!("skipped: {KUBECONFIG_VAR} is not set");
        return None;
    };
    let kubeconfig = Kubeconfig::read_from(&path).unwrap_or_else(|error| panic!("{path}: {error}"));
    let config = Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default())
        .await
        .unwrap_or_else(|error| panic!("{path}: {error}"));
    Some(Client::try_from(config).unwrap_or_else(|error| panic!("{path}: {error}")))
}

/// One candidate of a fresh election, with the default timings.
async fn make() -> Option<LeaseElection> {
    let client = client().await?;
    Some(LeaseElection::new(
        client,
        NAMESPACE,
        &unique("aap-leader"),
        CandidateId::new(unique("candidate")),
    ))
}
aap_ports::leader_election_conformance!(make);
