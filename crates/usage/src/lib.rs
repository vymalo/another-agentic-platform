//! The usage port (AD-060, amended by AD-069): a pod-minute event, and the two seams around it.
//!
//! The platform **provides OTel only** (AD-069): the operator's meter turns pods into [`PodMinuteEvent`]s and hands
//! each to a [`UsageSink`]; an adapter (the OTLP/HTTP JSON logs exporter, `aap-usage-otlp`) carries it to the
//! OpenTelemetry Collector, which routes, enriches and dedupes. This crate holds no ledger, no sum and no rate.
//!
//! **Breaking change: `UsageStore` is gone** (`insert`, `insert_many`, `spend`, `Window`, `SpendQuery`,
//! `SpendLine` and `Inserted` with it). A sink takes events; it answers nothing about them.
//!
//! * [`UsageSink::send`] takes one event and **never waits on a network**: the queue is the adapter's.
//! * [`UsageSink::outcomes`] says what became of the events a sink took (ANR-1151): an **acknowledgement**, so the
//!   meter does not call a minute delivered because it was queued. A sink that cannot say answers `None`, and
//!   taking the event is then all the meter knows.
//! * [`AccountSource`] says which account a pod's minutes are for. The account is an **opaque id from the
//!   broker**, never an e-mail: [`PodMinuteEvent::validate`] keeps a closed alphabet that has no `@`.
//!
//! With the feature `testkit` this crate also has [`testkit`], the conformance suite of a sink, and the fakes
//! [`MemoryUsageSink`] and [`MemoryAccounts`].

use std::future::Future;

use sha2::{Digest, Sha256};

mod tombstone;
pub use tombstone::{Loaded, MAX_TOMBSTONES, Tombstone, TombstoneError, TombstoneStore};

#[cfg(feature = "testkit")]
pub mod testkit;

#[cfg(feature = "testkit")]
mod memory;
#[cfg(feature = "testkit")]
pub use memory::{MemoryAccounts, MemoryTombstones, MemoryUsageSink};

/// The `type` of a pod-minute event, a log attribute of the OTLP record (AD-069).
pub const EVENT_TYPE: &str = "com.vymalo.agents.pod-minute.v1";

/// The weights an event may carry, in quarter-units: 1 is 0.25 of a unit (1 vCPU or 2 GiB), 32 is 8. It is
/// `aap_domain::SizeWeight::GRID` (AD-066); the controller's tests hold the two equal.
pub const WEIGHT_GRID: [u32; 6] = [1, 2, 4, 8, 16, 32];

const MAX_TEXT: usize = 128;

/// What a pod was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PodKind {
    /// A run pod, made by a coder for one run: it carries a run id.
    Run,
    /// A pod of an agent's own workload: it carries no run.
    Agent,
}

impl PodKind {
    /// The wire value of `podKind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Agent => "agent",
        }
    }
}

/// One pod, one minute of it. The names of the fields in the messages are the attributes' (AD-069).
///
/// `Debug` prints the `subject` (an account) as `<redacted>`, so no log line or panic message that formats an event
/// can carry it.
#[derive(Clone, PartialEq, Eq)]
pub struct PodMinuteEvent {
    /// The meter instance that reports it: a deployment constant (`source`).
    pub source: String,
    /// [`event_id`] of the pod's uid and the minute (`event_id`); the key the collector dedupes on.
    pub event_id: String,
    /// The minute, in whole minutes since the Unix epoch. The event's time is the **end** of it
    /// ([`end_unix_nanos`](Self::end_unix_nanos)), so a restart and a second replica give the same one.
    pub minute: u64,
    /// The account the minute is for (`subject`): an opaque id from the broker.
    pub subject: String,
    /// Weighted units of this minute, in quarter-units (`weightQuarters`): one of [`WEIGHT_GRID`].
    pub weight_quarters: u32,
    /// The node type the pod ran on (`nodeType`).
    pub node_type: String,
    /// What the pod was (`podKind`).
    pub pod_kind: PodKind,
    /// The agent service it belongs to (`agentService`).
    pub agent_service: String,
    /// The run it served (`run`): present for a run pod and for no other.
    pub run: Option<String>,
}

impl std::fmt::Debug for PodMinuteEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PodMinuteEvent")
            .field("source", &self.source)
            .field("event_id", &self.event_id)
            .field("minute", &self.minute)
            .field("subject", &"<redacted>")
            .field("weight_quarters", &self.weight_quarters)
            .field("node_type", &self.node_type)
            .field("pod_kind", &self.pod_kind)
            .field("agent_service", &self.agent_service)
            .field("run", &self.run)
            .finish()
    }
}

impl PodMinuteEvent {
    /// The end of the minute, in nanoseconds since the Unix epoch (`timeUnixNano` and `observedTimeUnixNano`).
    pub fn end_unix_nanos(&self) -> u64 {
        self.minute
            .saturating_add(1)
            .saturating_mul(60)
            .saturating_mul(1_000_000_000)
    }

    /// Checks every field: a closed alphabet (`[A-Za-z0-9._:/-]`, so an `@` is refused), at most 128 bytes, no
    /// empty text, an `event_id` that is 64 lower-case hex digits, a weight on the grid, and a run that is
    /// present exactly when the pod was a run pod. The error names the field, never its value.
    pub fn validate(&self) -> Result<(), UsageError> {
        let texts: [(&'static str, &str); 5] = [
            ("source", &self.source),
            ("subject", &self.subject),
            ("nodeType", &self.node_type),
            ("agentService", &self.agent_service),
            ("eventId", &self.event_id),
        ];
        for (field, value) in texts {
            check_text(field, value)?;
        }
        let hex = self.event_id.len() == 64
            && self
                .event_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hex {
            return Err(UsageError::Invalid { field: "eventId" });
        }
        if !WEIGHT_GRID.contains(&self.weight_quarters) {
            return Err(UsageError::Invalid {
                field: "weightQuarters",
            });
        }
        match (&self.run, self.pod_kind) {
            (Some(run), PodKind::Run) => check_text("run", run)?,
            (None, PodKind::Agent) => {}
            _ => return Err(UsageError::Invalid { field: "run" }),
        }
        Ok(())
    }
}

/// Whether `value` is a text field of a line: not empty, at most 128 bytes, and only `[A-Za-z0-9._:/-]`. It is
/// the one copy of the rule [`PodMinuteEvent::validate`] applies to each text field, so a producer (the meter, for
/// a run id it reads from a pod) can refuse a value before it builds a line.
pub fn is_line_text(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TEXT
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'/' | b'-'))
}

fn check_text(field: &'static str, value: &str) -> Result<(), UsageError> {
    if is_line_text(value) {
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

/// Why a call failed.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UsageError {
    /// The event breaks the contract; the field is named, its value never.
    #[error("the event's `{field}` is not valid")]
    Invalid {
        /// The name of the field, as the attribute is called.
        field: &'static str,
    },
    /// The sink could not take the event now (its queue is full, or it is closed). The caller keeps the minute
    /// and sends it again.
    #[error("the sink is unavailable")]
    Unavailable,
}

/// Where the meter puts what it measured. An adapter is a separate crate; none appears in a signature here.
pub trait UsageSink: Send + Sync {
    /// Takes the event. An invalid event is [`UsageError::Invalid`] and nothing is taken. The same event twice is
    /// two calls that both succeed: the sink does not dedupe, the collector does on `event_id`. It **never
    /// waits on a network**: an adapter queues, and answers [`UsageError::Unavailable`] when it cannot.
    fn send(&self, event: PodMinuteEvent) -> impl Future<Output = Result<(), UsageError>> + Send;

    /// What became of the events taken since the last call, each **once** (ANR-1151, additive): `Some(outcomes)`
    /// from a sink that reports them, `None` from one that cannot, for which taking the event is the delivery.
    /// A sink that answers `Some` must, sooner or later, say one [`Fate`] for each event it accepted, unless
    /// it is stopped with events still held (then the caller's process is ending and the same minutes are
    /// re-derived by the next one, with the same `event_id`s). It never waits on a network.
    fn outcomes(&self) -> Option<Vec<Outcome>> {
        None
    }
}

/// What became of one event a sink took.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    /// The collector acknowledged it (a 2xx). The meter may forget it.
    Delivered,
    /// The collector refused it for good (400, 415, 422, or a single event too large). Sending it again cannot
    /// help: the meter forgets it and counts it.
    Refused,
}

/// The [`Fate`] of the event named by `event_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// [`PodMinuteEvent::event_id`].
    pub event_id: String,
    /// What became of it.
    pub fate: Fate,
}

/// The pod an account is asked for. A run pod is known by its run id, an agent pod by its agent service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PodRef {
    /// A run pod: the agent service that made it and the run.
    Run {
        /// The agent service.
        service: String,
        /// The run's full id: the annotation `adam.vymalo.com/run-id` the coder puts on the pod, never its label
        /// `adam.vymalo.com/run`, which is a hash of the id (ANR-1160). A source that asks the broker sends it as one
        /// percent-encoded path segment.
        run: String,
    },
    /// A pod of the agent's own workload.
    ///
    /// **Breaking change (ANR-1100, AD-071): `namespace` is new**, so every construction and every pattern that
    /// names all fields must be updated (a pattern with `..` still compiles).
    Agent {
        /// The namespace of the `AgentService`: the meter merges every watched namespace, so the name alone is
        /// ambiguous.
        namespace: String,
        /// The agent service.
        service: String,
    },
}

/// Why an account could not be told now.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("the account source is unavailable")]
pub struct AccountUnavailable;

/// Says which account a pod's minutes are for. The source for a run pod is `aap-usage-broker` (it asks the
/// orchestrator's broker); the source for an agent pod is `aap-usage-kubernetes` (it reads `AgentService.spec.account`,
/// AD-071). `operator run` composes the two by pod kind.
///
/// **Breaking change (ANR-1100): [`PodRef::Agent`] gained a `namespace`**, which every implementer that matches on it
/// must now handle.
pub trait AccountSource: Send + Sync {
    /// The account's opaque id, `None` when the pod belongs to no known account (no line is made, and a metric
    /// counts it), or [`AccountUnavailable`] when the source cannot answer now (the meter holds the minute and
    /// asks again).
    fn account(
        &self,
        pod: &PodRef,
    ) -> impl Future<Output = Result<Option<String>, AccountUnavailable>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_never_prints_the_subject() {
        let e = PodMinuteEvent {
            source: "m1".into(),
            event_id: event_id("pod", 1),
            minute: 1,
            subject: "acct-SECRET-42".into(),
            weight_quarters: 4,
            node_type: "std".into(),
            pod_kind: PodKind::Agent,
            agent_service: "coder".into(),
            run: None,
        };
        let text = format!("{e:?}");
        assert!(!text.contains("acct-SECRET-42"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
        assert!(!format!("{:#?}", Some(e)).contains("acct-SECRET-42"));
    }
}
