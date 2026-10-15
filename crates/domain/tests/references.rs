//! The references of the admin dashboard, resolved (§60a, AD-029, slice D2): a `ModelEndpoint` and the
//! `ToolProvider`s an `AgentConfig` names, and the operator's default image.
//!
//! The property that carries all of it is *equivalence*: an agent that references an object gets exactly
//! what an agent with the same content written inline gets, the digest included, so the pod does not know
//! where a model address or an MCP server was written. The rest is what is checked of a referent, as the
//! inline form is checked, and what moves the digest.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use aap_api::{ModelEndpoint, ToolProvider};
use aap_domain::contract::env;
use aap_domain::{ConfigIssue, Externals, ResolvedAgent, references};
use aap_ports::{EnvValue, SecretRef};
use common::{
    example, issues_with, must_resolve, must_resolve_with, remove, repo_root, set, shared_objects,
    typed,
};
use serde_json::{Value, json};

const NS: &str = "another-agentic-system";
const DEFAULT_IMAGE: &str = "ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e@sha256:c3c178b7d73599f40d53f798616e510346abd3300a622b09b2bc233109f12b44";

fn endpoint(name: &str, spec: Value) -> ModelEndpoint {
    serde_json::from_value(json!({
        "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "ModelEndpoint",
        "metadata": {"name": name, "namespace": NS}, "spec": spec,
    }))
    .unwrap()
}

fn provider(name: &str, mcp: Value) -> ToolProvider {
    serde_json::from_value(json!({
        "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "ToolProvider",
        "metadata": {"name": name, "namespace": NS},
        "spec": {"title": name, "mcp": mcp},
    }))
    .unwrap()
}

/// The endpoint that holds what the example's inline `model` holds.
fn endpoint_like(config: &Value) -> ModelEndpoint {
    endpoint(
        "gateway",
        json!({
            "title": "Gateway",
            "baseUrl": config["spec"]["model"]["baseUrl"],
            "apiKeySecretRef": config["spec"]["model"]["apiKeySecretRef"],
        }),
    )
}

fn use_endpoint(config: &mut Value, name: &str) {
    remove(config, "/spec/model/baseUrl");
    remove(config, "/spec/model/apiKeySecretRef");
    set(config, "/spec/model/endpointRef", json!({ "name": name }));
}

/// The provider that holds what the example's inline server `id` holds.
fn provider_like(config: &Value, id: &str) -> ToolProvider {
    let server = &config["spec"]["tools"]["mcpServers"][id];
    provider(
        id,
        json!({
            "url": server["url"],
            "headers": server.get("headers").cloned().unwrap_or_else(|| json!({})),
            "tools": [],
            "optional": server.get("optional").cloned().unwrap_or(json!(false)),
        }),
    )
}

/// One server of `config` as a reference to the provider of the same name.
fn use_provider(config: &mut Value, id: &str) {
    set(
        config,
        &format!("/spec/tools/mcpServers/{id}"),
        json!({ "providerRef": { "name": id } }),
    );
}

fn no_change(_: &mut Value, _: &mut Value) {}

/// The extra MCP file of a resolved agent, parsed.
fn mcp_file(r: &ResolvedAgent) -> Value {
    let set = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name.ends_with("-mcp"))
        .expect("the MCP file set");
    serde_json::from_str(&set.files["mcp.json"]).unwrap()
}

fn env_value(r: &ResolvedAgent, name: &str) -> Option<EnvValue> {
    r.runtime.workloads[0]
        .container
        .env
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.value.clone())
}

/// The sentence of an issue after its field: what a referent and an inline form must say alike.
fn said(issues: &[ConfigIssue], field_part: &str) -> String {
    issues
        .iter()
        .find(|i| i.field.contains(field_part))
        .unwrap_or_else(|| panic!("no issue about {field_part:?} in {issues:#?}"))
        .message
        .clone()
}

// ------------------------------------------------------------------ equivalence

#[test]
fn an_endpoint_reference_gives_what_the_inline_model_gives() {
    // `chat` has its address as a value, `coder` as a Secret key: both forms of `baseUrl`.
    for name in ["chat", "coder"] {
        let inline = must_resolve(name, no_change);
        let (_, config) = example(name);
        let externals = Externals::default().with_model_endpoint(endpoint_like(&config));
        let by_reference = must_resolve_with(name, &externals, |_, c| use_endpoint(c, "gateway"));
        assert_eq!(
            by_reference, inline,
            "{name}: the whole resolved agent is the inline one's"
        );
        assert_eq!(by_reference.digest, inline.digest, "{name}");
        assert!(env_value(&by_reference, env::MODEL_BASE_URL).is_some());
        assert!(env_value(&by_reference, env::MODEL_API_KEY).is_some());
    }
}

#[test]
fn a_provider_reference_gives_what_the_inline_server_gives() {
    // `coder` has two servers, with headers and a prefix, one optional.
    let inline = must_resolve("coder", no_change);
    let (_, config) = example("coder");
    let externals = Externals::default()
        .with_tool_provider(provider_like(&config, "websearch"))
        .with_tool_provider(provider_like(&config, "context7"));
    let by_reference = must_resolve_with("coder", &externals, |_, c| {
        use_provider(c, "websearch");
        use_provider(c, "context7");
    });
    assert_eq!(by_reference, inline);
    assert_eq!(mcp_file(&by_reference), mcp_file(&inline));
    // The variables that carry the headers' Secrets are the same, named by the Secret keys.
    assert_eq!(
        env_value(&by_reference, "SEARCH_MCP_TOKEN"),
        Some(EnvValue::Secret(SecretRef::new(
            "coder-secrets",
            "SEARCH_MCP_TOKEN"
        )))
    );

    // One of the two by reference, the other inline: the same again.
    let one = must_resolve_with(
        "coder",
        &Externals::default().with_tool_provider(provider_like(&config, "websearch")),
        |_, c| use_provider(c, "websearch"),
    );
    assert_eq!(one, inline);
}

#[test]
fn a_whole_config_by_reference_equals_the_inline_one() {
    // The model and the servers together, on the example that has both and the default image taken
    // from the operator: the config of `coder-me` against the same content written inline.
    let externals = shared_objects("coder-me").with_default_image(DEFAULT_IMAGE);
    let by_reference = must_resolve_with("coder-me", &externals, no_change);

    let inline = must_resolve_with("coder-me", &Externals::default(), |_, c| {
        set(
            c,
            "/spec/model",
            json!({
                "model": "coding-model",
                "baseUrl": {"secretRef": {"name": "agent-models", "key": "GATEWAY_BASE_URL"}},
                "apiKeySecretRef": {"name": "agent-models", "key": "GATEWAY_API_KEY"},
            }),
        );
        set(
            c,
            "/spec/tools/mcpServers",
            json!({"websearch": {
                "url": "http://another-agentic-websearch.another-agentic-system.svc:8080/mcp",
                "headers": {"Authorization": {"prefix": "Bearer ", "secretRef": {"name": "agent-tools", "key": "SEARCH_MCP_TOKEN"}}},
                "optional": true,
            }}),
        );
        set(
            c,
            "/spec/environment/image",
            json!({ "ref": DEFAULT_IMAGE }),
        );
    });
    assert_eq!(by_reference, inline);
}

// ------------------------------------------------------------------ optional

#[test]
fn a_server_is_optional_when_the_agent_or_the_provider_says_so() {
    for (agent, provider_says) in [(false, false), (false, true), (true, false), (true, true)] {
        let wants = agent || provider_says;
        let externals = Externals::default().with_tool_provider(provider(
            "search",
            json!({"url": "https://search.example.invalid/mcp", "optional": provider_says}),
        ));
        let r = must_resolve_with("chat", &externals, |_, c| {
            set(
                c,
                "/spec/tools/mcpServers",
                json!({"search": {"providerRef": {"name": "search"}, "optional": agent}}),
            );
        });
        let file = mcp_file(&r);
        assert_eq!(
            file["mcpServers"]["search"].get("optional"),
            wants.then_some(&json!(true)),
            "agent {agent}, provider {provider_says}: {file}"
        );
        // and it is what the inline server with that `optional` is, digest included
        let inline = must_resolve("chat", |_, c| {
            set(
                c,
                "/spec/tools/mcpServers",
                json!({"search": {"url": "https://search.example.invalid/mcp", "optional": wants}}),
            );
        });
        assert_eq!(r, inline, "agent {agent}, provider {provider_says}");
    }
}

// ------------------------------------------------------------------ tools

fn search_with_tools(tools: Value) -> Externals {
    Externals::default().with_tool_provider(provider(
        "search",
        json!({"url": "https://search.example.invalid/mcp", "tools": tools}),
    ))
}

fn chat_with_search(externals: &Externals) -> ResolvedAgent {
    must_resolve_with("chat", externals, |_, c| {
        set(
            c,
            "/spec/tools/mcpServers",
            json!({"search": {"providerRef": {"name": "search"}}}),
        );
    })
}

#[test]
fn a_providers_allow_list_is_the_servers_tools_in_the_file() {
    let r = chat_with_search(&search_with_tools(json!(["web_search", "fetch"])));
    assert_eq!(
        mcp_file(&r)["mcpServers"]["search"]["tools"],
        json!(["fetch", "web_search"]),
        "adam's `tools` of the server, a set, written sorted"
    );

    // Said twice is said once; the order a person typed it in does not roll the pods.
    let twice = chat_with_search(&search_with_tools(json!([
        "web_search",
        "fetch",
        "web_search"
    ])));
    assert_eq!(mcp_file(&twice), mcp_file(&r));
    assert_eq!(twice.digest, r.digest);
    let reordered = chat_with_search(&search_with_tools(json!(["fetch", "web_search"])));
    assert_eq!(reordered.digest, r.digest);

    // A different allow-list is a different file, so a rollout.
    let more = chat_with_search(&search_with_tools(json!(["fetch", "web_search", "extra"])));
    assert_ne!(more.digest, r.digest);
    let fewer = chat_with_search(&search_with_tools(json!(["fetch"])));
    assert_ne!(fewer.digest, r.digest);
}

#[test]
fn an_empty_allow_list_means_every_tool_and_writes_no_key() {
    let all = chat_with_search(&search_with_tools(json!([])));
    assert!(
        mcp_file(&all)["mcpServers"]["search"]
            .get("tools")
            .is_none(),
        "no `tools` key: adam allows every tool the server offers"
    );
    // and so it is the inline server
    let inline = must_resolve("chat", |_, c| {
        set(
            c,
            "/spec/tools/mcpServers",
            json!({"search": {"url": "https://search.example.invalid/mcp"}}),
        );
    });
    assert_eq!(all, inline);
    // but an allow-list is never ignored: with one the digest is not the inline one's
    assert_ne!(
        chat_with_search(&search_with_tools(json!(["web_search"]))).digest,
        inline.digest
    );
}

#[test]
fn an_allow_list_adam_cannot_name_is_refused_not_ignored() {
    // adam-rs 0391809, crates/adam-agent-fs/src/load/mcp.rs: the entry must become `<server>__<tool>`.
    let too_long = "t".repeat(60); // `search__` + 60 > 64
    for bad in [
        "",
        "_hidden",
        "web.search",
        "has space",
        "écho",
        too_long.as_str(),
    ] {
        let found = issues_with("chat", &search_with_tools(json!(["ok", bad])), |_, c| {
            set(
                c,
                "/spec/tools/mcpServers",
                json!({"search": {"providerRef": {"name": "search"}}}),
            );
        });
        assert_eq!(found.len(), 1, "{bad:?}: {found:#?}");
        // The field is the provider's, at the entry; the sentence says what adam takes.
        assert_eq!(found[0].field, "ToolProvider search spec.mcp.tools[1]");
        assert!(
            found[0].message.contains("search__") && found[0].message.contains("64 characters"),
            "{}",
            found[0]
        );
    }
}

// ------------------------------------------------------------------ the digest

#[test]
fn the_digest_follows_the_content_of_a_referent() {
    let endpoint_spec = |base: Value, key_name: &str, title: &str, models: Value| {
        endpoint(
            "gateway",
            json!({
                "title": title, "baseUrl": base,
                "apiKeySecretRef": {"name": key_name, "key": "GATEWAY_API_KEY"},
                "models": models,
            }),
        )
    };
    let digest_of = |e: ModelEndpoint| {
        must_resolve_with(
            "chat",
            &Externals::default().with_model_endpoint(e),
            |_, c| use_endpoint(c, "gateway"),
        )
        .digest
    };
    let value = |url: &str| json!({ "value": url });
    let base = digest_of(endpoint_spec(
        value("https://gateway.example.invalid/v1"),
        "agent-models",
        "Gateway",
        json!([]),
    ));
    // What a pod reads moves it: an edit of an endpoint rolls every agent that uses it.
    assert_ne!(
        base,
        digest_of(endpoint_spec(
            value("https://other.example.invalid/v1"),
            "agent-models",
            "Gateway",
            json!([])
        )),
        "the address"
    );
    assert_ne!(
        base,
        digest_of(endpoint_spec(
            value("https://gateway.example.invalid/v1"),
            "other-secret",
            "Gateway",
            json!([])
        )),
        "the Secret of the key"
    );
    assert_ne!(
        base,
        digest_of(endpoint_spec(
            json!({"secretRef": {"name": "agent-models", "key": "URL"}}),
            "agent-models",
            "Gateway",
            json!([])
        )),
        "a Secret key instead of a value"
    );
    // What only a person reads does not.
    assert_eq!(
        base,
        digest_of(endpoint_spec(
            value("https://gateway.example.invalid/v1"),
            "agent-models",
            "A new title",
            json!(["coding-model", "chat-model"])
        )),
        "the title and the aliases are informative and reach no pod"
    );

    let digest_of = |p: ToolProvider, title: Option<&str>| {
        let mut p = p;
        if let Some(t) = title {
            t.clone_into(&mut p.spec.title);
        }
        must_resolve_with(
            "chat",
            &Externals::default().with_tool_provider(p),
            |_, c| {
                set(
                    c,
                    "/spec/tools/mcpServers",
                    json!({"search": {"providerRef": {"name": "search"}}}),
                );
            },
        )
        .digest
    };
    let mcp = |url: &str, header_key: &str, optional: bool| {
        provider(
            "search",
            json!({
                "url": url,
                "headers": {"Authorization": {"prefix": "Bearer ", "secretRef": {"name": "s", "key": header_key}}},
                "optional": optional,
            }),
        )
    };
    let base = digest_of(
        mcp("https://search.example.invalid/mcp", "TOKEN", false),
        None,
    );
    for (what, other) in [
        (
            "the URL",
            mcp("https://elsewhere.example.invalid/mcp", "TOKEN", false),
        ),
        (
            "the header's Secret key",
            mcp("https://search.example.invalid/mcp", "OTHER", false),
        ),
        (
            "optional",
            mcp("https://search.example.invalid/mcp", "TOKEN", true),
        ),
    ] {
        assert_ne!(base, digest_of(other, None), "{what}");
    }
    assert_eq!(
        base,
        digest_of(
            mcp("https://search.example.invalid/mcp", "TOKEN", false),
            Some("A new title")
        ),
        "the title reaches no pod"
    );
}

// ------------------------------------------------------------------ missing

#[test]
fn a_missing_referent_is_an_issue_that_names_it() {
    // The §60a example with none of its objects: both references are missing, and the image is taken
    // from the operator's default, so nothing else is wrong.
    let found = issues_with(
        "coder-me",
        &Externals::default().with_default_image(DEFAULT_IMAGE),
        no_change,
    );
    assert_eq!(found.len(), 2, "{found:#?}");
    let model = said(&found, "spec.model.endpointRef");
    assert!(
        model.contains("ModelEndpoint \"gateway\"")
            && model.contains(NS)
            && model.contains("not found"),
        "{model}"
    );
    let tools = said(&found, "mcpServers[websearch].providerRef");
    assert!(
        tools.contains("ToolProvider \"websearch\"")
            && tools.contains(NS)
            && tools.contains("not found"),
        "{tools}"
    );
    // The field is the config's: the reference that dangles.
    assert!(
        found
            .iter()
            .all(|i| i.field.starts_with("AgentConfig spec."))
    );

    // One of them there: only the other is reported.
    let endpoint_only = Externals::default()
        .with_default_image(DEFAULT_IMAGE)
        .with_model_endpoint(shared_objects("coder-me").model_endpoints["gateway"].clone());
    let found = issues_with("coder-me", &endpoint_only, no_change);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("ToolProvider \"websearch\""));

    // Not the other kind, and not another name.
    let wrong_name = Externals::default().with_default_image(DEFAULT_IMAGE).with_model_endpoint(
        endpoint(
            "another",
            json!({"title": "x", "baseUrl": {"value": "https://x.example.invalid/v1"}, "apiKeySecretRef": {"name": "s", "key": "K"}}),
        ),
    );
    let found = issues_with("coder-me", &wrong_name, no_change);
    assert!(
        found
            .iter()
            .any(|i| i.message.contains("ModelEndpoint \"gateway\"")),
        "{found:#?}"
    );
}

#[test]
fn a_referent_of_a_kind_the_cluster_does_not_serve_says_to_install_the_crds() {
    // The same example, with the composition root having found that neither kind is served: the issue
    // does not say "create it", which nobody can do, but what to install.
    let externals = Externals::default()
        .with_default_image(DEFAULT_IMAGE)
        .with_kind_not_installed("ModelEndpoint")
        .with_kind_not_installed("ToolProvider");
    let found = issues_with("coder-me", &externals, no_change);
    assert_eq!(found.len(), 2, "{found:#?}");
    for (field, who) in [
        ("spec.model.endpointRef", "ModelEndpoint \"gateway\""),
        (
            "mcpServers[websearch].providerRef",
            "ToolProvider \"websearch\"",
        ),
    ] {
        let said = said(&found, field);
        assert!(
            said.contains(who)
                && said.contains("is not installed")
                && said.contains("deploy/operator-crds")
                && !said.contains("create it"),
            "{said}"
        );
        // And the way out that needs no CRD stays in the sentence.
        assert!(said.contains("inline"), "{said}");
    }

    // One kind served, the other not: only the missing kind says so, and the other is an ordinary
    // missing object.
    let found = issues_with(
        "coder-me",
        &Externals::default()
            .with_default_image(DEFAULT_IMAGE)
            .with_kind_not_installed("ToolProvider"),
        no_change,
    );
    assert!(said(&found, "spec.model.endpointRef").contains("was not found"));
    assert!(said(&found, "mcpServers[websearch].providerRef").contains("is not installed"));

    // A kind that is not installed whose objects are not asked for changes nothing: an inline config.
    let (service, config) = typed_pair("chat");
    let inline = Externals::default().with_kind_not_installed("ModelEndpoint");
    assert!(aap_domain::validate(&service, &config, &inline).is_ok());
}

fn typed_pair(name: &str) -> (aap_api::AgentService, aap_api::AgentConfig) {
    let (service, config) = example(name);
    typed(&service, &config)
}

#[test]
fn a_referent_of_another_namespace_is_not_read() {
    let mut externals = shared_objects("coder-me").with_default_image(DEFAULT_IMAGE);
    externals
        .model_endpoints
        .get_mut("gateway")
        .unwrap()
        .metadata
        .namespace = Some("elsewhere".to_owned());
    let found = issues_with("coder-me", &externals, no_change);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0].message.contains("elsewhere") && found[0].message.contains("same namespace"),
        "{}",
        found[0]
    );
}

// ------------------------------------------------------------------ names

/// What a person can type in `name`: none of these is the name of an object, and each goes into the path of
/// a request to the API server (`.../modelendpoints/<name>`): a space breaks the request for good, `?` adds
/// a query to it, `/` and `..` leave the path, `%` is decoded by the server.
const NOT_NAMES: [&str; 10] = [
    "bad name",
    "x?watch=true",
    "x#y",
    "a/b",
    "../x",
    "UPPER",
    "a_b",
    "x%20y",
    "-a",
    "a..b",
];

#[test]
fn a_reference_whose_name_is_not_an_objects_is_refused_and_never_asked_for() {
    let too_long = "x".repeat(254);
    for name in NOT_NAMES.iter().copied().chain([too_long.as_str()]) {
        // The model's endpoint.
        let (s, mut c) = example("chat");
        use_endpoint(&mut c, name);
        let (_, config) = typed(&s, &c);
        assert!(
            references(&config).is_empty(),
            "{name:?} would be requested: {:?}",
            references(&config)
        );
        let found = issues_with("chat", &Externals::default(), |_, c| {
            use_endpoint(c, name);
        });
        let sentence = said(&found, "spec.model.endpointRef.name");
        assert!(sentence.contains("not the name of an object"), "{sentence}");
        // One issue about it: the name is not also reported as missing.
        assert_eq!(found.len(), 1, "{name:?}: {found:#?}");
        assert!(
            aap_domain::validate_config(&config)
                .unwrap_err()
                .iter()
                .any(|i| i.field == "AgentConfig spec.model.endpointRef.name"),
            "the config's own Valid condition says so too"
        );

        // A server's provider.
        let (s, mut c) = example("chat");
        set(
            &mut c,
            "/spec/tools/mcpServers",
            json!({"search": {"providerRef": {"name": name}}, "fine": {"providerRef": {"name": "fine"}}}),
        );
        let (_, config) = typed(&s, &c);
        assert_eq!(
            references(&config)
                .tool_providers
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["fine"],
            "{name:?}: only the name that is one is asked for"
        );
        let found = issues_with(
            "chat",
            &Externals::default().with_tool_provider(provider(
                "fine",
                json!({"url": "https://search.example.invalid/mcp"}),
            )),
            |_, c| {
                set(
                    c,
                    "/spec/tools/mcpServers",
                    json!({"search": {"providerRef": {"name": name}}, "fine": {"providerRef": {"name": "fine"}}}),
                );
            },
        );
        let sentence = said(&found, "mcpServers[search].providerRef.name");
        assert!(sentence.contains("not the name of an object"), "{sentence}");
        assert_eq!(found.len(), 1, "{name:?}: {found:#?}");
    }
}

#[test]
fn the_name_in_a_name_issue_is_cut_so_that_a_huge_one_cannot_fill_a_condition() {
    let huge = "A".repeat(20_000);
    let found = issues_with("chat", &Externals::default(), |_, c| {
        use_endpoint(c, &huge);
    });
    let said = said(&found, "spec.model.endpointRef.name");
    assert!(said.len() < 400, "{} characters", said.len());
    assert!(said.contains('…'), "{said}");
}

#[test]
fn a_name_with_dots_is_a_name() {
    // A DNS-1123 subdomain: dots are allowed, and the object is found under the name as typed.
    let mut externals = Externals::default().with_default_image(DEFAULT_IMAGE);
    let (_, chat) = example("chat");
    let mut gateway = endpoint_like(&chat);
    gateway.metadata.name = Some("models.gateway-v1".to_owned());
    externals = externals.with_model_endpoint(gateway);
    let found = issues_with("coder-me", &externals, |_, c| {
        set(
            c,
            "/spec/model/endpointRef",
            json!({"name": "models.gateway-v1"}),
        );
    });
    // Only the provider is still missing.
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(found[0].message.contains("ToolProvider"), "{}", found[0]);
}

// ------------------------------------------------------------------ checked as inline

/// Resolve `chat` with an endpoint of this content, which must be refused: the issues.
fn bad_endpoint(spec: Value) -> Vec<ConfigIssue> {
    issues_with(
        "chat",
        &Externals::default().with_model_endpoint(endpoint("gateway", spec)),
        |_, c| use_endpoint(c, "gateway"),
    )
}

/// The same content written inline in `chat`'s model: the issues.
fn bad_inline_model(base_url: Value, key: Value) -> Vec<ConfigIssue> {
    common::issues("chat", |_, c| {
        set(c, "/spec/model/baseUrl", base_url);
        set(c, "/spec/model/apiKeySecretRef", key);
    })
}

#[test]
fn an_endpoint_is_checked_as_the_inline_model_is() {
    let key = json!({"name": "agent-models", "key": "GATEWAY_API_KEY"});
    let spec = |base: &Value, key: &Value| json!({"title": "Gateway", "baseUrl": base, "apiKeySecretRef": key});
    // (what, the baseUrl, the key, the part of the field, a word of the sentence)
    let cases: Vec<(&str, Value, Value, &str, &str)> = vec![
        (
            "a user name and password in the address",
            json!({"value": "https://u:p@gateway.example.invalid/v1"}),
            key.clone(),
            "spec.baseUrl.value",
            "user name",
        ),
        (
            "an address that is not http",
            json!({"value": "gateway.example.invalid/v1"}),
            key.clone(),
            "spec.baseUrl.value",
            "not an http or https URL",
        ),
        (
            "white space in the address",
            json!({"value": "https://gateway.example.invalid/ v1"}),
            key.clone(),
            "spec.baseUrl.value",
            "whitespace",
        ),
        (
            "both a value and a Secret key",
            json!({"value": "https://g.example.invalid/v1", "secretRef": {"name": "s", "key": "K"}}),
            key.clone(),
            "spec.baseUrl",
            "exactly one of value and secretRef",
        ),
        (
            "neither",
            json!({}),
            key.clone(),
            "spec.baseUrl",
            "exactly one of value and secretRef",
        ),
        (
            "an address Secret with no key",
            json!({"secretRef": {"name": "s", "key": ""}}),
            key.clone(),
            "spec.baseUrl.secretRef",
            "needs a name and a key",
        ),
        (
            "an API key Secret with no name",
            json!({"value": "https://g.example.invalid/v1"}),
            json!({"name": "", "key": "K"}),
            "spec.apiKeySecretRef",
            "needs a name and a key",
        ),
    ];
    for (what, base, api_key, field, word) in cases {
        let found = bad_endpoint(spec(&base, &api_key));
        assert_eq!(found.len(), 1, "{what}: {found:#?}");
        // The issue is about the endpoint, which is what a person has to edit...
        assert_eq!(
            found[0].field,
            format!("ModelEndpoint gateway {field}"),
            "{what}"
        );
        assert!(found[0].message.contains(word), "{what}: {}", found[0]);
        // ...and says what the inline form says of the same content.
        let inline = bad_inline_model(base.clone(), api_key.clone());
        assert_eq!(
            found[0].message,
            said(
                &inline,
                field.trim_start_matches("spec.").split('.').next().unwrap()
            ),
            "{what}: the inline form's sentence"
        );
    }
}

fn bad_provider(mcp: Value, allow_insecure: bool) -> Vec<ConfigIssue> {
    issues_with(
        "chat",
        &Externals::default().with_tool_provider(provider("search", mcp)),
        |_, c| {
            set(
                c,
                "/spec/tools/mcpServers",
                json!({"search": {"providerRef": {"name": "search"}}}),
            );
            set(c, "/spec/tools/allowInsecureHttp", json!(allow_insecure));
        },
    )
}

fn bad_inline_server(server: Value, allow_insecure: bool) -> Vec<ConfigIssue> {
    common::issues("chat", |_, c| {
        set(c, "/spec/tools/mcpServers", json!({ "search": server }));
        set(c, "/spec/tools/allowInsecureHttp", json!(allow_insecure));
    })
}

#[test]
fn a_provider_is_checked_as_an_inline_server_is() {
    let secret = json!({"name": "s", "key": "TOKEN"});
    let header =
        |name: &str, prefix: &str| json!({ name: {"prefix": prefix, "secretRef": secret.clone()} });
    // (what, the mcp block, allowInsecureHttp, the part of the provider's field, the inline field, a word)
    let cases: Vec<(&str, Value, bool, &str, &str, &str)> = vec![
        (
            "a user name and password in the URL",
            json!({"url": "https://u:p@search.example.invalid/mcp"}),
            false,
            "spec.mcp.url",
            ".url",
            "user name",
        ),
        (
            "a ${VAR} in the URL",
            json!({"url": "https://search.example.invalid/${TENANT}/mcp"}),
            false,
            "spec.mcp.url",
            ".url",
            "${VAR}",
        ),
        (
            "a URL that is not http",
            json!({"url": "ftp://search.example.invalid/mcp"}),
            false,
            "spec.mcp.url",
            ".url",
            "not an http or https URL",
        ),
        (
            "an empty URL",
            json!({"url": ""}),
            false,
            "spec.mcp.url",
            ".url",
            "not an http or https URL",
        ),
        (
            "plain http to another machine without allowInsecureHttp",
            json!({"url": "http://search.ns.svc:8080/mcp"}),
            false,
            "spec.mcp.url",
            ".url",
            "allowInsecureHttp",
        ),
        (
            "a header name that is not an HTTP token",
            json!({"url": "https://search.example.invalid/mcp", "headers": header("Bad Header", "")}),
            false,
            "spec.mcp.headers[Bad Header]",
            ".headers[Bad Header]",
            "not an HTTP header name",
        ),
        (
            "a ${VAR} in a prefix",
            json!({"url": "https://search.example.invalid/mcp", "headers": header("Authorization", "Bearer ${X}")}),
            false,
            "spec.mcp.headers[Authorization].prefix",
            ".headers[Authorization].prefix",
            "plain text",
        ),
        (
            "a header Secret with no key",
            json!({"url": "https://search.example.invalid/mcp", "headers": {"Authorization": {"secretRef": {"name": "s", "key": ""}}}}),
            false,
            "spec.mcp.headers[Authorization].secretRef",
            ".headers[Authorization].secretRef",
            "needs a name and a key",
        ),
    ];
    for (what, mcp, insecure, field, inline_field, word) in cases {
        let found = bad_provider(mcp.clone(), insecure);
        let mine: Vec<_> = found
            .iter()
            .filter(|i| i.field.starts_with("ToolProvider search "))
            .collect();
        assert!(
            !mine.is_empty(),
            "{what}: no issue about the provider: {found:#?}"
        );
        let issue = mine
            .iter()
            .find(|i| i.field == format!("ToolProvider search {field}"))
            .unwrap_or_else(|| panic!("{what}: no issue at {field}: {found:#?}"));
        assert!(issue.message.contains(word), "{what}: {issue}");

        // The same content inline is refused with the same sentence, at the server's field.
        let mut server = mcp.clone();
        server.as_object_mut().unwrap().remove("tools");
        let inline = bad_inline_server(server, insecure);
        let inline_issue = inline
            .iter()
            .find(|i| i.field == format!("AgentConfig spec.tools.mcpServers[search]{inline_field}"))
            .unwrap_or_else(|| panic!("{what}: no inline issue at {inline_field}: {inline:#?}"));
        assert_eq!(
            issue.message, inline_issue.message,
            "{what}: the inline form's sentence"
        );
    }

    // And plain http is fine once the agent allows it, as it is inline.
    must_resolve_with(
        "chat",
        &Externals::default().with_tool_provider(provider(
            "search",
            json!({"url": "http://search.ns.svc:8080/mcp"}),
        )),
        |_, c| {
            set(
                c,
                "/spec/tools/mcpServers",
                json!({"search": {"providerRef": {"name": "search"}}}),
            );
            set(c, "/spec/tools/allowInsecureHttp", json!(true));
        },
    );
}

// ---------------------------------------------------------- the header variable

fn token_provider(name: &str, secret: &str, key: &str, header: &str) -> ToolProvider {
    provider(
        name,
        json!({
            "url": format!("https://{name}.example.invalid/mcp"),
            "headers": {header: {"prefix": "Bearer ", "secretRef": {"name": secret, "key": key}}},
        }),
    )
}

fn chat_with_providers(
    externals: &Externals,
    names: &[&str],
) -> Result<ResolvedAgent, Vec<ConfigIssue>> {
    common::resolved_with("chat", externals, |_, c| {
        let servers: serde_json::Map<String, Value> = names
            .iter()
            .map(|n| ((*n).to_owned(), json!({"providerRef": {"name": n}})))
            .collect();
        set(c, "/spec/tools/mcpServers", Value::Object(servers));
    })
}

#[test]
fn two_providers_that_want_one_variable_from_different_secrets_are_refused() {
    let externals = Externals::default()
        .with_tool_provider(token_provider(
            "alpha",
            "secret-a",
            "SEARCH_TOKEN",
            "Authorization",
        ))
        .with_tool_provider(token_provider(
            "beta",
            "secret-b",
            "SEARCH_TOKEN",
            "Authorization",
        ));
    let found = chat_with_providers(&externals, &["alpha", "beta"]).unwrap_err();
    assert_eq!(found.len(), 1, "{found:#?}");
    assert!(
        found[0]
            .field
            .contains("header Secret key \"SEARCH_TOKEN\"")
    );
    let message = &found[0].message;
    assert!(message.contains("different Secrets"), "{message}");
    // It names who wants it, so a person knows which two objects to change.
    assert!(
        message.contains("ToolProvider alpha") && message.contains("ToolProvider beta"),
        "{message}"
    );
    assert!(
        message.contains("secret-a") && message.contains("secret-b"),
        "{message}"
    );

    // Two headers of one provider, the same.
    let one = Externals::default().with_tool_provider(provider(
        "alpha",
        json!({
            "url": "https://alpha.example.invalid/mcp",
            "headers": {
                "Authorization": {"secretRef": {"name": "secret-a", "key": "TOKEN"}},
                "X-Api-Key": {"secretRef": {"name": "secret-b", "key": "TOKEN"}},
            },
        }),
    ));
    let found = chat_with_providers(&one, &["alpha"]).unwrap_err();
    assert!(found[0].message.contains("different Secrets"), "{found:#?}");
}

#[test]
fn the_same_secret_key_wanted_twice_is_one_variable() {
    // The same Secret and the same key: one variable carries one value, and both servers read it. This
    // is what the inline form does with two servers that share a Secret key.
    let externals = Externals::default()
        .with_tool_provider(token_provider(
            "alpha",
            "shared",
            "SEARCH_TOKEN",
            "Authorization",
        ))
        .with_tool_provider(token_provider(
            "beta",
            "shared",
            "SEARCH_TOKEN",
            "X-Api-Key",
        ));
    let r = chat_with_providers(&externals, &["alpha", "beta"]).unwrap();
    let vars: Vec<_> = r.runtime.workloads[0]
        .container
        .env
        .iter()
        .filter(|e| e.name == "SEARCH_TOKEN")
        .collect();
    assert_eq!(vars.len(), 1, "one variable, not two");
    assert_eq!(
        vars[0].value,
        EnvValue::Secret(SecretRef::new("shared", "SEARCH_TOKEN"))
    );
}

#[test]
fn a_providers_header_may_clash_with_an_inline_servers_too() {
    let externals = Externals::default().with_tool_provider(token_provider(
        "alpha",
        "secret-a",
        "SEARCH_TOKEN",
        "Authorization",
    ));
    let found = issues_with("chat", &externals, |_, c| {
        set(
            c,
            "/spec/tools/mcpServers",
            json!({
                "alpha": {"providerRef": {"name": "alpha"}},
                "docs": {
                    "url": "https://docs.example.invalid/mcp",
                    "headers": {"Authorization": {"secretRef": {"name": "secret-b", "key": "SEARCH_TOKEN"}}},
                },
            }),
        );
    });
    assert_eq!(found.len(), 1, "{found:#?}");
    let message = &found[0].message;
    assert!(
        message.contains("ToolProvider alpha") && message.contains("mcpServers[docs]"),
        "{message}"
    );
}

#[test]
fn a_providers_secret_key_must_be_a_usable_variable_name() {
    for (key, word) in [
        ("private-key.pem", "must be a variable name"),
        ("DATABASE_URL", "the operator sets"),
        ("MODEL_API_KEY", "the operator sets"),
    ] {
        let externals = Externals::default().with_tool_provider(token_provider(
            "alpha",
            "s",
            key,
            "Authorization",
        ));
        let found = chat_with_providers(&externals, &["alpha"]).unwrap_err();
        assert!(
            found.iter().any(|i| i.message.contains(word)),
            "{key}: {found:#?}"
        );
    }
}

// ------------------------------------------------------------------ the default image

const PINNED: &str = "ghcr.io/vymalo/another-adam-rs/coder:sha-0391809@sha256:814d8ee329a5e4d8634e8a82532c035999c4be27aac086508b742a635449ea40";

fn no_image(_: &mut Value, c: &mut Value) {
    remove(c, "/spec/environment/image");
}

#[test]
fn a_config_with_no_image_and_no_default_is_refused_naming_the_flag() {
    for name in ["coder", "chat"] {
        let found = issues_with(name, &Externals::default(), no_image);
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
        assert_eq!(found[0].field, "AgentConfig spec.environment.image");
        assert!(
            found[0].message.contains("--default-agent-image")
                && found[0].message.contains("no default"),
            "{}",
            found[0]
        );
        // A default of only white space is no default.
        let found = issues_with(
            name,
            &Externals::default().with_default_image("  "),
            no_image,
        );
        assert_eq!(found.len(), 1, "{name}: {found:#?}");
    }
}

#[test]
fn a_config_with_no_image_runs_the_operators_default_everywhere_the_image_goes() {
    let r = must_resolve_with(
        "coder",
        &Externals::default().with_default_image(DEFAULT_IMAGE),
        no_image,
    );
    let w = &r.runtime.workloads[0];
    assert_eq!(w.container.image, DEFAULT_IMAGE);
    assert_eq!(w.sidecars.len(), 1, "the coder has its GitHub MCP sidecar");
    assert_eq!(
        w.sidecars[0].image, DEFAULT_IMAGE,
        "the sidecar is the same image"
    );

    // The split topology: the front too.
    let split = must_resolve_with(
        "coder",
        &Externals::default().with_default_image(DEFAULT_IMAGE),
        |s, c| {
            no_image(s, c);
            set(s, "/spec/scaling/topology", json!("split"));
            set(s, "/spec/scaling/front", json!({"replicas": 1}));
            set(
                c,
                "/spec/harness/adam/coder/workspacePlacement",
                json!("isolated"),
            );
            set(
                c,
                "/spec/environment/volumes/0/source/persistent/perReplica",
                json!(true),
            );
            set(s, "/spec/scaling/workers", json!(2));
        },
    );
    assert!(
        split
            .runtime
            .workloads
            .iter()
            .all(|w| w.container.image == DEFAULT_IMAGE),
        "every workload: {:#?}",
        split
            .runtime
            .workloads
            .iter()
            .map(|w| &w.container.image)
            .collect::<Vec<_>>()
    );
}

#[test]
fn the_default_image_moves_the_digest_of_agents_that_name_none_and_of_no_other() {
    let with_default = |name: &str, default: &str, change: fn(&mut Value, &mut Value)| {
        must_resolve_with(
            name,
            &Externals::default().with_default_image(default),
            change,
        )
        .digest
    };
    let other = "ghcr.io/vymalo/another-adam-rs/coder:sha-aaaaaaa@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    for name in ["coder", "chat"] {
        // An agent that names none: a bump of the default rolls it.
        let before = with_default(name, DEFAULT_IMAGE, no_image);
        let after = with_default(name, other, no_image);
        assert_ne!(before, after, "{name}: a new default rolls the agent");
        assert_eq!(
            before,
            with_default(name, DEFAULT_IMAGE, no_image),
            "{name}: the same default, the same digest"
        );
        // An agent that pins its own: the default is none of its business.
        let pinned_before = with_default(name, DEFAULT_IMAGE, |_, _| {});
        let pinned_after = with_default(name, other, |_, _| {});
        let no_default = must_resolve(name, no_change).digest;
        assert_eq!(
            pinned_before, pinned_after,
            "{name}: a pinned agent does not move"
        );
        assert_eq!(
            pinned_before, no_default,
            "{name}: nor does having a default at all"
        );
    }

    // The default is an image like any other: an agent that pins the image the default names is the
    // agent that names none.
    let pinned_to_default = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/environment/image",
            json!({ "ref": DEFAULT_IMAGE }),
        );
    });
    let defaulted = must_resolve_with(
        "coder",
        &Externals::default().with_default_image(DEFAULT_IMAGE),
        no_image,
    );
    assert_eq!(pinned_to_default, defaulted);

    // And an agent's own image beats the default.
    let r = must_resolve_with(
        "coder",
        &Externals::default().with_default_image(DEFAULT_IMAGE),
        |_, c| set(c, "/spec/environment/image", json!({ "ref": PINNED })),
    );
    assert_eq!(r.runtime.workloads[0].container.image, PINNED);
}

#[test]
fn a_default_image_with_white_space_is_refused_only_when_it_is_used() {
    let bad = Externals::default().with_default_image("ghcr.io/x/y:tag with space");
    let found = issues_with("chat", &bad, no_image);
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].field, "operator --default-agent-image");
    // A config that pins its own image never reads it.
    must_resolve_with("chat", &bad, |_, _| {});
}

// ------------------------------------------------------------------ references()

#[test]
fn references_are_the_names_a_config_asks_for() {
    let (s, c) = example("coder-me");
    let (_, config) = typed(&s, &c);
    let found = references(&config);
    assert_eq!(found.model_endpoint.as_deref(), Some("gateway"));
    assert_eq!(
        found
            .tool_providers
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["websearch"]
    );
    assert!(!found.is_empty());

    // An inline config references nothing.
    for name in ["coder", "chat"] {
        let (s, c) = example(name);
        let (_, config) = typed(&s, &c);
        assert!(references(&config).is_empty(), "{name}");
    }

    // A provider named by two servers is one name; an empty name is not a reference.
    let (s, mut c) = example("chat");
    set(
        &mut c,
        "/spec/tools/mcpServers",
        json!({
            "a": {"providerRef": {"name": "shared"}},
            "b": {"providerRef": {"name": "shared"}},
            "c": {"providerRef": {"name": "other"}},
            "d": {"providerRef": {"name": ""}},
        }),
    );
    let (_, config) = typed(&s, &c);
    assert_eq!(
        references(&config)
            .tool_providers
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["other", "shared"]
    );
}

// ------------------------------------------------------------------ the dashboard example

#[test]
fn the_dashboard_example_resolves_with_its_objects_and_the_operators_default() {
    let externals = shared_objects("coder-me").with_default_image(DEFAULT_IMAGE);
    let r = must_resolve_with("coder-me", &externals, no_change);
    let w = &r.runtime.workloads[0];
    assert_eq!(w.container.image, DEFAULT_IMAGE, "no image: the default");
    assert_eq!(
        env_value(&r, env::MODEL_BASE_URL),
        Some(EnvValue::Secret(SecretRef::new(
            "agent-models",
            "GATEWAY_BASE_URL"
        )))
    );
    assert_eq!(
        env_value(&r, env::MODEL_API_KEY),
        Some(EnvValue::Secret(SecretRef::new(
            "agent-models",
            "GATEWAY_API_KEY"
        )))
    );
    let file = mcp_file(&r);
    assert_eq!(
        file["mcpServers"]["websearch"]["url"],
        "http://another-agentic-websearch.another-agentic-system.svc:8080/mcp"
    );
    assert_eq!(file["mcpServers"]["websearch"]["optional"], true);
    assert_eq!(
        file["mcpServers"]["websearch"]["headers"]["Authorization"],
        "Bearer ${SEARCH_MCP_TOKEN}"
    );
    // The audience is on the service and reaches no pod.
    assert!(repo_root().join("examples/coder-me.yaml").exists());
}
