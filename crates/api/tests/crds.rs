//! The generated CRDs: identity, every CEL rule §59a and §60a list, and the schema bounds those rules need.

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use aap_api::{AgentConfig, AgentService, GROUP, ModelEndpoint, ToolProvider, VERSION, crds};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::core::CustomResourceExt;
use serde_json::Value;

fn schema_of(crd: &CustomResourceDefinition) -> Value {
    let schema = crd.spec.versions[0]
        .schema
        .as_ref()
        .and_then(|s| s.open_api_v3_schema.as_ref())
        .expect("a derived CRD has a schema");
    serde_json::to_value(schema).expect("a schema serialises")
}

/// Every `(rule, message)` of `x-kubernetes-validations` in a schema, at any depth.
fn validations(v: &Value, out: &mut Vec<(String, String)>) {
    match v {
        Value::Object(map) => {
            if let Some(Value::Array(rules)) = map.get("x-kubernetes-validations") {
                for r in rules {
                    out.push((
                        r["rule"].as_str().unwrap_or_default().to_owned(),
                        r["message"].as_str().unwrap_or_default().to_owned(),
                    ));
                }
            }
            map.values().for_each(|c| validations(c, out));
        }
        Value::Array(items) => items.iter().for_each(|c| validations(c, out)),
        _ => {}
    }
}

fn all_rules() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for crd in crds() {
        validations(&schema_of(&crd), &mut out);
    }
    out
}

#[test]
fn every_kind_is_namespaced_v1alpha1_of_the_platform_group() {
    let all = crds();
    let names: Vec<_> = all
        .iter()
        .filter_map(|c| c.metadata.name.as_deref())
        .collect();
    assert_eq!(
        names,
        [
            "agentconfigs.agents.vymalo.com",
            "agentservices.agents.vymalo.com",
            "modelendpoints.agents.vymalo.com",
            "toolproviders.agents.vymalo.com"
        ]
    );
    for crd in &all {
        assert_eq!(crd.spec.group, GROUP);
        assert_eq!(crd.spec.group, "agents.vymalo.com");
        assert_eq!(crd.spec.scope, "Namespaced");
        assert_eq!(crd.spec.versions.len(), 1);
        let v = &crd.spec.versions[0];
        assert_eq!(v.name, VERSION);
        assert_eq!(v.name, "v1alpha1");
        assert!(v.served && v.storage);
        // The two kinds of §59a have a status the controller writes; the two of §60a are
        // configuration only (the Platform API computes "used by" itself).
        let has_status = v.subresources.as_ref().is_some_and(|s| s.status.is_some());
        let configuration_only =
            ["ModelEndpoint", "ToolProvider"].contains(&crd.spec.names.kind.as_str());
        assert_eq!(
            has_status, !configuration_only,
            "status subresource of {}",
            crd.spec.names.kind
        );
    }
    assert_eq!(AgentService::crd().spec.names.kind, "AgentService");
    assert_eq!(AgentConfig::crd().spec.names.kind, "AgentConfig");
    assert_eq!(ModelEndpoint::crd().spec.names.plural, "modelendpoints");
    assert_eq!(ToolProvider::crd().spec.names.plural, "toolproviders");
}

/// The rules §59a, "Validation", lists for the CRD. `(rule, message)`.
const SPEC_RULES: &[(&str, &str)] = &[
    // interfaces.responses.enabled and interfaces.mcp.enabled must be false (one field type, used twice)
    (
        "self == false",
        "v0 serves A2A only: this interface cannot be enabled",
    ),
    // exactly one of store.postgres.secretRef and store.postgres.cnpg
    (
        "has(self.secretRef) != has(self.cnpg)",
        "exactly one of store.postgres.secretRef and store.postgres.cnpg",
    ),
    // exactly one of github.app and github.token
    (
        "has(self.app) != has(self.token)",
        "exactly one of github.app and github.token",
    ),
    // exactly one of app.installationId and app.owners
    (
        "has(self.installationId) != has(self.owners)",
        "exactly one of github.app.installationId and github.app.owners",
    ),
    // exactly one of agent.folder.files and agent.folder.configMapRef
    (
        "has(self.files) != has(self.configMapRef)",
        "exactly one of agent.folder.files and agent.folder.configMapRef",
    ),
    // exactly one of folder and embedded
    (
        "has(self.folder) != has(self.embedded)",
        "exactly one of agent.folder and agent.embedded",
    ),
    // binary: adam-coder needs embedded and the coder block
    (
        "self.binary != 'adam-coder' || (has(self.agent.embedded) && has(self.coder))",
        "binary adam-coder needs agent.embedded and the coder block",
    ),
    // binary: adam-agent needs folder and no coder block
    (
        "self.binary != 'adam-agent' || (has(self.agent.folder) && !has(self.coder))",
        "binary adam-agent needs agent.folder and no coder block",
    ),
    // scaling.front only with topology: split
    (
        "!has(self.front) || self.topology == 'split'",
        "scaling.front is only allowed with topology: split",
    ),
    // beyond the list, same shape: a model endpoint is a value or a Secret key (an AgentConfig's
    // model.baseUrl and a ModelEndpoint's baseUrl)
    (
        "has(self.value) != has(self.secretRef)",
        "exactly one of baseUrl.value and baseUrl.secretRef",
    ),
    // §60a, AD-029: a model is an endpointRef, or the inline address and key, never both
    (
        "!has(self.endpointRef) || (!has(self.baseUrl) && !has(self.apiKeySecretRef))",
        "model.endpointRef is exclusive with model.baseUrl and model.apiKeySecretRef",
    ),
    // ... and the inline form stays complete: the schema used to require both fields
    (
        "has(self.endpointRef) || (has(self.baseUrl) && has(self.apiKeySecretRef))",
        "model needs an endpointRef, or both a baseUrl and an apiKeySecretRef",
    ),
    // §60a, AD-029: an MCP server is a providerRef, or the inline url and headers, never both
    (
        "!has(self.providerRef) || (!has(self.url) && (!has(self.headers) || self.headers.size() == 0))",
        "mcpServers.providerRef is exclusive with url and headers",
    ),
    // ... and the inline form stays complete: the schema used to require the url
    (
        "has(self.providerRef) || has(self.url)",
        "an MCP server needs a url, or a providerRef",
    ),
    // §60a, AD-028: access.audience, whose count and lengths are bounds of the schema (below)
    (
        "!self.exists(a, a == '*') || self.size() == 1",
        "access.audience: the value * is only allowed alone",
    ),
    (
        "self.all(a, a.matches('^[!-~]+$'))",
        "access.audience: a value is visible ASCII characters, with no space or control character",
    ),
];

#[test]
fn the_generated_crds_contain_every_cel_rule_of_the_spec() {
    let found = all_rules();
    for (rule, message) in SPEC_RULES {
        assert!(
            found.iter().any(|(r, m)| r == rule && m == message),
            "missing rule {rule:?} with message {message:?}; the CRDs have {found:#?}"
        );
    }
}

#[test]
fn the_crds_contain_no_cel_rule_the_tests_do_not_know() {
    let known: Vec<_> = SPEC_RULES.iter().map(|(r, _)| *r).collect();
    for (rule, _) in all_rules() {
        assert!(
            known.contains(&rule.as_str()),
            "a CEL rule with no test: {rule:?}"
        );
    }
}

#[test]
fn a_secret_is_never_a_value() {
    // No string field of either schema is named like a secret value: a secret is a `secretRef`
    // object (a name and a key), or the object that holds one (`github.token`).
    fn leaks(v: &Value, path: &str, out: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                if let Some(Value::Object(props)) = map.get("properties") {
                    for (name, schema) in props {
                        let l = name.to_lowercase();
                        let secretish = ["token", "password", "apikey", "privatekey", "bearer"]
                            .iter()
                            .any(|w| l.contains(w));
                        let is_string =
                            schema.get("type").and_then(Value::as_str) == Some("string");
                        if secretish && is_string {
                            out.push(format!("{path}.{name}"));
                        }
                    }
                }
                map.iter()
                    .for_each(|(k, c)| leaks(c, &format!("{path}.{k}"), out));
            }
            Value::Array(items) => items.iter().for_each(|c| leaks(c, path, out)),
            _ => {}
        }
    }
    let mut found = Vec::new();
    for crd in crds() {
        leaks(&schema_of(&crd), "", &mut found);
    }
    assert!(
        found.is_empty(),
        "string fields that look like secret values: {found:?}"
    );
}

fn property<'a>(schema: &'a Value, path: &[&str]) -> &'a Value {
    path.iter().fold(schema, |at, name| {
        at.get("properties")
            .and_then(|p| p.get(*name))
            .unwrap_or_else(|| panic!("no property {name} on the way to {path:?}"))
    })
}

#[test]
fn the_audience_is_at_most_32_values_of_1_to_64_characters() {
    // Bounds of the schema, not CEL rules: an API server prices a rule that walks a list by the
    // list's `maxItems` and the strings' `maxLength`, and refuses the CRD without them.
    let schema = schema_of(&AgentService::crd());
    let audience = property(&schema, &["spec", "access", "audience"]);
    assert_eq!(audience["type"], "array");
    assert_eq!(audience["maxItems"], 32);
    assert_eq!(audience["items"]["type"], "string");
    assert_eq!(audience["items"]["minLength"], 1);
    assert_eq!(audience["items"]["maxLength"], 64);
    // optional: absent and `[]` both mean administrators only, so it is not required, and the
    // CRD fills nothing in.
    let access = property(&schema, &["spec", "access"]);
    assert!(access.get("required").is_none(), "{access}");
    assert!(audience.get("default").is_none(), "{audience}");
}

#[test]
fn the_extra_mcp_servers_are_bounded_so_their_cel_rules_can_be_priced() {
    // The rules of one server run once per entry of this map: without `maxProperties` an API
    // server estimates the map at its largest and refuses the CRD (seen on kube-apiserver v1.35.8).
    let schema = schema_of(&AgentConfig::crd());
    let servers = property(&schema, &["spec", "tools", "mcpServers"]);
    assert_eq!(servers["maxProperties"], 32);
}

#[test]
fn the_references_and_the_image_are_optional_in_the_schema() {
    let schema = schema_of(&AgentConfig::crd());
    let required = |path: &[&str]| -> Vec<String> {
        property(&schema, path)["required"]
            .as_array()
            .map(|r| {
                r.iter()
                    .filter_map(|n| n.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    // model: only the alias is required; the address and key are the inline form's, or the endpoint's
    assert_eq!(required(&["spec", "model"]), ["model"]);
    // environment.image is optional (AD-029: the operator's default coder image)
    assert!(!required(&["spec", "environment"]).contains(&"image".to_owned()));
    // an MCP server: neither url nor providerRef is required by the schema; the CEL rules say one is
    let server = &property(&schema, &["spec", "tools", "mcpServers"])["additionalProperties"];
    assert!(server.get("required").is_none(), "{server}");
    // a reference names its object
    let provider_ref = &server["properties"]["providerRef"];
    for reference in [
        property(&schema, &["spec", "model", "endpointRef"]),
        provider_ref,
    ] {
        assert_eq!(reference["required"], serde_json::json!(["name"]));
        assert_eq!(reference["properties"]["name"]["minLength"], 1);
    }
}

/// A schema without the words around it: what a field accepts, not how it is described or
/// whether `null` is.
fn shape(v: &Value) -> Value {
    match v {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| !["description", "nullable"].contains(&k.as_str()))
                .map(|(k, c)| (k.clone(), shape(c)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(shape).collect()),
        other => other.clone(),
    }
}

#[test]
fn model_endpoint_and_tool_provider_have_the_shape_of_the_inline_forms() {
    // The same shapes as the inline forms, so slice D2 resolves a reference into the entry the
    // inline form gives.
    let config = schema_of(&AgentConfig::crd());

    let endpoint = schema_of(&ModelEndpoint::crd());
    let spec = &endpoint["properties"]["spec"];
    let mut required: Vec<_> = spec["required"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    required.sort_unstable();
    assert_eq!(required, ["apiKeySecretRef", "baseUrl", "title"]);
    let model = property(&config, &["spec", "model"]);
    for field in ["baseUrl", "apiKeySecretRef"] {
        assert_eq!(
            shape(&spec["properties"][field]),
            shape(&model["properties"][field]),
            "ModelEndpoint.spec.{field}"
        );
    }

    let provider = schema_of(&ToolProvider::crd());
    let mcp = &provider["properties"]["spec"]["properties"]["mcp"];
    assert_eq!(mcp["required"], serde_json::json!(["url"]));
    let servers = property(&config, &["spec", "tools", "mcpServers"]);
    let inline = &servers["additionalProperties"]["properties"];
    // The provider's map of headers has a bound the inline one does not (an inline server keeps
    // the schema it had, so an object stored before D1 stays as valid as it was).
    let mut headers = mcp["properties"]["headers"].clone();
    assert_eq!(headers["maxProperties"], 32);
    headers.as_object_mut().unwrap().remove("maxProperties");
    assert_eq!(shape(&headers), shape(&inline["headers"]), "headers");
    assert_eq!(
        shape(&mcp["properties"]["optional"]),
        shape(&inline["optional"]),
        "optional"
    );
}

#[test]
fn the_new_kinds_are_bounded_and_the_inline_forms_are_not_tightened() {
    // Sizes the dashboard's forms and the API server's cost estimate can rely on (the values are
    // in the README of this crate). Only the new kinds carry them.
    let endpoint = schema_of(&ModelEndpoint::crd());
    let spec = property(&endpoint, &["spec"]);
    assert_eq!(spec["properties"]["title"]["minLength"], 1);
    assert_eq!(spec["properties"]["title"]["maxLength"], 128);
    assert_eq!(spec["properties"]["models"]["maxItems"], 32);

    let provider = schema_of(&ToolProvider::crd());
    let spec = property(&provider, &["spec"]);
    assert_eq!(spec["properties"]["title"]["minLength"], 1);
    assert_eq!(spec["properties"]["title"]["maxLength"], 128);
    let mcp = &spec["properties"]["mcp"]["properties"];
    assert_eq!(mcp["url"]["minLength"], 1);
    assert_eq!(mcp["url"]["maxLength"], 2048);
    assert_eq!(mcp["headers"]["maxProperties"], 32);
    assert_eq!(mcp["tools"]["maxItems"], 32);

    // An object written before D1 is as valid as it was: no inline field got a bound.
    let config = schema_of(&AgentConfig::crd());
    let server = &property(&config, &["spec", "tools", "mcpServers"])["additionalProperties"];
    for bound in ["maxLength", "maxProperties", "maxItems"] {
        assert!(
            server["properties"]["url"].get(bound).is_none(),
            "url {bound}"
        );
        assert!(
            server["properties"]["headers"].get(bound).is_none(),
            "headers {bound}"
        );
        let base_url = property(&config, &["spec", "model", "baseUrl", "value"]);
        assert!(base_url.get(bound).is_none(), "baseUrl.value {bound}");
    }
}
