//! `ActivitySource`: what an agent says about its own work, so the operator can tell when it may sleep (AD-058).
//!
//! The answer is the agent's, not the operator's guess: `busy` counts the runs the agent holds, and `next_due_at`
//! is when a parked schedule fires next. The operator only asks. A failed question is an error, never an idle
//! answer: a caller that cannot tell must treat the agent as busy.

use std::future::Future;

use crate::{BoxError, Classify, ErrorClass, RuntimeId};

/// What one agent reports about its work now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// How many runs the agent holds (claimed, or being stepped). `0` is idle.
    pub busy: u32,
    /// When the next parked schedule fires, in seconds since the Unix epoch. `None`: nothing is scheduled.
    pub next_due_at: Option<i64>,
}

impl Activity {
    /// An agent with no work and nothing scheduled.
    pub const IDLE: Self = Self {
        busy: 0,
        next_due_at: None,
    };

    /// Whether the agent holds a run.
    pub const fn is_busy(&self) -> bool {
        self.busy > 0
    }
}

/// Why the agent could not answer, in the terms the operator logs: a closed set, so a warn never carries a URL,
/// a token or a status text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnavailableCause {
    /// The request did not complete in time. With only a limit on the whole request, packets a NetworkPolicy drops
    /// end here too.
    Timeout,
    /// The connection could not be made (a refused connection, a closed port, DNS).
    Connect,
    /// The agent answered a 5xx.
    ServerError,
    /// Anything else: another status, a body that could not be read.
    Other,
}

impl UnavailableCause {
    /// The short class the operator logs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::ServerError => "status 5xx",
            Self::Other => "other",
        }
    }
}

/// Why an [`ActivitySource`] could not answer.
#[derive(Debug, thiserror::Error)]
pub enum ActivityError {
    /// The agent could not be reached or did not answer in time. It may be starting, stopping or gone.
    #[error("the agent's activity is unavailable")]
    Unavailable {
        /// The short class, for the operator's warn.
        cause: UnavailableCause,
        /// What failed underneath. Not part of the message.
        #[source]
        source: BoxError,
    },
    /// The agent answered 401 or 403: the token or the policy is wrong, and it will not change by retrying.
    #[error("the agent's activity answer was refused ({status})")]
    Refused {
        /// The HTTP status the agent answered.
        status: u16,
    },
    /// The agent answered something this operator does not understand.
    #[error("the agent's activity answer was malformed: {0}")]
    Invalid(String),
}

impl Classify for ActivityError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::Unavailable { .. } => ErrorClass::Transient,
            Self::Refused { .. } | Self::Invalid(_) => ErrorClass::Invalid,
        }
    }
}

/// Asks an agent service what work it holds.
pub trait ActivitySource: Send + Sync {
    /// The activity of the service `id`.
    ///
    /// # Errors
    ///
    /// [`ActivityError`] when the agent cannot be asked or its answer is not understood. **An error is not
    /// idleness**: the caller counts it as busy.
    fn poll(&self, id: &RuntimeId) -> impl Future<Output = Result<Activity, ActivityError>> + Send;
}
