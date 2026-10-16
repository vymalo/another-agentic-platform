//! An issuer that is slow, or never answers: nobody waits for it who need not, a fetch is not lost with the
//! request that started it, and every request to the issuer is bounded by the timeout. Over real HTTP, against
//! servers on local ports whose delays a test sets.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use aap_auth_oidc::testkit::{Alg, Signing, TEST_AUDIENCE, TestIdp};
use aap_auth_oidc::{FetchFailure, KeysUnavailable, OidcAuthenticator, OidcConfig, Tuning};
use aap_ports::{AuthError, Authenticator, InvalidToken, Principal};
use axum::Router;
use axum::extract::State;
use axum::http::header::CONTENT_TYPE;
use axum::routing::get;
use common::{assert_accepted, assert_invalid, eventually, tuning};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::task::JoinHandle;

const SUBJECT: &str = "alice";

// ---- the servers ------------------------------------------------------------------------------------------------

struct Slow {
    upstream: String,
    http: reqwest::Client,
    delay_ms: AtomicU64,
    hang: AtomicBool,
    started: AtomicUsize,
    answered: AtomicUsize,
}

async fn slow_keys(State(slow): State<Arc<Slow>>) -> impl axum::response::IntoResponse {
    slow.started.fetch_add(1, Ordering::SeqCst);
    if slow.hang.load(Ordering::SeqCst) {
        std::future::pending::<()>().await;
    }
    tokio::time::sleep(Duration::from_millis(slow.delay_ms.load(Ordering::SeqCst))).await;
    let body = slow
        .http
        .get(&slow.upstream)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    slow.answered.fetch_add(1, Ordering::SeqCst);
    ([(CONTENT_TYPE, "application/json")], body)
}

/// The key endpoint of an issuer in front of a `TestIdp`'s: it serves what the test issuer publishes at that
/// moment, after a delay a test sets, or never.
struct SlowKeys {
    slow: Arc<Slow>,
    url: String,
    server: JoinHandle<()>,
}

impl Drop for SlowKeys {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl SlowKeys {
    async fn start(idp: &TestIdp) -> Self {
        let slow = Arc::new(Slow {
            upstream: idp.jwks_url(),
            http: reqwest::Client::builder().no_proxy().build().unwrap(),
            delay_ms: AtomicU64::new(0),
            hang: AtomicBool::new(false),
            started: AtomicUsize::new(0),
            answered: AtomicUsize::new(0),
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/jwks", listener.local_addr().unwrap());
        let router = Router::new()
            .route("/jwks", get(slow_keys))
            .with_state(Arc::clone(&slow));
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        Self { slow, url, server }
    }

    fn set_delay(&self, delay: Duration) {
        self.slow
            .delay_ms
            .store(delay.as_millis() as u64, Ordering::SeqCst);
    }

    fn set_hang(&self, hang: bool) {
        self.slow.hang.store(hang, Ordering::SeqCst);
    }

    /// Requests that reached the server.
    fn started(&self) -> usize {
        self.slow.started.load(Ordering::SeqCst)
    }

    /// Requests the server has answered.
    fn answered(&self) -> usize {
        self.slow.answered.load(Ordering::SeqCst)
    }
}

/// A test issuer, the slow place its authenticator reads the keys from, and the authenticator.
struct Setup {
    idp: TestIdp,
    keys: SlowKeys,
    auth: OidcAuthenticator,
}

impl Setup {
    async fn new(adjust: impl FnOnce(&mut Tuning)) -> Self {
        let idp = TestIdp::start().await;
        let keys = SlowKeys::start(&idp).await;
        let mut tuning = tuning();
        adjust(&mut tuning);
        let config = idp.config().with_jwks_url(keys.url.clone());
        let auth = OidcAuthenticator::with_tuning(config, tuning).unwrap();
        Self { idp, keys, auth }
    }

    fn good(&self) -> String {
        self.idp.token(TEST_AUDIENCE, SUBJECT)
    }

    /// A token signed by the key the test issuer has not published, so that the copy lacks its `kid`.
    fn unknown(&self) -> String {
        self.idp.mint(
            &self.idp.claims(TEST_AUDIENCE, SUBJECT),
            Signing::Unpublished(Alg::Rs256),
        )
    }

    async fn check(&self, token: &str) -> Result<Principal, AuthError> {
        self.auth.authenticate(Some(token)).await
    }

    /// The request, running on its own.
    fn spawn(&self, token: String) -> JoinHandle<Result<Principal, AuthError>> {
        let auth = self.auth.clone();
        tokio::spawn(async move { auth.authenticate(Some(&token)).await })
    }
}

fn cause<T: std::fmt::Debug>(result: Result<T, AuthError>) -> KeysUnavailable {
    match result {
        Err(AuthError::Unavailable(source)) => *source
            .downcast::<KeysUnavailable>()
            .unwrap_or_else(|e| panic!("the source is not a KeysUnavailable: {e:?}")),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

/// A listener that accepts connections and does what `serve` says with each, keeping them open. Returns the
/// issuer's base URL.
async fn raw_server<F, Fut>(serve: F) -> String
where
    F: Fn(tokio::net::TcpStream) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(serve(stream));
        }
    });
    base
}

/// Reads the request line and headers of `stream`.
async fn read_request(stream: &mut tokio::net::TcpStream) {
    let mut seen = Vec::new();
    let mut buf = [0u8; 1024];
    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream.read(&mut buf).await.unwrap();
        if n == 0 {
            return;
        }
        seen.extend_from_slice(&buf[..n]);
    }
}

// ---- nobody waits who need not -----------------------------------------------------------------------------------

#[tokio::test]
async fn a_good_token_is_answered_while_an_unknown_kid_fetch_to_a_slow_issuer_is_running() {
    let s = Setup::new(|t| t.kid_refetch_interval = Duration::from_millis(50)).await;
    let good = s.good();
    assert_accepted(s.check(&good).await, "the first fetch is quick");
    assert_eq!(s.keys.started(), 1);
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The issuer is slow from now on, and a token names a key it does not have: one fetch, which takes 2.5 s.
    s.keys.set_delay(Duration::from_millis(2500));
    let unknown = s.spawn(s.unknown());
    eventually("the fetch to be running", || s.keys.started() == 2).await;

    // Everybody else goes on with the copy they have.
    let began = Instant::now();
    for _ in 0..5 {
        assert_accepted(s.check(&good).await, "a good token meanwhile");
    }
    let waited = began.elapsed();
    assert!(
        !unknown.is_finished() && waited < Duration::from_secs(1),
        "five good tokens took {waited:?}: they waited for the fetch an unknown kid started"
    );

    // Only the request that named the unknown key waits for the fetch, and is then told what it is.
    assert_invalid(
        unknown.await.unwrap(),
        InvalidToken::UnknownKey,
        "the unknown kid, after the fetch",
    );
    assert_eq!(s.keys.started(), 2);
}

#[tokio::test]
async fn unknown_kids_that_arrive_while_a_fetch_is_running_join_it() {
    let s = Setup::new(|t| t.kid_refetch_interval = Duration::from_millis(50)).await;
    assert_accepted(s.check(&s.good()).await, "the first fetch");
    tokio::time::sleep(Duration::from_millis(100)).await;

    s.keys.set_delay(Duration::from_millis(600));
    // The issuer rotates; sixteen requests name its new key at the same moment, then a few more while the
    // fetch is running.
    s.idp.publish_unpublished_keys();
    let mut requests = Vec::new();
    for _ in 0..16 {
        requests.push(s.spawn(s.unknown()));
    }
    eventually("the fetch to be running", || s.keys.started() == 2).await;
    for _ in 0..4 {
        requests.push(s.spawn(s.unknown()));
    }
    for request in requests {
        assert_accepted(
            request.await.unwrap(),
            "the rotated key, found by the fetch",
        );
    }
    assert_eq!(s.keys.started(), 2, "one fetch for all of them");
}

#[tokio::test]
async fn a_stale_copy_is_served_at_once_and_refreshed_in_the_background_once() {
    let s = Setup::new(|t| {
        t.refresh_interval = Duration::from_millis(200);
        t.stale_grace = Duration::from_secs(60);
        t.timeout = Duration::from_secs(3);
    })
    .await;
    let good = s.good();
    assert_accepted(s.check(&good).await, "the first fetch");
    tokio::time::sleep(Duration::from_millis(300)).await;

    // The copy is older than the refresh interval, and the issuer takes the request and never answers.
    s.keys.set_hang(true);
    let began = Instant::now();
    for _ in 0..8 {
        assert_accepted(s.check(&good).await, "the stale copy, within its grace");
    }
    let waited = began.elapsed();
    assert!(
        waited < Duration::from_secs(1),
        "eight tokens took {waited:?} with the issuer hanging: the stale copy was not served at once"
    );
    eventually("the refresh to be running", || s.keys.started() == 2).await;
    assert_eq!(s.keys.started(), 2, "one refresh, however many requests");
}

#[tokio::test]
async fn ready_returns_at_once_while_a_copy_can_verify_and_waits_when_there_is_none() {
    let s = Setup::new(|t| {
        t.refresh_interval = Duration::from_millis(200);
        t.stale_grace = Duration::from_secs(60);
        t.timeout = Duration::from_millis(800);
    })
    .await;
    s.auth.ready().await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    s.keys.set_hang(true);

    // Stale, inside the grace: ready, and a refresh is started in the background.
    let began = Instant::now();
    s.auth.ready().await.unwrap();
    assert!(
        began.elapsed() < Duration::from_millis(500),
        "ready waited {:?} for a refresh while a copy could verify",
        began.elapsed()
    );
    eventually("the refresh to be running", || s.keys.started() == 2).await;

    // No copy at all: ready has to wait for the fetch, which gives up at the timeout.
    let down = Setup::new(|t| t.timeout = Duration::from_millis(800)).await;
    down.keys.set_hang(true);
    let began = Instant::now();
    let error = down.auth.ready().await.unwrap_err();
    assert!(matches!(error, AuthError::Unavailable(_)), "{error:?}");
    assert!(
        began.elapsed() >= Duration::from_millis(700),
        "ready did not wait for the only fetch: {:?}",
        began.elapsed()
    );
}

#[tokio::test]
async fn the_first_requests_wait_for_the_one_fetch_and_are_unavailable_when_it_times_out() {
    let s = Setup::new(|t| t.timeout = Duration::from_millis(600)).await;
    s.keys.set_delay(Duration::from_millis(300));
    let good = s.good();
    // Six first requests at once: one fetch, which they all wait for.
    let began = Instant::now();
    let requests: Vec<_> = (0..6).map(|_| s.spawn(good.clone())).collect();
    for request in requests {
        assert_accepted(request.await.unwrap(), "after the one fetch");
    }
    assert!(began.elapsed() >= Duration::from_millis(250));
    assert_eq!(s.keys.started(), 1);

    // And when the issuer never answers, they all fail closed, after the timeout, and not before.
    let down = Setup::new(|t| t.timeout = Duration::from_millis(600)).await;
    down.keys.set_hang(true);
    let good = down.good();
    let began = Instant::now();
    let requests: Vec<_> = (0..4).map(|_| down.spawn(good.clone())).collect();
    for request in requests {
        let result = request.await.unwrap();
        assert_eq!(
            cause(result),
            KeysUnavailable::NoCopy(Some(FetchFailure::Timeout))
        );
    }
    assert!(began.elapsed() >= Duration::from_millis(550));
    assert_eq!(down.keys.started(), 1);
}

// ---- a request that gives up does not take the fetch with it ------------------------------------------------------

#[tokio::test]
async fn a_request_cancelled_during_a_slow_fetch_does_not_lose_the_keys_it_asked_for() {
    // An unknown kid earns one fetch per interval. If the fetch died with the request that started it, the
    // interval would be spent and the rotated key refused until it was over.
    let interval = Duration::from_millis(1500);
    let s = Setup::new(|t| t.kid_refetch_interval = interval).await;
    assert_accepted(s.check(&s.good()).await, "the first fetch");
    tokio::time::sleep(interval + Duration::from_millis(100)).await;

    // The issuer rotates, and answers slowly. The first request to name the new key gives up after 100 ms.
    s.idp.publish_unpublished_keys();
    s.keys.set_delay(Duration::from_millis(400));
    let rotated = s.unknown();
    let cancelled = tokio::time::timeout(Duration::from_millis(100), s.check(&rotated)).await;
    assert!(cancelled.is_err(), "the request was cancelled");
    assert_eq!(s.keys.started(), 2);

    // Once the fetch is over, the keys it brought are the copy: the next request with that key is accepted,
    // well inside the interval of the fetch that brought them.
    eventually("the fetch to be answered", || s.keys.answered() == 2).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_accepted(
        s.check(&rotated).await,
        "the rotated key, from the fetch of a request that is gone",
    );
    assert_eq!(s.keys.started(), 2, "and nothing was fetched again");
}

#[tokio::test]
async fn an_unknown_kid_joins_the_refresh_of_a_stale_copy_that_is_already_running() {
    let s = Setup::new(|t| {
        t.refresh_interval = Duration::from_millis(200);
        t.stale_grace = Duration::from_secs(60);
    })
    .await;
    let good = s.good();
    assert_accepted(s.check(&good).await, "the first fetch");
    tokio::time::sleep(Duration::from_millis(300)).await;

    s.idp.publish_unpublished_keys();
    s.keys.set_delay(Duration::from_millis(500));
    // A good token: answered by the stale copy, and the refresh starts.
    assert_accepted(s.check(&good).await, "the stale copy");
    eventually("the refresh to be running", || s.keys.started() == 2).await;
    // A token of the rotated key: it waits for that refresh rather than asking again.
    assert_accepted(
        s.check(&s.unknown()).await,
        "the rotated key, from the refresh already running",
    );
    assert_eq!(s.keys.started(), 2);
}

// ---- every request to the issuer is bounded -----------------------------------------------------------------------

/// The failure of `ready()` for an issuer, and how long it took, with a guard so that a request that is not
/// bounded fails the test instead of hanging it.
async fn timed_out(auth: &OidcAuthenticator) -> (KeysUnavailable, Duration) {
    let began = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(8), auth.ready())
        .await
        .expect("the request to the issuer is bounded by the timeout");
    (cause(result), began.elapsed())
}

fn config(base: &str) -> OidcConfig {
    OidcConfig::new(base, [TEST_AUDIENCE], "agentic_roles")
}

#[tokio::test]
async fn a_discovery_request_that_is_never_answered_times_out() {
    // The server accepts the connection, reads the request, and says nothing.
    let base = raw_server(|mut stream| async move {
        read_request(&mut stream).await;
        std::future::pending::<()>().await;
    })
    .await;
    let mut tuning = tuning();
    tuning.timeout = Duration::from_millis(500);
    let auth = OidcAuthenticator::with_tuning(config(&base), tuning).unwrap();
    let (error, took) = timed_out(&auth).await;
    assert_eq!(error, KeysUnavailable::NoCopy(Some(FetchFailure::Timeout)));
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_secs(3),
        "{took:?}"
    );
}

#[tokio::test]
async fn a_jwks_request_that_is_never_answered_times_out_after_a_good_discovery() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let document = json!({"issuer": base, "jwks_uri": format!("{base}/jwks")});
    let router = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let document = document.clone();
                async move { axum::Json(document) }
            }),
        )
        .route(
            "/jwks",
            get(|| async {
                std::future::pending::<()>().await;
                "never"
            }),
        );
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let mut tuning = tuning();
    tuning.timeout = Duration::from_millis(500);
    let auth = OidcAuthenticator::with_tuning(config(&base), tuning).unwrap();
    let (error, took) = timed_out(&auth).await;
    assert_eq!(error, KeysUnavailable::NoCopy(Some(FetchFailure::Timeout)));
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_secs(3),
        "{took:?}"
    );
}

#[tokio::test]
async fn a_body_that_drips_in_times_out() {
    // The headers come at once and promise a lot; the body arrives a byte at a time, forever.
    let url = raw_server(|mut stream| async move {
        read_request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100000\r\n\r\n",
            )
            .await
            .unwrap();
        loop {
            if stream.write_all(b" ").await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    let mut tuning = tuning();
    tuning.timeout = Duration::from_millis(500);
    let auth =
        OidcAuthenticator::with_tuning(config(&url).with_jwks_url(format!("{url}/jwks")), tuning)
            .unwrap();
    let (error, took) = timed_out(&auth).await;
    assert_eq!(error, KeysUnavailable::NoCopy(Some(FetchFailure::Timeout)));
    assert!(
        took >= Duration::from_millis(450) && took < Duration::from_secs(3),
        "the drip went on {took:?}: the timeout is not on the whole request"
    );
}
