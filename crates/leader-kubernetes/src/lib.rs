//! Leader election on Kubernetes (K7a, AD-020): a `coordination.k8s.io/v1` `Lease` that one replica of the
//! operator holds, renews and releases, and that the others take over when it expires.
//!
//! # The loop
//!
//! Every [`Timings::retry`] a follower, and every [`Timings::renew`] a holder, one call:
//!
//! 1. read the lease `namespace/name` ([`LeaseElection::new`]);
//! 2. it is not there: create it with this candidate as `holderIdentity`, `leaseTransitions: 0` and
//!    `acquireTime`/`renewTime` now — a `409` means somebody else created it in between, and this candidate is
//!    a follower;
//! 3. it is ours: renew it (`renewTime` now, the lease duration of these timings) — a `409` means somebody took
//!    it from us, and this candidate is a follower **at once**;
//! 4. it is somebody else's and **expired** (`renewTime + leaseDuration < now`, on our clock):
//!    take it over — `holderIdentity` us, `acquireTime`/`renewTime` now, `leaseTransitions` one more — and a
//!    `409` again leaves a follower;
//! 5. otherwise: a follower.
//!
//! A holder that has not written for [`Timings::lease`] minus [`Timings::retry`] **demotes itself**, so it stops
//! claiming the election before another candidate's expiry check can let it take over. That margin is what keeps
//! two writers apart: `retry < renew < lease` (which [`Timings::check`] holds) makes the deadline earlier than
//! any takeover by at least `retry`.
//!
//! On the shutdown the holder clears `holderIdentity`, best effort and within two seconds: a stopped replica
//! does not make the next one wait for the lease to expire. The lease is never deleted, and no Secret is read.
//!
//! The clock is **ours**, not the API server's: the lease's times are written from the local clock and compared
//! with it, as `client-go` does. Two replicas whose clocks differ by more than the margin can overlap; that is
//! the deployment's problem to avoid, and the same one every Kubernetes leader election has.

#![warn(missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aap_ports::{CandidateId, LeaderElection, Leadership};
use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use kube::Client;
use kube::api::{Api, PostParams};
use tokio::sync::watch;

/// How long one lease lives, how often its holder renews it, and how often a follower tries (K7a).
///
/// The defaults are the ones `client-go` uses (15, 10 and 2 seconds). [`check`](Self::check) holds the one
/// invariant the loop relies on: `retry < renew < lease`, each at least a second.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timings {
    /// `leaseDurationSeconds`: how long a holder may go without renewing before another candidate may take the
    /// lease over.
    pub lease: Duration,
    /// How often the holder renews.
    pub renew: Duration,
    /// How often a follower tries, and how soon a holder that failed to renew looks again.
    pub retry: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            lease: Duration::from_secs(15),
            renew: Duration::from_secs(10),
            retry: Duration::from_secs(2),
        }
    }
}

impl Timings {
    /// Refuse timings the loop cannot honour, naming what is wrong: `retry < renew < lease`, each at least a
    /// second. A lease of no second, or a retry as long as a renewal, would let two candidates write at once.
    ///
    /// # Errors
    ///
    /// The timings do not satisfy that.
    pub fn check(&self) -> Result<(), String> {
        for (name, value) in [
            ("lease", self.lease),
            ("renew", self.renew),
            ("retry", self.retry),
        ] {
            if value < Duration::from_secs(1) {
                return Err(format!(
                    "{name} is {value:?}: the lease, the renewal and the retry are at least 1 s each"
                ));
            }
        }
        if !(self.retry < self.renew && self.renew < self.lease) {
            return Err(format!(
                "the timings must satisfy retry < renew < lease, and {:?} < {:?} < {:?} does not",
                self.retry, self.renew, self.lease
            ));
        }
        Ok(())
    }

    /// The moment after which a holder that has not written demotes itself: before any other candidate's expiry
    /// check may let it take the lease over, by [`Timings::retry`].
    fn deadline(&self) -> Duration {
        self.lease.saturating_sub(self.retry)
    }

    /// `leaseDurationSeconds`, as the lease holds it.
    fn lease_seconds(&self) -> i32 {
        i32::try_from(self.lease.as_secs()).unwrap_or(i32::MAX)
    }
}

/// What one call of the loop found.
enum Hold {
    /// The lease was written: this candidate holds it.
    Acquired,
    /// The lease is somebody else's and has not expired: this candidate is a follower.
    NotMine,
    /// A `409`: somebody else wrote the lease between our read and our write. This candidate is a follower, now.
    Lost,
}

/// How long the release on shutdown may take (best effort: the lease expires on its own after that).
const RELEASE: Duration = Duration::from_secs(2);

/// One candidate of an election that is a `coordination.k8s.io/v1` `Lease`. Clones are the same candidate; a
/// second replica is [`LeaseElection::new`] with another [`CandidateId`] (and, in tests,
/// [`LeaderElectionUnderTest::candidate`](aap_ports::testkit::LeaderElectionUnderTest::candidate)).
#[derive(Clone)]
pub struct LeaseElection {
    api: Api<Lease>,
    name: String,
    candidate: CandidateId,
    timings: Timings,
    killed: Arc<AtomicBool>,
    leadership: watch::Sender<Leadership>,
}

impl std::fmt::Debug for LeaseElection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeaseElection")
            .field("lease", &self.name)
            .field("candidate", &self.candidate)
            .field("timings", &self.timings)
            .finish_non_exhaustive()
    }
}

impl LeaseElection {
    /// A candidate of the election of the `Lease` `namespace/name`, through `client`.
    pub fn new(client: Client, namespace: &str, name: &str, candidate: CandidateId) -> Self {
        Self {
            api: Api::namespaced(client, namespace),
            name: name.to_owned(),
            candidate,
            timings: Timings::default(),
            killed: Arc::new(AtomicBool::new(false)),
            leadership: watch::channel(Leadership::Follower).0,
        }
    }

    /// The timings, after [`Timings::check`].
    ///
    /// # Errors
    ///
    /// The timings do not satisfy `retry < renew < lease`, each at least a second.
    pub fn with_timings(mut self, timings: Timings) -> Result<Self, String> {
        timings.check()?;
        self.timings = timings;
        Ok(self)
    }

    /// The timings this candidate campaigns with.
    pub fn timings(&self) -> &Timings {
        &self.timings
    }

    /// The name of the lease, in the namespace the API is for.
    pub fn lease(&self) -> &str {
        &self.name
    }

    /// Publish this candidate's leadership, and only when it changed.
    fn publish(&self, value: Leadership) {
        if *self.leadership.borrow() == value {
            return;
        }
        self.leadership.send_replace(value);
        tracing::debug!(
            lease = self.name,
            candidate = self.candidate.as_str(),
            leadership = %value,
            "the leader election changed hands"
        );
    }

    /// One call of the loop: read, then create, renew or take over.
    async fn hold(&self) -> Result<Hold, kube::Error> {
        let Some(current) = self.api.get_opt(&self.name).await? else {
            return self.create().await;
        };
        let spec = current.spec.clone().unwrap_or_default();
        let now = now();
        if spec.holder_identity.as_deref() == Some(self.candidate.as_str()) {
            let renewed = self.renewed_spec(&spec, now);
            return self.write(&current, renewed).await;
        }
        // A lease nobody holds is free at once: a replica that stopped released it, and there is no reason to wait
        // for the renewal time it wrote on the way out to pass.
        let free = spec
            .holder_identity
            .as_deref()
            .is_none_or(|holder| holder.is_empty());
        if free || self.expired(&spec, &now) {
            let acquired = self.acquired_spec(&spec, now);
            return self.write(&current, acquired).await;
        }
        Ok(Hold::NotMine)
    }

    /// The lease does not exist yet: create it, with this candidate as its first holder.
    async fn create(&self) -> Result<Hold, kube::Error> {
        let now = now();
        let lease = Lease {
            metadata: ObjectMeta {
                name: Some(self.name.clone()),
                ..ObjectMeta::default()
            },
            spec: Some(LeaseSpec {
                acquire_time: Some(now.clone()),
                holder_identity: Some(self.candidate.as_str().to_owned()),
                lease_duration_seconds: Some(self.timings.lease_seconds()),
                lease_transitions: Some(0),
                preferred_holder: None,
                renew_time: Some(now),
                strategy: None,
            }),
        };
        match self.api.create(&PostParams::default(), &lease).await {
            Ok(_) => Ok(Hold::Acquired),
            Err(error) if conflict(&error) => Ok(Hold::Lost),
            Err(error) => Err(error),
        }
    }

    /// Write `spec` over the lease we read, with the `resourceVersion` we read it at.
    async fn write(&self, current: &Lease, spec: LeaseSpec) -> Result<Hold, kube::Error> {
        let mut object = current.clone();
        object.spec = Some(spec);
        match self
            .api
            .replace(&self.name, &PostParams::default(), &object)
            .await
        {
            Ok(_) => Ok(Hold::Acquired),
            Err(error) if conflict(&error) => Ok(Hold::Lost),
            Err(error) => Err(error),
        }
    }

    /// The lease as a renewal leaves it: ours, renewed now, everything else as it was.
    fn renewed_spec(&self, spec: &LeaseSpec, now: MicroTime) -> LeaseSpec {
        LeaseSpec {
            acquire_time: spec.acquire_time.clone().or_else(|| Some(now.clone())),
            holder_identity: Some(self.candidate.as_str().to_owned()),
            lease_duration_seconds: Some(self.timings.lease_seconds()),
            lease_transitions: spec.lease_transitions,
            preferred_holder: spec.preferred_holder.clone(),
            renew_time: Some(now),
            strategy: spec.strategy.clone(),
        }
    }

    /// The lease as a takeover leaves it: ours, acquired now, and one transition more.
    fn acquired_spec(&self, spec: &LeaseSpec, now: MicroTime) -> LeaseSpec {
        LeaseSpec {
            acquire_time: Some(now.clone()),
            holder_identity: Some(self.candidate.as_str().to_owned()),
            lease_duration_seconds: Some(self.timings.lease_seconds()),
            lease_transitions: Some(spec.lease_transitions.unwrap_or(0).saturating_add(1)),
            preferred_holder: None,
            renew_time: Some(now),
            strategy: spec.strategy.clone(),
        }
    }

    /// Whether somebody else's lease may be taken over: its renewal plus its duration is behind `now`, or it
    /// names no holder and no renewal at all.
    fn expired(&self, spec: &LeaseSpec, now: &MicroTime) -> bool {
        let Some(renewed) = &spec.renew_time else {
            return true;
        };
        let lease = spec
            .lease_duration_seconds
            .and_then(|seconds| u64::try_from(seconds).ok())
            .map_or(self.timings.lease, Duration::from_secs);
        let micros = i64::try_from(lease.as_micros()).unwrap_or(i64::MAX);
        renewed.0.as_microsecond().saturating_add(micros) < now.0.as_microsecond()
    }

    /// Clear `holderIdentity`, if the lease is ours. Best effort and bounded: the lease expires anyway.
    async fn release(&self) {
        let work = async {
            let Ok(Some(current)) = self.api.get_opt(&self.name).await else {
                return;
            };
            let spec = current.spec.clone().unwrap_or_default();
            if spec.holder_identity.as_deref() != Some(self.candidate.as_str()) {
                return;
            }
            let mut object = current.clone();
            object.spec = Some(LeaseSpec {
                holder_identity: None,
                renew_time: Some(now()),
                ..spec
            });
            match self
                .api
                .replace(&self.name, &PostParams::default(), &object)
                .await
            {
                Ok(_) => {
                    tracing::info!(lease = self.name, "the leader election released the lease");
                }
                Err(error) => tracing::warn!(
                    lease = self.name,
                    "the leader election could not release the lease, which expires on its own: {error}"
                ),
            }
        };
        if tokio::time::timeout(RELEASE, work).await.is_err() {
            tracing::warn!(
                lease = self.name,
                "the leader election did not release the lease within {RELEASE:?}: it expires on its own"
            );
        }
    }

    async fn campaign(self, shutdown: BoxFuture<'static, ()>) {
        tokio::pin!(shutdown);
        let mut written: Option<tokio::time::Instant> = None;
        loop {
            if self.killed.load(Ordering::Acquire) {
                // A killed process renews nothing and releases nothing: the lease expires.
                self.publish(Leadership::Follower);
                return;
            }
            match self.hold().await {
                Ok(Hold::Acquired) => {
                    written = Some(tokio::time::Instant::now());
                    self.publish(Leadership::Leader);
                }
                Ok(Hold::Lost | Hold::NotMine) => {
                    self.publish(Leadership::Follower);
                    written = None;
                }
                Err(error) => {
                    // The API server cannot be reached: keep what we believe until the lease we wrote is close
                    // enough to expiring that another candidate may take it.
                    let stale = written.is_some_and(|at| at.elapsed() >= self.timings.deadline());
                    if stale || written.is_none() {
                        self.publish(Leadership::Follower);
                    }
                    if stale {
                        written = None;
                    }
                    tracing::warn!(
                        lease = self.name,
                        "the leader election could not reach the API server, and tries again: {error}"
                    );
                }
            }
            let wait = if self.is_leader() {
                self.timings.renew
            } else {
                self.timings.retry
            };
            tokio::select! {
                () = &mut shutdown => {
                    self.release().await;
                    self.publish(Leadership::Follower);
                    return;
                }
                () = tokio::time::sleep(wait) => {}
            }
        }
    }
}

impl LeaderElection for LeaseElection {
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

#[cfg(feature = "testkit")]
impl aap_ports::testkit::LeaderElectionUnderTest for LeaseElection {
    fn candidate(&self, id: &str) -> Self {
        Self {
            api: self.api.clone(),
            name: self.name.clone(),
            candidate: CandidateId::new(id),
            timings: self.timings,
            killed: Arc::new(AtomicBool::new(false)),
            leadership: watch::channel(Leadership::Follower).0,
        }
    }

    fn another(&self) -> Self {
        Self {
            api: self.api.clone(),
            name: format!("{}-{}", self.name, aap_ports::testkit::unique("e")),
            candidate: CandidateId::new(self.candidate.as_str()),
            timings: self.timings,
            killed: Arc::new(AtomicBool::new(false)),
            leadership: watch::channel(Leadership::Follower).0,
        }
    }

    async fn holder(&self) -> Option<String> {
        self.api
            .get_opt(&self.name)
            .await
            .ok()
            .flatten()
            .and_then(|lease| lease.spec)
            .and_then(|spec| spec.holder_identity)
            .filter(|holder| !holder.is_empty())
    }

    async fn kill(&self) {
        // There is no process to kill from here: the campaign of this handle stops renewing, and the lease it
        // wrote is left to expire, as a killed replica's is.
        self.killed.store(true, Ordering::Release);
    }
}

/// Now, on our clock, as the lease writes it.
fn now() -> MicroTime {
    MicroTime(k8s_openapi::jiff::Timestamp::now())
}

/// Whether an API server refused a write because the object moved under us.
fn conflict(error: &kube::Error) -> bool {
    matches!(error, kube::Error::Api(response) if response.is_conflict())
}
