//! The two servers: the API on its port, and `/healthz` and `/readyz` on another.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use futures::future::BoxFuture;

/// How long a readiness check may take before the pod is not ready.
const READY_TIMEOUT: Duration = Duration::from_secs(3);

/// What `/readyz` asks. A closed word per failure: the endpoint is for a probe, and says why in a word, never in the
/// cluster's or the issuer's own.
pub type Check = Arc<dyn Fn() -> BoxFuture<'static, Result<(), &'static str>> + Send + Sync>;

/// The state of the health endpoints.
#[derive(Clone)]
pub struct Health {
    /// Set when the process has been asked to stop: it takes no new requests from the Service from then on.
    pub draining: Arc<AtomicBool>,
    /// Whether the API can answer a request now.
    pub check: Check,
}

/// `/healthz`: the process is up (liveness). `/readyz`: it can answer a request.
pub fn health(state: Health) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/readyz", get(ready))
        .with_state(state)
}

async fn ready(State(state): State<Health>) -> impl IntoResponse {
    if state.draining.load(Ordering::Acquire) {
        return (StatusCode::SERVICE_UNAVAILABLE, "shutting down\n");
    }
    match tokio::time::timeout(READY_TIMEOUT, (state.check)()).await {
        Ok(Ok(())) => (StatusCode::OK, "ready\n"),
        Ok(Err(why)) => (StatusCode::SERVICE_UNAVAILABLE, why),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "the check timed out\n"),
    }
}

/// Serve `router` on `listener` until `shutdown` resolves, then let the requests in flight finish.
pub async fn serve(
    listener: tokio::net::TcpListener,
    router: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let addr = listener.local_addr().context("the listening address")?;
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

    async fn get(router: Router, path: &str) -> (StatusCode, String) {
        let res = router
            .oneshot(Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let body = axum::body::to_bytes(res.into_body(), 1 << 16)
            .await
            .unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    fn state(check: Check) -> Health {
        Health {
            draining: Arc::new(AtomicBool::new(false)),
            check,
        }
    }

    #[tokio::test]
    async fn healthz_is_up_and_readyz_follows_the_check() {
        let ok = Arc::new(AtomicBool::new(false));
        let flag = ok.clone();
        let router = health(state(Arc::new(move || {
            let up = flag.load(Ordering::Acquire);
            Box::pin(async move {
                if up {
                    Ok(())
                } else {
                    Err("the keys are not available\n")
                }
            })
        })));
        assert_eq!(get(router.clone(), "/healthz").await.0, StatusCode::OK);
        let (status, body) = get(router.clone(), "/readyz").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body, "the keys are not available\n");
        ok.store(true, Ordering::Release);
        assert_eq!(get(router.clone(), "/readyz").await.0, StatusCode::OK);
        assert_eq!(
            get(router, "/v1/me").await.0,
            StatusCode::NOT_FOUND,
            "the API is not on this port"
        );
    }

    #[tokio::test]
    async fn a_pod_that_is_draining_is_not_ready_and_is_still_alive() {
        let s = state(Arc::new(|| Box::pin(async { Ok(()) })));
        let router = health(s.clone());
        assert_eq!(get(router.clone(), "/readyz").await.0, StatusCode::OK);
        s.draining.store(true, Ordering::Release);
        assert_eq!(
            get(router.clone(), "/readyz").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(get(router, "/healthz").await.0, StatusCode::OK);
    }

    #[tokio::test(start_paused = true)]
    async fn a_check_that_never_answers_is_not_ready() {
        let router = health(state(Arc::new(|| Box::pin(std::future::pending()))));
        assert_eq!(
            get(router, "/readyz").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
