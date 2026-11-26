//! The source against a stub agent on a local port: what it asks, what it refuses, and that no failure is idleness.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aap_activity_http::{Bearer, HttpActivity, MAX_BODY};
use aap_ports::{Activity, ActivityError, ActivitySource, RuntimeId};
use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;

type Case = Box<dyn Fn() -> axum::response::Response + Send + Sync>;
type Seen = Arc<Mutex<Vec<Option<String>>>>;

async fn stub(
    reply: impl Fn() -> axum::response::Response + Clone + Send + Sync + 'static,
) -> (String, Seen) {
    let seen: Seen = Arc::default();
    let log = seen.clone();
    let app = Router::new().route(
        "/internal/activity",
        get(move |headers: HeaderMap| {
            let reply = reply.clone();
            let log = log.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                log.lock().unwrap().push(auth);
                reply()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

fn source(base: String, bearer: Option<Bearer>) -> HttpActivity {
    HttpActivity::with_base(move |_| base.clone(), Duration::from_secs(2), bearer).unwrap()
}

fn id() -> RuntimeId {
    RuntimeId::new("ns", "svc")
}

fn json(
    body: &'static str,
) -> impl Fn() -> axum::response::Response + Clone + Send + Sync + 'static {
    move || ([("content-type", "application/json")], body).into_response()
}

#[tokio::test]
async fn a_busy_agent_with_a_schedule() {
    let (base, seen) = stub(json(r#"{"busy":2,"nextDueAt":1790000000}"#)).await;
    let got = source(base, Bearer::new("tok\n"))
        .poll(&id())
        .await
        .unwrap();
    assert_eq!(
        got,
        Activity {
            busy: 2,
            next_due_at: Some(1_790_000_000)
        }
    );
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        [Some("Bearer tok".to_owned())]
    );
}

#[tokio::test]
async fn an_idle_agent_with_nothing_scheduled() {
    let (base, seen) = stub(json(r#"{"busy":0}"#)).await;
    assert_eq!(
        source(base, None).poll(&id()).await.unwrap(),
        Activity::IDLE
    );
    assert_eq!(seen.lock().unwrap().as_slice(), [None]);
}

#[tokio::test]
async fn every_failure_is_an_error_and_never_idle() {
    let cases: Vec<(&str, Case)> = vec![
        ("401", Box::new(|| StatusCode::UNAUTHORIZED.into_response())),
        (
            "500",
            Box::new(|| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        ),
        (
            "redirect",
            Box::new(|| (StatusCode::FOUND, [("location", "/x")]).into_response()),
        ),
        ("text", Box::new(|| "busy".into_response())),
        (
            "wrong type",
            Box::new(|| r#"{"busy":"no"}"#.into_response()),
        ),
        ("negative", Box::new(|| r#"{"busy":-1}"#.into_response())),
        (
            "unknown field",
            Box::new(|| r#"{"busy":0,"runs":[]}"#.into_response()),
        ),
        ("empty", Box::new(|| r"{}".into_response())),
    ];
    for (name, case) in cases {
        let case = Arc::new(case);
        let (base, _) = stub(move || case()).await;
        let got = source(base, None).poll(&id()).await;
        assert!(got.is_err(), "{name} must be an error, got {got:?}");
    }
}

#[tokio::test]
async fn an_oversized_answer_is_refused() {
    let big = "x".repeat(MAX_BODY + 1);
    let (base, _) = stub(move || big.clone().into_response()).await;
    assert!(matches!(
        source(base, None).poll(&id()).await,
        Err(ActivityError::Invalid(_))
    ));
}

#[tokio::test]
async fn an_unreachable_agent_is_unavailable() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let got = source(format!("http://{addr}"), None).poll(&id()).await;
    assert!(matches!(got, Err(ActivityError::Unavailable(_))));
}

#[tokio::test]
async fn a_slow_agent_times_out() {
    let (base, _) = stub(|| {
        std::thread::sleep(Duration::from_millis(600));
        "{}".into_response()
    })
    .await;
    let slow =
        HttpActivity::with_base(move |_| base.clone(), Duration::from_millis(100), None).unwrap();
    assert!(slow.poll(&id()).await.is_err());
}

#[tokio::test]
async fn the_token_and_the_url_stay_out_of_errors_and_debug() {
    let (base, _) = stub(|| StatusCode::UNAUTHORIZED.into_response()).await;
    let s = source(base.clone(), Bearer::new("s3cret-token"));
    let err = format!(
        "{:?} {}",
        s.poll(&id()).await.unwrap_err(),
        s.poll(&id()).await.unwrap_err()
    );
    assert!(!err.contains("s3cret") && !err.contains(&base), "{err}");
    assert!(!format!("{s:?}").contains("s3cret"));
    assert!(!format!("{:?}", Bearer::new("s3cret-token").unwrap()).contains("s3cret"));
}

#[test]
fn in_cluster_names_the_service() {
    // Built, not asked: the address is the service's cluster name on the health port.
    assert!(HttpActivity::in_cluster(8080, Duration::from_secs(1), None).is_ok());
}
