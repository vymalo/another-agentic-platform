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
use tokio::sync::Mutex;
use tokio::time::Instant;

/// How long a readiness check may take before the pod is not ready.
const READY_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the answer of a readiness check is kept. `/readyz` is open to whoever can reach the health port, and a check
/// calls the issuer and the cluster: the answer is shared, and asked again only when it is this old.
const READY_CACHE: Duration = Duration::from_secs(5);

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

/// What the health endpoints share: whether the pod is draining, and the check with the last answer it gave.
struct Shared {
    draining: Arc<AtomicBool>,
    check: Check,
    /// The last answer and when it was given. Held across the check, so that probes that arrive while it runs wait for its
    /// answer and do not start their own: one check at a time.
    last: Mutex<Option<(Instant, Result<(), &'static str>)>>,
}

impl Shared {
    /// The answer of the check: the kept one while it is young, else a new one (a check that takes too long is not ready,
    /// and that too is kept, so a hung issuer is asked once and not once per probe).
    async fn ready(&self) -> Result<(), &'static str> {
        let mut last = self.last.lock().await;
        if let Some((at, answer)) = *last
            && at.elapsed() < READY_CACHE
        {
            return answer;
        }
        let answer = match tokio::time::timeout(READY_TIMEOUT, (self.check)()).await {
            Ok(answer) => answer,
            Err(_) => Err("the check timed out\n"),
        };
        *last = Some((Instant::now(), answer));
        answer
    }
}

/// `/healthz`: the process is up (liveness). `/readyz`: it can answer a request.
pub fn health(state: Health) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/readyz", get(ready))
        .with_state(Arc::new(Shared {
            draining: state.draining,
            check: state.check,
            last: Mutex::new(None),
        }))
}

async fn ready(State(state): State<Arc<Shared>>) -> impl IntoResponse {
    // A pod that is stopping is not ready at once, whatever the last answer was.
    if state.draining.load(Ordering::Acquire) {
        return (StatusCode::SERVICE_UNAVAILABLE, "shutting down\n");
    }
    match state.ready().await {
        Ok(()) => (StatusCode::OK, "ready\n"),
        Err(why) => (StatusCode::SERVICE_UNAVAILABLE, why),
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
    use std::sync::atomic::AtomicUsize;

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

    #[tokio::test(start_paused = true)]
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
        // The answer is kept for a few seconds (below); it follows the check once they are over.
        tokio::time::advance(READY_CACHE + Duration::from_millis(1)).await;
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

    /// A check that counts the times it is asked, takes 200 ms and answers what `answer` holds.
    fn counted(asked: &Arc<AtomicUsize>, answer: Arc<AtomicBool>) -> Check {
        let asked = asked.clone();
        Arc::new(move || {
            let asked = asked.clone();
            let up = answer.load(Ordering::Acquire);
            Box::pin(async move {
                asked.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(200)).await;
                if up { Ok(()) } else { Err("not ready\n") }
            })
        })
    }

    #[tokio::test(start_paused = true)]
    async fn a_burst_of_probes_asks_the_check_once_and_the_answer_is_kept_for_a_few_seconds() {
        // `/readyz` is open to whoever reaches the port, and the check calls the issuer and the cluster: the answer is
        // shared, and asked again only when it is old.
        let asked = Arc::new(AtomicUsize::new(0));
        let up = Arc::new(AtomicBool::new(true));
        let router = health(state(counted(&asked, up.clone())));

        let burst =
            futures::future::join_all((0..50).map(|_| get(router.clone(), "/readyz"))).await;
        assert!(burst.iter().all(|(status, _)| *status == StatusCode::OK));
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "50 probes at once, one check"
        );

        up.store(false, Ordering::Release);
        tokio::time::advance(READY_CACHE - Duration::from_secs(1)).await;
        for _ in 0..10 {
            assert_eq!(get(router.clone(), "/readyz").await.0, StatusCode::OK);
        }
        assert_eq!(asked.load(Ordering::SeqCst), 1, "kept until it is old");

        tokio::time::advance(Duration::from_secs(2)).await;
        let burst =
            futures::future::join_all((0..50).map(|_| get(router.clone(), "/readyz"))).await;
        assert!(burst.iter().all(|(status, body)| {
            *status == StatusCode::SERVICE_UNAVAILABLE && body == "not ready\n"
        }));
        assert_eq!(
            asked.load(Ordering::SeqCst),
            2,
            "asked again when old, once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_failure_is_kept_too_and_a_check_that_timed_out_is_not_asked_again_at_once() {
        let asked = Arc::new(AtomicUsize::new(0));
        let up = Arc::new(AtomicBool::new(false));
        let router = health(state(counted(&asked, up.clone())));
        assert_eq!(
            get(router.clone(), "/readyz").await.0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        up.store(true, Ordering::Release);
        assert_eq!(
            get(router.clone(), "/readyz").await.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "kept"
        );
        assert_eq!(asked.load(Ordering::SeqCst), 1);

        let never = Arc::new(AtomicUsize::new(0));
        let hung = {
            let never = never.clone();
            health(state(Arc::new(move || {
                never.fetch_add(1, Ordering::SeqCst);
                Box::pin(std::future::pending())
            })))
        };
        let burst = futures::future::join_all((0..20).map(|_| get(hung.clone(), "/readyz"))).await;
        assert!(burst.iter().all(|(status, body)| {
            *status == StatusCode::SERVICE_UNAVAILABLE && body == "the check timed out\n"
        }));
        assert_eq!(
            never.load(Ordering::SeqCst),
            1,
            "20 probes behind one that hangs, one check"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn draining_is_not_kept() {
        let asked = Arc::new(AtomicUsize::new(0));
        let s = state(counted(&asked, Arc::new(AtomicBool::new(true))));
        let router = health(s.clone());
        assert_eq!(get(router.clone(), "/readyz").await.0, StatusCode::OK);
        s.draining.store(true, Ordering::Release);
        assert_eq!(
            get(router, "/readyz").await.0,
            StatusCode::SERVICE_UNAVAILABLE,
            "a pod that is stopping is not ready at once, whatever the last answer was"
        );
        assert_eq!(asked.load(Ordering::SeqCst), 1);
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
