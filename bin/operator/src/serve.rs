//! The two small HTTP servers of the operator: health on 8081 (`/healthz`, `/readyz`) and metrics on
//! 9090 (`/metrics`, Prometheus text). `serve` also serves the registry's router on 8080 (`aap-registry`).

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use aap_controller::{Metrics, Readiness};
use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;

/// `/healthz`: the process is up (liveness). `/readyz`: the controllers have listed the cluster.
pub fn health(readiness: Readiness) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/readyz", get(ready))
        .with_state(readiness)
}

async fn ready(State(readiness): State<Readiness>) -> impl IntoResponse {
    if readiness.is_ready() {
        (StatusCode::OK, "ready\n")
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "the caches have not synced yet\n",
        )
    }
}

/// `/metrics`: the counters of the controllers in the Prometheus text format.
#[cfg(test)]
pub fn metrics(metrics: Arc<Metrics>) -> Router {
    metrics_with_traces(metrics, aap_trace::TraceMetrics::none())
}

/// [`metrics`], and when tracing is on (K5) the two counters of the trace pipeline after the controllers': with tracing off
/// the text is exactly what it was.
pub fn metrics_with_traces(metrics: Arc<Metrics>, traces: aap_trace::TraceMetrics) -> Router {
    Router::new()
        .route(
            "/metrics",
            get(
                |State((m, t)): State<(Arc<Metrics>, aap_trace::TraceMetrics)>| async move {
                    let mut text = m.render();
                    text.push_str(&t.render());
                    (
                        [(
                            header::CONTENT_TYPE,
                            "text/plain; version=0.0.4; charset=utf-8",
                        )],
                        text,
                    )
                },
            ),
        )
        .with_state((metrics, traces))
}

/// The `/metrics` router of a process whose tracer is `tracer`: the controllers' counters, then the pipeline's.
#[cfg(feature = "runtime-kubernetes")]
pub fn metrics_for(metrics: Arc<Metrics>, tracer: &aap_trace::Tracer) -> Router {
    metrics_with_traces(metrics, aap_trace::TraceMetrics::of(tracer))
}

/// Serve `router` on `addr` until `shutdown` resolves.
pub async fn serve(
    addr: SocketAddr,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(%addr, "listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await
        .with_context(|| format!("serving {addr}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn get(router: Router, path: &str) -> (StatusCode, String, Option<String>) {
        let res = router
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let content_type = res
            .headers()
            .get(header::CONTENT_TYPE)
            .map(|v| v.to_str().unwrap().to_owned());
        let body = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            String::from_utf8(body.to_vec()).unwrap(),
            content_type,
        )
    }

    #[tokio::test]
    async fn healthz_is_always_up_and_readyz_waits_for_the_caches() {
        let readiness = Readiness::default();
        let router = health(readiness.clone());
        assert_eq!(get(router.clone(), "/healthz").await.0, StatusCode::OK);
        assert_eq!(
            get(router.clone(), "/readyz").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            get(router.clone(), "/metrics").await.0,
            StatusCode::NOT_FOUND
        );
        // The controllers hold a clone of the same handle and mark it when the caches have synced.
        readiness.mark_ready();
        assert_eq!(get(router, "/readyz").await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_are_prometheus_text() {
        let m = Arc::new(Metrics::new());
        m.inc("aap_runtime_signals_total", &[]);
        let (status, body, content_type) = get(metrics(m), "/metrics").await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            content_type
                .unwrap()
                .starts_with("text/plain; version=0.0.4")
        );
        assert!(body.contains("aap_runtime_signals_total 1"), "{body}");
        assert!(
            !body.contains("aap_trace_"),
            "tracing is off: its series are not there: {body}"
        );
    }

    #[tokio::test]
    async fn the_trace_series_follow_the_controllers_when_tracing_is_on() {
        let m = Arc::new(Metrics::new());
        m.inc("aap_runtime_signals_total", &[]);
        let (tracer, _) = aap_trace::Tracer::in_memory(aap_trace::Sampler::AlwaysOn);
        let traces = aap_trace::TraceMetrics::of(&tracer);
        let (_, body, _) = get(metrics_with_traces(m, traces), "/metrics").await;
        let first = body.find("aap_runtime_signals_total 1").unwrap();
        let trace = body.find("aap_trace_spans_exported_total 0").unwrap();
        assert!(first < trace, "{body}");
        assert!(body.contains("aap_trace_spans_dropped_total{reason=\"queue_full\"} 0"));
    }

    #[tokio::test]
    async fn the_operators_metrics_carry_the_trace_series_of_its_tracer() {
        let (tracer, _) = aap_trace::Tracer::in_memory(aap_trace::Sampler::AlwaysOn);
        let (_, body, _) = get(metrics_for(Arc::new(Metrics::new()), &tracer), "/metrics").await;
        assert!(body.contains("aap_trace_enabled 1\n"), "{body}");
        assert!(body.contains("aap_trace_spans_exported_total 0"), "{body}");
        let (_, off, _) = get(
            metrics_for(Arc::new(Metrics::new()), &aap_trace::Tracer::off()),
            "/metrics",
        )
        .await;
        assert!(!off.contains("aap_trace_"), "{off}");
    }

    #[tokio::test]
    async fn a_tracing_setting_that_was_given_and_is_off_says_so_with_one_gauge() {
        let m = Arc::new(Metrics::new());
        let plain = get(
            metrics_with_traces(m.clone(), aap_trace::TraceMetrics::none()),
            "/metrics",
        )
        .await
        .1;
        let off = aap_trace::TraceMetrics::of(&aap_trace::Tracer::off_but_asked());
        let (_, body, _) = get(metrics_with_traces(m, off), "/metrics").await;
        assert!(
            body.starts_with(&plain),
            "the process's own text is first and unchanged: {body}"
        );
        assert!(body.contains("aap_trace_enabled 0\n"), "{body}");
        assert!(!body.contains("aap_trace_spans"), "{body}");
    }
}
