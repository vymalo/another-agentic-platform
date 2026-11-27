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
use std::pin::Pin;

use futures::{Stream, StreamExt};

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
    /// The operator refused, for a reason that no retry changes.
    #[error("the wake was refused: {0}")]
    Refused(RefusedReason),
}

/// Why a wake was refused. The activator answers `404` for [`Unknown`](Self::Unknown) and `409` for
/// [`Suspended`](Self::Suspended), neither with `Retry-After`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefusedReason {
    /// The operator knows no such agent.
    Unknown,
    /// The agent is suspended by a person (`spec.suspend`), which is never woken (AD-058).
    Suspended,
}

impl std::fmt::Display for RefusedReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unknown => "unknown agent",
            Self::Suspended => "agent suspended",
        })
    }
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
/// The activator additionally bounds every wake by its own setting (default 5 m, the CRD's maximum, so it never cuts an agent
/// short below its own value), so a `Waker` that never answers still ends in a `503`; the two are the same answer to the
/// caller. **Cancel-safety:** at the activator's deadline the future returned by `wake` is dropped, so an implementation must
/// leave nothing half-done when it is dropped at any `.await`.
///
/// A wake of an agent that is already awake is [`WakeOutcome::Ready`] at once: the activator does not know whether the
/// agent sleeps, and asking is cheap.
pub trait Waker: Send + Sync {
    /// Wake `agent` and wait until it is `Ready`, or give up. The future may be dropped at the activator's deadline.
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

/// The body of an agent's answer: byte chunks, in order, as they arrive. An `Err` item ends the answer (the head has been
/// sent by then, so the activator can only end the connection). No HTTP library type appears here (AD-020).
pub type BodyStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, UpstreamError>> + Send>>;

/// An agent's answer: its head, and its body **as a stream**, so that a streamed answer (SSE, A2A `message:stream`) reaches the
/// caller as it is produced and the activator never holds it whole.
pub struct UpstreamResponse {
    /// The status code.
    pub status: u16,
    /// Header names and values, in order.
    pub headers: Vec<(String, String)>,
    /// The body, chunk by chunk.
    pub body: BodyStream,
}

impl UpstreamResponse {
    /// An answer whose body is already whole: one chunk, or none when `body` is empty.
    pub fn buffered(status: u16, headers: Vec<(String, String)>, body: Vec<u8>) -> Self {
        let chunks: Vec<Result<Vec<u8>, UpstreamError>> = if body.is_empty() {
            Vec::new()
        } else {
            vec![Ok(body)]
        };
        Self {
            status,
            headers,
            body: Box::pin(futures::stream::iter(chunks)),
        }
    }

    /// Read the whole body, for a caller that has no use for a stream (tests, mostly).
    ///
    /// # Errors
    ///
    /// The first error item of the stream.
    pub async fn collect_body(self) -> Result<Vec<u8>, UpstreamError> {
        let mut body = self.body;
        let mut out = Vec::new();
        while let Some(chunk) = body.next().await {
            out.extend(chunk?);
        }
        Ok(out)
    }
}

impl std::fmt::Debug for UpstreamResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .finish_non_exhaustive()
    }
}

/// Why a request could not be forwarded.
#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    /// The agent could not be reached or did not answer (the activator answers `502`), **or** its body failed after the head: then
    /// this is the error item of the [`BodyStream`], and the answer ends there (the activator can only end the connection).
    #[error("the agent could not be reached")]
    Unavailable(#[source] BoxError),
}

impl Classify for UpstreamError {
    fn class(&self) -> ErrorClass {
        ErrorClass::Transient
    }
}

/// Forwards one buffered request to an agent that is `Ready` and returns its answer's head and a body stream. **Once per request**: an implementation never retries a
/// request that may have reached the agent, because A2A methods are not idempotent.
pub trait Upstream: Send + Sync {
    /// Send `request` to `agent` by `route` and return its answer.
    ///
    /// # Errors
    ///
    /// [`UpstreamError`] when the agent cannot be reached before it answers. An answer with a 4xx or 5xx status is an `Ok`; a
    /// failure after the head is an `Err` item of the body.
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
