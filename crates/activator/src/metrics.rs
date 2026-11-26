//! One counter in the Prometheus text format, in the style of the Platform API's (`aap-platform-api`, `metrics.rs`): no client
//! library. **Every label value is one of [`Outcome`]'s words**, chosen here and never taken from a request, so the series are
//! bounded and no agent name, path or header can reach them.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

/// The name of the series.
pub const REQUESTS: &str = "aap_activator_requests_total";
/// The name of the wake series.
pub const WAKES: &str = "aap_activator_wakes_total";

/// How a request ended: the closed set of the `outcome` label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Outcome {
    /// The card was answered (no wake).
    Card,
    /// The card is not known or could not be read.
    CardMissing,
    /// Forwarded and answered.
    Forwarded,
    /// The agent could not be reached once awake.
    UpstreamFailed,
    /// The wake ended `TimedOut` or hit the activator's deadline.
    WakeTimeout,
    /// The wake failed or was refused.
    WakeFailed,
    /// The body was over the cap.
    TooLarge,
    /// The request named no agent or could not be forwarded as it was.
    BadRequest,
}

impl Outcome {
    /// The label word.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::CardMissing => "card_missing",
            Self::Forwarded => "forwarded",
            Self::UpstreamFailed => "upstream_failed",
            Self::WakeTimeout => "wake_timeout",
            Self::WakeFailed => "wake_failed",
            Self::TooLarge => "too_large",
            Self::BadRequest => "bad_request",
        }
    }
}

/// How a wake call began: by this request (`started`), or by joining one in flight (`shared`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WakeKind {
    /// This request made the call to the `Waker`.
    Started,
    /// This request waited on a call already made.
    Shared,
}

/// The counters. Share it in an `Arc`.
#[derive(Debug, Default)]
pub struct ActivatorMetrics {
    requests: Mutex<BTreeMap<Outcome, u64>>,
    wakes: Mutex<BTreeMap<WakeKind, u64>>,
}

impl ActivatorMetrics {
    pub(crate) fn request(&self, outcome: Outcome) {
        *self
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(outcome)
            .or_default() += 1;
    }

    pub(crate) fn wake(&self, kind: WakeKind) {
        *self
            .wakes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(kind)
            .or_default() += 1;
    }

    /// The count of one outcome.
    pub fn requests(&self, outcome: Outcome) -> u64 {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&outcome)
            .copied()
            .unwrap_or(0)
    }

    /// The exposition text.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "# HELP {REQUESTS} Requests of the activator, by outcome.\n# TYPE {REQUESTS} counter\n"
        ));
        for (o, n) in self
            .requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            out.push_str(&format!("{REQUESTS}{{outcome=\"{}\"}} {n}\n", o.label()));
        }
        out.push_str(&format!(
            "# HELP {WAKES} Waiting requests, by whether they made the wake call or shared one.\n# TYPE {WAKES} counter\n"
        ));
        for (k, n) in self
            .wakes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            let word = match k {
                WakeKind::Started => "started",
                WakeKind::Shared => "shared",
            };
            out.push_str(&format!("{WAKES}{{kind=\"{word}\"}} {n}\n"));
        }
        out
    }
}
