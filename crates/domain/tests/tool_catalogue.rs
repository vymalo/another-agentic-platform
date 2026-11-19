//! The tool catalogue's examples (slices K10a and K10b, AD-056): every manifest under `examples/toolproviders/` is parsed, the
//! `ToolProvider`s pass `aap_domain::validate_provider`, every image is pinned by tag and digest, every RBAC rule the example
//! carries is read-only, and the allow-list names only tools **the server the file runs** documents as read-only.
//!
//! The servers' tool names are the ones their READMEs list (*Tools*); if a release renames one, this test fails — which is the
//! point: an allow-list that names a tool the server no longer has is an agent that cannot use it. A file whose image has no
//! entry here fails too, so a new example cannot borrow another server's list.

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

/// The `prometheus-mcp-server` tools its README documents (its *Available Tools* section). `health_check` is the sixth and is
/// absent on purpose: it is a monitoring tool, and the pod's probe asks the server's own `/health` instead.
const PROMETHEUS_READ_ONLY: &[&str] = &[
    "execute_query",
    "execute_range_query",
    "get_metric_metadata",
    "get_targets",
    "list_metrics",
];

/// The `github-mcp-server` tools its README documents as reading, at `v2.2.0` (its *Tools* section), for the three toolsets
/// the example enables (`repos`, `issues`, `pull_requests`). The write tools of those toolsets (`create_branch`,
/// `create_or_update_file`, `push_files`, `create_pull_request`, `merge_pull_request`, `issue_write`, ...) are absent, and
/// the server's `--read-only` refuses them too.
const GITHUB_READ_ONLY: &[&str] = &[
    "get_commit",
    "get_file_contents",
    "get_latest_release",
    "issue_read",
    "list_branches",
    "list_commits",
    "list_issues",
    "list_pull_requests",
    "list_releases",
    "list_tags",
    "pull_request_read",
    "search_code",
    "search_commits",
    "search_issues",
    "search_pull_requests",
    "search_repositories",
];

/// The servers of the catalogue, keyed by the image repository the file runs: the tools each documents as reading, and nothing
/// else. **A new example adds its row**: a file whose image is not here fails, so its allow-list is never checked against
/// another server's list.
const SERVERS: &[(&str, &[&str])] = &[
    (
        "quay.io/containers/kubernetes_mcp_server",
        KUBERNETES_READ_ONLY,
    ),
    (
        "ghcr.io/pab1it0/prometheus-mcp-server",
        PROMETHEUS_READ_ONLY,
    ),
    ("ghcr.io/github/github-mcp-server", GITHUB_READ_ONLY),
];

/// How a server is told to refuse its own write tools, and where: an argument of its container, a line of the configuration
/// it mounts, or nothing (a server whose tools all read).
enum ReadOnly {
    /// An exact argument of the container that runs the server (`--read-only`).
    Arg(&'static str),
    /// A line of the ConfigMap the container mounts, under this key (`read_only = true`).
    ConfigLine {
        key: &'static str,
        line: &'static str,
    },
    /// No switch: every tool the server has reads. The row says so, so that a server that gains a write tool is looked at.
    None,
}

/// The servers of the catalogue, keyed by the image repository: the tools each documents as reading (above) and how the
/// server itself is told to refuse writes. **A new example adds its row here and in `SERVERS`**, or the test fails.
const READ_ONLY_SWITCH: &[(&str, ReadOnly)] = &[
    (
        "quay.io/containers/kubernetes_mcp_server",
        ReadOnly::ConfigLine {
            key: "config.toml",
            line: "read_only = true",
        },
    ),
    // The Prometheus server has no write tool at all: it queries a Prometheus.
    ("ghcr.io/pab1it0/prometheus-mcp-server", ReadOnly::None),
    (
        "ghcr.io/github/github-mcp-server",
        ReadOnly::Arg("--read-only"),
    ),
];

/// The repository of an image reference: `registry/repo`, without the tag and the digest.
fn repository(image: &str) -> &str {
    let before_digest = image.split('@').next().unwrap_or(image);
    match before_digest.rsplit_once(':') {
        // A registry with a port and no tag (`registry:5000/repo`) has no tag to cut: the colon is before the last `/`.
        Some((head, tail)) if !tail.contains('/') => head,
        _ => before_digest,
    }
}

/// The read-only tools of the server a file runs, from its `SERVERS` row.
fn read_only_of(image: &str) -> Option<&'static [&'static str]> {
    let repo = repository(image);
    SERVERS
        .iter()
        .find(|(name, _)| *name == repo)
        .map(|(_, tools)| *tools)
}

#[test]
fn the_catalogue_holds_at_least_one_example() {
    assert!(!files().is_empty(), "examples/toolproviders is empty");
}

/// The images of the workloads a file runs (a `Deployment`, a `StatefulSet` or a `Pod`).
fn images(docs: &[Value]) -> Vec<String> {
    let mut out = Vec::new();
    for doc in docs {
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
            if let Some(image) = container["image"].as_str() {
                out.push(image.to_owned());
            }
        }
    }
    out
}

#[test]
fn every_provider_names_only_tools_its_own_server_documents_as_read_only() {
    let mut providers = 0;
    for path in files() {
        let docs = documents(&path);
        let images = images(&docs);
        // Every image the file runs is a server this test knows: a new example must record its read-only tools here, or its
        // allow-list would be checked against nothing (or, worse, against another server's list).
        let mut read_only: BTreeSet<&str> = BTreeSet::new();
        for image in &images {
            let tools = read_only_of(image).unwrap_or_else(|| {
                panic!(
                    "{}: {} has no row in SERVERS: add the server's documented read-only tools",
                    path.display(),
                    repository(image)
                )
            });
            read_only.extend(tools.iter().copied());
        }
        for doc in &docs {
            if doc["kind"] != "ToolProvider" {
                continue;
            }
            let provider: ToolProvider = serde_json::from_value(doc.clone())
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            aap_domain::validate_provider(&provider)
                .unwrap_or_else(|issues| panic!("{}: {issues:?}", path.display()));
            providers += 1;
            for tool in &provider.spec.mcp.tools {
                assert!(
                    read_only.contains(tool.as_str()),
                    "{}: {tool} is not one of the tools {} documents as read-only",
                    path.display(),
                    images
                        .iter()
                        .map(|i| repository(i))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
    assert!(providers > 0, "no ToolProvider in the catalogue");
}

/// Every server the catalogue runs is told to refuse its own writes, in the place its row names: the container's arguments,
/// or the line of the configuration it mounts. Two controls, one in the platform's allow-list and one in the server.
#[test]
fn every_server_refuses_its_own_write_tools() {
    for path in files() {
        let docs = documents(&path);
        for image in images(&docs) {
            let repo = repository(&image);
            let (_, how) = READ_ONLY_SWITCH
                .iter()
                .find(|(name, _)| *name == repo)
                .unwrap_or_else(|| {
                    panic!("{}: {repo} has no READ_ONLY_SWITCH row", path.display())
                });
            match how {
                ReadOnly::Arg(arg) => {
                    let found = docs.iter().any(|doc| {
                        let containers = match doc["kind"].as_str() {
                            Some("Deployment") | Some("StatefulSet") => {
                                doc["spec"]["template"]["spec"]["containers"].clone()
                            }
                            Some("Pod") => doc["spec"]["containers"].clone(),
                            _ => Value::Null,
                        };
                        containers.as_array().is_some_and(|cs| {
                            cs.iter().any(|c| {
                                c["args"]
                                    .as_array()
                                    .is_some_and(|args| args.iter().any(|a| a == arg))
                            })
                        })
                    });
                    assert!(
                        found,
                        "{}: no container of {repo} passes {arg}",
                        path.display()
                    );
                }
                ReadOnly::ConfigLine { key, line } => {
                    let found = docs.iter().any(|doc| {
                        doc["kind"] == "ConfigMap"
                            && doc["data"][key]
                                .as_str()
                                .is_some_and(|text| text.lines().any(|l| l.trim() == *line))
                    });
                    assert!(
                        found,
                        "{}: no ConfigMap of {repo} says {line:?} under {key}",
                        path.display()
                    );
                }
                ReadOnly::None => {}
            }
        }
    }
}

#[test]
fn every_image_is_pinned_by_tag_and_digest() {
    for path in files() {
        for image in images(&documents(&path)) {
            assert!(
                image.contains("@sha256:") && repository(&image) != image,
                "{}: {image} is not pinned by tag and digest",
                path.display()
            );
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
