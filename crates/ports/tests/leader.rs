//! The leader election suite against the in-memory election (K7a): a real competition, a takeover and a
//! release. A lease on an API server runs the same macro in `aap-leader-kubernetes`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use aap_ports::memory::MemoryElection;
use aap_ports::testkit::LeaderElectionUnderTest;
use aap_ports::{CandidateId, LeaderElection, Leadership};
use futures::future::BoxFuture;
use futures::stream::BoxStream;

async fn make() -> Option<MemoryElection> {
    Some(MemoryElection::new(
        &aap_ports::testkit::unique("election"),
        &aap_ports::testkit::unique("a"),
    ))
}
aap_ports::leader_election_conformance!(make);

/// A broken election: it delegates everything to a real `MemoryElection` except `is_leader`, which always says
/// yes. The suite must catch it, or it proves nothing.
#[derive(Clone)]
struct AlwaysLeader(MemoryElection);

impl LeaderElection for AlwaysLeader {
    fn candidate(&self) -> &CandidateId {
        LeaderElection::candidate(&self.0)
    }

    fn is_leader(&self) -> bool {
        true
    }

    fn run(&self, shutdown: BoxFuture<'static, ()>) -> BoxFuture<'static, ()> {
        self.0.run(shutdown)
    }

    fn following(&self) -> BoxStream<'static, Leadership> {
        self.0.following()
    }
}

impl LeaderElectionUnderTest for AlwaysLeader {
    fn candidate(&self, id: &str) -> Self {
        Self(LeaderElectionUnderTest::candidate(&self.0, id))
    }

    fn another(&self) -> Self {
        Self(self.0.another())
    }

    fn holder(&self) -> impl Future<Output = Option<String>> + Send {
        self.0.holder()
    }

    fn kill(&self) -> impl Future<Output = ()> + Send {
        self.0.kill()
    }
}

/// The suite is not vacuous: a candidate that always claims leadership fails the case that says the loser writes
/// nothing.
#[tokio::test]
#[should_panic]
async fn the_suite_fails_a_candidate_that_always_claims_leadership() {
    let election = AlwaysLeader(MemoryElection::new(
        &aap_ports::testkit::unique("election"),
        &aap_ports::testkit::unique("a"),
    ));
    aap_ports::testkit::leader::the_loser_writes_nothing(election).await;
}
