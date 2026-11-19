//! Leader election (K7a, AD-020): the seam that lets several replicas of the operator run, of which **one**
//! holds the election and writes.
//!
//! Two replicas both keep their caches, serve `/healthz`, `/readyz`, `/metrics` and the registry; only the
//! lease holder's reconcile passes write. The seam says who holds it ([`LeaderElection::is_leader`]), runs the
//! campaign ([`LeaderElection::run`]) and reports every change of it ([`LeaderElection::following`]).
//!
//! [`AlwaysLeader`] is the one-replica case, and the default of the operator: nothing is asked of the cluster,
//! and every pass writes. The lease-backed implementation is `aap-leader-kubernetes` (a
//! `coordination.k8s.io/v1` `Lease`); with the feature `testkit` this crate also has the conformance suite
//! both must pass and an in-memory election that really competes.

use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;

use crate::CandidateId;

/// Whether a candidate holds the election now, as far as it knows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Leadership {
    /// This candidate holds the lease: its passes write.
    Leader,
    /// Another candidate holds it, or nobody does yet: this one's passes write nothing.
    #[default]
    Follower,
}

impl Leadership {
    /// `true` for [`Leadership::Leader`].
    pub fn is_leader(self) -> bool {
        matches!(self, Self::Leader)
    }
}

impl std::fmt::Display for Leadership {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Leader => "leader",
            Self::Follower => "follower",
        })
    }
}

/// Who holds one election (K7a, AD-020).
///
/// The three questions a controller asks, and the one call a composition root makes:
///
/// * [`candidate`](Self::candidate) is this process's identity, written as the lease's holder;
/// * [`is_leader`](Self::is_leader) is **a hint, not a lock**: a pass that reads it and then writes may have
///   lost the election in between, and the election's own design (a lease that expires) is what keeps two
///   writers apart;
/// * [`run`](Self::run) campaigns until the shutdown resolves — it renews while it holds, tries while it does
///   not, and gives the lease up when it is told to stop;
/// * [`following`](Self::following) is every change of this candidate's leadership, the current one first.
///
/// Object-safe on purpose (boxed futures, no generic method): a composition root holds one
/// `Arc<dyn LeaderElection>` and hands it to the operator, which names no implementation.
pub trait LeaderElection: Send + Sync {
    /// This candidate's identity: what it writes as the lease's holder.
    fn candidate(&self) -> &CandidateId;

    /// Whether this candidate holds the election **now**, as far as it knows.
    fn is_leader(&self) -> bool;

    /// Campaign until `shutdown` resolves: renew, try, take over an expired lease, and release on the way out.
    /// Polling the returned future is what runs the election; dropping it stops the campaign without the
    /// release.
    fn run(&self, shutdown: BoxFuture<'static, ()>) -> BoxFuture<'static, ()>;

    /// Every change of this candidate's leadership, **the current one first**, then each transition.
    fn following(&self) -> BoxStream<'static, Leadership>;
}

/// The election of a single replica: this candidate always holds it, and [`run`](LeaderElection::run) waits
/// for the shutdown. What the operator uses when leader election is off.
#[derive(Clone, Debug)]
pub struct AlwaysLeader {
    candidate: CandidateId,
}

impl Default for AlwaysLeader {
    /// The candidate `operator`: a name for a process that is the only one, used by tests and by the
    /// composition root when it has no pod name.
    fn default() -> Self {
        Self::new(CandidateId::new("operator"))
    }
}

impl AlwaysLeader {
    /// An election this candidate always holds.
    pub fn new(candidate: CandidateId) -> Self {
        Self { candidate }
    }
}

impl LeaderElection for AlwaysLeader {
    fn candidate(&self) -> &CandidateId {
        &self.candidate
    }

    fn is_leader(&self) -> bool {
        true
    }

    fn run(&self, shutdown: BoxFuture<'static, ()>) -> BoxFuture<'static, ()> {
        shutdown
    }

    fn following(&self) -> BoxStream<'static, Leadership> {
        futures::stream::once(async { Leadership::Leader }).boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_always_leader_holds_from_the_first_question() {
        let election = AlwaysLeader::new(CandidateId::new("pod-1"));
        assert_eq!(election.candidate().as_str(), "pod-1");
        assert!(election.is_leader());
        let mut changes = election.following();
        assert_eq!(changes.next().await, Some(Leadership::Leader));
        assert_eq!(
            changes.next().await,
            None,
            "one value, and the stream is done"
        );
    }

    #[tokio::test]
    async fn the_always_leader_runs_until_the_shutdown() {
        let election = AlwaysLeader::default();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let run = election.run(Box::pin(async move {
            let _ = stopped.await;
        }));
        let mut run = std::pin::pin!(run);
        assert!(
            futures::poll!(&mut run).is_pending(),
            "it does not end before the shutdown"
        );
        let _ = stop.send(());
        run.await;
    }

    #[test]
    fn leadership_has_one_word_for_each_and_none_for_the_other() {
        assert!(Leadership::Leader.is_leader());
        assert!(!Leadership::Follower.is_leader());
        assert_eq!(Leadership::default(), Leadership::Follower);
        assert_eq!(Leadership::Leader.to_string(), "leader");
        assert_eq!(Leadership::Follower.to_string(), "follower");
    }
}
