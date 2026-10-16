//! The authenticator's use of the issuer: where the keys come from, when they are fetched again, what happens
//! when the issuer is odd or down, and the rotation of keys. Over real HTTP, against issuers on local ports.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aap_auth_oidc::testkit::{ALGORITHMS, Alg, Signing, TEST_AUDIENCE, TestIdp};
use aap_auth_oidc::{FetchFailure, KeysUnavailable, OidcAuthenticator, OidcConfig};
use aap_ports::{AuthError, Authenticator, InvalidToken};
use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use common::{Rig, assert_accepted, assert_invalid, eventually, tuning, unavailable};
use serde_json::{Value, json};

const SUBJECT: &str = "alice";

fn config(idp: &TestIdp) -> OidcConfig {
    idp.config()
}

async fn check(auth: &OidcAuthenticator, token: &str) -> Result<aap_ports::Principal, AuthError> {
    auth.authenticate(Some(token)).await
}

/// The cause the authenticator gives for being unavailable.
fn cause<T: std::fmt::Debug>(result: Result<T, AuthError>) -> KeysUnavailable {
    match result {
        Err(AuthError::Unavailable(source)) => *source
            .downcast::<KeysUnavailable>()
            .unwrap_or_else(|e| panic!("the source is not a KeysUnavailable: {e:?}")),
        other => panic!("expected Unavailable, got {other:?}"),
    }
}

// ---- where the keys come from and when --------------------------------------------------------------------------

#[tokio::test]
async fn the_keys_come_from_the_discovery_document_and_are_fetched_on_first_use() {
    let idp = TestIdp::start().await;
    let auth = OidcAuthenticator::with_tuning(config(&idp), tuning()).unwrap();
    assert_eq!(idp.jwks_fetches(), 0, "building reads nothing");
    let token = idp.token(TEST_AUDIENCE, SUBJECT);
    assert_eq!(check(&auth, &token).await.unwrap().subject(), SUBJECT);
    assert_eq!((idp.discovery_fetches(), idp.jwks_fetches()), (1, 1));
    // The copy is fresh: no more requests.
    for _ in 0..3 {
        check(&auth, &token).await.unwrap();
    }
    assert_eq!((idp.discovery_fetches(), idp.jwks_fetches()), (1, 1));
}

#[tokio::test]
async fn a_configured_jwks_url_needs_no_discovery() {
    let idp = TestIdp::start().await;
    idp.set_discovery_down(true);
    let auth = OidcAuthenticator::with_tuning(config(&idp).with_jwks_url(idp.jwks_url()), tuning())
        .unwrap();
    check(&auth, &idp.token(TEST_AUDIENCE, SUBJECT))
        .await
        .unwrap();
    assert_eq!(idp.discovery_fetches(), 0);
}

#[tokio::test]
async fn ready_fetches_the_keys_and_says_when_it_cannot() {
    let rig = Rig::new().await;
    rig.auth.ready().await.unwrap();
    assert_eq!(
        rig.idp.jwks_fetches(),
        1,
        "ready fetched the keys before any token"
    );
    check(&rig.auth, &rig.idp.token(TEST_AUDIENCE, SUBJECT))
        .await
        .unwrap();
    assert_eq!(rig.idp.jwks_fetches(), 1, "and the first token used them");

    let down = Rig::new().await;
    down.idp.set_jwks_down(true);
    let error = down.auth.ready().await.unwrap_err();
    assert!(matches!(error, AuthError::Unavailable(_)), "{error:?}");
    down.idp.set_jwks_down(false);
    down.wait_out_the_interval().await;
    down.auth.ready().await.unwrap();
}

#[tokio::test]
async fn the_keys_are_fetched_again_when_they_are_old() {
    let rig = Rig::with(
        |c| c,
        |mut t| {
            t.refresh_interval = Duration::from_millis(200);
            t
        },
    )
    .await;
    let token = rig.idp.token(TEST_AUDIENCE, SUBJECT);
    check(&rig.auth, &token).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    // The copy is old: it answers this request, and a refresh is started in the background.
    check(&rig.auth, &token).await.unwrap();
    eventually("the refresh", || rig.idp.jwks_fetches() == 2).await;
    // The new copy is fresh: no more requests.
    tokio::time::sleep(Duration::from_millis(50)).await;
    for _ in 0..3 {
        check(&rig.auth, &token).await.unwrap();
    }
    assert_eq!(rig.idp.jwks_fetches(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_requests_make_one_fetch() {
    let rig = Rig::new().await;
    let token = Arc::new(rig.idp.token(TEST_AUDIENCE, SUBJECT));
    let mut tasks = Vec::new();
    for _ in 0..24 {
        let (auth, token) = (rig.auth.clone(), Arc::clone(&token));
        tasks.push(tokio::spawn(
            async move { check(&auth, &token).await.is_ok() },
        ));
    }
    for task in tasks {
        assert!(task.await.unwrap());
    }
    assert_eq!(rig.idp.jwks_fetches(), 1);
}

// ---- rotation ------------------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_unknown_kid_fetches_the_keys_again_once_and_no_more_within_the_interval() {
    // A second, so that a slow machine does not make the flood last longer than the interval.
    let interval = Duration::from_secs(1);
    let rig = Rig::with(
        |c| c,
        |mut t| {
            t.kid_refetch_interval = interval;
            t
        },
    )
    .await;
    assert_accepted(rig.check_claims(&rig.claims()).await, "a good token");
    tokio::time::sleep(interval + Duration::from_millis(100)).await;
    let before = rig.idp.jwks_fetches();
    // Eight tokens with invented key ids, at once: the first asks the issuer, the others find it asked.
    for _ in 0..8 {
        let token = rig.mint(&rig.claims(), Signing::Unpublished(Alg::Rs256));
        assert_invalid(
            rig.check(&token).await,
            InvalidToken::UnknownKey,
            "an unknown kid",
        );
    }
    assert_eq!(
        rig.idp.jwks_fetches() - before,
        1,
        "one fetch for the whole flood"
    );
    // Within the interval of that fetch, a different kid does not fetch again.
    let other = rig.mint(&rig.claims(), Signing::Unpublished(Alg::Es256));
    assert_invalid(
        rig.check(&other).await,
        InvalidToken::UnknownKey,
        "another unknown kid",
    );
    assert_eq!(rig.idp.jwks_fetches() - before, 1);
    // After it, the next unknown kid may ask again.
    tokio::time::sleep(interval + Duration::from_millis(100)).await;
    assert_invalid(
        rig.check(&other).await,
        InvalidToken::UnknownKey,
        "an unknown kid, later",
    );
    assert_eq!(rig.idp.jwks_fetches() - before, 2);
}

#[tokio::test]
async fn a_rotated_key_is_found_once_published() {
    let rig = Rig::new().await;
    assert_accepted(rig.check_claims(&rig.claims()).await, "a good token");
    for alg in ALGORITHMS {
        let token = rig.mint(&rig.claims(), Signing::Unpublished(alg));
        assert_invalid(
            rig.check(&token).await,
            InvalidToken::UnknownKey,
            &format!("{alg:?} before the rotation"),
        );
    }
    rig.idp.publish_unpublished_keys();
    rig.wait_out_the_interval().await;
    for alg in ALGORITHMS {
        let token = rig.mint(&rig.claims(), Signing::Unpublished(alg));
        assert_accepted(
            rig.check(&token).await,
            &format!("{alg:?} after the rotation, no restart"),
        );
    }
    // The old keys still verify: the issuer publishes both.
    assert_accepted(
        rig.check_claims(&rig.claims()).await,
        "a token of the old key",
    );
}

#[tokio::test]
async fn a_key_the_issuer_withdraws_stops_verifying_at_the_next_refresh() {
    // The copy is the issuer's word: once it is refreshed without a key, the key verifies nothing.
    let rig = Rig::with(
        |c| c,
        |mut t| {
            t.refresh_interval = Duration::from_millis(300);
            t
        },
    )
    .await;
    rig.idp.publish_unpublished_keys();
    let token = rig.mint(&rig.claims(), Signing::Unpublished(Alg::Rs256));
    assert_accepted(rig.check(&token).await, "published");
    rig.idp.withdraw_unpublished_keys();
    assert_accepted(rig.check(&token).await, "withdrawn, but the copy is fresh");
    tokio::time::sleep(Duration::from_millis(400)).await;
    // The copy is old: it answers the first request after that, withdrawn key and all, while the refresh runs
    // (a request does not wait for a refresh while a copy can verify).
    assert_accepted(
        rig.check(&token).await,
        "withdrawn, the copy old: served while the refresh runs",
    );
    // Once the refresh is over the key is gone.
    let give_up = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match rig.check(&token).await {
            Err(AuthError::Invalid(InvalidToken::UnknownKey)) => break,
            Ok(_) if std::time::Instant::now() < give_up => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            other => panic!("withdrawn, and the copy refreshed: {other:?}"),
        }
    }
}

#[tokio::test]
async fn an_unknown_kid_while_the_issuer_cannot_be_reached_is_unavailable_and_not_a_refusal() {
    // The key may be one the issuer published since: a client is not told that a good token is bad.
    let rig = Rig::new().await;
    assert_accepted(rig.check_claims(&rig.claims()).await, "a good token");
    rig.wait_out_the_interval().await;
    rig.idp.set_jwks_down(true);
    let rotated = rig.mint(&rig.claims(), Signing::Unpublished(Alg::Rs256));
    assert_eq!(
        cause(rig.check(&rotated).await),
        KeysUnavailable::Unrefreshed(FetchFailure::Status(503))
    );
    // Within the interval nothing is fetched again, and the answer is the same.
    let fetches = rig.idp.jwks_fetches();
    assert!(unavailable(&rig.check(&rotated).await));
    assert_eq!(rig.idp.jwks_fetches(), fetches);
    // The copy still verifies the keys it holds.
    assert_accepted(
        rig.check_claims(&rig.claims()).await,
        "the copy verifies what it holds",
    );
    // The issuer is back and does not publish that key: now the token is refused.
    rig.idp.set_jwks_down(false);
    rig.wait_out_the_interval().await;
    assert_invalid(
        rig.check(&rotated).await,
        InvalidToken::UnknownKey,
        "the issuer is back",
    );
}

// ---- the issuer down ----------------------------------------------------------------------------------------------

#[tokio::test]
async fn an_issuer_that_is_down_at_the_start_is_unavailable_and_recovers() {
    let rig = Rig::new().await;
    rig.idp.set_jwks_down(true);
    let token = rig.idp.token(TEST_AUDIENCE, SUBJECT);
    let error = cause(check(&rig.auth, &token).await);
    assert_eq!(
        error,
        KeysUnavailable::NoCopy(Some(FetchFailure::Status(503)))
    );
    rig.idp.set_jwks_down(false);
    rig.wait_out_the_interval().await;
    assert_accepted(check(&rig.auth, &token).await, "the issuer is back");
}

#[tokio::test]
async fn a_failed_fetch_is_not_repeated_within_the_retry_interval() {
    let retry = Duration::from_secs(1);
    let rig = Rig::with(
        |c| c,
        |mut t| {
            t.retry_interval = retry;
            t
        },
    )
    .await;
    rig.idp.set_jwks_down(true);
    let token = rig.idp.token(TEST_AUDIENCE, SUBJECT);
    for _ in 0..6 {
        assert!(unavailable(&check(&rig.auth, &token).await));
        assert!(rig.auth.ready().await.is_err());
    }
    assert_eq!(rig.idp.jwks_fetches(), 1, "the issuer is not hammered");
    tokio::time::sleep(retry + Duration::from_millis(100)).await;
    assert!(unavailable(&check(&rig.auth, &token).await));
    assert_eq!(rig.idp.jwks_fetches(), 2);
    rig.idp.set_jwks_down(false);
    tokio::time::sleep(retry + Duration::from_millis(100)).await;
    check(&rig.auth, &token).await.unwrap();
    rig.auth.ready().await.unwrap();
}

#[tokio::test]
async fn keys_that_cannot_be_refreshed_go_on_verifying_for_the_grace_and_no_longer() {
    let rig = Rig::with(
        |c| c,
        |mut t| {
            t.refresh_interval = Duration::from_millis(300);
            t.stale_grace = Duration::from_millis(1000);
            t.retry_interval = Duration::from_millis(50);
            t
        },
    )
    .await;
    let token = rig.idp.token(TEST_AUDIENCE, SUBJECT);
    check(&rig.auth, &token).await.unwrap();

    rig.idp.set_jwks_down(true);
    tokio::time::sleep(Duration::from_millis(400)).await;
    // Older than the refresh interval, the refresh fails, the copy is still inside the grace.
    assert_accepted(check(&rig.auth, &token).await, "the stale copy verifies");
    eventually("a refresh to be tried", || rig.idp.jwks_fetches() >= 2).await;
    rig.auth
        .ready()
        .await
        .expect("ready while the copy verifies");

    tokio::time::sleep(Duration::from_millis(1200)).await;
    // Past refresh + grace: fail closed, and say that the copy is what is too old.
    assert_eq!(
        cause(check(&rig.auth, &token).await),
        KeysUnavailable::TooOld(Some(FetchFailure::Status(503)))
    );
    assert!(rig.auth.ready().await.is_err());

    rig.idp.set_jwks_down(false);
    tokio::time::sleep(Duration::from_millis(100)).await;
    check(&rig.auth, &token).await.unwrap();
}

// ---- an issuer that is not quite one -------------------------------------------------------------------------------

/// An issuer that is not quite one: `router` builds what it serves from its own base URL.
async fn odd_issuer(router: impl FnOnce(String) -> Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = router(base.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

fn odd_config(issuer: &str) -> OidcConfig {
    OidcConfig::new(issuer, [TEST_AUDIENCE], "agentic_roles")
}

fn authenticator(config: OidcConfig) -> OidcAuthenticator {
    OidcAuthenticator::with_tuning(config, tuning()).unwrap()
}

async fn unavailable_because(auth: OidcAuthenticator, token: &str, expected: FetchFailure) {
    let result = check(&auth, token).await;
    assert_eq!(cause(result).cause(), Some(expected));
    assert!(auth.ready().await.is_err());
}

#[tokio::test]
async fn a_discovery_document_for_another_issuer_is_not_trusted() {
    let idp = TestIdp::start().await;
    let base = odd_issuer(|base| {
        Router::new().route(
            "/.well-known/openid-configuration",
            get(move || {
                let jwks = format!("{base}/jwks");
                async move {
                    axum::Json(json!({"issuer": "https://elsewhere.example", "jwks_uri": jwks}))
                }
            }),
        )
    })
    .await;
    unavailable_because(
        authenticator(odd_config(&base)),
        &idp.token(TEST_AUDIENCE, SUBJECT),
        FetchFailure::DiscoveryIssuerMismatch,
    )
    .await;
}

#[tokio::test]
async fn a_discovery_document_without_a_usable_jwks_uri_is_unusable() {
    let idp = TestIdp::start().await;
    for (document, expected) in [
        (json!({}), FetchFailure::DiscoveryIssuerMismatch),
        (json!({"issuer": "SELF"}), FetchFailure::NoJwksUri),
        (
            json!({"issuer": "SELF", "jwks_uri": 7}),
            FetchFailure::NoJwksUri,
        ),
        (
            json!({"issuer": "SELF", "jwks_uri": "ftp://x/jwks"}),
            FetchFailure::BadJwksUri,
        ),
        (
            json!({"issuer": "SELF", "jwks_uri": "https://u:p@x/jwks"}),
            FetchFailure::BadJwksUri,
        ),
    ] {
        let served = document.clone();
        let base = odd_issuer(move |base| {
            let document = served.to_string().replace("SELF", &base);
            Router::new().route(
                "/.well-known/openid-configuration",
                get(move || {
                    let document = document.clone();
                    async move { ([(header::CONTENT_TYPE, "application/json")], document) }
                }),
            )
        })
        .await;
        unavailable_because(
            authenticator(odd_config(&base)),
            &idp.token(TEST_AUDIENCE, SUBJECT),
            expected,
        )
        .await;
    }
}

#[tokio::test]
async fn a_redirect_is_never_followed() {
    let idp = TestIdp::start().await;
    let target = idp.jwks_url();
    let base = odd_issuer(move |_| {
        Router::new().route(
            "/jwks",
            get(move || {
                let target = target.clone();
                async move { (StatusCode::FOUND, [(header::LOCATION, target)]).into_response() }
            }),
        )
    })
    .await;
    let config = odd_config(&base).with_jwks_url(format!("{base}/jwks"));
    unavailable_because(
        authenticator(config),
        &idp.token(TEST_AUDIENCE, SUBJECT),
        FetchFailure::Status(302),
    )
    .await;
    assert_eq!(idp.jwks_fetches(), 0, "the redirect was not followed");
    let message = FetchFailure::Status(302).to_string();
    assert!(message.contains("redirect"), "{message}");
}

#[tokio::test]
async fn an_answer_over_a_mebibyte_is_refused() {
    let idp = TestIdp::start().await;
    let base = odd_issuer(|_| {
        Router::new().route(
            "/jwks",
            get(|| async { (StatusCode::OK, "x".repeat(1024 * 1024 + 1)) }),
        )
    })
    .await;
    let config = odd_config(&base).with_jwks_url(format!("{base}/jwks"));
    unavailable_because(
        authenticator(config),
        &idp.token(TEST_AUDIENCE, SUBJECT),
        FetchFailure::TooLarge,
    )
    .await;
}

#[tokio::test]
async fn an_answer_that_is_not_a_key_set_or_has_no_usable_key_is_unavailable() {
    let idp = TestIdp::start().await;
    for (body, expected) in [
        (json!("text"), FetchFailure::NotAKeySet),
        (json!({"keys": "no"}), FetchFailure::NotAKeySet),
        (json!([]), FetchFailure::NotAKeySet),
        (json!({"keys": []}), FetchFailure::NoUsableKey),
        // A symmetric key is no key for a token of the allowed algorithms.
        (
            json!({"keys": [{"kty": "oct", "kid": "pub-rsa", "k": "c2VjcmV0c2VjcmV0c2VjcmV0"}]}),
            FetchFailure::NoUsableKey,
        ),
    ] {
        let served = body.clone();
        let base = odd_issuer(move |_| {
            Router::new().route(
                "/jwks",
                get(move || {
                    let body = served.clone();
                    async move { axum::Json(body) }
                }),
            )
        })
        .await;
        let config = odd_config(&base).with_jwks_url(format!("{base}/jwks"));
        unavailable_because(
            authenticator(config),
            &idp.token(TEST_AUDIENCE, SUBJECT),
            expected,
        )
        .await;
    }
    // And one that is not JSON at all.
    let base =
        odd_issuer(|_| Router::new().route("/jwks", get(|| async { "<html></html>" }))).await;
    let config = odd_config(&base).with_jwks_url(format!("{base}/jwks"));
    unavailable_because(
        authenticator(config),
        &idp.token(TEST_AUDIENCE, SUBJECT),
        FetchFailure::NotJson,
    )
    .await;
}

#[tokio::test]
async fn an_unreachable_issuer_is_unavailable_and_not_a_hang() {
    // Nothing listens there.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let idp = TestIdp::start().await;
    unavailable_because(
        authenticator(odd_config(&base)),
        &idp.token(TEST_AUDIENCE, SUBJECT),
        FetchFailure::Connect,
    )
    .await;
}

#[tokio::test]
async fn a_key_the_build_cannot_use_beside_a_good_one_does_not_spoil_the_set() {
    let idp = TestIdp::start().await;
    let real: Value = serde_json::from_str(
        &reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(idp.jwks_url())
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    let mut keys = real["keys"].as_array().unwrap().clone();
    keys.insert(
        0,
        json!({"kty": "oct", "kid": "pub-rsa", "k": "c2VjcmV0c2VjcmV0c2VjcmV0"}),
    );
    keys.insert(0, json!({"kty": "RSA", "kid": "broken"}));
    let served = json!({ "keys": keys });
    let base = odd_issuer(move |_| {
        Router::new().route(
            "/jwks",
            get(move || {
                let body = served.clone();
                async move { axum::Json(body) }
            }),
        )
    })
    .await;
    // The token's issuer is the test issuer's; the keys come from the odd one.
    let config = config(&idp).with_jwks_url(format!("{base}/jwks"));
    let auth = authenticator(config);
    for alg in [Alg::Rs256, Alg::Es256, Alg::EdDsa] {
        let token = idp.mint(&idp.claims(TEST_AUDIENCE, SUBJECT), Signing::Published(alg));
        assert_accepted(check(&auth, &token).await, &format!("{alg:?}"));
    }
}

#[tokio::test]
async fn an_issuer_with_a_trailing_slash_is_discovered_without_a_double_slash() {
    let idp = TestIdp::start().await;
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    // The issuer is written with a slash; its discovery document must say exactly that.
    let base = odd_issuer(move |base| {
        let issuer = format!("{base}/");
        Router::new().route(
            "/.well-known/openid-configuration",
            get(move || {
                counter.fetch_add(1, Ordering::SeqCst);
                let issuer = issuer.clone();
                let jwks = format!("{issuer}jwks");
                async move { axum::Json(json!({"issuer": issuer, "jwks_uri": jwks})) }
            }),
        )
    })
    .await;
    let auth = authenticator(odd_config(&format!("{base}/")));
    let _ = check(&auth, &idp.token(TEST_AUDIENCE, SUBJECT)).await;
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the document was found at one slash"
    );
}

#[tokio::test]
async fn only_one_terminating_slash_of_the_issuer_is_removed_to_find_the_discovery_document() {
    // OpenID Connect Discovery 1.0, 4: "a terminating /" is removed, one, and the path is appended.
    let paths = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = Arc::clone(&paths);
    let base = odd_issuer(move |_| {
        Router::new().fallback(move |uri: axum::http::Uri| {
            seen.lock().unwrap().push(uri.path().to_owned());
            async { StatusCode::NOT_FOUND }
        })
    })
    .await;
    for slashes in ["", "/", "//", "///"] {
        paths.lock().unwrap().clear();
        let auth = authenticator(odd_config(&format!("{base}{slashes}")));
        let error = auth.ready().await.unwrap_err();
        assert!(matches!(error, AuthError::Unavailable(_)), "{error:?}");
        let expected = format!(
            "{}/.well-known/openid-configuration",
            slashes.strip_prefix('/').unwrap_or("")
        );
        assert_eq!(
            *paths.lock().unwrap(),
            [expected],
            "{} terminating slashes",
            slashes.len()
        );
    }
}

// ---- the testkit's issuer at an address of the caller's choosing (the kind end-to-end of the Platform API) ----------

#[tokio::test]
async fn an_issuer_started_at_an_address_names_itself_by_the_host_it_is_told() {
    // Bound on one address and named by another host, as an issuer on the runner is for a pod that reaches the runner at the
    // docker network's gateway: `iss` and the discovery document say the name the client was configured with.
    let idp = TestIdp::start_at("127.0.0.1:0", "localhost").await;
    assert!(
        idp.issuer().starts_with("http://localhost:"),
        "{}",
        idp.issuer()
    );
    let port = idp.issuer().rsplit(':').next().unwrap().to_owned();
    assert!(port.parse::<u16>().unwrap() > 0, "{port}");

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let discovery: Value = serde_json::from_str(
        &client
            .get(format!("{}/.well-known/openid-configuration", idp.issuer()))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(discovery["issuer"], json!(idp.issuer()));
    assert_eq!(discovery["jwks_uri"], json!(idp.jwks_url()));

    let auth = OidcAuthenticator::with_tuning(idp.config(), tuning()).unwrap();
    let roles = ["platform:agents.read", "platform:agents.write"];
    let token = idp.token_with_roles(TEST_AUDIENCE, SUBJECT, "agentic_roles", &roles);
    let principal = assert_accepted(check(&auth, &token).await, "a token of an issuer at a name");
    assert_eq!(principal.subject(), SUBJECT);
    assert_eq!(principal.permissions().collect::<Vec<_>>(), roles);
}

#[tokio::test]
async fn tokens_with_roles_differ_and_nest_the_claim_at_a_dotted_path() {
    let idp = TestIdp::start().await;
    let config = OidcConfig::new(
        idp.issuer(),
        [TEST_AUDIENCE],
        "resource_access.another-agentic.roles",
    );
    let auth = OidcAuthenticator::with_tuning(config, tuning()).unwrap();
    let roles = ["platform:agents.read".to_owned()];
    let first = idp.token_with_roles(
        TEST_AUDIENCE,
        SUBJECT,
        "resource_access.another-agentic.roles",
        &roles,
    );
    let second = idp.token_with_roles(
        TEST_AUDIENCE,
        SUBJECT,
        "resource_access.another-agentic.roles",
        &roles,
    );
    assert_ne!(first, second, "the same second, two tokens");
    for token in [first, second] {
        let principal = assert_accepted(check(&auth, &token).await, "a nested roles claim");
        assert!(principal.holds("platform:agents.read"));
    }
    // No roles is a good token that holds nothing.
    let none: [&str; 0] = [];
    let token = idp.token_with_roles(
        TEST_AUDIENCE,
        SUBJECT,
        "resource_access.another-agentic.roles",
        &none,
    );
    let principal = assert_accepted(check(&auth, &token).await, "no roles");
    assert_eq!(principal.permissions().count(), 0);
}
