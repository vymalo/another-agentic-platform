//! `HttpActivity` against a stub agent on a local port: the shared conformance suite, then what the wire adds.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aap_activity_http::{HttpActivity, MAX_BODY, Token};
use aap_ports::testkit::ActivityUnderTest;
use aap_ports::{Activity, ActivityError, ActivitySource, RuntimeId, UnavailableCause};
use axum::Router;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;

#[derive(Default)]
struct Stub {
    /// service path -> (status, body)
    answers: BTreeMap<String, (u16, String)>,
    bearers: Vec<Option<String>>,
}

type Shared = Arc<Mutex<Stub>>;

async fn serve(
    State(s): State<Shared>,
    Path((scope, name)): Path<(String, String)>,
    headers: HeaderMap,
) -> (StatusCode, String) {
    let mut s = s.lock().unwrap();
    s.bearers.push(
        headers
            .get("authorization")
            .map(|v| v.to_str().unwrap().to_owned()),
    );
    match s.answers.get(&format!("{scope}/{name}")) {
        Some((code, body)) => (StatusCode::from_u16(*code).unwrap(), body.clone()),
        None => (StatusCode::SERVICE_UNAVAILABLE, String::new()),
    }
}

struct Harness {
    source: HttpActivity,
    stub: Shared,
}

async fn start(token: Option<&str>, timeout: Duration) -> Harness {
    let stub: Shared = Arc::default();
    let app = Router::new()
        .route("/{scope}/{name}/internal/activity", get(serve))
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let source = HttpActivity::with_resolver(
        move |id| {
            format!(
                "http://{addr}/{}/{}/internal/activity",
                id.scope(),
                id.name()
            )
        },
        timeout,
        token.map(|t| Token::new(t).unwrap()),
    )
    .unwrap();
    Harness { source, stub }
}

impl Harness {
    fn answer(&self, id: &RuntimeId, code: u16, body: &str) {
        self.stub.lock().unwrap().answers.insert(
            format!("{}/{}", id.scope(), id.name()),
            (code, body.to_owned()),
        );
    }
}

fn id() -> RuntimeId {
    RuntimeId::new("agents", "coder")
}

impl ActivitySource for Harness {
    async fn poll(&self, id: &RuntimeId) -> Result<Activity, ActivityError> {
        self.source.poll(id).await
    }
}

impl ActivityUnderTest for Harness {
    async fn set(&self, id: &RuntimeId, activity: Activity) {
        let mut body = format!("{{\"busy\":{}", activity.busy);
        if let Some(at) = activity.next_due_at {
            // The agent writes RFC 3339; the stub does the same from the seconds the case set.
            body.push_str(&format!(",\"nextDueAt\":\"{}\"", rfc3339(at)));
        }
        body.push('}');
        self.answer(id, 200, &body);
    }

    async fn break_source(&self, id: &RuntimeId) {
        self.stub
            .lock()
            .unwrap()
            .answers
            .remove(&format!("{}/{}", id.scope(), id.name()));
    }
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SSZ` (test side only, the inverse of the crate's parser).
fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

async fn make() -> Option<Harness> {
    Some(start(Some("t0ken"), Duration::from_secs(5)).await)
}

mod conformance {
    aap_ports::activity_source_conformance!(super::make);
}

#[tokio::test]
async fn the_bearer_is_sent_and_no_token_means_no_header() {
    let h = start(Some("secret-value"), Duration::from_secs(5)).await;
    h.answer(&id(), 200, r#"{"busy":1}"#);
    h.source.poll(&id()).await.unwrap();
    let h2 = start(None, Duration::from_secs(5)).await;
    h2.answer(&id(), 200, r#"{"busy":0}"#);
    h2.source.poll(&id()).await.unwrap();
    assert_eq!(
        h.stub.lock().unwrap().bearers,
        vec![Some("Bearer secret-value".to_owned())]
    );
    assert_eq!(h2.stub.lock().unwrap().bearers, vec![None]);
}

/// ANR-1192: the bearer is asked at each poll, so a rotated token is sent without a restart.
#[tokio::test]
async fn a_token_source_is_asked_at_each_poll() {
    let h = start(Some("first"), Duration::from_secs(5)).await;
    let now = Arc::new(Mutex::new("first".to_owned()));
    let asked = Arc::clone(&now);
    let source = h
        .source
        .clone()
        .with_token_source(move || Some(Token::new(&asked.lock().unwrap()).unwrap()));
    h.answer(&id(), 200, r#"{"busy":1}"#);
    source.poll(&id()).await.unwrap();
    *now.lock().unwrap() = "second".to_owned();
    source.poll(&id()).await.unwrap();
    assert_eq!(
        h.stub.lock().unwrap().bearers,
        vec![
            Some("Bearer first".to_owned()),
            Some("Bearer second".to_owned())
        ]
    );
}

/// Neither the message of an error nor of anything under it holds a URL or the token.
fn assert_says_no_url_and_no_token(err: &ActivityError, token: &str) {
    let mut next: Option<&dyn std::error::Error> = Some(err);
    while let Some(e) = next {
        let message = e.to_string();
        for secret in ["http://", "https://", token] {
            assert!(!message.contains(secret), "{message}");
        }
        next = e.source();
    }
}

#[tokio::test]
async fn a_401_and_a_403_are_refused_with_their_status() {
    let h = start(Some("secret-value"), Duration::from_secs(5)).await;
    for code in [401, 403] {
        h.answer(&id(), code, r#"{"busy":0}"#);
        let err = h.source.poll(&id()).await.unwrap_err();
        assert!(
            matches!(err, ActivityError::Refused { status } if status == code),
            "{code}: {err:?}"
        );
        assert_says_no_url_and_no_token(&err, "secret-value");
    }
}

#[tokio::test]
async fn another_status_is_unavailable_by_cause_and_never_idle() {
    let h = start(Some("secret-value"), Duration::from_secs(5)).await;
    for (code, wanted) in [
        (500, UnavailableCause::ServerError),
        (503, UnavailableCause::ServerError),
        (404, UnavailableCause::Other),
        (302, UnavailableCause::Other),
    ] {
        h.answer(&id(), code, r#"{"busy":0}"#);
        let err = h.source.poll(&id()).await.unwrap_err();
        assert!(
            matches!(err, ActivityError::Unavailable { cause, .. } if cause == wanted),
            "{code}: {err:?}"
        );
        assert_says_no_url_and_no_token(&err, "secret-value");
    }
}

#[tokio::test]
async fn a_malformed_answer_is_invalid() {
    let h = start(None, Duration::from_secs(5)).await;
    for body in [
        "",
        "busy",
        "[]",
        "{}",
        r#"{"busy":-1}"#,
        r#"{"busy":1.5}"#,
        r#"{"busy":"1"}"#,
        r#"{"busy":4294967296}"#,
        r#"{"busy":0,"nextDueAt":12}"#,
        r#"{"busy":0,"nextDueAt":"soon"}"#,
    ] {
        h.answer(&id(), 200, body);
        assert!(
            matches!(h.source.poll(&id()).await, Err(ActivityError::Invalid(_))),
            "{body}"
        );
    }
}

#[tokio::test]
async fn a_null_or_absent_schedule_is_none_and_a_fraction_and_an_offset_are_read() {
    let h = start(None, Duration::from_secs(5)).await;
    h.answer(&id(), 200, r#"{"busy":3,"nextDueAt":null}"#);
    assert_eq!(
        h.source.poll(&id()).await.unwrap(),
        Activity {
            busy: 3,
            next_due_at: None
        }
    );
    h.answer(
        &id(),
        200,
        r#"{"busy":0,"nextDueAt":"2026-11-26T02:00:00.5+01:00","extra":true}"#,
    );
    assert_eq!(
        h.source.poll(&id()).await.unwrap(),
        Activity {
            busy: 0,
            next_due_at: Some(1_795_654_800)
        }
    );
}

#[tokio::test]
async fn an_answer_that_is_too_long_is_refused() {
    let h = start(None, Duration::from_secs(5)).await;
    h.answer(
        &id(),
        200,
        &format!(r#"{{"busy":0,"pad":"{}"}}"#, "x".repeat(MAX_BODY)),
    );
    assert!(matches!(
        h.source.poll(&id()).await,
        Err(ActivityError::Invalid(_))
    ));
}

#[tokio::test]
async fn nothing_listening_is_connect_and_a_slow_agent_is_timeout() {
    let closed = HttpActivity::with_resolver(
        |_| "http://127.0.0.1:9/internal/activity".into(),
        Duration::from_secs(1),
        Some(Token::new("secret-value").unwrap()),
    )
    .unwrap();
    let err = closed.poll(&id()).await.unwrap_err();
    assert!(
        matches!(
            err,
            ActivityError::Unavailable {
                cause: UnavailableCause::Connect,
                ..
            }
        ),
        "{err:?}"
    );
    assert_says_no_url_and_no_token(&err, "secret-value");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Accepts and never answers.
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = listener.accept().await {
            held.push(s);
        }
    });
    let slow = HttpActivity::with_resolver(
        move |_| format!("http://{addr}/internal/activity"),
        Duration::from_millis(300),
        Some(Token::new("secret-value").unwrap()),
    )
    .unwrap();
    let err = slow.poll(&id()).await.unwrap_err();
    assert!(
        matches!(
            err,
            ActivityError::Unavailable {
                cause: UnavailableCause::Timeout,
                ..
            }
        ),
        "{err:?}"
    );
    assert_says_no_url_and_no_token(&err, "secret-value");
}

#[test]
fn settings_are_checked_and_the_token_prints_nothing() {
    assert!(Token::new("  ").is_err());
    assert!(Token::new("a\nb").is_err());
    assert_eq!(format!("{:?}", Token::new("hunter2").unwrap()), "Token(..)");
    assert!(HttpActivity::in_cluster(8080, Duration::ZERO, None).is_err());
    let source = HttpActivity::in_cluster(
        8080,
        Duration::from_secs(3),
        Some(Token::new("hunter2").unwrap()),
    )
    .unwrap();
    assert!(!format!("{source:?}").contains("hunter2"));
}
