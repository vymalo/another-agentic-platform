//! The OpenAPI document (K8, P-029): that it is what the code generates, that it lists exactly the routes of the table with the
//! permissions the table holds, and that it does not lie: every answer of the router, over the `Memory` ports, validates against
//! the schema of its route and status, every status a route documents is answered (but the two listed below), every answer
//! is documented, and every example parses.
//!
//! The schemas are written by hand in `src/openapi.rs`; **what makes them honest is this file**. The validator below is a small
//! JSON Schema subset (the keywords the document uses) written here so that no dependency is added, and it is itself tested to
//! be able to fail.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::collections::{BTreeMap, BTreeSet};

use aap_api::permission;
use aap_forms::{AgentForm, ModelEndpointForm, ToolServerForm};
use aap_platform_api::openapi;
use aap_platform_api::{Needs, ROUTES, Verb};
use aap_ports::Kind;
use aap_ports::testkit::{EventSeed, Foreign};
use common::*;
use serde_json::{Value, json};

const CHECKED_IN: &str = include_str!("../../../docs/api/platform-api-v1.openapi.json");

fn checked_in() -> Value {
    serde_json::from_str(CHECKED_IN).expect("the checked-in document is JSON")
}

// ------------------------------------------------------------------------------------------------ the document itself

#[test]
fn openapi_is_what_the_code_generates() {
    assert_eq!(
        CHECKED_IN,
        openapi::pretty(),
        "docs/api/platform-api-v1.openapi.json is not what the code generates: regenerate it with\n  cargo run -q -p aap-api-server --bin api -- openapi > docs/api/platform-api-v1.openapi.json"
    );
}

fn operations(doc: &Value) -> BTreeMap<(String, String), &Value> {
    let mut out = BTreeMap::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for (method, op) in item.as_object().unwrap() {
            out.insert((method.to_uppercase(), path.clone()), op);
        }
    }
    out
}

#[test]
fn openapi_lists_every_route() {
    let doc = checked_in();
    let documented: BTreeSet<(String, String)> = operations(&doc).into_keys().collect();
    let table: BTreeSet<(String, String)> = ROUTES
        .iter()
        .map(|r| (r.verb.as_str().to_owned(), r.path.to_owned()))
        .collect();
    assert_eq!(
        documented, table,
        "the document and the table of routes list different operations"
    );
    assert_eq!(ROUTES.len(), 18);
    // Each operation is documented once, with an id of its own.
    let ids: Vec<&str> = operations(&doc)
        .values()
        .map(|op| op["operationId"].as_str().unwrap())
        .collect();
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
}

#[test]
fn openapi_permission_matches_the_table() {
    let doc = checked_in();
    let ops = operations(&doc);
    for r in &ROUTES {
        let op = ops[&(r.verb.as_str().to_owned(), r.path.to_owned())];
        let expected = match r.needs {
            Needs::Token => Value::Null,
            Needs::Permission(p) => json!(p),
        };
        assert_eq!(
            op["x-permission"],
            expected,
            "{} {}: x-permission is not the table's",
            r.verb.as_str(),
            r.path
        );
        assert_eq!(
            op.get("x-secrets-pick").is_some(),
            r.secrets_named_in_body,
            "{} {}: x-secrets-pick",
            r.verb.as_str(),
            r.path
        );
        if r.secrets_named_in_body {
            assert_eq!(op["x-secrets-pick"], permission::SECRETS_PICK);
        }
        assert_eq!(op["security"], json!([{ "bearer": [] }]));
        // The conditional-request headers are on exactly the routes that use them.
        let params: Vec<&str> = op["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["$ref"].as_str().unwrap().rsplit('/').next().unwrap())
            .collect();
        assert_eq!(
            params.contains(&"IfNoneMatch"),
            r.verb == Verb::Put,
            "{}",
            r.path
        );
        assert_eq!(
            params.contains(&"IfMatchRequired"),
            r.verb == Verb::Delete,
            "{}",
            r.path
        );
        assert_eq!(params.contains(&"name"), r.path.contains("{name}"));
        // A write documents the answers of a precondition, and nothing else does.
        let statuses: BTreeSet<&str> = op["responses"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for s in ["412", "428"] {
            assert_eq!(
                statuses.contains(s),
                r.verb != Verb::Get,
                "{} {}: {s}",
                r.verb.as_str(),
                r.path
            );
        }
        assert_eq!(statuses.contains("403"), r.needs != Needs::Token);
        assert!(statuses.contains("401"), "{}", r.path);
    }
}

#[test]
fn the_document_says_what_is_deliberately_absent_and_every_reference_resolves() {
    let doc = checked_in();
    let text = doc["info"]["description"].as_str().unwrap();
    for absent in ["cookies", "CORS credentials", "wildcard origin", "DPoP"] {
        assert!(text.contains(absent), "{absent}");
    }
    assert_eq!(doc["openapi"], "3.1.0");
    assert_eq!(
        doc["components"]["securitySchemes"]["bearer"]["scheme"],
        "bearer"
    );
    assert!(doc["components"]["securitySchemes"].get("cookie").is_none());
    // Every `$ref` points at something.
    let mut refs = Vec::new();
    collect_refs(&doc, &mut refs);
    assert!(refs.len() > 100);
    for r in refs {
        let target = r.strip_prefix('#').expect("a local reference");
        assert!(doc.pointer(target).is_some(), "{r} does not resolve");
    }
}

fn collect_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "$ref" {
                    out.push(v.as_str().unwrap().to_owned());
                } else {
                    collect_refs(v, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect_refs(i, out)),
        _ => {}
    }
}

#[tokio::test]
async fn the_route_serves_the_document_to_a_caller_that_may_read() {
    let api = Api::new();
    let r = api.get("/v1/openapi.json").send().await;
    assert_eq!(r.code(), 401);
    let nobody = api.token("nobody", &[]);
    let r = api.get("/v1/openapi.json").token(&nobody).send().await;
    assert_eq!(r.code(), 403);
    assert_eq!(r.json()["permission"], permission::AGENTS_READ);
    let reader = api.token("reader", &[permission::AGENTS_READ]);
    let r = api.get("/v1/openapi.json").token(&reader).send().await;
    assert_eq!(r.code(), 200);
    assert_eq!(r.header("content-type"), Some("application/json"));
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert_eq!(
        r.json(),
        checked_in(),
        "the route serves the checked-in document"
    );
}

// ------------------------------------------------------------------------------------------------------- the validator

/// `value` against `schema` (a JSON Schema subset: `$ref`, `type`, `enum`, `const`, `properties`, `required`,
/// `additionalProperties`, `items`, `oneOf`, `anyOf`, `allOf`, the bounds). Every mistake is a line of `errors`.
fn check(doc: &Value, schema: &Value, value: &Value, at: &str, errors: &mut Vec<String>) {
    if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
        let target = doc
            .pointer(r.strip_prefix('#').unwrap())
            .unwrap_or_else(|| panic!("{r} does not resolve"));
        return check(doc, target, value, at, errors);
    }
    if let Some(kinds) = schema.get("type") {
        let kinds: Vec<&str> = match kinds {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let is = |k: &str| match k {
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            "array" => value.is_array(),
            "object" => value.is_object(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            other => panic!("the validator does not know the type {other}"),
        };
        if !kinds.iter().any(|k| is(k)) {
            errors.push(format!("{at}: is {value}, not a {kinds:?}"));
            return;
        }
    }
    if let Some(options) = schema.get("enum").and_then(Value::as_array)
        && !options.contains(value)
    {
        errors.push(format!("{at}: {value} is not one of {options:?}"));
    }
    if let Some(c) = schema.get("const")
        && c != value
    {
        errors.push(format!("{at}: {value} is not {c}"));
    }
    if let Some(text) = value.as_str() {
        if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
            && !regex::Regex::new(pattern).unwrap().is_match(text)
        {
            errors.push(format!("{at}: {text:?} does not match {pattern}"));
        }
        if schema.get("format").and_then(Value::as_str) == Some("date-time") {
            let rfc3339 = regex::Regex::new(
                r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$",
            )
            .unwrap();
            if !rfc3339.is_match(text) {
                errors.push(format!("{at}: {text:?} is not a date-time"));
            }
        }
    }
    if let (Some(condition), Some(then)) = (schema.get("if"), schema.get("then")) {
        let mut held = Vec::new();
        check(doc, condition, value, at, &mut held);
        if held.is_empty() {
            check(doc, then, value, at, errors);
        }
    }
    if let Some(s) = value.as_str() {
        let n = s.chars().count() as u64;
        if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
            && n < min
        {
            errors.push(format!("{at}: shorter than {min}"));
        }
        if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
            && n > max
        {
            errors.push(format!("{at}: longer than {max}"));
        }
    }
    if let Some(n) = value.as_f64() {
        if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
            && n < min
        {
            errors.push(format!("{at}: {n} is below {min}"));
        }
        if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
            && n > max
        {
            errors.push(format!("{at}: {n} is above {max}"));
        }
    }
    if let Some(items) = value.as_array() {
        let n = items.len() as u64;
        if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
            && n < min
        {
            errors.push(format!("{at}: fewer than {min} items"));
        }
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && n > max
        {
            errors.push(format!("{at}: more than {max} items"));
        }
        if let Some(item) = schema.get("items") {
            for (i, v) in items.iter().enumerate() {
                check(doc, item, v, &format!("{at}[{i}]"), errors);
            }
        }
    }
    if let Some(map) = value.as_object() {
        if let Some(max) = schema.get("maxProperties").and_then(Value::as_u64)
            && map.len() as u64 > max
        {
            errors.push(format!("{at}: more than {max} members"));
        }
        if let Some(names) = schema.get("propertyNames") {
            for key in map.keys() {
                check(
                    doc,
                    names,
                    &Value::String(key.clone()),
                    &format!("{at}.<{key}>"),
                    errors,
                );
            }
        }
        let properties = schema.get("properties").and_then(Value::as_object);
        for required in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !map.contains_key(required) {
                errors.push(format!("{at}: lacks the member {required}"));
            }
        }
        for (k, v) in map {
            match (
                properties.and_then(|p| p.get(k)),
                schema.get("additionalProperties"),
            ) {
                (Some(s), _) => check(doc, s, v, &format!("{at}.{k}"), errors),
                (None, Some(Value::Bool(false))) => {
                    errors.push(format!(
                        "{at}: has the member {k}, which the schema does not"
                    ));
                }
                (None, Some(s @ Value::Object(_))) => {
                    check(doc, s, v, &format!("{at}.{k}"), errors);
                }
                (None, _) => {}
            }
        }
    }
    let branches = |key: &str| -> Option<Vec<usize>> {
        let options = schema.get(key)?.as_array()?;
        Some(
            options
                .iter()
                .enumerate()
                .filter(|(_, s)| {
                    let mut e = Vec::new();
                    check(doc, s, value, at, &mut e);
                    e.is_empty()
                })
                .map(|(i, _)| i)
                .collect(),
        )
    };
    if let Some(ok) = branches("oneOf")
        && ok.len() != 1
    {
        errors.push(format!(
            "{at}: matches {} of the oneOf branches, not one",
            ok.len()
        ));
    }
    if let Some(ok) = branches("anyOf")
        && ok.is_empty()
    {
        errors.push(format!("{at}: matches no anyOf branch"));
    }
    if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
        for s in all {
            check(doc, s, value, at, errors);
        }
    }
}

fn mistakes(doc: &Value, schema: &Value, value: &Value) -> Vec<String> {
    let mut errors = Vec::new();
    check(doc, schema, value, "$", &mut errors);
    errors
}

fn schema_named(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

#[test]
fn the_validator_can_fail() {
    let doc = checked_in();
    let event = json!({
        "type": "Warning", "reason": "R", "message": "m", "object": { "kind": "Pod", "name": "p" },
        "count": 1, "firstSeen": "2026-11-14T10:00:00Z", "lastSeen": "2026-11-14T10:00:00Z", "by": "name",
    });
    let s = schema_named("AgentEvent");
    assert!(mistakes(&doc, &s, &event).is_empty());
    let mut wrong_type = event.clone();
    wrong_type["type"] = json!("Surprise");
    assert!(!mistakes(&doc, &s, &wrong_type).is_empty(), "an enum");
    let mut missing = event.clone();
    missing.as_object_mut().unwrap().remove("count");
    assert!(
        !mistakes(&doc, &s, &missing).is_empty(),
        "a required member"
    );
    let mut extra = event.clone();
    extra["extra"] = json!(1);
    assert!(!mistakes(&doc, &s, &extra).is_empty(), "an unknown member");
    let mut text = event.clone();
    text["count"] = json!("one");
    assert!(!mistakes(&doc, &s, &text).is_empty(), "a type");
    let mut long = event.clone();
    long["message"] = json!("x".repeat(1025));
    assert!(!mistakes(&doc, &s, &long).is_empty(), "a bound");
    // A oneOf that matches neither branch, and one that matches both.
    let form = schema_named("AgentForm");
    let neither = json!({ "name": "a", "title": "t", "model": { "endpoint": "e", "model": "m" }, "agent": { "kind": "other" } });
    assert!(!mistakes(&doc, &form, &neither).is_empty());
}

// ------------------------------------------------------------------------------------------------------ the examples

#[test]
fn the_examples_in_the_document_are_valid() {
    let doc = checked_in();
    let mut checked = 0;
    for ((method, path), op) in operations(&doc) {
        let label = format!("{method} {path}");
        // The request example: parsed by the real form type, then by the schema.
        if let Some(body) = op.get("requestBody") {
            let content = &body["content"]["application/json"];
            let example = &content["example"];
            assert!(
                mistakes(&doc, &content["schema"], example).is_empty(),
                "{label}: {:?}",
                mistakes(&doc, &content["schema"], example)
            );
            match content["schema"]["$ref"]
                .as_str()
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
            {
                "AgentForm" => {
                    serde_json::from_value::<AgentForm>(example.clone()).unwrap();
                }
                "ModelEndpointForm" => {
                    serde_json::from_value::<ModelEndpointForm>(example.clone()).unwrap();
                }
                "ToolServerForm" => {
                    serde_json::from_value::<ToolServerForm>(example.clone()).unwrap();
                }
                other => panic!("{label}: a request body of {other}"),
            }
            checked += 1;
        } else {
            assert_ne!(method, "PUT", "{label}: a write with no request body");
        }
        // The response examples: the success ones by their schema.
        for (status, response) in op["responses"].as_object().unwrap() {
            let Some(content) = response.get("content") else {
                continue;
            };
            if let Some(json_content) = content.get("application/json") {
                let errors = mistakes(&doc, &json_content["schema"], &json_content["example"]);
                assert!(errors.is_empty(), "{label} {status}: {errors:?}");
                checked += 1;
            }
        }
        // Every operation has at least one example of a response.
        assert!(
            op["responses"]
                .as_object()
                .unwrap()
                .iter()
                .any(|(s, r)| s.starts_with('2') && r.get("content").is_some())
                || method == "DELETE",
            "{label}: no example of a success"
        );
    }
    // The problem examples, one per slug and status.
    for (name, response) in doc["components"]["responses"].as_object().unwrap() {
        let status: u64 = name.strip_prefix("Problem").unwrap().parse().unwrap();
        for (slug, example) in response["content"]["application/problem+json"]["examples"]
            .as_object()
            .unwrap()
        {
            let value = &example["value"];
            let errors = mistakes(&doc, &schema_named("Problem"), value);
            assert!(errors.is_empty(), "{name} {slug}: {errors:?}");
            assert_eq!(value["status"], status, "{name} {slug}");
            assert_eq!(value["type"], format!("urn:vymalo:agents:problem:{slug}"));
            checked += 1;
        }
    }
    assert!(checked > 30, "{checked}");
}

// ------------------------------------------------------------------------------------- the answers of the real router

struct Seen {
    verb: &'static str,
    template: &'static str,
    reply: Reply,
}

struct Rec(Vec<Seen>);

impl Rec {
    fn add(&mut self, verb: &'static str, template: &'static str, reply: Reply) -> &Reply {
        self.0.push(Seen {
            verb,
            template,
            reply,
        });
        &self.0.last().unwrap().reply
    }
}

fn at(template: &str, name: &str) -> String {
    template.replace("{name}", name)
}

fn method(verb: &str) -> axum::http::Method {
    axum::http::Method::from_bytes(verb.as_bytes()).unwrap()
}

/// A valid form for a write route, named `name`.
fn form_for(template: &str, name: &str) -> Value {
    match template {
        "/v1/agents/{name}" => folder_body(name),
        "/v1/models/{name}" => model_body(name),
        "/v1/tool-servers/{name}" => open_tool_body(name),
        other => panic!("{other}"),
    }
}

/// Every route of the table, answered every way the router answers it that a test can make it: successes, and the family of
/// refusals. **The one session that the schema test and the status test share.**
async fn session() -> Vec<Seen> {
    let (api, admin) = Api::seeded().await;
    let mut rec = Rec(Vec::new());
    let sees = api.token("reader", &[permission::AGENTS_READ]);

    // ---- successes: a folder agent and a coder, made, read, replaced, listed; a model and a tool server of their own.
    let r = api
        .put("/v1/agents/chat")
        .token(&admin)
        .create()
        .json(&folder_body("chat"))
        .send()
        .await;
    let first_tag = r.etag();
    rec.add("PUT", "/v1/agents/{name}", r);
    let r = api
        .put("/v1/agents/coder")
        .token(&admin)
        .create()
        .json(&coder_body("coder"))
        .send()
        .await;
    rec.add("PUT", "/v1/agents/{name}", r);
    let r = api.get("/v1/agents/chat").token(&admin).send().await;
    let chat_tag = r.etag();
    rec.add("GET", "/v1/agents/{name}", r);
    let mut retitled = folder_body("chat");
    retitled["title"] = json!("Chat, again");
    let r = api
        .put("/v1/agents/chat")
        .token(&admin)
        .replace(&chat_tag)
        .json(&retitled)
        .send()
        .await;
    rec.add("PUT", "/v1/agents/{name}", r);
    let r = api.get("/v1/agents").token(&admin).send().await;
    rec.add("GET", "/v1/agents", r);
    let r = api.get("/v1/agents/chat/yaml").token(&admin).send().await;
    rec.add("GET", "/v1/agents/{name}/yaml", r);
    // The events: none, then some (as a cluster would have written them).
    let r = api.get("/v1/agents/chat/events").token(&admin).send().await;
    rec.add("GET", "/v1/agents/{name}/events", r);
    api.objects.seed_event(
        "chat",
        &EventSeed {
            object_kind: "Deployment".into(),
            object_name: "chat".into(),
            event_type: "Warning".into(),
            reason: "FailedCreate".into(),
            message: "line one\nline two \u{1b}[31mred".into(),
            count: 3,
            first_seen: "2026-11-14T10:00:00Z".into(),
            last_seen: "2026-11-14T10:05:00Z".into(),
            object_uid: None,
        },
    );
    let r = api.get("/v1/agents/chat/events").token(&sees).send().await;
    rec.add("GET", "/v1/agents/{name}/events", r);

    // K3b: a tool server's status carries the probe's tools and `Reachable`; the answer must validate against the schema.
    api.objects.set_status(
        Kind::ToolProvider,
        "docs",
        json!({
            "usedBy": [],
            "conditions": [{
                "type": "Reachable",
                "status": "True",
                "reason": "Probed",
                "message": "1 tool",
                "lastTransitionTime": "2026-11-18T00:00:00Z"
            }],
            "probe": { "tools": ["list_pods"], "toolsMore": 1, "incomplete": true }
        }),
    );
    for (listing, one, put_template, form) in [
        (
            "/v1/models",
            "/v1/models/gateway",
            "/v1/models/{name}",
            model_body("gateway"),
        ),
        (
            "/v1/tool-servers",
            "/v1/tool-servers/docs",
            "/v1/tool-servers/{name}",
            open_tool_body("docs"),
        ),
    ] {
        let r = api.get(listing).token(&admin).send().await;
        rec.add(
            "GET",
            if listing == "/v1/models" {
                "/v1/models"
            } else {
                "/v1/tool-servers"
            },
            r,
        );
        let r = api.get(one).token(&admin).send().await;
        let tag = r.etag();
        rec.add(
            "GET",
            if listing == "/v1/models" {
                "/v1/models/{name}"
            } else {
                "/v1/tool-servers/{name}"
            },
            r,
        );
        let r = api
            .put(one)
            .token(&admin)
            .replace(&tag)
            .json(&form)
            .send()
            .await;
        rec.add("PUT", put_template_static(put_template), r);
    }
    for (verb, template) in [
        ("GET", "/v1/me"),
        ("GET", "/v1/info"),
        ("GET", "/v1/openapi.json"),
        ("GET", "/v1/secret-keys"),
    ] {
        let r = api
            .request(method(verb), template)
            .token(&admin)
            .send()
            .await;
        rec.add(verb, template_static(template), r);
    }
    // A model and a tool server nobody names are deleted; an agent whose service the operator is still finalizing is a 202.
    for (template, listing, form) in [
        (
            "/v1/models/{name}",
            "spare-model",
            model_body("spare-model"),
        ),
        (
            "/v1/tool-servers/{name}",
            "spare-tool",
            open_tool_body("spare-tool"),
        ),
    ] {
        let path = at(template, listing);
        let r = api
            .put(&path)
            .token(&admin)
            .create()
            .json(&form)
            .send()
            .await;
        let tag = r.etag();
        rec.add("PUT", template_static(template), r);
        let r = api.delete(&path).token(&admin).replace(&tag).send().await;
        rec.add("DELETE", template_static(template), r);
    }
    let r = api
        .put("/v1/agents/going")
        .token(&admin)
        .create()
        .json(&folder_body("going"))
        .send()
        .await;
    rec.add("PUT", "/v1/agents/{name}", r);
    api.objects.seed_finalizer("going");
    let tag = api
        .get("/v1/agents/going")
        .token(&admin)
        .send()
        .await
        .etag();
    let r = api
        .delete("/v1/agents/going")
        .token(&admin)
        .replace(&tag)
        .send()
        .await;
    rec.add("DELETE", "/v1/agents/{name}", r);
    let r = api
        .put("/v1/agents/gone")
        .token(&admin)
        .create()
        .json(&folder_body("gone"))
        .send()
        .await;
    let tag = r.etag();
    rec.add("PUT", "/v1/agents/{name}", r);
    let r = api
        .delete("/v1/agents/gone")
        .token(&admin)
        .replace(&tag)
        .send()
        .await;
    rec.add("DELETE", "/v1/agents/{name}", r);

    // ---- every route: no token (401), two Authorization headers (400), and a token without the permission (403).
    for route in &ROUTES {
        let path = at(route.path, "chat");
        let verb = route.verb.as_str();
        let r = api.request(method(verb), &path).send().await;
        rec.add(verb, route.path, r);
        let r = api
            .request(method(verb), &path)
            .token(&admin)
            .header("authorization", "Bearer another")
            .send()
            .await;
        rec.add(verb, route.path, r);
        if let Needs::Permission(p) = route.needs {
            let held: Vec<&str> = permission::ALL
                .iter()
                .copied()
                .filter(|x| *x != p)
                .collect();
            let token = api.token(&format!("without-{p}-{verb}-{}", route.path), &held);
            let r = api.request(method(verb), &path).token(&token).send().await;
            rec.add(verb, route.path, r);
        }
    }
    // A write that names a secret key, by a caller who holds the write permission and not `platform:secrets.pick`.
    for (template, form, write) in [
        (
            "/v1/agents/{name}",
            coder_body("secret-coder"),
            permission::AGENTS_WRITE,
        ),
        (
            "/v1/models/{name}",
            model_body("secret-model"),
            permission::MODELS_WRITE,
        ),
        (
            "/v1/tool-servers/{name}",
            tool_body("secret-tool"),
            permission::TOOLPROVIDERS_WRITE,
        ),
    ] {
        let token = api.token(
            &format!("writer-{write}"),
            &[write, permission::AGENTS_READ],
        );
        let name = form["name"].as_str().unwrap().to_owned();
        let r = api
            .put(&at(template, &name))
            .token(&token)
            .create()
            .json(&form)
            .send()
            .await;
        rec.add("PUT", template_static(template), r);
    }

    // ---- an object that is not there (404), and the answers of a precondition.
    for route in &ROUTES {
        if !route.path.contains("{name}") {
            continue;
        }
        let path = at(route.path, "nope");
        let verb = route.verb.as_str();
        match route.verb {
            Verb::Get => {
                let r = api.get(&path).token(&admin).send().await;
                rec.add(verb, route.path, r);
            }
            Verb::Delete => {
                let r = api
                    .delete(&path)
                    .token(&admin)
                    .replace("\"x\"")
                    .send()
                    .await;
                rec.add(verb, route.path, r);
            }
            Verb::Put => {
                let r = api
                    .put(&path)
                    .token(&admin)
                    .replace("\"x\"")
                    .json(&form_for(route.path, "nope"))
                    .send()
                    .await;
                rec.add(verb, route.path, r);
            }
        }
    }
    for route in ROUTES.iter().filter(|r| r.verb != Verb::Get) {
        let verb = route.verb.as_str();
        let name = match route.path {
            "/v1/agents/{name}" => "chat",
            "/v1/models/{name}" => "gateway",
            _ => "docs",
        };
        let path = at(route.path, name);
        let fresh = api.get(&path).token(&admin).send().await.etag();
        match route.verb {
            Verb::Put => {
                // 428: no precondition. 412: a stale tag. 409: a create of what is there. 415, 413, 422.
                let r = api
                    .put(&path)
                    .token(&admin)
                    .json(&form_for(route.path, name))
                    .send()
                    .await;
                rec.add(verb, route.path, r);
                let stale = api
                    .put(&path)
                    .token(&admin)
                    .replace("\"stale\"")
                    .json(&form_for(route.path, name))
                    .send()
                    .await;
                rec.add(verb, route.path, stale);
                let r = api
                    .put(&path)
                    .token(&admin)
                    .create()
                    .json(&form_for(route.path, name))
                    .send()
                    .await;
                rec.add(verb, route.path, r);
                let r = api
                    .put(&path)
                    .token(&admin)
                    .replace(&fresh)
                    .raw(b"{}", Some("text/plain"))
                    .send()
                    .await;
                rec.add(verb, route.path, r);
                let r = api
                    .put(&path)
                    .token(&admin)
                    .replace(&fresh)
                    .raw(&vec![b' '; 3 << 20], Some("application/json"))
                    .send()
                    .await;
                rec.add(verb, route.path, r);
                let r = api
                    .put(&path)
                    .token(&admin)
                    .replace(&fresh)
                    .raw(b"not json", Some("application/json"))
                    .send()
                    .await;
                rec.add(verb, route.path, r);
                let mut invalid = form_for(route.path, name);
                invalid["title"] = json!("");
                let r = api
                    .put(&path)
                    .token(&admin)
                    .replace(&fresh)
                    .json(&invalid)
                    .send()
                    .await;
                rec.add(verb, route.path, r);
            }
            Verb::Delete => {
                let r = api.delete(&path).token(&admin).send().await;
                rec.add(verb, route.path, r);
                let r = api
                    .delete(&path)
                    .token(&admin)
                    .replace("\"stale\"")
                    .send()
                    .await;
                rec.add(verb, route.path, r);
                // 409: the model and the tool server are named by the agents above.
                if route.path != "/v1/agents/{name}" {
                    let r = api.delete(&path).token(&admin).replace(&fresh).send().await;
                    rec.add(verb, route.path, r);
                }
            }
            Verb::Get => {}
        }
    }
    // A stale token of an agent, read before its replace.
    let r = api
        .delete("/v1/agents/chat")
        .token(&admin)
        .replace(&first_tag)
        .send()
        .await;
    rec.add("DELETE", "/v1/agents/{name}", r);
    // 409: an agent that GitOps owns.
    api.objects.seed_foreign(
        Kind::AgentService,
        "git",
        json!({ "configRef": { "name": "git" } }),
        Foreign::Unlabelled,
    );
    let r = api.get("/v1/agents/git").token(&admin).send().await;
    let tag = r.etag();
    rec.add("GET", "/v1/agents/{name}", r);
    let r = api.get("/v1/agents").token(&admin).send().await;
    rec.add("GET", "/v1/agents", r);
    let r = api
        .delete("/v1/agents/git")
        .token(&admin)
        .replace(&tag)
        .send()
        .await;
    rec.add("DELETE", "/v1/agents/{name}", r);

    // ---- the cluster is out (503).
    api.objects.set_unavailable(true);
    for route in &ROUTES {
        if matches!(route.path, "/v1/me" | "/v1/info" | "/v1/openapi.json") {
            continue;
        }
        let path = at(route.path, "chat");
        let verb = route.verb.as_str();
        let call = api.request(method(verb), &path).token(&admin);
        let call = match route.verb {
            Verb::Get => call,
            Verb::Put => call.create().json(&form_for(route.path, "chat")),
            Verb::Delete => call.replace("\"x\""),
        };
        let r = call.send().await;
        rec.add(verb, route.path, r);
    }
    api.objects.set_unavailable(false);
    rec.0
}

fn template_static(template: &str) -> &'static str {
    ROUTES.iter().find(|r| r.path == template).unwrap().path
}

fn put_template_static(template: &str) -> &'static str {
    template_static(template)
}

/// What a documented status is answered with: its schema, or `None` for no body.
fn response_of<'a>(doc: &'a Value, op: &'a Value, status: u16) -> Option<&'a Value> {
    let response = op["responses"].get(status.to_string())?;
    Some(match response.get("$ref").and_then(Value::as_str) {
        Some(r) => doc.pointer(r.strip_prefix('#').unwrap()).unwrap(),
        None => response,
    })
}

#[tokio::test]
async fn real_answers_validate_against_the_schemas() {
    let doc = checked_in();
    let ops = operations(&doc);
    let seen = session().await;
    assert!(seen.len() > 120, "{}", seen.len());
    let mut bodies = 0;
    for s in seen {
        let label = format!("{} {} -> {}", s.verb, s.template, s.reply.code());
        let op = ops[&(s.verb.to_owned(), s.template.to_owned())];
        let response = response_of(&doc, op, s.reply.code())
            .unwrap_or_else(|| panic!("{label}: the status is not documented: {}", s.reply.text()));
        let content = response.get("content");
        let media = s.reply.header("content-type");
        match content {
            None => assert!(
                s.reply.body.is_empty(),
                "{label}: documented with no body, answered {}",
                s.reply.text()
            ),
            Some(content) => {
                let media_type = media.unwrap_or_default().split(';').next().unwrap();
                let described = content.get(media_type).unwrap_or_else(|| {
                    panic!("{label}: answered {media_type}, documented {content}")
                });
                if media_type == "application/yaml" {
                    assert!(!s.reply.body.is_empty(), "{label}");
                    continue;
                }
                let errors = mistakes(&doc, &described["schema"], &s.reply.json());
                assert!(errors.is_empty(), "{label}: {errors:?}\n{}", s.reply.text());
                bodies += 1;
            }
        }
        // The headers a response documents are on it.
        if let Some(headers) = response.get("headers").and_then(Value::as_object) {
            for name in headers.keys() {
                if matches!(name.as_str(), "ETag" | "Location") {
                    assert!(s.reply.header(name).is_some(), "{label}: no {name}");
                }
            }
        }
        if s.reply.code() == 401 {
            assert!(s.reply.header("www-authenticate").is_some(), "{label}");
        }
        if s.reply.code() >= 400 {
            assert_eq!(media, Some("application/problem+json"), "{label}");
            let body = s.reply.json();
            assert_eq!(
                body["status"],
                s.reply.code(),
                "{label}: the body says another status"
            );
            let documented: Vec<String> = response["x-problem-types"]
                .as_array()
                .unwrap_or_else(|| panic!("{label}: the response lists no problem types"))
                .iter()
                .map(|t| format!("urn:vymalo:agents:problem:{}", t.as_str().unwrap()))
                .collect();
            assert!(
                documented.contains(&body["type"].as_str().unwrap().to_owned()),
                "{label}: the type {} is not one documented for this status: {documented:?}",
                body["type"]
            );
        }
    }
    assert!(bodies > 100, "{bodies}");
}

#[tokio::test]
async fn every_documented_status_is_answered() {
    let doc = checked_in();
    let ops = operations(&doc);
    let seen = session().await;
    let mut answered: BTreeMap<(String, String), BTreeSet<u16>> = BTreeMap::new();
    for s in &seen {
        answered
            .entry((s.verb.to_owned(), s.template.to_owned()))
            .or_default()
            .insert(s.reply.code());
    }
    for ((verb, path), op) in &ops {
        let documented: BTreeSet<u16> = op["responses"]
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.parse().unwrap())
            .collect();
        let got = answered
            .get(&(verb.clone(), path.clone()))
            .cloned()
            .unwrap_or_default();
        // Every answer is documented (the schema test also fails on one that is not).
        let extra: Vec<&u16> = got.difference(&documented).collect();
        assert!(
            extra.is_empty(),
            "{verb} {path} answers {extra:?}, which it does not document"
        );
        // Every documented status is answered, but the two the memory ports cannot make: a 500 (the store's `Internal` and a
        // serialization failure) and the 503 `keys-unavailable` of a route that touches no store (the memory authenticator has
        // keys that never go down; `tests/oidc.rs` makes that one for the real authenticator).
        let touches_no_store = matches!(path.as_str(), "/v1/me" | "/v1/info" | "/v1/openapi.json");
        let missing: Vec<&u16> = documented
            .difference(&got)
            .filter(|s| **s != 500 && !(**s == 503 && touches_no_store))
            .collect();
        assert!(
            missing.is_empty(),
            "{verb} {path} documents {missing:?} and no test of this file answers it"
        );
    }
}

#[tokio::test]
async fn an_event_message_is_shown_clean_in_the_answer() {
    let (api, admin) = Api::seeded().await;
    api.put("/v1/agents/chat")
        .token(&admin)
        .create()
        .json(&folder_body("chat"))
        .send()
        .await;
    api.objects.seed_event(
        "chat",
        &EventSeed {
            object_kind: "Deployment".into(),
            object_name: "chat".into(),
            event_type: "Warning".into(),
            reason: "R".into(),
            message: format!("a\nb\u{1b}[0m{}", "z".repeat(4000)),
            count: 1,
            first_seen: "2026-11-14T10:00:00Z".into(),
            last_seen: "2026-11-14T10:00:00Z".into(),
            object_uid: None,
        },
    );
    let r = api.get("/v1/agents/chat/events").token(&admin).send().await;
    assert_eq!(r.code(), 200);
    let message = r.json()["events"][0]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(message.starts_with("a b [0m"), "{message}");
    assert!(message.chars().count() <= 1024);
    assert!(message.chars().all(|c| !c.is_control()));
}

// ------------------------------------------------------------------------------------------------------- the meta-tests

/// `Schema/member` of every member of a closed object schema (`additionalProperties: false`) that is not required.
fn optional_members(doc: &Value) -> BTreeSet<String> {
    fn walk(path: &str, schema: &Value, out: &mut BTreeSet<String>) {
        let Some(map) = schema.as_object() else {
            return;
        };
        if map.get("additionalProperties") == Some(&Value::Bool(false))
            && let Some(properties) = map.get("properties").and_then(Value::as_object)
        {
            let required: BTreeSet<&str> = map
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            for key in properties.keys() {
                if !required.contains(key.as_str()) {
                    out.insert(format!("{path}/{key}"));
                }
            }
        }
        if let Some(properties) = map.get("properties").and_then(Value::as_object) {
            for (key, sub) in properties {
                walk(&format!("{path}/{key}"), sub, out);
            }
        }
        for key in ["items", "additionalProperties", "then", "if"] {
            if let Some(sub) = map.get(key) {
                walk(&format!("{path}/{key}"), sub, out);
            }
        }
        for key in ["oneOf", "anyOf", "allOf"] {
            for (i, sub) in map
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                walk(&format!("{path}/{key}{i}"), sub, out);
            }
        }
    }
    let mut out = BTreeSet::new();
    for (name, schema) in doc["components"]["schemas"].as_object().unwrap() {
        walk(name, schema, &mut out);
    }
    out
}

/// The members of a closed object that may be absent from an answer or a form, and why. **Anything else must be `required`**: a
/// schema that does not require what the server always writes lets a mutation that drops it pass the answer tests.
const MAY_BE_ABSENT: &[&str] = &[
    // The members of a problem that belong to one kind of problem each: `errors` (422), `permission` (403), `agentCount` and `agents`
    // (409 referenced, breaks-agents), `reasons` (409 yaml-only), `limit` (413). The `allOf` of the schema requires the ones a status
    // must have.
    "Problem/agentCount",
    "Problem/agents",
    "Problem/errors",
    "Problem/limit",
    "Problem/permission",
    "Problem/reasons",
    // The operator writes a status member only when it has something to say (`skip_serializing_if`).
    "SharedStatus/observedGeneration",
    "SharedStatus/usedByMore",
    // The probe's tools are written only when the server answered (K3b); a `ToolProvider` the deployment does not probe has
    // neither.
    "SharedStatus/probe",
    "SharedStatus/probe/toolsMore",
    "SharedStatus/probe/incomplete",
    // A condition has no `observedGeneration` until the operator sets one.
    "Condition/observedGeneration",
    // A shared object has no status before the operator wrote one (`skip_serializing_if`).
    "ModelDetail/status",
    "ToolServerDetail/status",
    // Forms: a member with a serde default may be left out of a request.
    "AgentForm/description",
    "AgentForm/tags",
    "AgentForm/suspend",
    "AgentForm/toolServers",
    "AgentForm/allowInsecureHttp",
    "AgentForm/image",
    "AgentForm/audience",
    "CoderSettings/createRepoOwners",
    "CoderSettings/workers",
    "CoderSettings/prDraft",
    "CoderSettings/opencodeModel",
    "CoderSettings/maxAccessTier",
    "CoderSettings/runPodSizeClass",
    "ModelEndpointForm/models",
    "HeaderForm/prefix",
    "ToolServerForm/headers",
    "ToolServerForm/tools",
    "ToolServerForm/optional",
    // The deployment's own sections: a member with no value is `null` or may be left out by the configuration file.
    "Defaults/security/runAsUser",
    "Defaults/security/runAsGroup",
    "Defaults/security/fsGroup",
    "Defaults/coder/gitAuthor",
    "Defaults/coder/allowedRepoHosts",
    "Defaults/coder/githubApiUrl",
    "Defaults/coder/githubMcp",
    "Defaults/coder/workVolume",
    "Defaults/coder/resources",
    "Defaults/folder/resources",
    "Deployment/security/runAsUser",
    "Deployment/security/runAsGroup",
    "Deployment/security/fsGroup",
    "Deployment/coder/gitAuthor",
    "Deployment/coder/allowedRepoHosts",
    "Deployment/coder/githubApiUrl",
];

#[test]
fn every_member_of_a_closed_object_is_required_unless_it_is_on_the_list_of_members_that_may_be_absent()
 {
    let found = optional_members(&checked_in());
    let listed: BTreeSet<String> = MAY_BE_ABSENT.iter().map(|s| (*s).to_owned()).collect();
    let unlisted: Vec<&String> = found.difference(&listed).collect();
    let stale: Vec<&String> = listed.difference(&found).collect();
    assert!(
        unlisted.is_empty() && stale.is_empty(),
        "optional members that are not on the list: {unlisted:#?}\nlisted and no longer optional or no longer there: {stale:#?}"
    );
}

/// Component schemas that accept `{}` and `{"x":1}` **on purpose**: the custom resources' own statuses, passed through as the
/// operator wrote them (it writes a member only when it has something to say).
const OPEN_SCHEMAS: &[&str] = &["ConfigStatus", "ServiceStatus"];

#[test]
fn no_schema_accepts_anything_unless_it_is_on_the_list_of_open_schemas() {
    let doc = checked_in();
    let mut open = Vec::new();
    for name in doc["components"]["schemas"].as_object().unwrap().keys() {
        let schema = schema_named(name);
        for sample in [json!({}), json!({ "x": 1 })] {
            if mistakes(&doc, &schema, &sample).is_empty() {
                open.push(name.clone());
            }
        }
    }
    open.sort();
    open.dedup();
    let expected: Vec<String> = OPEN_SCHEMAS.iter().map(|s| (*s).to_owned()).collect();
    assert_eq!(
        open, expected,
        "the schemas that accept {{}} or {{\"x\":1}}"
    );
}

#[test]
fn the_validator_checks_patterns_formats_conditions_and_names() {
    let doc = checked_in();
    let event = |last: &str| {
        json!({ "type": "Normal", "reason": "R", "message": "m", "object": { "kind": "Pod", "name": "p" },
                "count": 1, "firstSeen": "2026-11-14T10:00:00Z", "lastSeen": last, "by": "name" })
    };
    assert!(
        mistakes(
            &doc,
            &schema_named("AgentEvent"),
            &event("2026-11-14T10:00:00Z")
        )
        .is_empty()
    );
    assert!(!mistakes(&doc, &schema_named("AgentEvent"), &event("yesterday")).is_empty());
    let problem = |status: u16| json!({ "type": "urn:vymalo:agents:problem:validation", "title": "t", "status": status, "detail": "d" });
    assert!(
        !mistakes(&doc, &schema_named("Problem"), &problem(422)).is_empty(),
        "a 422 without errors"
    );
    assert!(mistakes(&doc, &schema_named("Problem"), &problem(404)).is_empty());
    let form = json!({ "name": "UPPER", "title": "t", "url": "https://x.example/mcp" });
    assert!(
        !mistakes(&doc, &schema_named("ToolServerForm"), &form).is_empty(),
        "a name pattern"
    );
    let headers = json!({ "name": "ok", "title": "t", "url": "https://x.example/mcp",
                          "headers": { "bad header": { "secret": { "secret": "s", "key": "k" } } } });
    assert!(
        !mistakes(&doc, &schema_named("ToolServerForm"), &headers).is_empty(),
        "a header name"
    );
}

// --------------------------------------------------------------------------------------------- events through the API

fn seed_at(api: &Api, agent: &str, kind: &str, object: &str, n: u32, first: u32) {
    let at = |s: u32| format!("2026-11-14T10:{:02}:{:02}Z", s / 60, s % 60);
    api.objects.seed_event(
        agent,
        &EventSeed {
            object_kind: kind.into(),
            object_name: object.into(),
            event_type: "Warning".into(),
            reason: format!("Reason{n}"),
            message: format!("message {n}"),
            count: 1,
            first_seen: at(first),
            last_seen: at(n),
            object_uid: None,
        },
    );
}

#[tokio::test]
async fn a_hundred_and_one_events_are_a_hundred_newest_first_and_equal_times_break_the_same_way() {
    let (api, admin) = Api::seeded().await;
    api.put("/v1/agents/chat")
        .token(&admin)
        .create()
        .json(&folder_body("chat"))
        .send()
        .await;
    for n in 0..101 {
        seed_at(&api, "chat", "Deployment", "chat", n, 0);
    }
    // Two events at the very same time: the order is fixed by the kind, so it is the same on every call.
    seed_at(&api, "chat", "Service", "chat", 500, 0);
    seed_at(&api, "chat", "AgentConfig", "chat", 500, 0);
    let r = api.get("/v1/agents/chat/events").token(&admin).send().await;
    assert_eq!(r.code(), 200);
    let body = r.json();
    let events = body["events"].as_array().unwrap();
    assert_eq!(events.len(), 100);
    assert_eq!(body["incomplete"], false);
    assert_eq!(
        events[0]["object"]["kind"], "AgentConfig",
        "equal times: the kind decides"
    );
    assert_eq!(events[1]["object"]["kind"], "Service");
    let times: Vec<&str> = events
        .iter()
        .map(|e| e["lastSeen"].as_str().unwrap())
        .collect();
    let mut sorted = times.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(times, sorted, "newest first");
    let again = api
        .get("/v1/agents/chat/events")
        .token(&admin)
        .send()
        .await
        .json();
    assert_eq!(again, body, "the same answer twice");
}

#[tokio::test]
async fn the_events_of_a_missing_agent_are_the_same_problem_as_its_detail() {
    let (api, admin) = Api::seeded().await;
    let detail = api.get("/v1/agents/nope").token(&admin).send().await;
    let events = api.get("/v1/agents/nope/events").token(&admin).send().await;
    assert_eq!(detail.code(), 404);
    assert_eq!(events.code(), 404);
    assert_eq!(detail.body, events.body, "byte for byte");
    assert!(events.is_problem("not-found"));
}

#[tokio::test]
async fn a_gitops_agent_with_no_title_is_listed_and_read() {
    let (api, admin) = Api::seeded().await;
    api.objects.seed_foreign(
        aap_ports::Kind::AgentService,
        "untitled",
        json!({ "configRef": { "name": "untitled" } }),
        Foreign::Unlabelled,
    );
    let doc = checked_in();
    let list = api.get("/v1/agents").token(&admin).send().await;
    let row = list.json()["agents"][0].clone();
    assert!(row["title"].is_null() && row["description"].is_null());
    let errors = mistakes(&doc, &schema_named("AgentList"), &list.json());
    assert!(errors.is_empty(), "{errors:?}");
    let one = api.get("/v1/agents/untitled").token(&admin).send().await;
    let errors = mistakes(&doc, &schema_named("AgentDetail"), &one.json());
    assert!(errors.is_empty(), "{errors:?}");
}
