//! The meter's durable facts (ANR-1198, AD-069 amended): a **tombstone** is what the meter knows of a pod that has
//! vanished while some of its minutes are not known delivered. It is written once, when the pod is found gone, and
//! deleted when its minutes are acknowledged (or when the 24 h backfill bound has passed). A new leader reads the
//! tombstones when it starts to lead and sends the minutes again, with the same `event_id`s.
//!
//! [`TombstoneStore`] is the seam. The adapter for a Kubernetes `ConfigMap` is in `aap-controller`; with the feature
//! `testkit` this crate has [`MemoryTombstones`](crate::MemoryTombstones) and the conformance suite
//! [`testkit::tombstones`](crate::testkit::tombstones).

use std::future::Future;

use crate::PodKind;

/// The most tombstones the meter keeps in one namespace. A tombstone is a few hundred bytes (every text field is at
/// most 128 bytes), so this stays far under the 1 MiB a `ConfigMap` may hold. **At the bound a new tombstone is not
/// written** (the pod is still metered, from memory, as without a store) and the meter counts it
/// (`aap_meter_tombstones_unbacked_total`): metering never stops for want of room.
pub const MAX_TOMBSTONES: usize = 512;

/// What the meter knew of one vanished pod. Self-contained: replaying it needs no pod, and no account lookup when
/// `subject` is known.
#[derive(Clone, PartialEq, Eq)]
pub struct Tombstone {
    /// The pod's namespace: where the tombstone is kept.
    pub namespace: String,
    /// The pod's `metadata.uid`: the key, and what the minutes' `event_id`s are made from.
    pub uid: String,
    /// The agent service it belonged to.
    pub service: String,
    /// The run it served: present for a run pod and for no other.
    pub run: Option<String>,
    /// What the pod was.
    pub pod_kind: PodKind,
    /// The account of its minutes, when it was known. `None`: ask the account source again.
    pub subject: Option<String>,
    /// The pod's weight, in quarter-units.
    pub weight_quarters: u32,
    /// The node type it ran on.
    pub node_type: String,
    /// When its app containers started (seconds since the Unix epoch).
    pub started_at: i64,
    /// When it is counted to have ended: the exclusive end the meter closed it at (seconds since the Unix epoch).
    pub ended_at: i64,
    /// The first minute not known delivered when the tombstone was written.
    pub first_minute: u64,
    /// When it was written (seconds since the Unix epoch): the 24 h bound counts from here.
    pub written_at: i64,
}

impl std::fmt::Debug for Tombstone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tombstone")
            .field("namespace", &self.namespace)
            .field("uid", &self.uid)
            .field("service", &self.service)
            .field("run", &self.run)
            .field("pod_kind", &self.pod_kind)
            .field("subject", &self.subject.as_ref().map(|_| "<redacted>"))
            .field("weight_quarters", &self.weight_quarters)
            .field("node_type", &self.node_type)
            .field("started_at", &self.started_at)
            .field("ended_at", &self.ended_at)
            .field("first_minute", &self.first_minute)
            .field("written_at", &self.written_at)
            .finish()
    }
}

/// Why a store call failed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TombstoneError {
    /// The store could not be read or written now. Nothing changed; the meter holds and asks again.
    #[error("the tombstone store is unavailable")]
    Unavailable,
    /// The set is more than the store can hold. Nothing changed.
    #[error("the tombstones do not fit in the store")]
    TooLarge,
    /// The stored set changed since the caller read it (another writer, or it was deleted). Nothing changed; the
    /// caller reads again before it writes.
    #[error("the stored tombstones changed since they were read")]
    Conflict,
}

/// Where the meter keeps its tombstones. One **set** per namespace; the meter writes only from the leader, and a
/// store must refuse a write made over a set that changed since it was read (an overlapping writer), as
/// [`TombstoneError::Conflict`], so that the meter reads again.
pub trait TombstoneStore: Send + Sync {
    /// The tombstones of `scope`: one namespace, or every namespace when `scope` is empty. Never a tombstone of
    /// another namespace; an absent set is empty.
    fn load(
        &self,
        scope: &str,
    ) -> impl Future<Output = Result<Vec<Tombstone>, TombstoneError>> + Send;

    /// Replace the whole set of `namespace` with `set` (an empty set clears it). Atomic: on an error the stored set
    /// is what it was. The caller has read the set in this leadership term, so a replacement does not lose what
    /// another leader wrote.
    fn save(
        &self,
        namespace: &str,
        set: &[Tombstone],
    ) -> impl Future<Output = Result<(), TombstoneError>> + Send;
}
