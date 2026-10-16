//! `/v1/me`, `/v1/info`, and what the state refuses at start.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_platform_api::{ApiState, StateError, router};
use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
use common::*;
use serde_json::json;

#[tokio::test]
async fn me_is_the_platform_permissions_of_the_token_in_byte_order_and_only_those() {
    let api = Api::new();
    let token = api.token(
        "alice",
        &[
            "platform:secrets.pick",
            "platform:agents.read",
            "agent.use:coder-me",
            "user",
            "admin",
            "platform:future.thing",
            "notplatform:agents.write",
        ],
    );
    let r = api.get("/v1/me").token(&token).send().await;
    assert_eq!(r.code(), 200);
    assert_eq!(
        r.json(),
        json!({ "permissions": ["platform:agents.read", "platform:future.thing", "platform:secrets.pick"] })
    );
    // A person who holds none of them is a 200 with an empty list: the web draws no /admin.
    let nobody = api.token("bob", &["user"]);
    let r = api.get("/v1/me").token(&nobody).send().await;
    assert_eq!(r.json(), json!({ "permissions": [] }));
}

#[tokio::test]
async fn info_is_the_capability_document() {
    let (api, token) = Api::seeded().await;
    let r = api.get("/v1/info").token(&token).send().await;
    assert_eq!(r.code(), 200, "{}", r.text());
    let info = r.json();
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(info["namespace"], NS);
    assert_eq!(info["offerLabel"], "agents.vymalo.com/offer");
    assert_eq!(
        info["runPodSizeClasses"],
        json!([]),
        "adam-rs has none yet (D15)"
    );
    assert_eq!(
        info["limits"]["maxBodyBytes"],
        aap_platform_api::MAX_BODY_BYTES
    );
    // The defaults as the forms show them: references to Secrets, never a value.
    assert_eq!(info["defaults"]["namespace"], NS);
    assert_eq!(info["defaults"]["store"]["cnpg"]["instances"], 1);
    assert_eq!(
        info["defaults"]["a2aBearer"],
        json!({ "name": "agent-a2a", "key": "AGENT_REGISTRY_AGENT_TOKEN" })
    );
    assert_eq!(
        info["defaults"]["coder"]["allowedRepoHosts"],
        json!(["github.com"])
    );
}

#[tokio::test]
async fn the_version_is_the_binarys_when_it_says_so() {
    let objects = MemoryObjects::in_namespace(NS);
    let auth = MemoryAuthenticator::new().with_token("t", "alice", ["platform:agents.read"]);
    let state = ApiState::new(objects.clone(), auth, defaults())
        .unwrap()
        .with_version("9.9.9");
    let api = Api::from_router(router(state), objects);
    let r = api.get("/v1/info").token("t").send().await;
    assert_eq!(r.json()["version"], "9.9.9");
}

#[test]
fn defaults_the_operator_would_refuse_stop_the_api_before_the_first_save() {
    let mut bad = serde_json::to_value(defaults()).unwrap();
    bad["a2aBearer"]["key"] = json!("");
    bad["store"] = json!({ "cnpg": { "instances": 0, "storage": { "size": "5Gi" } } });
    let bad: aap_forms::Defaults = serde_json::from_value(bad).unwrap();
    let Err(StateError::Defaults(problems)) = ApiState::new(
        MemoryObjects::in_namespace(NS),
        MemoryAuthenticator::new(),
        bad,
    ) else {
        panic!("the defaults were accepted");
    };
    assert!(!problems.is_empty());
}
