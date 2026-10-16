//! `/v1/agents`: create, read, replace, delete, and what each refuses.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_ports::testkit::Foreign;
use aap_ports::{AgentObjects, Kind};
use common::*;
use serde_json::{Value, json};

async fn create(api: &Api, token: &str, body: &Value) -> Reply {
    let name = body["name"].as_str().unwrap().to_owned();
    api.put(&format!("/v1/agents/{name}"))
        .token(token)
        .create()
        .json(body)
        .send()
        .await
}

fn field_errors(r: &Reply) -> Vec<(String, String)> {
    r.json()["errors"]
        .as_array()
        .unwrap_or_else(|| panic!("no errors in {}", r.text()))
        .iter()
        .map(|e| {
            (
                e["field"].as_str().unwrap().to_owned(),
                e["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_folder_agent_is_created_read_listed_and_exported() {
    let (api, token) = Api::seeded().await;
    let r = create(&api, &token, &folder_body("chat")).await;
    assert_eq!(r.code(), 201, "{}", r.text());
    assert_eq!(r.header("location"), Some("/v1/agents/chat"));
    assert_eq!(r.header("content-type"), Some("application/json"));
    let created = r.json();
    assert_eq!(created["name"], "chat");
    assert_eq!(created["kind"], "folder");
    assert_eq!(created["managedBy"], "dashboard");
    assert_eq!(
        created["state"], "saving",
        "the operator has not seen it yet"
    );
    assert_eq!(created["representation"]["form"]["form"]["title"], "Chat");
    // A new agent gets the deployment's defaults.
    assert!(
        created["representation"]["form"]["deployment"]["store"]["postgres"]["cnpg"].is_object()
    );

    let got = api.get("/v1/agents/chat").token(&token).send().await;
    assert_eq!(got.code(), 200);
    assert_eq!(
        got.etag(),
        r.etag(),
        "the token of a read is the token of the write"
    );
    assert_eq!(got.json(), created);

    let list = api.get("/v1/agents").token(&token).send().await;
    assert_eq!(list.code(), 200);
    let rows = list.json()["agents"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "chat");
    assert_eq!(rows[0]["audience"], json!(["*"]));
    assert!(
        rows[0].get("representation").is_none(),
        "a row carries no folder"
    );

    let yaml = api.get("/v1/agents/chat/yaml").token(&token).send().await;
    assert_eq!(yaml.code(), 200);
    assert_eq!(yaml.header("content-type"), Some("application/yaml"));
    assert_eq!(yaml.etag(), r.etag());
    let text = yaml.text();
    assert!(
        text.contains("kind: AgentConfig") && text.contains("kind: AgentService"),
        "{text}"
    );
    assert!(text.contains("\n---\n"), "two documents: {text}");
}

#[tokio::test]
async fn a_coder_is_created_with_the_secret_it_picks() {
    let (api, token) = Api::seeded().await;
    let r = create(&api, &token, &coder_body("coder-me")).await;
    assert_eq!(r.code(), 201, "{}", r.text());
    let body = r.json();
    assert_eq!(body["kind"], "coder");
    assert_eq!(
        body["representation"]["form"]["form"]["agent"]["github"]["privateKey"],
        json!({ "secret": "github-app", "key": "private-key.pem" })
    );
}

#[tokio::test]
async fn the_etag_round_trips_and_a_stale_one_is_412() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    let first = created.etag();

    let mut body = folder_body("chat");
    body["title"] = json!("Chat, renamed");
    let replaced = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace(&first)
        .json(&body)
        .send()
        .await;
    assert_eq!(replaced.code(), 200, "{}", replaced.text());
    assert_ne!(replaced.etag(), first, "a change moves the token");
    assert_eq!(
        replaced.json()["representation"]["form"]["form"]["title"],
        "Chat, renamed"
    );
    assert!(replaced.header("location").is_none());

    // The first tag is stale now: somebody saved first.
    body["title"] = json!("Chat, again");
    let stale = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace(&first)
        .json(&body)
        .send()
        .await;
    assert_eq!(stale.code(), 412, "{}", stale.text());
    assert!(stale.is_problem("stale"));
    let kept = api.get("/v1/agents/chat").token(&token).send().await;
    assert_eq!(
        kept.json()["representation"]["form"]["form"]["title"],
        "Chat, renamed"
    );

    // And the new one works.
    let again = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace(&replaced.etag())
        .json(&body)
        .send()
        .await;
    assert_eq!(again.code(), 200);
}

#[tokio::test]
async fn the_operator_writing_its_status_makes_a_token_stale() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    api.objects.set_status(
        Kind::AgentService,
        "chat",
        json!({ "observedGeneration": 1, "state": "Ready", "conditions": [] }),
    );
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace(&created.etag())
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 412);
    // Read again, and the state is the operator's.
    let got = api.get("/v1/agents/chat").token(&token).send().await;
    assert_eq!(got.json()["state"], "ready");
    assert_ne!(got.etag(), created.etag());
}

#[tokio::test]
async fn a_second_create_of_a_name_is_409_already_exists() {
    let (api, token) = Api::seeded().await;
    assert_eq!(create(&api, &token, &folder_body("chat")).await.code(), 201);
    let again = create(&api, &token, &folder_body("chat")).await;
    assert_eq!(again.code(), 409, "{}", again.text());
    assert!(again.is_problem("already-exists"));
}

#[tokio::test]
async fn a_write_that_is_not_conditional_is_428() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    let none = api
        .put("/v1/agents/chat")
        .token(&token)
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(none.code(), 428, "{}", none.text());
    assert!(none.is_problem("precondition-required"));
    let any = api
        .put("/v1/agents/chat")
        .token(&token)
        .header("if-match", "*")
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(
        any.code(),
        428,
        "If-Match: * is no precondition the port can honour"
    );
    let delete = api.delete("/v1/agents/chat").token(&token).send().await;
    assert_eq!(delete.code(), 428);
    assert!(delete.is_problem("precondition-required"));
    // Nothing was written or deleted.
    let kept = api.get("/v1/agents/chat").token(&token).send().await;
    assert_eq!(kept.etag(), created.etag());
}

#[tokio::test]
async fn a_weak_tag_never_matches_and_a_malformed_one_is_400() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    let weak = format!("W/{}", created.etag());
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .header("if-match", &weak)
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 412);
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .header("if-match", "no quotes")
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 400);
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .header("if-match", &created.etag())
        .header("if-none-match", "*")
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 400, "a create and a replace at once");
}

#[tokio::test]
async fn a_replace_of_an_agent_that_is_not_there_is_404() {
    let (api, token) = Api::seeded().await;
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace("\"some-tag\"")
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 404, "{}", r.text());
    assert!(r.is_problem("not-found"));
    assert_eq!(
        api.get("/v1/agents/chat").token(&token).send().await.code(),
        404
    );
    assert_eq!(
        api.get("/v1/agents/chat/yaml")
            .token(&token)
            .send()
            .await
            .code(),
        404
    );
    let d = api
        .delete("/v1/agents/chat")
        .token(&token)
        .replace("\"some-tag\"")
        .send()
        .await;
    assert_eq!(d.code(), 404);
}

#[tokio::test]
async fn a_stale_tag_is_412_before_the_form_is_looked_at() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    api.objects
        .set_status(Kind::AgentService, "chat", json!({ "state": "Ready" }));
    let mut bad = folder_body("chat");
    bad["title"] = json!("");
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace(&created.etag())
        .json(&bad)
        .send()
        .await;
    assert_eq!(r.code(), 412, "{}", r.text());
}

#[tokio::test]
async fn a_delete_takes_the_tag_and_the_agent_is_gone() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    let stale = api
        .delete("/v1/agents/chat")
        .token(&token)
        .replace("\"AgentService:4:chat:1:1.-\"")
        .send()
        .await;
    assert_eq!(stale.code(), 412, "{}", stale.text());
    let deleted = api
        .delete("/v1/agents/chat")
        .token(&token)
        .replace(&created.etag())
        .send()
        .await;
    assert_eq!(deleted.code(), 204, "{}", deleted.text());
    assert!(deleted.body.is_empty());
    assert_eq!(
        api.get("/v1/agents/chat").token(&token).send().await.code(),
        404
    );
    let list = api.get("/v1/agents").token(&token).send().await;
    assert_eq!(list.json(), json!({ "agents": [] }));
}

#[tokio::test]
async fn an_agent_the_operator_is_still_releasing_is_202_and_refuses_writes() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    api.objects.seed_finalizer("chat");
    let current = api.get("/v1/agents/chat").token(&token).send().await;
    let _ = created;
    let deleted = api
        .delete("/v1/agents/chat")
        .token(&token)
        .replace(&current.etag())
        .send()
        .await;
    assert_eq!(deleted.code(), 202, "{}", deleted.text());
    assert_eq!(deleted.json()["state"], "deleting");
    assert_eq!(deleted.json()["deleting"], true);

    // Listed and read while it goes, refused every write.
    let got = api.get("/v1/agents/chat").token(&token).send().await;
    assert_eq!(got.json()["state"], "deleting");
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace(&got.etag())
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 409);
    assert!(r.is_problem("deleting"), "{}", r.text());
    let again = create(&api, &token, &folder_body("chat")).await;
    assert_eq!(again.code(), 409);
    assert!(again.is_problem("deleting"), "{}", again.text());

    api.objects.run_finalizer("chat");
    assert_eq!(
        api.get("/v1/agents/chat").token(&token).send().await.code(),
        404
    );
}

#[tokio::test]
async fn an_agent_that_gitops_owns_is_read_only_and_every_write_is_409() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    let agent = api.objects.get_agent("chat").await.unwrap().unwrap();
    api.objects.seed_foreign(
        Kind::AgentService,
        "chat",
        agent.service.spec.clone(),
        Foreign::ArgoTracked,
    );
    let _ = created;

    let got = api.get("/v1/agents/chat").token(&token).send().await;
    assert_eq!(got.code(), 200, "a GitOps agent is read");
    assert_eq!(got.json()["managedBy"], "gitops");
    let rows = api.get("/v1/agents").token(&token).send().await.json();
    assert_eq!(rows["agents"][0]["managedBy"], "gitops");
    assert_eq!(
        api.get("/v1/agents/chat/yaml")
            .token(&token)
            .send()
            .await
            .code(),
        200
    );

    let put = api
        .put("/v1/agents/chat")
        .token(&token)
        .replace(&got.etag())
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(put.code(), 409, "{}", put.text());
    assert!(put.is_problem("managed-by-gitops"));
    assert!(put.json()["detail"].as_str().unwrap().contains("GitOps"));
    let delete = api
        .delete("/v1/agents/chat")
        .token(&token)
        .replace(&got.etag())
        .send()
        .await;
    assert_eq!(delete.code(), 409);
    assert!(delete.is_problem("managed-by-gitops"));
    let create_over = create(&api, &token, &folder_body("chat")).await;
    assert_eq!(create_over.code(), 409, "a create whose name exists is 409");
}

#[tokio::test]
async fn a_config_that_another_agent_names_is_never_touched() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &folder_body("chat")).await;
    let service = api
        .objects
        .get_agent("chat")
        .await
        .unwrap()
        .unwrap()
        .service
        .spec;
    api.objects
        .seed_foreign(Kind::AgentService, "twin", service, Foreign::Unlabelled);
    let r = api
        .delete("/v1/agents/chat")
        .token(&token)
        .replace(&created.etag())
        .send()
        .await;
    assert_eq!(r.code(), 409, "{}", r.text());
    assert!(r.is_problem("referenced"));
    assert_eq!(r.json()["agents"], json!(["twin"]));
}

#[tokio::test]
async fn an_agent_no_form_says_is_read_as_yaml_and_a_form_may_not_replace_it() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &coder_body("coder-me")).await;
    // A tunable of the coder no form carries: the objects are YAML only.
    let agent = api.objects.get_agent("coder-me").await.unwrap().unwrap();
    let mut config = agent.config.unwrap().spec;
    config["harness"]["adam"]["coder"]["maxCheckCycles"] = json!(7);
    let _ = created;
    api.objects.seed_orphan_config("coder-me", &config);
    let got = api.get("/v1/agents/coder-me").token(&token).send().await;
    assert_eq!(got.code(), 200, "{}", got.text());
    let reasons = got.json()["representation"]["yamlOnly"].clone();
    assert!(reasons.is_array(), "{}", got.text());
    assert!(reasons.to_string().contains("maxCheckCycles"), "{reasons}");

    let r = api
        .put("/v1/agents/coder-me")
        .token(&token)
        .replace(&got.etag())
        .json(&coder_body("coder-me"))
        .send()
        .await;
    assert_eq!(r.code(), 409, "{}", r.text());
    assert!(r.is_problem("yaml-only"));
    assert!(r.json()["reasons"].to_string().contains("maxCheckCycles"));
    // The YAML is there to edit.
    assert!(
        api.get("/v1/agents/coder-me/yaml")
            .token(&token)
            .send()
            .await
            .text()
            .contains("maxCheckCycles")
    );
}

#[tokio::test]
async fn a_coder_cannot_become_a_folder_agent() {
    let (api, token) = Api::seeded().await;
    let created = create(&api, &token, &coder_body("thing")).await;
    let r = api
        .put("/v1/agents/thing")
        .token(&token)
        .replace(&created.etag())
        .json(&folder_body("thing"))
        .send()
        .await;
    assert_eq!(r.code(), 422, "{}", r.text());
    assert!(
        field_errors(&r).iter().any(|(f, _)| f == "agent.kind"),
        "{}",
        r.text()
    );
}

// ---------------------------------------------------------------------------- 422

#[tokio::test]
async fn a_form_that_is_not_valid_is_422_with_the_field_of_every_error() {
    let (api, token) = Api::seeded().await;
    let mut body = folder_body("chat");
    body["title"] = json!("");
    body["tags"] = json!(["fine", "Not Fine"]);
    body["model"] = json!({ "endpoint": "nowhere", "model": "m" });
    let r = create(&api, &token, &body).await;
    assert_eq!(r.code(), 422, "{}", r.text());
    assert!(r.is_problem("validation"));
    let fields: Vec<String> = field_errors(&r).into_iter().map(|(f, _)| f).collect();
    for expected in ["title", "tags[1]", "model.endpoint"] {
        assert!(
            fields.iter().any(|f| f == expected),
            "{expected} in {fields:?}"
        );
    }
    // Nothing was written.
    assert_eq!(
        api.get("/v1/agents/chat").token(&token).send().await.code(),
        404
    );
    // Every error has a message.
    for e in r.json()["errors"].as_array().unwrap() {
        assert!(!e["message"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn the_name_of_the_form_is_the_name_of_the_path() {
    let (api, token) = Api::seeded().await;
    let r = api
        .put("/v1/agents/other")
        .token(&token)
        .create()
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 422, "{}", r.text());
    assert_eq!(field_errors(&r)[0].0, "name");
}

#[tokio::test]
async fn a_key_that_is_not_offered_is_422_on_the_field_that_names_it() {
    let (api, token) = Api::seeded().await;
    let mut body = coder_body("coder-me");
    body["agent"]["github"]["privateKey"] = json!({ "secret": "orchestrator-db", "key": "uri" });
    let r = create(&api, &token, &body).await;
    assert_eq!(r.code(), 422, "{}", r.text());
    assert!(
        field_errors(&r)
            .iter()
            .any(|(f, _)| f == "agent.github.privateKey"),
        "{}",
        r.text()
    );
}

#[tokio::test]
async fn a_tool_server_the_agent_names_must_exist() {
    let (api, token) = Api::seeded().await;
    let mut body = coder_body("coder-me");
    body["toolServers"] = json!(["nonexistent"]);
    let r = create(&api, &token, &body).await;
    assert_eq!(r.code(), 422);
    assert!(
        field_errors(&r)
            .iter()
            .any(|(f, _)| f.starts_with("toolServers")),
        "{}",
        r.text()
    );
}

// ---------------------------------------------------------------- secrets.pick

#[tokio::test]
async fn a_write_that_names_a_secret_key_needs_secrets_pick_besides_its_own_permission() {
    let (api, _) = Api::seeded().await;
    let writer = api.token("writer", &["platform:agents.write", "platform:agents.read"]);
    let r = create(&api, &writer, &coder_body("coder-me")).await;
    assert_eq!(r.code(), 403, "{}", r.text());
    assert!(r.is_problem("forbidden"));
    assert_eq!(r.json()["permission"], "platform:secrets.pick");
    assert!(
        !r.text().contains("platform:agents.write"),
        "it names the missing one only: {}",
        r.text()
    );
    assert_eq!(
        api.get("/v1/agents/coder-me")
            .token(&writer)
            .send()
            .await
            .code(),
        404,
        "nothing written"
    );

    // The same person writes an agent that names no secret.
    let ok = create(&api, &writer, &folder_body("chat")).await;
    assert_eq!(ok.code(), 201, "{}", ok.text());

    // With the permission, the coder is made.
    let picker = api.token(
        "picker",
        &["platform:agents.write", "platform:secrets.pick"],
    );
    assert_eq!(
        create(&api, &picker, &coder_body("coder-me")).await.code(),
        201
    );
}

#[tokio::test]
async fn secrets_pick_alone_does_not_write_an_agent() {
    let (api, _) = Api::seeded().await;
    let picker = api.token("picker", &["platform:secrets.pick"]);
    let r = create(&api, &picker, &coder_body("coder-me")).await;
    assert_eq!(r.code(), 403);
    assert_eq!(r.json()["permission"], "platform:agents.write");
}

// ------------------------------------------------------------ what the body may be

#[tokio::test]
async fn a_body_that_is_not_json_of_the_form_is_refused_and_quotes_nothing() {
    let (api, token) = Api::seeded().await;
    const SENTINEL: &str = "sentinel-secret-value-3d94";
    let put = |body: Value| {
        let api = &api;
        let token = &token;
        async move {
            api.put("/v1/agents/chat")
                .token(token)
                .create()
                .json(&body)
                .send()
                .await
        }
    };
    // A member the form does not have: how a value sent in a field called `value` or `password` is stopped.
    let mut with_password = folder_body("chat");
    with_password["password"] = json!(SENTINEL);
    let r = put(with_password).await;
    assert_eq!(r.code(), 400, "{}", r.text());
    assert!(r.is_problem("invalid-body"));
    assert!(r.text().contains("password"), "{}", r.text());
    assert!(!r.text().contains(SENTINEL), "{}", r.text());

    // A value of the wrong type is not quoted.
    let mut wrong_type = folder_body("chat");
    wrong_type["suspend"] = json!(SENTINEL);
    let r = put(wrong_type).await;
    assert_eq!(r.code(), 400);
    assert!(!r.text().contains(SENTINEL), "{}", r.text());

    // A secret slot with a value in it.
    let mut value_in_a_pick = coder_body("chat");
    value_in_a_pick["agent"]["github"]["privateKey"] =
        json!({ "secret": "github-app", "key": "k", "value": SENTINEL });
    let r = put(value_in_a_pick).await;
    assert_eq!(r.code(), 400);
    assert!(!r.text().contains(SENTINEL), "{}", r.text());

    // Not JSON at all.
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .create()
        .raw(
            format!("{{\"title\": \"{SENTINEL}").as_bytes(),
            Some("application/json"),
        )
        .send()
        .await;
    assert_eq!(r.code(), 400);
    assert!(!r.text().contains(SENTINEL), "{}", r.text());
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .create()
        .raw(b"", Some("application/json"))
        .send()
        .await;
    assert_eq!(r.code(), 400);
}

#[tokio::test]
async fn a_body_that_is_not_declared_as_json_is_415() {
    let (api, token) = Api::seeded().await;
    let body = serde_json::to_vec(&folder_body("chat")).unwrap();
    for content_type in [
        Some("text/plain"),
        Some("application/yaml"),
        Some("application/json; charset=latin1"),
        None,
    ] {
        let r = api
            .put("/v1/agents/chat")
            .token(&token)
            .create()
            .raw(&body, content_type)
            .send()
            .await;
        assert_eq!(r.code(), 415, "{content_type:?}: {}", r.text());
        assert!(r.is_problem("unsupported-media-type"));
    }
}

#[tokio::test]
async fn a_body_over_the_limit_is_413_whether_or_not_it_says_how_long_it_is() {
    let (api, token) = Api::seeded().await;
    let limit = aap_platform_api::MAX_BODY_BYTES;
    let mut body = folder_body("chat");
    body["agent"]["files"]["big.md"] = json!("x".repeat(limit));
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .create()
        .json(&body)
        .send()
        .await;
    assert_eq!(r.code(), 413, "{}", r.text());
    assert!(r.is_problem("payload-too-large"));
    assert_eq!(r.json()["limit"], limit);

    // A body that says how long it is, and is too long, is refused before a byte of it is read.
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .create()
        .header("content-length", &(limit + 1).to_string())
        .raw(b"{}", Some("application/json"))
        .send()
        .await;
    assert_eq!(r.code(), 413, "{}", r.text());
}

#[tokio::test]
async fn a_folder_of_one_mebibyte_fits_in_the_body_limit() {
    // §60a lets a folder be 1 MiB; its JSON is a little larger, and the limit leaves room for it.
    let (api, token) = Api::seeded().await;
    let mut body = folder_body("chat");
    body["agent"]["files"]["notes.md"] = json!("line\n".repeat(200_000));
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .create()
        .json(&body)
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
}

// ------------------------------------------------------------------------- 503

#[tokio::test]
async fn an_object_store_that_is_down_is_503_and_not_a_mistake_of_the_request() {
    let (api, token) = Api::seeded().await;
    api.objects.set_unavailable(true);
    for r in [
        api.get("/v1/agents").token(&token).send().await,
        api.get("/v1/agents/chat").token(&token).send().await,
        api.get("/v1/models").token(&token).send().await,
        api.get("/v1/secret-keys").token(&token).send().await,
        create(&api, &token, &folder_body("chat")).await,
    ] {
        assert_eq!(r.code(), 503, "{}", r.text());
        assert!(r.is_problem("unavailable"));
    }
}

#[tokio::test]
async fn keys_that_cannot_be_fetched_are_503_never_401() {
    let (api, token) = Api::seeded().await;
    api.auth.set_unavailable(true);
    for path in ["/v1/me", "/v1/agents", "/v1/info"] {
        let r = api.get(path).token(&token).send().await;
        assert_eq!(r.code(), 503, "{path}: {}", r.text());
        assert!(r.is_problem("keys-unavailable"));
        assert!(
            r.header("www-authenticate").is_none(),
            "a good token is not told it is bad"
        );
    }
    // A request with no token needs no keys.
    assert_eq!(api.get("/v1/me").send().await.code(), 401);
}

#[tokio::test]
async fn a_status_the_operator_wrote_is_shown_and_a_sentinel_in_it_is_not_exported() {
    let (api, token) = Api::seeded().await;
    create(&api, &token, &folder_body("chat")).await;
    api.objects.set_status(
        Kind::AgentService,
        "chat",
        json!({
            "observedGeneration": 1, "state": "Degraded",
            "config": { "name": "chat", "digest": "sha256:abc" },
            "conditions": [{ "type": "RuntimeReady", "status": "False", "reason": "ImagePull", "message": "pulling", "lastTransitionTime": "2026-10-16T00:00:00Z" }],
            "sentinel-in-status": "status-sentinel-5b1c"
        }),
    );
    let got = api.get("/v1/agents/chat").token(&token).send().await.json();
    assert_eq!(got["state"], "degraded");
    assert_eq!(got["configDigest"], "sha256:abc");
    assert_eq!(got["conditions"][0]["reason"], "ImagePull");
    let yaml = api
        .get("/v1/agents/chat/yaml")
        .token(&token)
        .send()
        .await
        .text();
    assert!(
        !yaml.contains("status-sentinel-5b1c") && !yaml.contains("Degraded"),
        "{yaml}"
    );
}
