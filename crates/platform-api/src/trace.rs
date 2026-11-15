//! One `tracing` span and one event per request, and the headers every answer carries.
//!
//! The event holds the **method, the route template, the status and the latency**; the subject is added to the span once the
//! token has verified. The template (`/v1/agents/{name}`), not the path: it has a bounded set of values and holds nothing a
//! person typed. A name is not a secret, but a path of a request that matched nothing is whatever a client sent, and it is
//! not logged. **Never logged: the token, a header, a body, the answer.**

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, HeaderName};
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;

use aap_trace::{Attr, Method, RouteTemplate, SpanKind, SpanName, Tracer};

use crate::metrics::ApiMetrics;
use crate::problem::ProblemSlug;

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

/// One count per request, with the template that matched (or none), the method and the status. Outside the guard, so a refusal
/// is counted too.
pub(crate) async fn count(
    State(metrics): State<Arc<ApiMetrics>>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let template = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned());
    let response = next.run(request).await;
    metrics.request(template.as_deref(), &method, response.status().as_u16());
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

/// The span of a request (K5): kind SERVER, named `{METHOD} {template}`, or `{METHOD}` when nothing matched. **The raw path
/// is never read**, so a path a person typed is in no name and no attribute; the template is the table's own
/// `&'static str`. The status, the slug of a problem answer and (from the guard and the ports) the auth result, the missing
/// permission and the children are added while it runs. A request is always a root: no inbound `traceparent` is read.
pub(crate) async fn span(State(tracer): State<Tracer>, request: Request, next: Next) -> Response {
    if !tracer.enabled() {
        return next.run(request).await;
    }
    let method = Method::from_http(request.method().as_str());
    let template = request
        .extensions()
        .get::<MatchedPath>()
        .and_then(|m| crate::routes::template(m.as_str()))
        .and_then(RouteTemplate::new);
    let name = match template {
        Some(t) => SpanName::route(method, t),
        None => SpanName::method(method),
    };
    let span = tracer.root(name, SpanKind::Server);
    span.set(Attr::HttpMethod(method));
    if let Some(t) = template {
        span.set(Attr::HttpRoute(t));
    }
    let response = span.scope(next.run(request)).await;
    let status = response.status().as_u16();
    span.set(Attr::HttpStatus(status));
    if let Some(ProblemSlug(slug)) = response.extensions().get::<ProblemSlug>().copied() {
        if let Some(a) = Attr::problem_type(slug) {
            span.set(a);
        }
        if status >= 500
            && let Some(a) = Attr::error_type(slug)
        {
            span.set(a);
        }
    }
    if status >= 500 {
        span.fail();
    }
    span.end();
    response
}
