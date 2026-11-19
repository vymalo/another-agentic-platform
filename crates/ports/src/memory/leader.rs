//! A [`LeaderElection`](crate::LeaderElection) that really competes, in memory: one holder, a renewal time and
//! the same timings a lease has, so that the conformance suite exercises a takeover and a demotion rather than
//! a constant.
//!
//! It is not a simulation of an API server: [`kill`](crate::testkit::LeaderElectionUnderTest::kill) drops the
//! holder outright (there is no lease left to expire), and every candidate of one election shares one `Mutex`
//! and one clock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use tokio::sync::watch;

use crate::CandidateId;
use crate::leader::{LeaderElection, Leadership};

/// The timings of an in-memory election. The defaults are short enough that a suite of cases takes a second
/// or two, and long enough that a follower's attempts and a holder's renewals interleave.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryTimings {
    /// How long a holder may go without renewing before another candidate may take over.
    pub lease: Duration,
    /// How often the holder renews.
    pub renew: Duration,
    /// How often a follower tries.
    pub retry: Duration,
}

impl Default for MemoryTimings {
    fn default() -> Self {
        Self {
            lease: Duration::from_millis(600),
            renew: Duration::from_millis(200),
            retry: Duration::from_millis(50),
        }
    }
}

struct Shared {
    /// Who holds it, by candidate name.
    holder: Option<String>,
    /// When the holder last renewed, on our clock.
    renewed: Option<tokio::time::Instant>,
    timings: MemoryTimings,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One candidate of an in-memory election. Clones are the same candidate (they share the state, the killed flag
/// and the channel of its leadership changes); [`candidate`](crate::testkit::LeaderElectionUnderTest::candidate)
/// is a second one.
#[derive(Clone)]
pub struct MemoryElection {
    /// The election this candidate is in. Two elections of different names never share a holder.
    election: String,
    shared: Arc<Mutex<Shared>>,
    candidate: CandidateId,
    killed: Arc<AtomicBool>,
    leadership: watch::Sender<Leadership>,
}

impl MemoryElection {
    /// A candidate `candidate` of the election `election`.
    pub fn new(election: &str, candidate: &str) -> Self {
        Self::with_timings(election, candidate, MemoryTimings::default())
    }

    /// [`new`](Self::new) with the timings given.
    pub fn with_timings(election: &str, candidate: &str, timings: MemoryTimings) -> Self {
        Self {
            election: election.to_owned(),
            shared: Arc::new(Mutex::new(Shared {
                holder: None,
                renewed: None,
                timings,
            })),
            candidate: CandidateId::new(candidate),
            killed: Arc::new(AtomicBool::new(false)),
            leadership: watch::channel(Leadership::Follower).0,
        }
    }

    /// The name of the election this candidate is in.
    pub fn election(&self) -> &str {
        &self.election
    }

    /// Publish this candidate's leadership, and only when it changed (a stream of transitions, not of polls).
    fn publish(&self, value: Leadership) {
        if *self.leadership.borrow() != value {
            self.leadership.send_replace(value);
        }
    }

    fn release(&self) {
        let mut s = lock(&self.shared);
        if s.holder.as_deref() == Some(self.candidate.as_str()) {
            s.holder = None;
            s.renewed = None;
        }
        drop(s);
        self.publish(Leadership::Follower);
    }

    /// Try to hold the election now: keep it if it is ours, take it if nobody holds it or its renewal is older
    /// than the lease.
    fn try_hold(&self) -> bool {
        let now = tokio::time::Instant::now();
        let mut s = lock(&self.shared);
        let lease = s.timings.lease;
        let mine = s.holder.as_deref() == Some(self.candidate.as_str());
        let expired = s
            .renewed
            .is_none_or(|renewed| now.duration_since(renewed) >= lease);
        if mine || s.holder.is_none() || expired {
            s.holder = Some(self.candidate.as_str().to_owned());
            s.renewed = Some(now);
            true
        } else {
            false
        }
    }

    async fn campaign(self, shutdown: BoxFuture<'static, ()>) {
        tokio::pin!(shutdown);
        loop {
            if self.killed.load(Ordering::Acquire) {
                self.publish(Leadership::Follower);
                return;
            }
            let held = self.try_hold();
            self.publish(if held {
                Leadership::Leader
            } else {
                Leadership::Follower
            });
            let wait = {
                let s = lock(&self.shared);
                if held {
                    s.timings.renew
                } else {
                    s.timings.retry
                }
            };
            tokio::select! {
                () = &mut shutdown => {
                    self.release();
                    return;
                }
                () = tokio::time::sleep(wait) => {}
            }
        }
    }
}

impl LeaderElection for MemoryElection {
    fn candidate(&self) -> &CandidateId {
        &self.candidate
    }

    fn is_leader(&self) -> bool {
        self.leadership.borrow().is_leader()
    }

    fn run(&self, shutdown: BoxFuture<'static, ()>) -> BoxFuture<'static, ()> {
        let this = self.clone();
        Box::pin(async move { this.campaign(shutdown).await })
    }

    fn following(&self) -> BoxStream<'static, Leadership> {
        // The value now, then each change: `borrow_and_update` marks it seen, so the next `changed` is a real
        // transition.
        let rx = self.leadership.subscribe();
        futures::stream::unfold((rx, true), |(mut rx, first)| async move {
            if !first && rx.changed().await.is_err() {
                return None;
            }
            let value = *rx.borrow_and_update();
            Some((value, (rx, false)))
        })
        .boxed()
    }
}

impl crate::testkit::LeaderElectionUnderTest for MemoryElection {
    fn candidate(&self, id: &str) -> Self {
        Self {
            election: self.election.clone(),
            shared: self.shared.clone(),
            candidate: CandidateId::new(id),
            killed: Arc::new(AtomicBool::new(false)),
            leadership: watch::channel(Leadership::Follower).0,
        }
    }

    fn another(&self) -> Self {
        let timings = lock(&self.shared).timings;
        Self::with_timings(
            &crate::testkit::unique("election"),
            self.candidate.as_str(),
            timings,
        )
    }

    async fn holder(&self) -> Option<String> {
        lock(&self.shared).holder.clone()
    }

    async fn kill(&self) {
        self.killed.store(true, Ordering::Release);
        self.release();
    }
}
