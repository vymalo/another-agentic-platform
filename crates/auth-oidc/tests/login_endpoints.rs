//! The sign-in endpoints of the testkit issuer (K9b-2): `/authorize`, `/token` and `/revoke`, over real HTTP. They are what `aapctl
//! login` is tried against, so what they check of a client is held here: the code is used once, the redirect and the client are
//! those of the request, the verifier matches the challenge, a refresh token rotates. And the rule that matters beyond the tests:
//! **until a test sets a login, `/authorize` hands out nothing**, so a pod that reaches the issuer's port cannot mint a token.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use aap_auth_oidc::testkit::{TEST_AUDIENCE, TestAuthenticator, TestIdp};
use aap_ports::Authenticator;
use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
const REDIRECT: &str = "http://127.0.0.1:51004/callback";

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .unwrap()
}

async fn authorize(
    idp: &TestIdp,
    redirect: &str,
    client: &str,
    challenge: &str,
) -> reqwest::Response {
    let mut url = url::Url::parse(&format!("{}/authorize", idp.issuer())).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client)
        .append_pair("redirect_uri", redirect)
        .append_pair("scope", "openid")
        .append_pair("state", "st-1")
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    http().get(url).send().await.unwrap()
}

fn location(response: &reqwest::Response) -> url::Url {
    url::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap()
}

async fn code_for(idp: &TestIdp) -> String {
    let response = authorize(idp, REDIRECT, "aapctl", CHALLENGE).await;
    assert_eq!(response.status(), 302);
    let to = location(&response);
    assert_eq!(to.as_str().split('?').next().unwrap(), REDIRECT);
    let q: std::collections::HashMap<_, _> = to.query_pairs().into_owned().collect();
    assert_eq!(q["state"], "st-1");
    q["code"].clone()
}

async fn post(idp: &TestIdp, path: &str, pairs: &[(&str, &str)]) -> (u16, Value) {
    let mut body = url::form_urlencoded::Serializer::new(String::new());
    for (k, v) in pairs {
        body.append_pair(k, v);
    }
    let response = http()
        .post(format!("{}{path}", idp.issuer()))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(body.finish())
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

fn exchange<'a>(code: &'a str, verifier: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", REDIRECT),
        ("client_id", "aapctl"),
        ("code_verifier", verifier),
    ]
}

#[test]
fn the_vector_is_the_rfcs() {
    // RFC 7636 Appendix B, so that the other tests' constants are the specification's and not this file's.
    let digest = Sha256::digest(VERIFIER.as_bytes());
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest),
        CHALLENGE
    );
}

#[tokio::test]
async fn authorize_does_nothing_until_a_login_is_set() {
    let idp = TestIdp::start().await;
    // No login: 403, no redirect, no code, whatever the request.
    let response = authorize(&idp, REDIRECT, "aapctl", CHALLENGE).await;
    assert_eq!(response.status(), 403);
    assert!(response.headers().get("location").is_none());
    assert!(idp.issued_codes().is_empty());
    // A code that was never issued is nothing at the token endpoint, and mints nothing.
    let (status, body) = post(&idp, "/token", &exchange("made-up", VERIFIER)).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (400, Some("invalid_grant"))
    );
    assert!(idp.issued_access_tokens().is_empty());
    // A refresh token that was never issued is nothing either.
    let (status, body) = post(
        &idp,
        "/token",
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", "made-up"),
            ("client_id", "aapctl"),
        ],
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (400, Some("invalid_grant"))
    );
    assert!(idp.issued_access_tokens().is_empty());
    assert_eq!(idp.authorize_requests(), 1);
}

#[tokio::test]
async fn a_code_with_the_right_verifier_is_a_token_the_authenticator_reads() {
    let rig = TestAuthenticator::start().await;
    let idp = rig.idp();
    idp.set_login("alice", &["platform:agents.read", "user"]);
    let code = code_for(idp).await;
    let (status, body) = post(idp, "/token", &exchange(&code, VERIFIER)).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["expires_in"], 300);
    let access = body["access_token"].as_str().unwrap();
    let principal = rig.authenticate(Some(access)).await.unwrap();
    assert_eq!(principal.subject(), "alice");
    assert!(principal.holds("platform:agents.read"));
    assert!(body["refresh_token"].as_str().is_some());
    assert_eq!(idp.token_grants(), ["authorization_code"]);
}

#[tokio::test]
async fn a_code_is_used_once() {
    let idp = TestIdp::start().await;
    idp.set_login("alice", &[]);
    let code = code_for(&idp).await;
    assert_eq!(
        post(&idp, "/token", &exchange(&code, VERIFIER)).await.0,
        200
    );
    let (status, body) = post(&idp, "/token", &exchange(&code, VERIFIER)).await;
    assert_eq!(
        (status, body["error"].as_str()),
        (400, Some("invalid_grant"))
    );
}

#[tokio::test]
async fn the_token_request_must_match_the_authorization_request() {
    let idp = TestIdp::start().await;
    idp.set_login("alice", &[]);
    // Another verifier.
    let code = code_for(&idp).await;
    let (status, body) = post(
        &idp,
        "/token",
        &exchange(&code, "another-verifier-another-verifier-another-1"),
    )
    .await;
    assert_eq!(
        (status, body["error"].as_str()),
        (400, Some("invalid_grant"))
    );
    // Another redirect.
    let code = code_for(&idp).await;
    let mut form = exchange(&code, VERIFIER);
    form[2] = ("redirect_uri", "http://127.0.0.1:1/callback");
    assert_eq!(post(&idp, "/token", &form).await.0, 400);
    // Another client.
    let code = code_for(&idp).await;
    let mut form = exchange(&code, VERIFIER);
    form[3] = ("client_id", "another-client");
    assert_eq!(post(&idp, "/token", &form).await.0, 400);
    // Told to expect another challenge, the right verifier is refused.
    let code = code_for(&idp).await;
    idp.expect_challenge(Some("not-the-challenge"));
    assert_eq!(
        post(&idp, "/token", &exchange(&code, VERIFIER)).await.0,
        400
    );
    assert!(idp.issued_access_tokens().is_empty());
}

#[tokio::test]
async fn authorize_redirects_only_to_a_loopback_address_with_a_port_and_a_pkce_request() {
    let idp = TestIdp::start().await;
    idp.set_login("alice", &[]);
    for bad in [
        "http://localhost:1/callback",
        "https://127.0.0.1:1/callback",
        "http://127.0.0.1/callback",
        "http://evil.example/callback",
        "http://127.0.0.1.evil.example:1/callback",
        "http://user:pw@127.0.0.1:1/callback",
        "not a url",
    ] {
        let response = authorize(&idp, bad, "aapctl", CHALLENGE).await;
        assert_eq!(response.status(), 400, "{bad}");
        assert!(response.headers().get("location").is_none(), "{bad}");
    }
    // Not PKCE S256.
    let mut url = url::Url::parse(&format!("{}/authorize", idp.issuer())).unwrap();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", "aapctl")
        .append_pair("redirect_uri", REDIRECT)
        .append_pair("code_challenge", CHALLENGE)
        .append_pair("code_challenge_method", "plain");
    assert_eq!(http().get(url).send().await.unwrap().status(), 400);
    assert!(idp.issued_codes().is_empty());
}

#[tokio::test]
async fn a_denial_is_sent_back_as_an_error_with_the_state() {
    let idp = TestIdp::start().await;
    idp.set_login("alice", &[]);
    idp.set_deny(Some("access_denied"));
    let response = authorize(&idp, REDIRECT, "aapctl", CHALLENGE).await;
    assert_eq!(response.status(), 302);
    let q: std::collections::HashMap<_, _> =
        location(&response).query_pairs().into_owned().collect();
    assert_eq!(q["error"], "access_denied");
    assert_eq!(q["state"], "st-1");
    assert!(!q.contains_key("code"));
}

#[tokio::test]
async fn a_refresh_token_rotates_and_the_old_one_stops_working() {
    let idp = TestIdp::start().await;
    idp.set_login("alice", &["user"]);
    let code = code_for(&idp).await;
    let (_, first) = post(&idp, "/token", &exchange(&code, VERIFIER)).await;
    let r1 = first["refresh_token"].as_str().unwrap().to_owned();
    let refresh = |r: &str| {
        vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", r),
            ("client_id", "aapctl"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect::<Vec<_>>()
    };
    let go = |pairs: Vec<(String, String)>| {
        let idp = &idp;
        async move {
            let refs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(a, b)| (a.as_str(), b.as_str()))
                .collect();
            post(idp, "/token", &refs).await
        }
    };
    let (status, second) = go(refresh(&r1)).await;
    assert_eq!(status, 200, "{second}");
    let r2 = second["refresh_token"].as_str().unwrap().to_owned();
    assert_ne!(r1, r2);
    assert!(!idp.accepts_refresh_token(&r1));
    assert_eq!(go(refresh(&r1)).await.1["error"], "invalid_grant");
    // Without rotation the same token goes on working and no new one is given.
    idp.set_rotation(false);
    let (_, third) = go(refresh(&r2)).await;
    assert!(third.get("refresh_token").is_none());
    assert!(idp.accepts_refresh_token(&r2));
    // And on request every refresh is refused.
    idp.set_refresh_invalid(true);
    assert_eq!(go(refresh(&r2)).await.1["error"], "invalid_grant");
}

#[tokio::test]
async fn the_knobs_change_what_token_says_and_what_it_mints() {
    let rig = TestAuthenticator::start().await;
    let idp = rig.idp();
    idp.set_login("alice", &["user"]);
    idp.set_expires_in(10);
    idp.set_gives_refresh_token(false);
    idp.set_token_type("MAC");
    idp.queue_lifetimes(&[-300]);
    let code = code_for(idp).await;
    let (_, body) = post(idp, "/token", &exchange(&code, VERIFIER)).await;
    assert_eq!(body["expires_in"], 10);
    assert_eq!(body["token_type"], "MAC");
    assert!(body.get("refresh_token").is_none());
    // The first token was made already expired (beyond the 60 s of leeway), the next is the usual five minutes.
    assert!(
        rig.authenticate(Some(body["access_token"].as_str().unwrap()))
            .await
            .is_err()
    );
    idp.set_audience("another-audience");
    let code = code_for(idp).await;
    let (_, body) = post(idp, "/token", &exchange(&code, VERIFIER)).await;
    assert!(
        rig.authenticate(Some(body["access_token"].as_str().unwrap()))
            .await
            .is_err()
    );
    idp.set_audience(TEST_AUDIENCE);
    let code = code_for(idp).await;
    let (_, body) = post(idp, "/token", &exchange(&code, VERIFIER)).await;
    assert!(
        rig.authenticate(Some(body["access_token"].as_str().unwrap()))
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn discovery_names_the_endpoints_and_the_pkce_method_and_can_be_patched() {
    let idp = TestIdp::start().await;
    let get = || async {
        let text = http()
            .get(format!("{}/.well-known/openid-configuration", idp.issuer()))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        serde_json::from_str::<Value>(&text).unwrap()
    };
    let doc = get().await;
    let issuer = idp.issuer();
    assert_eq!(doc["issuer"], json!(issuer));
    assert_eq!(
        doc["authorization_endpoint"],
        json!(format!("{issuer}/authorize"))
    );
    assert_eq!(doc["token_endpoint"], json!(format!("{issuer}/token")));
    assert_eq!(doc["code_challenge_methods_supported"], json!(["S256"]));
    assert!(doc.get("revocation_endpoint").is_none());
    idp.enable_revocation(false);
    assert_eq!(
        get().await["revocation_endpoint"],
        json!(format!("{issuer}/revoke"))
    );
    idp.patch_discovery(
        &json!({"issuer": "https://other.example", "code_challenge_methods_supported": null}),
    );
    let doc = get().await;
    assert_eq!(doc["issuer"], "https://other.example");
    assert!(doc.get("code_challenge_methods_supported").is_none());
}

#[tokio::test]
async fn revoke_forgets_the_token_and_can_be_made_to_fail() {
    let idp = TestIdp::start().await;
    idp.seed_refresh_token("r-seeded", "aapctl", "alice", &[]);
    idp.enable_revocation(false);
    let (status, _) = post(
        &idp,
        "/revoke",
        &[("token", "r-seeded"), ("client_id", "aapctl")],
    )
    .await;
    assert_eq!(status, 200);
    assert!(!idp.accepts_refresh_token("r-seeded"));
    assert_eq!(idp.revoked_tokens(), ["r-seeded"]);
    idp.enable_revocation(true);
    assert_eq!(post(&idp, "/revoke", &[("token", "x")]).await.0, 500);
}

#[tokio::test]
async fn an_issuer_started_without_login_mounts_no_sign_in_route_and_names_none() {
    // What a pod can reach (`examples/test-issuer.rs`): discovery and the JWKS, and nothing that mints a token or keeps a list.
    let idp = TestIdp::try_start_without_login("127.0.0.1:0", "127.0.0.1")
        .await
        .unwrap();
    // Even a login set by the owner of the issuer opens nothing: there is no route to open.
    idp.set_login("alice", &["platform:agents.read"]);
    let discovery: Value = serde_json::from_str(
        &http()
            .get(format!("{}/.well-known/openid-configuration", idp.issuer()))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(discovery["issuer"], idp.issuer());
    assert!(discovery["jwks_uri"].is_string());
    for member in [
        "authorization_endpoint",
        "token_endpoint",
        "revocation_endpoint",
    ] {
        assert!(discovery.get(member).is_none(), "{member} is advertised");
    }
    assert_eq!(
        authorize(&idp, REDIRECT, "aapctl", CHALLENGE)
            .await
            .status(),
        404
    );
    for path in ["/token", "/revoke"] {
        let (status, _) = post(&idp, path, &[("grant_type", "refresh_token")]).await;
        assert_eq!(status, 404, "{path}");
    }
    assert_eq!(idp.authorize_requests(), 0);
    assert!(idp.token_grants().is_empty());
    let jwks = http().get(idp.jwks_url()).send().await.unwrap().status();
    assert_eq!(jwks, 200);
}

#[tokio::test]
async fn padding_makes_the_tokens_as_large_as_a_test_says() {
    let idp = TestIdp::start().await;
    idp.set_login("alice", &["platform:agents.read"]);
    idp.set_padding(5_000, 3_000);
    let code = code_for(&idp).await;
    let (status, answer) = post(&idp, "/token", &exchange(&code, VERIFIER)).await;
    assert_eq!(status, 200);
    assert!(answer["access_token"].as_str().unwrap().len() > 5_000);
    assert!(answer["refresh_token"].as_str().unwrap().len() > 3_000);
}
