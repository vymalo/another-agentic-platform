//! `validate_endpoint` and `validate_provider` (K2): a shared object judged on its own, each issue naming its field, and the
//! examples accepted.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use aap_api::{ModelEndpoint, ToolProvider};
use aap_domain::{validate_endpoint, validate_provider};
use common::{assert_issue, documents, repo_root};
use serde_json::{Value, json};

fn objects() -> (Value, Value) {
    let docs = documents(&repo_root().join("examples/coder-me.yaml"));
    let of = |kind: &str| docs.iter().find(|d| d["kind"] == kind).cloned().unwrap();
    (of("ModelEndpoint"), of("ToolProvider"))
}

fn endpoint(change: impl FnOnce(&mut Value)) -> ModelEndpoint {
    let mut v = objects().0;
    change(&mut v);
    serde_json::from_value(v).unwrap()
}

fn provider(change: impl FnOnce(&mut Value)) -> ToolProvider {
    let mut v = objects().1;
    change(&mut v);
    serde_json::from_value(v).unwrap()
}

#[test]
fn the_examples_are_accepted() {
    validate_endpoint(&endpoint(|_| {})).unwrap();
    validate_provider(&provider(|_| {})).unwrap();
}

#[test]
fn an_endpoint_is_judged_by_the_rules_of_an_inline_model() {
    let bad_url = endpoint(|e| {
        e["spec"]["baseUrl"] = json!({ "value": "ftp://models.example/v1" });
    });
    let issues = validate_endpoint(&bad_url).unwrap_err();
    assert_issue(&issues, "ModelEndpoint gateway spec.baseUrl.value", "ftp");

    let credentials = endpoint(|e| {
        e["spec"]["baseUrl"] = json!({ "value": "https://user:hunter2@models.example/v1" });
    });
    let issues = validate_endpoint(&credentials).unwrap_err();
    assert_issue(&issues, "ModelEndpoint gateway spec.baseUrl.value", "");
    assert!(
        issues.iter().all(|i| !i.to_string().contains("hunter2")),
        "{issues:?}"
    );

    let key = endpoint(|e| {
        e["spec"]["apiKeySecretRef"] = json!({ "name": "", "key": "K" });
    });
    assert_issue(
        &validate_endpoint(&key).unwrap_err(),
        "ModelEndpoint gateway spec.apiKeySecretRef",
        "a name and a key",
    );

    let both = endpoint(|e| {
        e["spec"]["baseUrl"] =
            json!({ "value": "https://m.example/v1", "secretRef": {"name": "s", "key": "k"} });
    });
    assert_issue(
        &validate_endpoint(&both).unwrap_err(),
        "ModelEndpoint gateway spec.baseUrl",
        "exactly one",
    );
}

#[test]
fn a_provider_is_judged_by_the_rules_of_an_inline_server() {
    type Case = (
        &'static str,
        Box<dyn Fn(&mut Value)>,
        &'static str,
        &'static str,
    );
    let cases: Vec<Case> = vec![
        (
            "a url with a password",
            Box::new(|p| p["spec"]["mcp"]["url"] = json!("https://u:hunter2@s.example/mcp")),
            "ToolProvider websearch spec.mcp.url",
            "no user name",
        ),
        (
            "not http",
            Box::new(|p| p["spec"]["mcp"]["url"] = json!("file:///etc/passwd")),
            "ToolProvider websearch spec.mcp.url",
            "",
        ),
        (
            "a header that is not a token",
            Box::new(|p| {
                p["spec"]["mcp"]["headers"] =
                    json!({ "Bad Header": { "secretRef": { "name": "s", "key": "K" } } });
            }),
            "ToolProvider websearch spec.mcp.headers[Bad Header]",
            "header name",
        ),
        (
            "a prefix that writes a variable",
            Box::new(|p| {
                p["spec"]["mcp"]["headers"]["Authorization"]["prefix"] = json!("${TOKEN}");
            }),
            "spec.mcp.headers[Authorization].prefix",
            "plain text",
        ),
        (
            "a key that is not a variable name",
            Box::new(|p| {
                p["spec"]["mcp"]["headers"]["Authorization"]["secretRef"]["key"] =
                    json!("token.pem");
            }),
            "spec.mcp.headers[Authorization].secretRef.key",
            "variable name",
        ),
        (
            "a tool adam cannot name",
            Box::new(|p| p["spec"]["mcp"]["tools"] = json!(["ok", "_hidden", "has space"])),
            "ToolProvider websearch spec.mcp.tools[1]",
            "cannot be named",
        ),
        (
            "an empty title",
            Box::new(|p| p["spec"]["title"] = json!(" ")),
            "ToolProvider websearch spec.title",
            "empty",
        ),
    ];
    for (what, change, field, word) in cases {
        let issues = validate_provider(&provider(|p| change(p))).unwrap_err();
        assert_issue(&issues, field, word);
        assert!(
            issues.iter().all(|i| !i.to_string().contains("hunter2")),
            "{what}: {issues:?}"
        );
    }
}

#[test]
fn every_issue_is_reported_not_the_first() {
    let p = provider(|p| {
        p["spec"]["mcp"]["url"] = json!("file:///x");
        p["spec"]["mcp"]["tools"] = json!(["a b", "c d"]);
    });
    let issues = validate_provider(&p).unwrap_err();
    assert_eq!(issues.len(), 3, "{issues:?}");
}

/// A plain `http://` URL to another machine is the config's rule (`tools.allowInsecureHttp`), so a provider alone is not
/// refused for it: the agents that name it are.
#[test]
fn plain_http_is_left_to_the_config() {
    let p = provider(|p| p["spec"]["mcp"]["url"] = json!("http://search.example.com/mcp"));
    validate_provider(&p).unwrap();
}
