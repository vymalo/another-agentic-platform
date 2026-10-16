//! An agent's lifecycle as the dashboard shows it (§60a, *An agent's lifecycle, as the dashboard shows it*), and every member of
//! a row of `GET /v1/agents` and of `GET /v1/agents/{name}`. The operator's words are written with `set_status`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_ports::testkit::Foreign;
use aap_ports::{AgentObjects, Kind};
use common::*;
use serde_json::{Value, json};

async fn create(api: &Api, token: &str, body: &Value) -> Reply {
    let name = body["name"].as_str().unwrap().to_owned();
    let r = api
        .put(&format!("/v1/agents/{name}"))
        .token(token)
        .create()
        .json(body)
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
    r
}

/// What the operator writes of a service it has seen: its `state`, the generations it resolved.
fn status(state: Option<&str>, service: Option<i64>, config: Option<i64>) -> Value {
    let mut status = json!({ "conditions": [] });
    if let Some(state) = state {
        status["state"] = json!(state);
    }
    if let Some(generation) = service {
        status["observedGeneration"] = json!(generation);
    }
    status["config"] = json!({ "name": "chat", "digest": "sha256:abc" });
    if let Some(generation) = config {
        status["config"]["observedGeneration"] = json!(generation);
    }
    status
}

/// The state of `name`, read from the detail and from its row in the list: they must say the same.
async fn state_of(api: &Api, token: &str, name: &str) -> String {
    let detail = api
        .get(&format!("/v1/agents/{name}"))
        .token(token)
        .send()
        .await
        .json();
    let list = api.get("/v1/agents").token(token).send().await.json();
    let row = list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == name)
        .unwrap_or_else(|| panic!("{name} is not listed"))
        .clone();
    assert_eq!(
        row["state"], detail["state"],
        "the row and the detail differ"
    );
    detail["state"].as_str().unwrap().to_owned()
}

async fn agent_with_status(
    state: Option<&str>,
    service: Option<i64>,
    config: Option<i64>,
) -> String {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &folder_body("chat")).await;
    api.objects
        .set_status(Kind::AgentService, "chat", status(state, service, config));
    state_of(&api, &token, "chat").await
}

#[tokio::test]
async fn an_agent_the_operator_has_not_written_a_status_for_is_saving() {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &folder_body("chat")).await;
    assert_eq!(state_of(&api, &token, "chat").await, "saving");
}

#[tokio::test]
async fn once_the_operator_has_seen_the_generation_the_state_is_the_one_it_wrote() {
    for (written, shown) in [
        ("Ready", "ready"),
        ("Degraded", "degraded"),
        ("Suspended", "suspended"),
        ("Blocked", "blocked"),
    ] {
        assert_eq!(
            agent_with_status(Some(written), Some(1), Some(1)).await,
            shown,
            "state {written}"
        );
    }
}

#[tokio::test]
async fn a_status_with_no_state_is_still_saving_even_when_the_generation_is_seen() {
    assert_eq!(agent_with_status(None, Some(1), Some(1)).await, "saving");
}

#[tokio::test]
async fn a_generation_the_operator_has_not_observed_is_saving_whatever_state_it_last_wrote() {
    // The state is the one of the previous generation: the page must not show it as the result of the save.
    for state in ["Ready", "Degraded", "Suspended", "Blocked"] {
        assert_eq!(
            agent_with_status(Some(state), Some(0), Some(1)).await,
            "saving",
            "{state}, observedGeneration 0"
        );
        assert_eq!(
            agent_with_status(Some(state), None, Some(1)).await,
            "saving",
            "{state}, no observedGeneration"
        );
    }
}

#[tokio::test]
async fn a_config_the_operator_resolved_a_generation_ago_is_saving() {
    // A config the operator has not resolved yet; the service's own generation is seen.
    assert_eq!(
        agent_with_status(Some("Ready"), Some(1), Some(0)).await,
        "saving"
    );
}

#[tokio::test]
async fn an_operator_that_does_not_say_which_config_it_resolved_does_not_hold_the_page_in_saving() {
    // No `config.observedGeneration` is no claim that the config is behind.
    assert_eq!(
        agent_with_status(Some("Ready"), Some(1), None).await,
        "ready"
    );
}

#[tokio::test]
async fn an_edit_of_the_service_is_saving_until_the_operator_has_seen_it() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    api.objects.set_status(
        Kind::AgentService,
        "chat",
        status(Some("Ready"), Some(1), Some(1)),
    );
    assert_eq!(state_of(&api, &token, "chat").await, "ready");

    let mut body = folder_body("chat");
    body["title"] = json!("Chat, renamed");
    let current = api.get("/v1/agents/chat").token(&token).send().await;
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace(&current.etag())
        .json(&body)
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
    let _ = created;
    let agent = api.objects.get_agent("chat").await.unwrap().unwrap();
    assert_eq!(
        (
            agent.service.generation,
            agent.config.as_ref().unwrap().generation
        ),
        (2, 1),
        "a title is the service's"
    );
    assert_eq!(r.json()["state"], "saving");
    assert_eq!(state_of(&api, &token, "chat").await, "saving");

    api.objects.set_status(
        Kind::AgentService,
        "chat",
        status(Some("Ready"), Some(2), Some(1)),
    );
    assert_eq!(state_of(&api, &token, "chat").await, "ready");
}

#[tokio::test]
async fn an_edit_of_the_config_alone_does_not_move_the_services_generation_and_is_saving_all_the_same()
 {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &folder_body("chat")).await;
    api.objects.set_status(
        Kind::AgentService,
        "chat",
        status(Some("Ready"), Some(1), Some(1)),
    );
    assert_eq!(state_of(&api, &token, "chat").await, "ready");

    // The instructions are the config's; nothing of the service changes.
    let mut body = folder_body("chat");
    body["agent"]["files"]["instructions.md"] =
        json!("---\nname: chat\n---\nYour name is Chatty.\n");
    let current = api.get("/v1/agents/chat").token(&token).send().await;
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace(&current.etag())
        .json(&body)
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
    let agent = api.objects.get_agent("chat").await.unwrap().unwrap();
    assert_eq!(
        (
            agent.service.generation,
            agent.config.as_ref().unwrap().generation
        ),
        (1, 2),
        "the instructions are the config's: the service's generation does not move"
    );
    assert_eq!(
        state_of(&api, &token, "chat").await,
        "saving",
        "the operator has seen the service's generation and not the config's"
    );

    api.objects.set_status(
        Kind::AgentService,
        "chat",
        status(Some("Ready"), Some(1), Some(2)),
    );
    assert_eq!(state_of(&api, &token, "chat").await, "ready");
}

#[tokio::test]
async fn an_agent_that_is_being_deleted_is_deleting_whatever_its_status_says() {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &folder_body("chat")).await;
    api.objects.set_status(
        Kind::AgentService,
        "chat",
        status(Some("Ready"), Some(1), Some(1)),
    );
    api.objects.seed_terminating("chat");
    // The deletion moved the generation, and the status says Ready: Deleting comes first.
    assert_eq!(state_of(&api, &token, "chat").await, "deleting");
    api.objects.set_status(
        Kind::AgentService,
        "chat",
        status(Some("Ready"), Some(99), Some(99)),
    );
    assert_eq!(state_of(&api, &token, "chat").await, "deleting");
}

#[tokio::test]
async fn a_gitops_agent_goes_through_the_same_states_and_is_read_only() {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &folder_body("chat")).await;
    let agent = api.objects.get_agent("chat").await.unwrap().unwrap();
    api.objects.seed_foreign(
        Kind::AgentService,
        "chat",
        agent.service.spec.clone(),
        Foreign::ArgoTracked,
    );
    assert_eq!(state_of(&api, &token, "chat").await, "saving");
    api.objects.set_status(
        Kind::AgentService,
        "chat",
        status(Some("Blocked"), Some(1), Some(1)),
    );
    assert_eq!(state_of(&api, &token, "chat").await, "blocked");
    let got = api.get("/v1/agents/chat").token(&token).send().await.json();
    assert_eq!(got["managedBy"], "gitops");
}

// ------------------------------------------------------------------ the members of a row

/// The conditions the operator writes, in the order it writes them: the first false one is the reason to show.
fn conditions() -> Value {
    json!([
        { "type": "ConfigResolved", "status": "True", "reason": "Resolved", "message": "", "lastTransitionTime": "2026-10-16T00:00:00Z" },
        { "type": "StoreReady", "status": "True", "reason": "Ready", "message": "", "lastTransitionTime": "2026-10-16T00:00:01Z" },
        { "type": "RuntimeReady", "status": "False", "reason": "ImagePull", "message": "pulling", "lastTransitionTime": "2026-10-16T00:00:02Z" },
        { "type": "Listed", "status": "False", "reason": "NotReady", "message": "", "lastTransitionTime": "2026-10-16T00:00:03Z" },
        { "type": "Ready", "status": "False", "reason": "NotReady", "message": "", "lastTransitionTime": "2026-10-16T00:00:04Z" }
    ])
}

fn described_body() -> Value {
    let mut body = folder_body("chat");
    body["title"] = json!("Chat title");
    body["description"] = json!("Answers questions about the platform.");
    body["tags"] = json!(["chat", "docs"]);
    body["suspend"] = json!(true);
    body["audience"] = json!(["agent.use:chat", "staff"]);
    body
}

#[tokio::test]
async fn a_row_of_the_list_has_every_member_and_no_other() {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &described_body()).await;
    api.objects.set_status(
        Kind::AgentService,
        "chat",
        json!({
            "observedGeneration": 1,
            "state": "Suspended",
            "config": { "name": "chat", "observedGeneration": 1, "digest": "sha256:feed" },
            "conditions": conditions(),
        }),
    );
    let list = api.get("/v1/agents").token(&token).send().await.json();
    assert_eq!(
        list,
        json!({ "agents": [{
            "name": "chat",
            "title": "Chat title",
            "description": "Answers questions about the platform.",
            "kind": "folder",
            "state": "suspended",
            "conditions": conditions(),
            "suspend": true,
            "tags": ["chat", "docs"],
            "audience": ["agent.use:chat", "staff"],
            "managedBy": "dashboard",
            "deleting": false,
            "configDigest": "sha256:feed",
        }] })
    );
}

#[tokio::test]
async fn a_row_of_an_agent_that_says_little_has_null_and_empty_and_not_a_missing_member() {
    let (api, token) = Api::seeded().await;
    let mut body = folder_body("chat");
    body.as_object_mut().unwrap().remove("audience");
    body.as_object_mut().unwrap().remove("tags");
    create(&api, &token, &body).await;
    let list = api.get("/v1/agents").token(&token).send().await.json();
    assert_eq!(
        list["agents"][0],
        json!({
            "name": "chat",
            "title": "Chat",
            "description": null,
            "kind": "folder",
            "state": "saving",
            "conditions": [],
            "suspend": false,
            "tags": [],
            "audience": null,
            "managedBy": "dashboard",
            "deleting": false,
            "configDigest": null,
        })
    );
}

#[tokio::test]
async fn a_coder_is_a_coder_and_a_folder_agent_a_folder_and_an_agent_without_a_config_is_neither() {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &folder_body("chat")).await;
    create(&api, &token, &coder_body("coder-me")).await;
    let service = api
        .objects
        .get_agent("chat")
        .await
        .unwrap()
        .unwrap()
        .service
        .spec;
    let mut orphan = service.clone();
    orphan["configRef"] = json!({ "name": "no-such-config" });
    api.objects
        .seed_foreign(Kind::AgentService, "bare", orphan, Foreign::Unlabelled);
    let list = api.get("/v1/agents").token(&token).send().await.json();
    let kinds: Vec<(String, Value)> = list["agents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r["name"].as_str().unwrap().to_owned(), r["kind"].clone()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("bare".to_owned(), Value::Null),
            ("chat".to_owned(), json!("folder")),
            ("coder-me".to_owned(), json!("coder")),
        ]
    );
}

#[tokio::test]
async fn the_detail_is_the_row_and_the_form_and_both_statuses_whole() {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &described_body()).await;
    let service_status = json!({
        "observedGeneration": 1,
        "state": "Degraded",
        "config": { "name": "chat", "observedGeneration": 1, "digest": "sha256:feed" },
        "runtime": { "provider": "kubernetes", "phase": "Running" },
        "conditions": conditions(),
        "operator-word": "kept as the operator wrote it",
    });
    let config_status = json!({ "observedGeneration": 1, "valid": true, "note": "kept too" });
    api.objects
        .set_status(Kind::AgentService, "chat", service_status.clone());
    api.objects
        .set_status(Kind::AgentConfig, "chat", config_status.clone());

    let list = api.get("/v1/agents").token(&token).send().await.json();
    let row = list["agents"][0].clone();
    let got = api.get("/v1/agents/chat").token(&token).send().await;
    let detail = got.json();

    // Every member of the row is in the detail, with the same value.
    for (member, value) in row.as_object().unwrap() {
        assert_eq!(&detail[member], value, "{member}");
    }
    assert_eq!(
        detail["status"], service_status,
        "AgentService.status, whole"
    );
    assert_eq!(
        detail["configStatus"], config_status,
        "AgentConfig.status, whole"
    );
    assert_eq!(
        detail["representation"]["form"]["form"]["title"],
        "Chat title"
    );
    let members: Vec<&str> = detail
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .filter(|k| !row.as_object().unwrap().contains_key(*k))
        .collect();
    assert_eq!(members, ["configStatus", "representation", "status"]);

    // Both are null until the operator has written them.
    create(&api, &token, &folder_body("quiet")).await;
    let quiet = api
        .get("/v1/agents/quiet")
        .token(&token)
        .send()
        .await
        .json();
    assert_eq!(
        (quiet["status"].clone(), quiet["configStatus"].clone()),
        (Value::Null, Value::Null)
    );
}

#[tokio::test]
async fn the_conditions_are_in_the_order_the_operator_wrote_them() {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &folder_body("chat")).await;
    api.objects.set_status(
        Kind::AgentService,
        "chat",
        json!({ "observedGeneration": 1, "state": "Degraded", "conditions": conditions() }),
    );
    let got = api.get("/v1/agents/chat").token(&token).send().await.json();
    let types: Vec<&str> = got["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        [
            "ConfigResolved",
            "StoreReady",
            "RuntimeReady",
            "Listed",
            "Ready"
        ],
        "the first false one is the reason to show"
    );
}
