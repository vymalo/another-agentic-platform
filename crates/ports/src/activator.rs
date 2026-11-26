//! The three seams of the activator (AD-058, slice P3b): [`Waker`], [`Upstream`] and [`CardSource`].
//!
//! While an agent sleeps its traffic reaches the activator, a small front that stays up. It answers the agent card
//! without waking anything ([`CardSource`]), holds any other request while the operator wakes the workload
//! ([`Waker`]), and forwards the request once the agent is `Ready` ([`Upstream`]). The activator only asks; the
//! operator scales (§21a). Each seam is a trait so that the Kubernetes adapters (ANR-1068) and the operator's wake
//! path (ANR-1067) are separate crates and the router is tested over the `Memory` fakes (AD-020).
//!
//! The agent is a [`RuntimeId`], the identity every other seam of this crate uses.

use std::future::Future;

use crate::{BoxError, Classify, ErrorClass, RuntimeId};

/// How a wake ended, when it ended without an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeOutcome {
    /// The workload is `Ready` (or was already): a request may be forwarded.
    Ready,
    /// The operator gave up at the agent's own `wakeTimeout` (`scaling.wakeTimeout`). The caller is told to retry.
    TimedOut,
}

/// Why a [`Waker`] could not ask for, or learn the result of, a wake.
#[derive(Debug, thiserror::Error)]
pub enum WakeError {
    /// The operator could not be reached or did not answer. A later attempt may succeed.
    #[error("the wake could not be requested or followed")]
    Unavailable(#[source] BoxError),
    /// The operator refused: the agent is unknown, or is suspended by a person (`spec.suspend` is never woken, AD-058).
    #[error("the wake was refused: {0}")]
    Refused(String),
}

impl Classify for WakeError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::Unavailable(_) => ErrorClass::Transient,
            Self::Refused(_) => ErrorClass::Invalid,
        }
    }
}

/// Asks the operator to wake an agent and says when it is `Ready`: the seam's one call, `Wake(agent) -> Ready | Timeout`
/// of §21a.
///
/// **Who owns the deadline.** An adapter that knows the agent's `wakeTimeout` answers [`WakeOutcome::TimedOut`] at it.
/// The activator additionally bounds every wake by its own setting, so a `Waker` that never answers still ends in a
/// `503`; the two are the same answer to the caller.
///
/// A wake of an agent that is already awake is [`WakeOutcome::Ready`] at once: the activator does not know whether the
/// agent sleeps, and asking is cheap.
pub trait Waker: Send + Sync {
    /// Wake `agent` and wait until it is `Ready`, or give up.
    ///
    /// # Errors
    ///
    /// [`WakeError`] when the operator cannot be asked or refuses.
    fn wake(
        &self,
        agent: &RuntimeId,
    ) -> impl Future<Output = Result<WakeOutcome, WakeError>> + Send;
}

/// Where a request goes among the places an agent can be reached. Today there is one; AD-059 (R5) makes the activator
/// the release-channels A2A front, and a channel will be another variant. The activator resolves it before it forwards,
/// so a [`Upstream`] never learns how a route was chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Route {
    /// The agent's one workload.
    #[default]
    Default,
}

/// A buffered request to an agent, in the neutral terms of HTTP: no `http` crate type crosses the seam.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamRequest {
    /// The method, as sent (`POST`).
    pub method: String,
    /// The path and query, as received, starting with `/`.
    pub path_and_query: String,
    /// Header names (lowercase) and values, in order. The activator has already dropped the hop-by-hop ones.
    pub headers: Vec<(String, String)>,
    /// The whole body.
    pub body: Vec<u8>,
}

/// An agent's answer, buffered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpstreamResponse {
    /// The status code.
    pub status: u16,
    /// Header names and values, in order.
    pub headers: Vec<(String, String)>,
    /// The whole body.
    pub body: Vec<u8>,
}

/// Why a request could not be forwarded.
#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    /// The agent could not be reached or did not answer. The activator answers `502`.
    #[error("the agent could not be reached")]
    Unavailable(#[source] BoxError),
}

impl Classify for UpstreamError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Transient
    }
}

/// Forwards one buffered request to an agent that is `Ready`. **Once per request**: an implementation never retries a
/// request that may have reached the agent, because A2A methods are not idempotent.
pub trait Upstream: Send + Sync {
    /// Send `request` to `agent` by `route` and return its answer.
    ///
    /// # Errors
    ///
    /// [`UpstreamError`] when the agent cannot be reached. An answer with a 4xx or 5xx status is an `Ok`.
    fn forward(
        &self,
        agent: &RuntimeId,
        route: &Route,
        request: UpstreamRequest,
    ) -> impl Future<Output = Result<UpstreamResponse, UpstreamError>> + Send;
}

/// An agent card as the activator serves it: the bytes the agent published, and their media type. The activator never
/// reads or rewrites them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// The `Content-Type` of `bytes` (`application/json`).
    pub content_type: String,
    /// The card, as published.
    pub bytes: Vec<u8>,
}

/// Why a [`CardSource`] could not answer.
#[derive(Debug, thiserror::Error)]
pub enum CardError {
    /// The source could not be read. A later attempt may succeed.
    #[error("the card is unavailable")]
    Unavailable(#[source] BoxError),
}

impl Classify for CardError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Transient
    }
}

/// The card of an agent, readable while the agent sleeps. The registry holds no card body (it lists each agent's card
/// `href`, §21a), so an adapter keeps its own copy of what the agent last served.
pub trait CardSource: Send + Sync {
    /// The card of `agent`, or `None` when the source has none.
    ///
    /// # Errors
    ///
    /// [`CardError`] when the source cannot be read. A missing card is `Ok(None)`.
    fn card(
        &self,
        agent: &RuntimeId,
    ) -> impl Future<Output = Result<Option<Card>, CardError>> + Send;
}
