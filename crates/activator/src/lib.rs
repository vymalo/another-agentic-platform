//! The activator (AD-058, slice P3b of [§21a](../../../docs/architecture/05-runtime.md#21a-sleep-when-idle-the-plan)): the small
//! front that stays up while an agent sleeps.
//!
//! * The agent card is answered from a [`CardSource`] with **no wake at all**, so discovery needs no running agent.
//! * Any other request is buffered (up to [`Settings::max_body_bytes`], `413` past it), then waits on **one wake per agent**
//!   (single flight: concurrent requests for one agent make one call to the [`Waker`]) and is forwarded **once** through
//!   the [`Upstream`] when the agent is `Ready`.
//! * At the deadline ([`Settings::wake_timeout`], or the `Waker` saying `TimedOut`) the answer is `503` with `Retry-After`.
//!
//! It makes no other decision. **There is no authentication here**: AD-058 gives the activator none, the agent checks
//! the caller's token itself and the request is forwarded with its headers untouched. **The activator is therefore safe
//! only behind a NetworkPolicy that admits only the callers that may reach the agents**; that is a requirement on the chart
//! of ANR-1068, not something this crate can enforce. A forged `Host` header can name any agent in the namespace and wake
//! it, which is bounded here by [`Settings::max_in_flight`] and by the policy, not prevented.
//!
//! **Provisional (ANR-Q135):** which agent a request is for comes from the `Host` header (`<name>.<namespace>`, what a
//! Kubernetes Service name gives, optionally ending in [`Settings::host_suffix`]); the agent's own Service is what selects
//! the activator while it sleeps. Both labels are DNS-1123 labels. The way an agent is identified is not decided, and every
//! default of [`Settings`] is provisional and set by the deployment.
//!
//! **The deadline is an upper bound.** [`Settings::wake_timeout`] caps every wake; an adapter that knows the agent's own
//! `scaling.wakeTimeout` (5 s to 5 m) answers `TimedOut` at it, sooner. The default is the CRD's maximum, so the router
//! never cuts an agent short below its own value. When the deadline fires the wake future is **dropped**, so a
//! [`Waker::wake`] must be cancel-safe. A wake is not cancelled when its *waiters* go away (asking the operator is
//! idempotent, and the next request would only ask again). A request that joins a wake already running shares the first
//! request's deadline, so it may wait less than `wake_timeout`.
//!
//! **Known limit: no streaming.** Request and response bodies are buffered, so a streamed answer (SSE, A2A `message:stream`)
//! arrives whole, after the agent finishes, and a stream longer than `forward_timeout` is cut. A follow-up card will cover it.
//!
//! Routing by channel (AD-059, R5) will be a step between the request and [`Upstream::forward`]: [`Activator::route`] is
//! that step and always answers [`Route::Default`] today.

#![warn(missing_docs)]

mod metrics;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aap_ports::{
    CardSource, RefusedReason, Route, RuntimeId, Upstream, UpstreamRequest, WakeError, WakeOutcome,
    Waker,
};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST, RETRY_AFTER};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use tokio::sync::{Semaphore, watch};

pub use metrics::{ActivatorMetrics, Outcome, WakeKind};

/// The A2A card's path, and the older name of it, answered without a wake.
pub const DEFAULT_CARD_PATHS: [&str; 2] =
    ["/.well-known/agent-card.json", "/.well-known/agent.json"];

/// What the deployment sets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The largest request body buffered; a larger one is `413`. A body is held in memory for as long as the wake takes, so
    /// the cap is also the activator's memory bound per waiting request.
    pub max_body_bytes: usize,
    /// The upper bound of a wake, after which the caller is told to retry. The default is 5 m, the largest value the CRD allows
    /// for `scaling.wakeTimeout`, so that the agent's own (sooner) answer is what decides. Provisional.
    pub wake_timeout: Duration,
    /// The longest `Upstream::forward` may take; past it the answer is `504`. **After a `504` the agent may still have received
    /// the request and be processing it**: the future is dropped, the request is not recalled. A `POST` is not idempotent, so a
    /// client that retries can run it twice. Provisional.
    pub forward_timeout: Duration,
    /// The longest reading a request body may take; past it the answer is `408`. Provisional.
    pub body_read_timeout: Duration,
    /// How many non-card requests may be held or forwarded at once; past it the answer is `503` with `Retry-After` and no wake
    /// is started. Memory is bounded by this times `max_body_bytes`. Provisional.
    pub max_in_flight: usize,
    /// When set (`.svc.cluster.local`), the `Host` must be `<name>.<namespace>` followed by exactly this, so another suffix is
    /// `400`. When unset, only `<name>.<namespace>` is accepted. A leading `.` is added if missing. Provisional (ANR-Q135).
    pub host_suffix: Option<String>,
    /// The seconds the `Retry-After` header says.
    pub retry_after_secs: u32,
    /// The paths answered from the [`CardSource`], for `GET` and `HEAD`.
    pub card_paths: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_body_bytes: 1024 * 1024,
            wake_timeout: Duration::from_secs(300),
            forward_timeout: Duration::from_secs(120),
            body_read_timeout: Duration::from_secs(30),
            max_in_flight: 256,
            host_suffix: None,
            retry_after_secs: 10,
            card_paths: DEFAULT_CARD_PATHS.iter().map(|p| (*p).to_owned()).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Flight {
    Ready,
    TimedOut,
    Unavailable,
    Refused(RefusedReason),
}

type Flights = Mutex<HashMap<RuntimeId, watch::Receiver<Option<Flight>>>>;

/// Removes an agent's flight when the task that ran it ends, by a panic as well as normally, so a stale entry never
/// makes later requests wait on a wake that no longer runs.
struct Landing {
    flights: Arc<Flights>,
    agent: RuntimeId,
}

impl Drop for Landing {
    fn drop(&mut self) {
        self.flights
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.agent);
    }
}

struct Inner<W, U, C> {
    waker: Arc<W>,
    upstream: U,
    cards: C,
    settings: Settings,
    permits: Semaphore,
    flights: Arc<Flights>,
    metrics: Arc<ActivatorMetrics>,
}

/// The activator over its three ports.
pub struct Activator<W, U, C> {
    inner: Arc<Inner<W, U, C>>,
}

impl<W, U, C> Clone for Activator<W, U, C> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<W, U, C> Activator<W, U, C>
where
    W: Waker + 'static,
    U: Upstream + 'static,
    C: CardSource + 'static,
{
    /// An activator that wakes through `waker`, forwards through `upstream` and answers cards from `cards`.
    pub fn new(waker: W, upstream: U, cards: C, mut settings: Settings) -> Self {
        if let Some(sfx) = settings.host_suffix.as_mut()
            && !sfx.starts_with('.')
        {
            sfx.insert(0, '.');
        }
        let permits = Semaphore::new(settings.max_in_flight);
        Self {
            inner: Arc::new(Inner {
                waker: Arc::new(waker),
                upstream,
                cards,
                settings,
                permits,
                flights: Arc::default(),
                metrics: Arc::default(),
            }),
        }
    }

    /// The counters, for the binary's metrics listener.
    pub fn metrics(&self) -> Arc<ActivatorMetrics> {
        Arc::clone(&self.inner.metrics)
    }

    /// The router: every path is the activator's, there is no other route.
    pub fn router(&self) -> Router {
        Router::new()
            .fallback(handle::<W, U, C>)
            .with_state(self.clone())
    }

    /// Where a request for `agent` goes. One place today; AD-059 (R5) will read the channel from the request here.
    fn route(&self, _agent: &RuntimeId, _request: &Request) -> Route {
        Route::Default
    }

    /// Wait for the wake of `agent`, making the call to the `Waker` only if no one has.
    async fn wake(&self, agent: &RuntimeId) -> Flight {
        let mut rx = {
            let mut flights = self
                .inner
                .flights
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(rx) = flights.get(agent) {
                self.inner.metrics.wake(WakeKind::Shared);
                rx.clone()
            } else {
                self.inner.metrics.wake(WakeKind::Started);
                let (tx, rx) = watch::channel(None);
                flights.insert(agent.clone(), rx.clone());
                let inner = Arc::clone(&self.inner);
                let agent = agent.clone();
                tokio::spawn(async move {
                    let landing = Landing {
                        flights: Arc::clone(&inner.flights),
                        agent: agent.clone(),
                    };
                    let result =
                        tokio::time::timeout(inner.settings.wake_timeout, inner.waker.wake(&agent))
                            .await;
                    let flight = match result {
                        Ok(Ok(WakeOutcome::Ready)) => Flight::Ready,
                        Ok(Ok(WakeOutcome::TimedOut)) | Err(_) => Flight::TimedOut,
                        Ok(Err(WakeError::Unavailable(_))) => Flight::Unavailable,
                        Ok(Err(WakeError::Refused(why))) => Flight::Refused(why),
                    };
                    // The entry goes first, so a request that arrives after the answer makes a wake of its own; a
                    // request that joined before it reads the value from its receiver.
                    drop(landing);
                    let _ = tx.send(Some(flight));
                });
                rx
            }
        };
        match rx.wait_for(Option::is_some).await {
            Ok(v) => v.clone().unwrap_or(Flight::Unavailable),
            // The task ended without an answer (a panic): the caller is told to retry.
            Err(_) => Flight::Unavailable,
        }
    }
}

fn reply(status: StatusCode, reason: &'static str, retry_after: Option<u32>) -> Response {
    let mut r = Response::new(Body::from(format!("{{\"error\":\"{reason}\"}}")));
    *r.status_mut() = status;
    r.headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(s) = retry_after {
        r.headers_mut().insert(RETRY_AFTER, HeaderValue::from(s));
    }
    r
}

/// A DNS-1123 label: lowercase alphanumerics and `-`, not starting or ending with `-`, at most 63 characters.
fn dns_label(l: &str) -> bool {
    !l.is_empty()
        && l.len() <= 63
        && !l.starts_with('-')
        && !l.ends_with('-')
        && l.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `<name>.<namespace>` followed by exactly `suffix` (or by nothing when there is none), with an optional port. Anything else
/// names no agent: a further label, another suffix, an upper-case or malformed label.
fn agent_from_host(host: &str, suffix: Option<&str>) -> Option<RuntimeId> {
    // Host names are case-insensitive (RFC 9110 §7.2) and may end in the root dot: both are normalised before the DNS-1123
    // rules, which stay as strict as they were (one trailing dot only; an upper-case suffix setting is lower-cased too).
    let host = host.to_ascii_lowercase();
    let suffix = suffix.map(str::to_ascii_lowercase);
    let suffix = suffix.as_deref();
    let host = host.as_str();
    let host = host.rsplit_once(':').map_or(host, |(h, p)| {
        if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    let host = host.strip_suffix('.').unwrap_or(host);
    let rest = match suffix {
        Some(sfx) => host.strip_suffix(sfx)?,
        None => host,
    };
    let (name, scope) = rest.split_once('.')?;
    (dns_label(name) && dns_label(scope)).then(|| RuntimeId::new(scope, name))
}

/// The headers that describe one connection, not the message (RFC 9110 §7.6.1): dropped in both directions, with every
/// header the `Connection` header names.
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn strip_hop_by_hop(headers: &[(String, String)]) -> Vec<(String, String)> {
    // Names are compared in lower case: hyper lower-cases a request's, but an `Upstream` adapter hands back whatever case the
    // agent wrote, and header names are case-insensitive (RFC 9110 §5.1). The tokens `Connection` names are trimmed and
    // lower-cased for the same reason.
    let named: Vec<String> = headers
        .iter()
        .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
        .flat_map(|(_, v)| v.split(',').map(|t| t.trim().to_ascii_lowercase()))
        .filter(|t| !t.is_empty())
        .collect();
    headers
        .iter()
        .filter(|(k, _)| {
            let k = k.to_ascii_lowercase();
            !HOP_BY_HOP.contains(&k.as_str()) && !named.contains(&k)
        })
        .cloned()
        .collect()
}

async fn handle<W, U, C>(State(a): State<Activator<W, U, C>>, request: Request) -> Response
where
    W: Waker + 'static,
    U: Upstream + 'static,
    C: CardSource + 'static,
{
    let (response, outcome) = serve(&a, request).await;
    a.inner.metrics.request(outcome);
    response
}

async fn serve<W, U, C>(a: &Activator<W, U, C>, request: Request) -> (Response, Outcome)
where
    W: Waker + 'static,
    U: Upstream + 'static,
    C: CardSource + 'static,
{
    let s = &a.inner.settings;
    let Some(agent) = request
        .headers()
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| agent_from_host(h, s.host_suffix.as_deref()))
    else {
        return (
            reply(StatusCode::BAD_REQUEST, "no_agent", None),
            Outcome::BadRequest,
        );
    };

    let is_card = matches!(*request.method(), Method::GET | Method::HEAD)
        && s.card_paths.iter().any(|p| p == request.uri().path());
    if is_card {
        return match a.inner.cards.card(&agent).await {
            Ok(Some(card)) => {
                let mut r = Response::new(if request.method() == Method::HEAD {
                    Body::empty()
                } else {
                    Body::from(card.bytes)
                });
                if let Ok(v) = HeaderValue::from_str(&card.content_type) {
                    r.headers_mut().insert(CONTENT_TYPE, v);
                }
                (r, Outcome::Card)
            }
            Ok(None) => (
                reply(StatusCode::NOT_FOUND, "no_card", None),
                Outcome::CardMissing,
            ),
            Err(_) => (
                reply(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "card_unavailable",
                    Some(s.retry_after_secs),
                ),
                Outcome::CardUnavailable,
            ),
        };
    }

    // A request that declares a body over the cap is refused before anything is held or woken.
    let declared = match declared_length(request.headers()) {
        Ok(n) => n,
        Err(()) => {
            return (
                reply(StatusCode::BAD_REQUEST, "bad_content_length", None),
                Outcome::BadRequest,
            );
        }
    };
    if declared.is_some_and(|n| n > s.max_body_bytes as u64) {
        return (
            reply(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", None),
            Outcome::TooLarge,
        );
    }

    // Past the limit nothing is buffered and no wake is started, so a flood of forged hosts cannot start wakes or hold memory.
    let Ok(_permit) = a.inner.permits.try_acquire() else {
        return (
            reply(
                StatusCode::SERVICE_UNAVAILABLE,
                "busy",
                Some(s.retry_after_secs),
            ),
            Outcome::Shed,
        );
    };

    let route = a.route(&agent, &request);
    let (parts, body) = request.into_parts();
    let mut headers = Vec::with_capacity(parts.headers.len());
    for (k, v) in &parts.headers {
        let Ok(v) = v.to_str() else {
            return (
                reply(StatusCode::BAD_REQUEST, "bad_header", None),
                Outcome::BadRequest,
            );
        };
        headers.push((k.as_str().to_owned(), v.to_owned()));
    }
    let body = match read_body(body, s.max_body_bytes, s.body_read_timeout).await {
        Ok(b) => b,
        Err(BodyFail::TooLarge) => {
            return (
                reply(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", None),
                Outcome::TooLarge,
            );
        }
        Err(BodyFail::Timeout) => {
            return (
                reply(StatusCode::REQUEST_TIMEOUT, "body_timeout", None),
                Outcome::BodyTimeout,
            );
        }
        Err(BodyFail::Unreadable) => {
            return (
                reply(StatusCode::BAD_REQUEST, "body_unreadable", None),
                Outcome::BadRequest,
            );
        }
    };

    match a.wake(&agent).await {
        Flight::Ready => {}
        Flight::TimedOut => {
            return (
                reply(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "wake_timeout",
                    Some(s.retry_after_secs),
                ),
                Outcome::WakeTimeout,
            );
        }
        Flight::Unavailable => {
            return (
                reply(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "wake_unavailable",
                    Some(s.retry_after_secs),
                ),
                Outcome::WakeFailed,
            );
        }
        // Neither is worth a retry: an unknown agent is not there, a suspended one is a person's choice (AD-058).
        Flight::Refused(RefusedReason::Suspended) => {
            return (
                reply(StatusCode::CONFLICT, "agent_suspended", None),
                Outcome::WakeFailed,
            );
        }
        Flight::Refused(RefusedReason::Unknown) => {
            return (
                reply(StatusCode::NOT_FOUND, "agent_unknown", None),
                Outcome::WakeFailed,
            );
        }
        // A reason added to the `#[non_exhaustive]` enum later: refused, and not worth a retry, but not known to be "unknown".
        // `409 agent_refused` says that and claims nothing else.
        Flight::Refused(_) => {
            return (
                reply(StatusCode::CONFLICT, "agent_refused", None),
                Outcome::WakeFailed,
            );
        }
    }

    let forwarded = UpstreamRequest {
        method: parts.method.as_str().to_owned(),
        path_and_query: parts
            .uri
            .path_and_query()
            .map_or_else(|| "/".to_owned(), |p| p.as_str().to_owned()),
        headers: strip_hop_by_hop(&headers),
        body: body.to_vec(),
    };
    match tokio::time::timeout(
        s.forward_timeout,
        a.inner.upstream.forward(&agent, &route, forwarded),
    )
    .await
    {
        Err(_) => (
            reply(StatusCode::GATEWAY_TIMEOUT, "upstream_timeout", None),
            Outcome::UpstreamTimeout,
        ),
        Ok(Ok(answer)) => {
            // A status that is not a valid HTTP status is the agent's fault and the caller's `502`.
            let Ok(status) = StatusCode::from_u16(answer.status) else {
                return (
                    reply(StatusCode::BAD_GATEWAY, "upstream_invalid_status", None),
                    Outcome::UpstreamFailed,
                );
            };
            let mut r = Response::new(Body::from(answer.body));
            *r.status_mut() = status;
            for (k, v) in strip_hop_by_hop(&answer.headers) {
                // The length is the body's own, which the server sets.
                if k == "content-length" {
                    continue;
                }
                if let (Ok(k), Ok(v)) = (HeaderName::try_from(k), HeaderValue::try_from(v)) {
                    r.headers_mut().append(k, v);
                }
            }
            (r, Outcome::Forwarded)
        }
        Ok(Err(_)) => (
            reply(StatusCode::BAD_GATEWAY, "upstream_unavailable", None),
            Outcome::UpstreamFailed,
        ),
    }
}

/// The declared body length, read as RFC 9110 §8.6 has it: every `Content-Length` field and every element of a comma-separated
/// list is digits only (so no `+`, `-` or inner space), leading zeros are fine, and they must all be the same number, or the
/// request is ambiguous and refused. A number too large for `u64` is read as `u64::MAX`, which is over any cap.
fn declared_length(headers: &axum::http::HeaderMap) -> Result<Option<u64>, ()> {
    let mut found: Option<u64> = None;
    for value in headers.get_all(CONTENT_LENGTH) {
        let text = value.to_str().map_err(|_| ())?;
        for element in text.split(',') {
            let element = element.trim_matches(|c| c == ' ' || c == '\t');
            if element.is_empty() || !element.bytes().all(|b| b.is_ascii_digit()) {
                return Err(());
            }
            let n = element.parse::<u64>().unwrap_or(u64::MAX);
            match found {
                Some(prev) if prev != n => return Err(()),
                _ => found = Some(n),
            }
        }
    }
    Ok(found)
}

enum BodyFail {
    TooLarge,
    Timeout,
    Unreadable,
}

/// Read a whole body of at most `cap` bytes within `timeout`. This is the fallback for a body with no `Content-Length`
/// (chunked): the declared case is refused earlier.
async fn read_body(
    body: Body,
    cap: usize,
    timeout: Duration,
) -> Result<axum::body::Bytes, BodyFail> {
    match tokio::time::timeout(timeout, to_bytes(body, cap)).await {
        Err(_) => Err(BodyFail::Timeout),
        Ok(Ok(b)) => Ok(b),
        Ok(Err(e)) => {
            // `to_bytes` reports a length-limit error as a boxed `LengthLimitError` whose Display is "length limit exceeded"; the
            // only other failure is the peer going away mid-body. There is no direct dependency to downcast with, so the text is
            // matched, and `a_body_over_the_cap_with_no_content_length_is_413` pins it: if axum changes the text that test fails.
            let too_large = std::error::Error::source(&e)
                .is_some_and(|s| s.to_string().contains("length limit"));
            Err(if too_large {
                BodyFail::TooLarge
            } else {
                BodyFail::Unreadable
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_the_agent() {
        let id = |ns: &str, n: &str| Some(RuntimeId::new(ns, n));
        assert_eq!(agent_from_host("coder.team-a", None), id("team-a", "coder"));
        assert_eq!(
            agent_from_host("coder.team-a:8080", None),
            id("team-a", "coder")
        );
        assert_eq!(
            agent_from_host("coder.team-a.svc", None),
            None,
            "no further label without a suffix"
        );
        assert_eq!(agent_from_host("localhost", None), None);
        // Case and one root dot are normalised; anything else that was refused still is.
        assert_eq!(agent_from_host("Coder.TEAM-a", None), id("team-a", "coder"));
        assert_eq!(
            agent_from_host("coder.team-a.", None),
            id("team-a", "coder")
        );
        assert_eq!(agent_from_host("coder.team-a..", None), None);
        assert_eq!(agent_from_host("coder_x.team-a", None), None);
        assert_eq!(agent_from_host(".team-a", None), None);
        assert_eq!(agent_from_host("a b.c", None), None);
        assert_eq!(agent_from_host("-a.b", None), None);
        assert_eq!(agent_from_host("a-.b", None), None);
        assert_eq!(
            agent_from_host(&format!("{}.b", "a".repeat(64)), None),
            None
        );
        assert_eq!(
            agent_from_host(&format!("{}.b", "a".repeat(63)), None).map(|_| ()),
            Some(())
        );
    }

    #[test]
    fn a_suffix_is_matched_exactly() {
        let sfx = Some(".svc.cluster.local");
        let id = Some(RuntimeId::new("team-a", "coder"));
        assert_eq!(agent_from_host("coder.team-a.svc.cluster.local", sfx), id);
        assert_eq!(
            agent_from_host("coder.team-a.svc.cluster.local:8080", sfx),
            id
        );
        assert_eq!(agent_from_host("coder.team-a.evil.example", sfx), None);
        assert_eq!(agent_from_host("coder.team-a", sfx), None);
        assert_eq!(
            agent_from_host("x.coder.team-a.svc.cluster.local", sfx),
            None
        );
        assert_eq!(
            agent_from_host("coder.team-a.svc.cluster.local.evil.example", sfx),
            None
        );
    }

    #[test]
    fn hop_by_hop_headers_and_those_named_by_connection_are_dropped() {
        let h = |k: &str, v: &str| (k.to_owned(), v.to_owned());
        let got = strip_hop_by_hop(&[
            h("connection", "x-private, keep-alive"),
            h("x-private", "1"),
            h("keep-alive", "timeout=5"),
            h("transfer-encoding", "chunked"),
            h("authorization", "Bearer t"),
        ]);
        assert_eq!(got, vec![h("authorization", "Bearer t")]);
    }
}
