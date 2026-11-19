//! The tool catalogue's examples (slice K10a, AD-056): every manifest under `examples/toolproviders/` is parsed, the
//! `ToolProvider`s pass `aap_domain::validate_provider`, every image is pinned by tag and digest, every RBAC rule the example
//! carries is read-only, and the allow-list names only tools the server itself documents as read-only.
//!
//! The server's tool names are the ones its README lists (*Tools*); if a release renames one, this test fails — which is the
//! point: an allow-list that names a tool the server no longer has is an agent that cannot use it.

#![allow(clippy::unwrap_used, clippy::expect_used)] // tests may

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use aap_api::ToolProvider;
use serde::Deserialize;
use serde_json::Value;

fn catalogue() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/toolproviders")
}

fn files() -> Vec<PathBuf> {
    let mut files: Vec<_> = fs::read_dir(catalogue())
        .expect("examples/toolproviders")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .collect();
    files.sort();
    files
}

fn documents(path: &Path) -> Vec<Value> {
    let text =
        fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_yaml::Deserializer::from_str(&text)
        .map(|d| Value::deserialize(d).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .collect()
}

/// The `kubernetes_mcp_server` tools its README documents as reading the cluster and nothing else (its *Tools* section). The
/// write ones (`pods_delete`, `pods_exec`, `pods_run`, `resources_create_or_update`, `resources_delete`, `resources_scale`)
/// and `nodes_log` are deliberately absent.
const KUBERNETES_READ_ONLY: &[&str] = &[
    "configuration_contexts_list",
    "configuration_view",
    "events_list",
    "namespaces_list",
    "nodes_stats_summary",
    "nodes_top",
    "pods_get",
    "pods_list",
    "pods_list_in_namespace",
    "pods_log",
    "pods_top",
    "resources_get",
    "resources_list",
    "targets_list",
];

#[test]
fn the_catalogue_holds_at_least_one_example() {
    assert!(!files().is_empty(), "examples/toolproviders is empty");
}

#[test]
fn every_provider_is_valid_and_names_only_read_only_kubernetes_tools() {
    let read_only: BTreeSet<&str> = KUBERNETES_READ_ONLY.iter().copied().collect();
    let mut providers = 0;
    for path in files() {
        for doc in documents(&path) {
            if doc["kind"] != "ToolProvider" {
                continue;
            }
            let provider: ToolProvider =
                serde_json::from_value(doc).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            aap_domain::validate_provider(&provider)
                .unwrap_or_else(|issues| panic!("{}: {issues:?}", path.display()));
            providers += 1;
            for tool in &provider.spec.mcp.tools {
                assert!(
                    read_only.contains(tool.as_str()),
                    "{}: {tool} is not one of the server's read-only tools",
                    path.display()
                );
            }
        }
    }
    assert!(providers > 0, "no ToolProvider in the catalogue");
}

#[test]
fn every_image_is_pinned_by_tag_and_digest() {
    for path in files() {
        for doc in documents(&path) {
            let containers = match doc["kind"].as_str() {
                Some("Deployment") | Some("StatefulSet") => {
                    doc["spec"]["template"]["spec"]["containers"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                }
                Some("Pod") => doc["spec"]["containers"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            for container in containers {
                let image = container["image"].as_str().unwrap_or_default();
                assert!(
                    image.contains("@sha256:") && image.contains(':'),
                    "{}: {image} is not pinned by tag and digest",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn every_rule_the_catalogue_grants_is_read_only() {
    for path in files() {
        for doc in documents(&path) {
            let rules = match doc["kind"].as_str() {
                Some("Role") | Some("ClusterRole") => {
                    doc["rules"].as_array().cloned().unwrap_or_default()
                }
                _ => Vec::new(),
            };
            for rule in rules {
                for verb in rule["verbs"].as_array().cloned().unwrap_or_default() {
                    let verb = verb.as_str().unwrap_or_default();
                    assert!(
                        matches!(verb, "get" | "list" | "watch"),
                        "{}: the example grants {verb}",
                        path.display()
                    );
                }
            }
        }
    }
}

#[test]
fn the_server_is_reachable_on_the_port_the_provider_names_and_nowhere_else() {
    for path in files() {
        let docs = documents(&path);
        let providers: Vec<&Value> = docs
            .iter()
            .filter(|d| d["kind"] == "ToolProvider")
            .collect();
        let services: Vec<&Value> = docs.iter().filter(|d| d["kind"] == "Service").collect();
        for provider in &providers {
            let url = provider["spec"]["mcp"]["url"].as_str().unwrap_or_default();
            let port = url
                .rsplit(':')
                .next()
                .and_then(|p| p.split('/').next())
                .unwrap_or_default();
            let port: u16 = port
                .parse()
                .unwrap_or_else(|_| panic!("{}: {url}", path.display()));
            assert!(
                services.iter().any(|s| s["spec"]["ports"]
                    .as_array()
                    .is_some_and(|ps| ps.iter().any(|p| p["port"] == port))),
                "{}: no Service listens on {port}, the provider's port",
                path.display()
            );
        }
        let policies: Vec<&Value> = docs
            .iter()
            .filter(|d| d["kind"] == "NetworkPolicy")
            .collect();
        assert!(
            !policies.is_empty(),
            "{}: no NetworkPolicy, so every pod of the cluster may reach the server",
            path.display()
        );
    }
}
