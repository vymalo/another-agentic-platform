//! The usage ingest's port (AD-060): a pod-minute event, taken in **once**, and what the ledger reads back.
//!
//! The meter reports one [`PodMinuteEvent`] per pod and minute. [`UsageStore::insert`] takes it idempotently on
//! `(source, id)` and says whether it was new. [`UsageStore::spend`] answers, per account and node type, the
//! weighted pod-minutes **ingested** in a half-open window `[from, to)`. The store records no amount and
//! knows no rate: pricing is the reader's.
//!
//! With the feature `testkit` this crate also has [`testkit`], the conformance suite every adapter must pass, and
//! [`MemoryUsageStore`], the fake that passes it.

use std::future::Future;

use sha2::{Digest, Sha256};

#[cfg(feature = "testkit")]
pub mod testkit;

#[cfg(feature = "testkit")]
mod memory;
#[cfg(feature = "testkit")]
pub use memory::MemoryUsageStore;

/// The CloudEvents `type` of a pod-minute event.
pub const EVENT_TYPE: &str = "com.vymalo.agents.pod-minute.v1";

const MAX_TEXT: usize = 128;

/// One pod, one minute of it. Fields are what the contract (`docs/api/usage-ingest-v1.md`) calls them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodMinuteEvent {
    /// The meter instance that reported it (CloudEvents `source`).
    pub source: String,
    /// [`event_id`] of the pod's uid and the minute (CloudEvents `id`); with `source`, the idempotency key.
    pub id: String,
    /// The account the minute is for (CloudEvents `subject`).
    pub subject: String,
    /// Weighted units of this minute (`data.weight`), at least 1.
    pub weight: u32,
    /// The node type the pod ran on (`data.nodeType`).
    pub node_type: String,
    /// What the pod was (`data.podKind`), for example `agent` or `run`.
    pub pod_kind: String,
    /// The agent service it belongs to (`data.agentService`).
    pub agent_service: String,
    /// The run it served, when it served one (`data.run`).
    pub run: Option<String>,
}

impl PodMinuteEvent {
    /// Checks every field against the contract: a closed alphabet (`[A-Za-z0-9._:/-]`), at most 128 bytes, no
    /// empty text and a weight of at least 1, so free text and personal data cannot pass.
    pub fn validate(&self) -> Result<(), UsageError> {
        let texts: [(&'static str, &str); 6] = [
            ("source", &self.source),
            ("id", &self.id),
            ("subject", &self.subject),
            ("nodeType", &self.node_type),
            ("podKind", &self.pod_kind),
            ("agentService", &self.agent_service),
        ];
        for (field, value) in texts {
            check_text(field, value)?;
        }
        if let Some(run) = &self.run {
            check_text("run", run)?;
        }
        if self.weight == 0 {
            return Err(UsageError::Invalid { field: "weight" });
        }
        Ok(())
    }
}

fn check_text(field: &'static str, value: &str) -> Result<(), UsageError> {
    let ok = !value.is_empty()
        && value.len() <= MAX_TEXT
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(UsageError::Invalid { field })
    }
}

/// The event id: the lower-case hex of `sha256(pod uid, 0x00, minute)`, `minute` being whole minutes since the
/// Unix epoch in decimal.
pub fn event_id(pod_uid: &str, minute: u64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(pod_uid.as_bytes());
    hasher.update([0]);
    hasher.update(minute.to_string().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Whether [`UsageStore::insert`] stored the event or had it already.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Inserted {
    /// First time `(source, id)` was seen: it counts.
    New,
    /// Seen before: nothing changed, the first copy stands.
    Duplicate,
}

/// A half-open window of **ingestion** time in seconds since the Unix epoch: `from <= ingested_at < to`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    /// First second inside the window.
    pub from: u64,
    /// First second after the window.
    pub to: u64,
}

/// What a reader asks of [`UsageStore::spend`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpendQuery {
    /// The ingestion window.
    pub window: Window,
    /// Only this account, or every account.
    pub account: Option<String>,
}

/// Weighted pod-minutes of one account on one node type.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SpendLine {
    /// The account.
    pub account: String,
    /// The node type.
    pub node_type: String,
    /// The sum of the events' weights.
    pub weighted_minutes: u64,
}

/// Why a store call failed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UsageError {
    /// The event breaks the contract; the field is named, its value never.
    #[error("the event's `{field}` is not valid")]
    Invalid {
        /// The contract's name of the field.
        field: &'static str,
    },
    /// The window is empty or reversed.
    #[error("the window is empty")]
    EmptyWindow,
    /// The sum does not fit; a reader must narrow the window.
    #[error("the sum overflows")]
    Overflow,
    /// The store could not answer.
    #[error("the store is unavailable")]
    Unavailable,
}

/// The store behind the ingest. An adapter is a separate crate; none appears in a signature here.
pub trait UsageStore: Send + Sync {
    /// Takes the event once. `ingested_at` is the second the ingest received it; a duplicate keeps the first
    /// copy and its time. An invalid event is [`UsageError::Invalid`] and stores nothing.
    fn insert(
        &self,
        event: PodMinuteEvent,
        ingested_at: u64,
    ) -> impl Future<Output = Result<Inserted, UsageError>> + Send;

    /// Takes a batch at one `ingested_at`, in one round trip where the store can. The answer has one entry per
    /// event, in order: the first copy of a `(source, id)` is `New` (also against an earlier call), a copy of it
    /// later in the batch is `Duplicate`. An invalid event is [`UsageError::Invalid`] and the whole batch stores
    /// nothing. An empty batch answers an empty list.
    fn insert_many(
        &self,
        events: Vec<PodMinuteEvent>,
        ingested_at: u64,
    ) -> impl Future<Output = Result<Vec<Inserted>, UsageError>> + Send;

    /// Weighted minutes per account and node type ingested in the window, sorted by account then node type,
    /// with no line of zero. An empty or reversed window is [`UsageError::EmptyWindow`].
    fn spend(
        &self,
        query: SpendQuery,
    ) -> impl Future<Output = Result<Vec<SpendLine>, UsageError>> + Send;
}
