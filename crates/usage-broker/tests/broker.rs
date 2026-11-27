//! `BrokerAccounts` against a stub broker on a local port: what it sends (one raw path segment, the bearer) and what each
//! answer means.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aap_usage::{AccountSource, AccountUnavailable, PodRef};
use aap_usage_broker::{BrokerAccounts, MAX_BODY, Token};
use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use url::Url;

#[derive(Default)]
struct Stub {
    status: u16,
    body: String,
    delay: Duration,
    /// The raw path and the authorization header of each request.
    seen: Vec<(String, Option<String>)>,
}

type Shared = Arc<Mutex<Stub>>;

async fn answer(State(s): State<Shared>, uri: Uri, headers: HeaderMap) -> (StatusCode, String) {
    let (status, body, delay) = {
        let mut s = s.lock().unwrap();
        s.seen.push((
            uri.path().to_owned(),
            headers
                .get("authorization")
                .map(|v| v.to_str().unwrap().to_owned()),
        ));
        (s.status, s.body.clone(), s.delay)
    };
    tokio::time::sleep(delay).await;
    (StatusCode::from_u16(status).unwrap(), body)
}

async fn start(timeout: Duration) -> (BrokerAccounts, Shared) {
    let stub: Shared = Arc::new(Mutex::new(Stub {
        status: 200,
        body: r#"{"accountId":"acct-1"}"#.into(),
        ..Stub::default()
    }));
    let app = Router::new().fallback(answer).with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = Url::parse(&format!("http://{addr}/")).unwrap();
    let source = BrokerAccounts::new(&base, Token::new("meter-token").unwrap(), timeout).unwrap();
    (source, stub)
}

fn run(id: &str) -> PodRef {
    PodRef::Run {
        service: "coder".into(),
        run: id.into(),
    }
}

fn set(stub: &Shared, status: u16, body: &str) {
    let mut s = stub.lock().unwrap();
    s.status = status;
    s.body = body.to_owned();
}

#[tokio::test]
async fn it_asks_one_encoded_segment_with_the_bearer() {
    let (source, stub) = start(Duration::from_secs(5)).await;
    assert_eq!(
        source.account(&run("job/1:a.b")).await,
        Ok(Some("acct-1".to_owned()))
    );
    let s = stub.lock().unwrap();
    assert_eq!(
        s.seen,
        vec![(
            "/internal/runs/job%2F1%3Aa%2Eb/account".to_owned(),
            Some("Bearer meter-token".to_owned())
        )]
    );
}

#[tokio::test]
async fn a_run_id_that_cannot_be_a_segment_is_not_asked() {
    let (source, stub) = start(Duration::from_secs(5)).await;
    for id in ["..", ".", "", "a@b", "a b"] {
        assert_eq!(source.account(&run(id)).await, Ok(None), "{id:?}");
    }
    assert!(stub.lock().unwrap().seen.is_empty());
}

#[tokio::test]
async fn an_agent_pod_has_no_source_yet() {
    let (source, stub) = start(Duration::from_secs(5)).await;
    let agent = PodRef::Agent {
        service: "coder".into(),
    };
    assert_eq!(source.account(&agent).await, Ok(None));
    assert!(stub.lock().unwrap().seen.is_empty());
}

#[tokio::test]
async fn only_the_two_codes_that_say_no_account_are_no_account() {
    let (source, stub) = start(Duration::from_secs(5)).await;
    for (status, code) in [(404, "run_not_bound"), (422, "account_not_opaque")] {
        set(&stub, status, &format!(r#"{{"code":"{code}"}}"#));
        assert_eq!(source.account(&run("r")).await, Ok(None), "{status}");
    }
}

#[tokio::test]
async fn any_other_404_422_or_400_is_held_so_a_wrong_identity_is_seen() {
    let (source, stub) = start(Duration::from_secs(5)).await;
    let cases = [
        (404, r#"{"code":"account_not_opaque"}"#),
        (404, r#"{"code":"forbidden"}"#),
        (404, r#"{"error":"run_not_bound"}"#),
        (404, "not json"),
        (404, ""),
        (422, r#"{"code":"run_not_bound"}"#),
        (422, r#"{"code":"other"}"#),
        (400, r#"{"code":"run_not_bound"}"#),
        (400, r#"{"code":"account_not_opaque"}"#),
    ];
    for (status, body) in cases {
        set(&stub, status, body);
        assert_eq!(
            source.account(&run("r")).await,
            Err(AccountUnavailable),
            "{status} {body}"
        );
    }
}

#[tokio::test]
async fn an_answer_time_or_a_person_can_fix_is_unavailable() {
    let (source, stub) = start(Duration::from_secs(5)).await;
    for status in [301, 302, 401, 403, 408, 429, 500, 502, 503] {
        set(&stub, status, r#"{"accountId":"acct-1"}"#);
        assert_eq!(
            source.account(&run("r")).await,
            Err(AccountUnavailable),
            "{status}"
        );
    }
}

#[tokio::test]
async fn a_body_that_is_not_the_contract_is_unavailable_never_an_account() {
    let (source, stub) = start(Duration::from_secs(5)).await;
    let long = format!(r#"{{"accountId":"a","pad":"{}"}}"#, "x".repeat(MAX_BODY));
    for body in [
        "",
        "not json",
        "{}",
        r#"{"accountId":5}"#,
        r#"{"accountId":""}"#,
        // An e-mail is not an opaque id: it is never taken, whatever the broker says.
        r#"{"accountId":"person@example.com"}"#,
        r#"{"accountId":"has space"}"#,
        long.as_str(),
    ] {
        set(&stub, 200, body);
        assert_eq!(
            source.account(&run("r")).await,
            Err(AccountUnavailable),
            "{}",
            &body[..body.len().min(40)]
        );
    }
}

#[tokio::test]
async fn a_slow_or_absent_broker_is_unavailable() {
    let (source, stub) = start(Duration::from_millis(200)).await;
    stub.lock().unwrap().delay = Duration::from_secs(2);
    assert_eq!(source.account(&run("r")).await, Err(AccountUnavailable));

    let gone = BrokerAccounts::new(
        &Url::parse("http://127.0.0.1:1/").unwrap(),
        Token::new("t").unwrap(),
        Duration::from_secs(1),
    )
    .unwrap();
    assert_eq!(gone.account(&run("r")).await, Err(AccountUnavailable));
}
