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

/// Why an [`ActivitySource`] could not answer.
#[derive(Debug, thiserror::Error)]
pub enum ActivityError {
    /// The agent could not be reached or did not answer in time. It may be starting, stopping or gone.
    #[error("the agent's activity is unavailable")]
    Unavailable(#[source] BoxError),
    /// The agent answered something this operator does not understand, or refused the question.
    #[error("the agent's activity answer was refused or malformed: {0}")]
    Invalid(String),
}

impl Classify for ActivityError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::Unavailable(_) => ErrorClass::Transient,
            Self::Invalid(_) => ErrorClass::Invalid,
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
