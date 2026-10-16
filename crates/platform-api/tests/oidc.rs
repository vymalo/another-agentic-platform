//! The router over the real authenticator: tokens signed by an issuer on a local port, checked against its keys.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_auth_oidc::testkit::{
    Alg, Signing, TEST_AUDIENCE, TEST_ROLES_CLAIM, TestAuthenticator, now, set_path,
};
use aap_platform_api::{ApiState, router};
use aap_ports::memory::MemoryObjects;
use aap_ports::testkit::AuthenticatorUnderTest;
use common::*;
use serde_json::json;

async fn api_over(auth: TestAuthenticator) -> (Api, TestAuthenticator) {
    let objects = MemoryObjects::in_namespace(NS);
    let state = ApiState::new(objects.clone(), auth.authenticator().clone(), defaults()).unwrap();
    (Api::from_router(router(state), objects), auth)
}

#[tokio::test]
async fn a_signed_token_carries_its_permissions_through_the_router() {
    let (api, issuer) = api_over(TestAuthenticator::start().await).await;
    let reader = issuer.token_for("alice", &["platform:agents.read", "agent.use:chat", "user"]);

    let r = api.get("/v1/me").token(&reader).send().await;
    assert_eq!(r.code(), 200, "{}", r.text());
    assert_eq!(r.json(), json!({ "permissions": ["platform:agents.read"] }));
    assert_eq!(api.get("/v1/info").token(&reader).send().await.code(), 200);
    assert_eq!(
        api.get("/v1/agents").token(&reader).send().await.json(),
        json!({ "agents": [] })
    );

    // The permission is the claim's, not a role: a write is refused by name.
    let r = api
        .put("/v1/agents/chat")
        .token(&reader)
        .create()
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 403);
    assert_eq!(r.json()["permission"], "platform:agents.write");

    let writer = issuer.token_for(
        "bob",
        &[
            "platform:agents.write",
            "platform:models.write",
            "platform:secrets.pick",
        ],
    );
    offer_everything(&api.objects);
    let r = api
        .put("/v1/models/gateway")
        .token(&writer)
        .create()
        .json(&model_body("gateway"))
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
}

#[tokio::test]
async fn a_token_that_does_not_verify_is_401_for_the_reason_it_does_not() {
    let (api, issuer) = api_over(TestAuthenticator::start().await).await;
    let idp = issuer.idp();
    let good = issuer.token_for("alice", &["platform:agents.read"]);
    assert_eq!(api.get("/v1/me").token(&good).send().await.code(), 200);

    let mut expired = idp.claims(TEST_AUDIENCE, "alice");
    expired.insert("exp".into(), json!(now() - 3600));
    set_path(
        &mut expired,
        TEST_ROLES_CLAIM,
        json!(["platform:agents.read"]),
    );
    let mut other_audience = idp.claims("somebody-elses-api", "alice");
    set_path(
        &mut other_audience,
        TEST_ROLES_CLAIM,
        json!(["platform:agents.read"]),
    );
    let mut forged = good.clone();
    let last = forged.pop().unwrap();
    forged.push(if last == 'A' { 'B' } else { 'A' });

    for (why, token) in [
        (
            "expired",
            idp.mint(&expired, Signing::Published(Alg::Rs256)),
        ),
        (
            "wrong audience",
            idp.mint(&other_audience, Signing::Published(Alg::Rs256)),
        ),
        ("bad signature", forged),
        (
            "alg none",
            idp.mint(&idp.claims(TEST_AUDIENCE, "alice"), Signing::NoneAlg),
        ),
        (
            "unpublished key",
            idp.mint(
                &idp.claims(TEST_AUDIENCE, "alice"),
                Signing::Unpublished(Alg::Rs256),
            ),
        ),
        ("not a token", "garbage".to_owned()),
    ] {
        let r = api.get("/v1/me").token(&token).send().await;
        assert_eq!(r.code(), 401, "{why}: {}", r.text());
        assert_eq!(
            r.header("www-authenticate"),
            Some("Bearer error=\"invalid_token\""),
            "{why}"
        );
        assert!(r.is_problem("invalid-token"), "{why}");
        assert!(!r.text().contains(&token), "{why}: the token is not echoed");
    }
}

#[tokio::test]
async fn an_issuer_whose_keys_cannot_be_fetched_is_503_and_a_request_without_a_token_is_401() {
    let (off, token) = TestAuthenticator::start()
        .await
        .without_keys()
        .await
        .unwrap();
    let (api, _issuer) = api_over(off).await;
    let r = api.get("/v1/me").token(&token).send().await;
    assert_eq!(r.code(), 503, "{}", r.text());
    assert!(r.is_problem("keys-unavailable"));
    assert!(r.header("www-authenticate").is_none());
    assert_eq!(api.get("/v1/me").send().await.code(), 401);
}
