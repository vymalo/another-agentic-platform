//! The examples: the valid ones are accepted, each invalid one is refused for the reason it
//! states, and every CEL rule has an invalid example.
//!
//! The rules are evaluated with `kube-cel`, a client-side CEL implementation. It is a proxy for an
//! API server, not one: the `kind` job of `.github/workflows/operator.yml` is the real check. It
//! does not check the OpenAPI schema either; `bounds` below does the three keywords the examples use.

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use std::fs;
use std::path::{Path, PathBuf};

use aap_api::{AgentConfig, AgentService, ModelEndpoint, ToolProvider, crds};
use kube::core::cel::Validator;
use serde::Deserialize;
use serde_json::Value;

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn yaml_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .collect();
    files.sort();
    files
}

/// The documents of a (multi-document) YAML file, as JSON.
fn documents(path: &Path) -> Vec<Value> {
    let text =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_yaml::Deserializer::from_str(&text)
        .map(|d| Value::deserialize(d).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .collect()
}

/// Parse a document into its kind, and serialise it back: defaults are filled in as an API server
/// would, a field the types do not know is dropped.
fn through_the_types(doc: &Value) -> Value {
    match doc["kind"].as_str() {
        Some("AgentService") => {
            let typed: AgentService = serde_json::from_value(doc.clone()).expect("an AgentService");
            serde_json::to_value(&typed).expect("serialises")
        }
        Some("AgentConfig") => {
            let typed: AgentConfig = serde_json::from_value(doc.clone()).expect("an AgentConfig");
            serde_json::to_value(&typed).expect("serialises")
        }
        Some("ModelEndpoint") => {
            let typed: ModelEndpoint =
                serde_json::from_value(doc.clone()).expect("a ModelEndpoint");
            serde_json::to_value(&typed).expect("serialises")
        }
        Some("ToolProvider") => {
            let typed: ToolProvider = serde_json::from_value(doc.clone()).expect("a ToolProvider");
            serde_json::to_value(&typed).expect("serialises")
        }
        other => panic!("unexpected kind {other:?}"),
    }
}

fn schema_for(kind: &str) -> Value {
    let crd = crds()
        .into_iter()
        .find(|c| c.spec.names.kind == kind)
        .unwrap_or_else(|| panic!("no CRD of kind {kind}"));
    let schema = crd.spec.versions[0]
        .schema
        .as_ref()
        .and_then(|s| s.open_api_v3_schema.as_ref());
    serde_json::to_value(schema.expect("schema")).expect("serialises")
}

/// Every key and value of `a` is in `b`: nothing the author wrote was lost by the types.
fn subset(a: &Value, b: &Value, path: &str) -> Result<(), String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            for (k, v) in x {
                let Some(w) = y.get(k) else {
                    return Err(format!("{path}.{k} is not in the typed form"));
                };
                subset(v, w, &format!("{path}.{k}"))?;
            }
            Ok(())
        }
        (Value::Array(x), Value::Array(y)) if x.len() == y.len() => x
            .iter()
            .zip(y)
            .enumerate()
            .try_for_each(|(i, (v, w))| subset(v, w, &format!("{path}[{i}]"))),
        _ if a == b => Ok(()),
        _ => Err(format!("{path}: {a} became {b}")),
    }
}

/// The bounds of the OpenAPI schema `kube-cel` does not check, worded as kube-apiserver words them
/// (*verified 2026-10-15*, v1.35.8): `maxItems`, `maxLength` and `minLength`, the three keywords
/// the audience uses. Not a schema validator: a keyword the examples do not use is not read.
fn bounds(schema: &Value, value: &Value, path: &str, out: &mut Vec<String>) {
    match value {
        Value::Array(items) => {
            if let Some(max) = schema["maxItems"]
                .as_u64()
                .filter(|m| items.len() as u64 > *m)
            {
                out.push(format!(
                    "Too many: {}: must have at most {max} items",
                    items.len()
                ));
            }
            for (i, item) in items.iter().enumerate() {
                bounds(&schema["items"], item, &format!("{path}[{i}]"), out);
            }
        }
        Value::String(text) => {
            if let Some(max) = schema["maxLength"]
                .as_u64()
                .filter(|m| text.len() as u64 > *m)
            {
                out.push(format!("Too long: may not be more than {max} bytes"));
            }
            if let Some(min) = schema["minLength"]
                .as_u64()
                .filter(|m| (text.chars().count() as u64) < *m)
            {
                out.push(format!(
                    "{path} in body should be at least {min} chars long"
                ));
            }
        }
        Value::Object(map) => {
            for (key, child) in map {
                let at = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let declared = &schema["properties"][key.as_str()];
                let of = if declared.is_null() {
                    &schema["additionalProperties"]
                } else {
                    declared
                };
                bounds(of, child, &at, out);
            }
        }
        _ => {}
    }
}

fn validate(doc: &Value) -> Result<(), Vec<String>> {
    let typed = through_the_types(doc);
    let kind = doc["kind"].as_str().expect("kind");
    let schema = schema_for(kind);
    let mut found = Vec::new();
    bounds(&schema, &typed, "", &mut found);
    if let Err(errors) = Validator::new().validate(&schema, &typed, None) {
        found.extend(errors.iter().map(|e| e.message.clone()));
    }
    if found.is_empty() { Ok(()) } else { Err(found) }
}

#[test]
fn examples_round_trip_through_the_types_and_lose_nothing() {
    let files = yaml_files(&examples_dir());
    assert!(
        files.iter().any(|f| f.ends_with("coder.yaml"))
            && files.iter().any(|f| f.ends_with("chat.yaml"))
    );
    for file in files {
        for doc in documents(&file) {
            let typed = through_the_types(&doc);
            if let Err(why) = subset(&doc, &typed, "") {
                panic!("{}: {why}", file.display());
            }
            // and the typed form is a fixed point
            assert_eq!(through_the_types(&typed), typed, "{}", file.display());
        }
    }
}

#[test]
fn each_example_is_an_agent_service_and_its_agent_config() {
    for name in ["coder.yaml", "chat.yaml"] {
        let docs = documents(&examples_dir().join(name));
        let kinds: Vec<_> = docs
            .iter()
            .map(|d| d["kind"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(kinds, ["AgentService", "AgentConfig"], "{name}");
        // the service names the config
        assert_eq!(
            docs[0]["spec"]["configRef"]["name"], docs[1]["metadata"]["name"],
            "{name}"
        );
    }
}

#[test]
fn the_dashboard_example_names_what_it_references() {
    let docs = documents(&examples_dir().join("coder-me.yaml"));
    let kinds: Vec<_> = docs
        .iter()
        .map(|d| d["kind"].as_str().unwrap_or_default())
        .collect();
    // the referents first, then the config, then the service that names it
    assert_eq!(
        kinds,
        [
            "ModelEndpoint",
            "ToolProvider",
            "AgentConfig",
            "AgentService"
        ]
    );
    let (endpoint, provider, config, service) = (&docs[0], &docs[1], &docs[2], &docs[3]);
    assert_eq!(
        config["spec"]["model"]["endpointRef"]["name"],
        endpoint["metadata"]["name"]
    );
    assert_eq!(
        config["spec"]["tools"]["mcpServers"]["websearch"]["providerRef"]["name"],
        provider["metadata"]["name"]
    );
    assert_eq!(
        service["spec"]["configRef"]["name"],
        config["metadata"]["name"]
    );
    // the reference replaces what the inline form says, and so does the default image
    assert!(config["spec"]["model"].get("baseUrl").is_none());
    assert!(config["spec"]["model"].get("apiKeySecretRef").is_none());
    assert!(config["spec"]["environment"].get("image").is_none());
    assert_eq!(
        service["spec"]["access"]["audience"],
        serde_json::json!(["agent.use:coder-me"])
    );
}

/// The `chat` example with another `access.audience`, validated like any example.
fn audience_validates(audience: Option<Value>) -> Result<(), Vec<String>> {
    let mut service = documents(&examples_dir().join("chat.yaml")).remove(0);
    match audience {
        Some(a) => service["spec"]["access"] = serde_json::json!({ "audience": a }),
        None => service["spec"]["access"] = serde_json::json!({}),
    }
    validate(&service)
}

#[test]
fn an_audience_is_absent_empty_a_star_alone_or_up_to_32_visible_values() {
    let many = |n: usize, len: usize| -> Value {
        Value::Array(
            (0..n)
                .map(|i| Value::String(format!("{i:0>len$}")))
                .collect(),
        )
    };
    for (what, audience) in [
        ("absent", None),
        ("empty", Some(serde_json::json!([]))),
        ("a star alone", Some(serde_json::json!(["*"]))),
        (
            "a use permission",
            Some(serde_json::json!(["agent.use:coder-me"])),
        ),
        (
            "two values",
            Some(serde_json::json!([
                "agent.use:chat",
                "agent.use:researcher"
            ])),
        ),
        ("32 values", Some(many(32, 3))),
        ("a value of 64 characters", Some(many(1, 64))),
        (
            "a star in a longer value",
            Some(serde_json::json!(["agent.*", "*.use"])),
        ),
    ] {
        if let Err(errors) = audience_validates(audience) {
            panic!("{what} was refused: {errors:#?}");
        }
    }
    // The refusals the examples pin one by one, and the ones they share a rule with.
    for (what, audience, reason) in [
        (
            "a star beside a value",
            serde_json::json!(["*", "agent.use:chat"]),
            "access.audience: the value * is only allowed alone",
        ),
        (
            "a star twice",
            serde_json::json!(["*", "*"]),
            "access.audience: the value * is only allowed alone",
        ),
        (
            "a space",
            serde_json::json!(["agent use"]),
            "access.audience: a value is visible ASCII characters, with no space or control character",
        ),
        (
            "a tab",
            serde_json::json!(["agent\tuse"]),
            "access.audience: a value is visible ASCII characters, with no space or control character",
        ),
        (
            "a non-ASCII character",
            serde_json::json!(["agent.use:é"]),
            "access.audience: a value is visible ASCII characters, with no space or control character",
        ),
        (
            "33 values",
            many(33, 3),
            "Too many: 33: must have at most 32 items",
        ),
        (
            "65 characters",
            many(1, 65),
            "Too long: may not be more than 64 bytes",
        ),
    ] {
        match audience_validates(Some(audience)) {
            Ok(()) => panic!("{what} was accepted"),
            Err(errors) => assert!(errors.contains(&reason.to_owned()), "{what}: {errors:#?}"),
        }
    }
}

#[test]
fn valid_examples_pass_every_cel_rule() {
    for file in yaml_files(&examples_dir()) {
        for doc in documents(&file) {
            if let Err(errors) = validate(&doc) {
                panic!("{} was refused: {errors:#?}", file.display());
            }
        }
    }
}

fn expectation(path: &Path) -> String {
    let text = fs::read_to_string(path).expect("readable");
    text.lines()
        .find_map(|l| l.strip_prefix("# expect: "))
        .unwrap_or_else(|| panic!("{} has no `# expect:` line", path.display()))
        .trim()
        .to_owned()
}

#[test]
fn each_invalid_example_is_refused_for_the_reason_it_states() {
    let files = yaml_files(&examples_dir().join("invalid"));
    assert!(!files.is_empty());
    for file in files {
        let docs = documents(&file);
        assert_eq!(
            docs.len(),
            1,
            "{}: one document, so the whole file is refused",
            file.display()
        );
        let expected = expectation(&file);
        match validate(&docs[0]) {
            Ok(()) => panic!("{} was accepted", file.display()),
            Err(errors) => assert!(
                errors.contains(&expected),
                "{}: expected {expected:?}, got {errors:#?}",
                file.display()
            ),
        }
    }
}

#[test]
fn every_cel_rule_has_an_invalid_example() {
    let expected: Vec<String> = yaml_files(&examples_dir().join("invalid"))
        .iter()
        .map(|f| expectation(f))
        .collect();
    let mut messages = Vec::new();
    for crd in crds() {
        let v = serde_json::to_value(&crd).expect("serialises");
        collect_messages(&v, &mut messages);
    }
    assert!(!messages.is_empty());
    for m in messages {
        assert!(
            expected.contains(&m),
            "no examples/invalid file expects {m:?}"
        );
    }
}

fn collect_messages(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            if let Some(Value::Array(rules)) = map.get("x-kubernetes-validations") {
                out.extend(
                    rules
                        .iter()
                        .filter_map(|r| r["message"].as_str().map(str::to_owned)),
                );
            }
            map.values().for_each(|c| collect_messages(c, out));
        }
        Value::Array(items) => items.iter().for_each(|c| collect_messages(c, out)),
        _ => {}
    }
}
