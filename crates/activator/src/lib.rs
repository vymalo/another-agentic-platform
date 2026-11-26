//! The activator (AD-058, slice P3b of [§21a](../../../docs/architecture/05-runtime.md#21a-sleep-when-idle-the-plan)): the small
//! front that stays up while an agent sleeps.
//!
//! * The agent card is answered from a [`CardSource`] with **no wake at all**, so discovery needs no running agent.
//! * Any other request is buffered (up to [`Settings::max_body_bytes`], `413` past it), then waits on **one wake per agent**
//!   (single flight: concurrent requests for one agent make one call to the [`Waker`]) and is forwarded **once** through
//!   the [`Upstream`] when the agent is `Ready`.
//! * At the deadline ([`Settings::wake_timeout`], or the `Waker` saying `TimedOut`) the answer is `503` with `Retry-After`.
//!
//! It makes no other decision. There is **no authentication here**: AD-058 gives the activator none, the agent checks
//! the caller's token itself and the request is forwarded with its headers untouched. Which agent a request is for comes
//! from the `Host` header (`<name>.<namespace>[.…]`, what a Kubernetes Service name gives); the agent's own Service is
//! what selects the activator while it sleeps.
//!
//! The deadline is owned by this crate (see [`Waker`]); an adapter that knows the agent's own `wakeTimeout` answers
//! `TimedOut` at it. A wake that is in flight is not cancelled when its waiters go away, because asking the operator to
//! wake is idempotent and the next request would only ask again.
//!
//! Routing by channel (AD-059, R5) will be a step between the request and [`Upstream::forward`]: [`Activator::route`] is
//! that step and always answers [`Route::Default`] today.

#![warn(missing_docs)]

mod metrics;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aap_ports::{
    CardSource, Route, RuntimeId, Upstream, UpstreamRequest, WakeError, WakeOutcome, Waker,
};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::header::{CONTENT_TYPE, HOST, RETRY_AFTER};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode};
use axum::response::Response;
use tokio::sync::watch;

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
    /// How long a wake may take before the caller is told to retry. §21a's default for an agent's `wakeTimeout` is 60 s.
    pub wake_timeout: Duration,
    /// The seconds the `Retry-After` header says.
    pub retry_after_secs: u32,
    /// The paths answered from the [`CardSource`], for `GET` and `HEAD`.
    pub card_paths: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_body_bytes: 1024 * 1024,
            wake_timeout: Duration::from_secs(60),
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
    Refused,
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
    pub fn new(waker: W, upstream: U, cards: C, settings: Settings) -> Self {
        Self {
            inner: Arc::new(Inner {
                waker: Arc::new(waker),
                upstream,
                cards,
                settings,
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
                        Ok(Err(WakeError::Refused(_))) => Flight::Refused,
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

/// `<name>.<namespace>[.…]`, with an optional port, as a `Host` header of a Kubernetes Service. Anything else names no agent.
fn agent_from_host(host: &str) -> Option<RuntimeId> {
    let host = host.rsplit_once(':').map_or(host, |(h, p)| {
        if p.bytes().all(|b| b.is_ascii_digit()) {
            h
        } else {
            host
        }
    });
    let mut labels = host.split('.');
    let (name, scope) = (labels.next()?, labels.next()?);
    let ok = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    };
    (ok(name) && ok(scope)).then(|| RuntimeId::new(scope, name))
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
    let named: Vec<String> = headers
        .iter()
        .filter(|(k, _)| k == "connection")
        .flat_map(|(_, v)| v.split(',').map(|t| t.trim().to_ascii_lowercase()))
        .collect();
    headers
        .iter()
        .filter(|(k, _)| !HOP_BY_HOP.contains(&k.as_str()) && !named.contains(k))
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
        .and_then(agent_from_host)
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
                Outcome::CardMissing,
            ),
        };
    }

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
    let body = match to_bytes(body, s.max_body_bytes).await {
        Ok(b) => b,
        Err(e) => {
            // `to_bytes` reports a length-limit error as a boxed `LengthLimitError`; the one other failure is the
            // peer going away mid-body.
            let too_large = std::error::Error::source(&e)
                .is_some_and(|s| s.to_string().contains("length limit"));
            return if too_large {
                (
                    reply(StatusCode::PAYLOAD_TOO_LARGE, "body_too_large", None),
                    Outcome::TooLarge,
                )
            } else {
                (
                    reply(StatusCode::BAD_REQUEST, "body_unreadable", None),
                    Outcome::BadRequest,
                )
            };
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
        Flight::Refused => {
            return (
                reply(StatusCode::BAD_GATEWAY, "wake_refused", None),
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
    match a.inner.upstream.forward(&agent, &route, forwarded).await {
        Ok(answer) => {
            let status = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::BAD_GATEWAY);
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
        Err(_) => (
            reply(StatusCode::BAD_GATEWAY, "upstream_unavailable", None),
            Outcome::UpstreamFailed,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_the_agent() {
        assert_eq!(
            agent_from_host("coder.team-a.svc.cluster.local:8080"),
            Some(RuntimeId::new("team-a", "coder"))
        );
        assert_eq!(
            agent_from_host("coder.team-a"),
            Some(RuntimeId::new("team-a", "coder"))
        );
        assert_eq!(agent_from_host("localhost"), None);
        assert_eq!(agent_from_host("Coder.team-a"), None);
        assert_eq!(agent_from_host(".team-a"), None);
        assert_eq!(agent_from_host("a b.c"), None);
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
