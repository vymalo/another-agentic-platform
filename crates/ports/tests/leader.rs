//! The leader election suite against the in-memory election (K7a): a real competition, a takeover and a
//! release. A lease on an API server runs the same macro in `aap-leader-kubernetes`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aap_ports::memory::MemoryElection;

async fn make() -> Option<MemoryElection> {
    Some(MemoryElection::new(
        &aap_ports::testkit::unique("election"),
        &aap_ports::testkit::unique("a"),
    ))
}
aap_ports::leader_election_conformance!(make);
