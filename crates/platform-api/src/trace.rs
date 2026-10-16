//! One `tracing` span and one event per request, and the headers every answer carries.
//!
//! The event holds the **method, the route template, the status and the latency**; the subject is added to the span once the
//! token has verified. The template (`/v1/agents/{name}`), not the path: it has a bounded set of values and holds nothing a
//! person typed. A name is not a secret, but a path of a request that matched nothing is whatever a client sent, and it is
//! not logged. **Never logged: the token, a header, a body, the answer.**

use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, HeaderName};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

const NOSNIFF: HeaderName = HeaderName::from_static("x-content-type-options");

/// The span and the event of a request.
pub(crate) async fn request(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "-".to_owned(), |m| m.as_str().to_owned());
    let span = tracing::info_span!(
        "request",
        %method,
        route = %route,
        subject = tracing::field::Empty,
        status = tracing::field::Empty,
        latency_ms = tracing::field::Empty,
    );
    let started = Instant::now();
    let response = next.run(request).instrument(span.clone()).await;
    let status = response.status();
    span.record("status", status.as_u16());
    span.record(
        "latency_ms",
        u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    );
    span.in_scope(|| {
        if status.is_server_error() {
            tracing::warn!("request");
        } else {
            tracing::info!("request");
        }
    });
    response
}

/// Every answer: not to be cached (it holds one person's view of the cluster) and not to be sniffed as another type.
pub(crate) async fn harden(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(NOSNIFF, HeaderValue::from_static("nosniff"));
    response
}
