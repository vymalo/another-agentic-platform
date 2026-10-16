//! `/v1/models` and `/v1/tool-servers`: the objects many agents name.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_ports::testkit::Foreign;
use aap_ports::{AgentObjects, Kind, SharedKind};
use common::*;
use serde_json::{Value, json};

async fn make_agent(api: &Api, token: &str, body: &Value) -> Reply {
    let name = body["name"].as_str().unwrap().to_owned();
    api.put(&format!("/v1/agents/{name}"))
        .token(token)
        .create()
        .json(body)
        .send()
        .await
}

#[tokio::test]
async fn a_model_is_created_read_listed_replaced_and_deleted() {
    let api = Api::new();
    offer_everything(&api.objects);
    let token = api.admin();
    let created = api
        .put("/v1/models/gateway")
        .token(&token)
        .create()
        .json(&model_body("gateway"))
        .send()
        .await;
    assert_eq!(created.code(), 201, "{}", created.text());
    assert_eq!(created.header("location"), Some("/v1/models/gateway"));
    let body = created.json();
    assert_eq!(body["name"], "gateway");
    assert_eq!(body["title"], "Gateway");
    assert_eq!(body["managedBy"], "dashboard");
    assert_eq!(body["usedBy"], json!([]));
    assert_eq!(
        body["representation"]["form"]["apiKey"],
        json!({ "secret": "agent-models", "key": "GATEWAY_API_KEY" })
    );

    let got = api.get("/v1/models/gateway").token(&token).send().await;
    assert_eq!(got.etag(), created.etag());
    let list = api.get("/v1/models").token(&token).send().await.json();
    assert_eq!(list["models"].as_array().unwrap().len(), 1);
    assert_eq!(list["models"][0]["name"], "gateway");

    let mut changed = model_body("gateway");
    changed["title"] = json!("Gateway 2");
    let replaced = api
        .put("/v1/models/gateway")
        .token(&token)
        .replace(&created.etag())
        .json(&changed)
        .send()
        .await;
    assert_eq!(replaced.code(), 200, "{}", replaced.text());
    assert_ne!(replaced.etag(), created.etag());
    let stale = api
        .put("/v1/models/gateway")
        .token(&token)
        .replace(&created.etag())
        .json(&changed)
        .send()
        .await;
    assert_eq!(stale.code(), 412);

    let no_tag = api.delete("/v1/models/gateway").token(&token).send().await;
    assert_eq!(no_tag.code(), 428);
    let gone = api
        .delete("/v1/models/gateway")
        .token(&token)
        .replace(&replaced.etag())
        .send()
        .await;
    assert_eq!(gone.code(), 204);
    assert_eq!(
        api.get("/v1/models/gateway")
            .token(&token)
            .send()
            .await
            .code(),
        404
    );
}

#[tokio::test]
async fn a_model_always_names_a_secret_so_it_needs_secrets_pick() {
    let api = Api::new();
    offer_everything(&api.objects);
    let writer = api.token("w", &["platform:models.write"]);
    let r = api
        .put("/v1/models/gateway")
        .token(&writer)
        .create()
        .json(&model_body("gateway"))
        .send()
        .await;
    assert_eq!(r.code(), 403, "{}", r.text());
    assert_eq!(r.json()["permission"], "platform:secrets.pick");
    let both = api.token("both", &["platform:models.write", "platform:secrets.pick"]);
    let r = api
        .put("/v1/models/gateway")
        .token(&both)
        .create()
        .json(&model_body("gateway"))
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
    // The wrong write permission is not helped by the pick: it is checked first.
    let tools_writer = api.token(
        "t",
        &["platform:toolproviders.write", "platform:secrets.pick"],
    );
    let r = api
        .put("/v1/models/other")
        .token(&tools_writer)
        .create()
        .json(&model_body("other"))
        .send()
        .await;
    assert_eq!(r.code(), 403);
    assert_eq!(r.json()["permission"], "platform:models.write");
}

#[tokio::test]
async fn a_tool_server_that_names_no_secret_needs_only_its_own_permission() {
    let api = Api::new();
    offer_everything(&api.objects);
    let writer = api.token("w", &["platform:toolproviders.write"]);
    let open = api
        .put("/v1/tool-servers/docs")
        .token(&writer)
        .create()
        .json(&open_tool_body("docs"))
        .send()
        .await;
    assert_eq!(open.code(), 201, "{}", open.text());
    assert_eq!(open.header("location"), Some("/v1/tool-servers/docs"));
    // With a header that picks a secret it needs the pick as well.
    let r = api
        .put("/v1/tool-servers/search")
        .token(&writer)
        .create()
        .json(&tool_body("search"))
        .send()
        .await;
    assert_eq!(r.code(), 403, "{}", r.text());
    assert_eq!(r.json()["permission"], "platform:secrets.pick");
    let picker = api.token(
        "p",
        &["platform:toolproviders.write", "platform:secrets.pick"],
    );
    let r = api
        .put("/v1/tool-servers/search")
        .token(&picker)
        .create()
        .json(&tool_body("search"))
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
    let list = api
        .get("/v1/tool-servers")
        .token(&api.admin())
        .send()
        .await
        .json();
    let names: Vec<&str> = list["toolServers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["docs", "search"]);
}

#[tokio::test]
async fn an_object_agents_name_is_listed_with_them_and_cannot_be_deleted() {
    let (api, token) = Api::seeded().await;
    assert_eq!(
        make_agent(&api, &token, &folder_body("chat")).await.code(),
        201
    );
    assert_eq!(
        make_agent(&api, &token, &coder_body("coder-me"))
            .await
            .code(),
        201
    );

    let model = api.get("/v1/models/gateway").token(&token).send().await;
    assert_eq!(model.json()["usedBy"], json!(["chat", "coder-me"]));
    let tool = api.get("/v1/tool-servers/docs").token(&token).send().await;
    assert_eq!(tool.json()["usedBy"], json!(["coder-me"]));
    let list = api.get("/v1/models").token(&token).send().await.json();
    assert_eq!(list["models"][0]["usedBy"], json!(["chat", "coder-me"]));

    let r = api
        .delete("/v1/models/gateway")
        .token(&token)
        .replace(&model.etag())
        .send()
        .await;
    assert_eq!(r.code(), 409, "{}", r.text());
    assert!(r.is_problem("referenced"));
    assert_eq!(r.json()["agents"], json!(["chat", "coder-me"]));
    let r = api
        .delete("/v1/tool-servers/docs")
        .token(&token)
        .replace(&tool.etag())
        .send()
        .await;
    assert_eq!(r.code(), 409);
    assert_eq!(r.json()["agents"], json!(["coder-me"]));
    assert_eq!(
        api.get("/v1/models/gateway")
            .token(&token)
            .send()
            .await
            .code(),
        200,
        "still there"
    );
}

#[tokio::test]
async fn a_change_the_agents_cannot_survive_is_refused_with_the_agents_it_breaks() {
    let (api, token) = Api::seeded().await;
    // The coder names `docs` and does not allow plain http.
    assert_eq!(
        make_agent(&api, &token, &coder_body("coder-me"))
            .await
            .code(),
        201
    );
    assert_eq!(
        make_agent(&api, &token, &folder_body("chat")).await.code(),
        201
    );
    let docs = api.get("/v1/tool-servers/docs").token(&token).send().await;

    let mut plain = open_tool_body("docs");
    plain["url"] = json!("http://docs.elsewhere.example/mcp");
    let r = api
        .put("/v1/tool-servers/docs")
        .token(&token)
        .replace(&docs.etag())
        .json(&plain)
        .send()
        .await;
    assert_eq!(r.code(), 409, "{}", r.text());
    assert!(r.is_problem("breaks-agents"));
    let body = r.json();
    assert_eq!(body["agents"], json!(["coder-me"]), "chat does not name it");
    assert!(
        body["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("coder-me"),
        "{body}"
    );
    // Nothing was written.
    let kept = api.get("/v1/tool-servers/docs").token(&token).send().await;
    assert_eq!(kept.etag(), docs.etag());
    assert_eq!(
        kept.json()["representation"]["form"]["url"],
        "https://docs.example.com/mcp"
    );

    // The agent allows plain http: the same change is fine.
    let coder = api.get("/v1/agents/coder-me").token(&token).send().await;
    let mut allowing = coder_body("coder-me");
    allowing["allowInsecureHttp"] = json!(true);
    let r = api
        .put("/v1/agents/coder-me")
        .token(&token)
        .replace(&coder.etag())
        .json(&allowing)
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
    let r = api
        .put("/v1/tool-servers/docs")
        .token(&token)
        .replace(&docs.etag())
        .json(&plain)
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
}

#[tokio::test]
async fn a_shared_form_names_the_field_of_every_error() {
    let api = Api::new();
    offer_everything(&api.objects);
    let token = api.admin();
    let mut model = model_body("gateway");
    model["baseUrl"] = json!({ "value": "https://user:pw@gateway.example.com/v1" });
    model["apiKey"] = json!({ "secret": "orchestrator-db", "key": "uri" });
    let r = api
        .put("/v1/models/gateway")
        .token(&token)
        .create()
        .json(&model)
        .send()
        .await;
    assert_eq!(r.code(), 422, "{}", r.text());
    let fields: Vec<String> = r.json()["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["field"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        fields.contains(&"baseUrl.value".to_owned()) && fields.contains(&"apiKey".to_owned()),
        "{fields:?}"
    );
    assert!(
        !r.text().contains("pw@"),
        "an address is never quoted: {}",
        r.text()
    );

    let mut tool = tool_body("search");
    tool["headers"]["Authorization"]["prefix"] = json!("sk-pasted-a-token");
    let r = api
        .put("/v1/tool-servers/search")
        .token(&token)
        .create()
        .json(&tool)
        .send()
        .await;
    assert_eq!(r.code(), 422, "{}", r.text());
    assert_eq!(
        r.json()["errors"][0]["field"],
        "headers[Authorization].prefix"
    );
    assert!(!r.text().contains("sk-pasted-a-token"), "{}", r.text());
}

#[tokio::test]
async fn the_name_of_the_form_is_the_name_of_the_path_for_shared_objects() {
    let api = Api::new();
    offer_everything(&api.objects);
    let r = api
        .put("/v1/models/other")
        .token(&api.admin())
        .create()
        .json(&model_body("gateway"))
        .send()
        .await;
    assert_eq!(r.code(), 422);
    assert_eq!(r.json()["errors"][0]["field"], "name");
}

#[tokio::test]
async fn a_shared_object_of_gitops_is_read_and_never_written() {
    let (api, token) = Api::seeded().await;
    let spec = api
        .objects
        .get_shared(SharedKind::Model, "gateway")
        .await
        .unwrap()
        .unwrap()
        .spec;
    api.objects
        .seed_foreign(Kind::ModelEndpoint, "gateway", spec, Foreign::ArgoTracked);
    let got = api.get("/v1/models/gateway").token(&token).send().await;
    assert_eq!(got.json()["managedBy"], "gitops");
    let put = api
        .put("/v1/models/gateway")
        .token(&token)
        .replace(&got.etag())
        .json(&model_body("gateway"))
        .send()
        .await;
    assert_eq!(put.code(), 409, "{}", put.text());
    assert!(put.is_problem("managed-by-gitops"));
    let del = api
        .delete("/v1/models/gateway")
        .token(&token)
        .replace(&got.etag())
        .send()
        .await;
    assert_eq!(del.code(), 409);
    assert!(del.is_problem("managed-by-gitops"));
    let again = api
        .put("/v1/models/gateway")
        .token(&token)
        .create()
        .json(&model_body("gateway"))
        .send()
        .await;
    assert_eq!(again.code(), 409);
    assert!(again.is_problem("already-exists"));
}

#[tokio::test]
async fn shared_writes_have_the_same_body_rules_as_agents() {
    let api = Api::new();
    offer_everything(&api.objects);
    let token = api.admin();
    const SENTINEL: &str = "sentinel-api-key-value-8e7a";
    let mut body = model_body("gateway");
    body["apiKey"]["value"] = json!(SENTINEL);
    let r = api
        .put("/v1/models/gateway")
        .token(&token)
        .create()
        .json(&body)
        .send()
        .await;
    assert_eq!(r.code(), 400, "{}", r.text());
    assert!(!r.text().contains(SENTINEL));
    body = model_body("gateway");
    body["apiKey"] = json!(SENTINEL);
    let r = api
        .put("/v1/models/gateway")
        .token(&token)
        .create()
        .json(&body)
        .send()
        .await;
    assert_eq!(r.code(), 400);
    assert!(!r.text().contains(SENTINEL), "{}", r.text());

    let raw = serde_json::to_vec(&model_body("gateway")).unwrap();
    let r = api
        .put("/v1/models/gateway")
        .token(&token)
        .create()
        .raw(&raw, Some("text/plain"))
        .send()
        .await;
    assert_eq!(r.code(), 415);
    let r = api
        .put("/v1/models/gateway")
        .token(&token)
        .json(&model_body("gateway"))
        .send()
        .await;
    assert_eq!(r.code(), 428);
    let r = api
        .put("/v1/models/gateway")
        .token(&token)
        .replace("\"x\"")
        .json(&model_body("gateway"))
        .send()
        .await;
    assert_eq!(r.code(), 404);
}

#[tokio::test]
async fn the_offered_keys_are_listed_and_a_value_is_never_among_them() {
    let api = Api::new();
    offer_everything(&api.objects);
    api.objects.offer_any_key("freeform", "freeform-es");
    api.objects.decline("hidden", "KEY", "hidden-es");
    api.objects
        .plant_secret_value("agent-models", "GATEWAY_API_KEY", "secret-value-9c3b");
    let reader = api.token("picker", &["platform:secrets.pick"]);
    let r = api.get("/v1/secret-keys").token(&reader).send().await;
    assert_eq!(r.code(), 200, "{}", r.text());
    let body = r.json();
    assert_eq!(body["offerLabel"], "agents.vymalo.com/offer");
    let keys = body["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 4);
    assert!(
        keys.contains(
            &json!({ "secret": "freeform", "key": null, "externalSecret": "freeform-es" })
        )
    );
    assert!(keys.contains(&json!({ "secret": "agent-models", "key": "GATEWAY_API_KEY", "externalSecret": "agent-models-es" })));
    assert!(!r.text().contains("secret-value-9c3b") && !r.text().contains("hidden"));
}

#[tokio::test]
async fn an_object_with_a_member_no_form_says_is_yaml_only_and_a_form_may_not_replace_it() {
    let (api, token) = Api::seeded().await;
    let docs = api
        .objects
        .get_shared(SharedKind::ToolServer, "docs")
        .await
        .unwrap()
        .unwrap();
    let mut spec = docs.spec.clone();
    spec["mcp"]["futureField"] = json!("a newer version's");
    let draft = aap_ports::SharedDraft {
        name: "docs".to_owned(),
        spec,
    };
    api.objects
        .put_shared(
            SharedKind::ToolServer,
            &draft,
            &aap_ports::Precondition::Matches(docs.etag()),
        )
        .await
        .unwrap();

    let got = api.get("/v1/tool-servers/docs").token(&token).send().await;
    assert!(
        got.json()["representation"]["yamlOnly"]
            .to_string()
            .contains("futureField"),
        "{}",
        got.text()
    );
    let r = api
        .put("/v1/tool-servers/docs")
        .token(&token)
        .replace(&got.etag())
        .json(&open_tool_body("docs"))
        .send()
        .await;
    assert_eq!(r.code(), 409, "{}", r.text());
    assert!(r.is_problem("yaml-only"));
}
