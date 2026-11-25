//! `aapctl` through the built binary: what it prints, and how it exits. Offline: no test needs a cluster, and `render` is held
//! equal to the goldens of `aap-runtime-kubernetes`, the files the operator's own tests hold its objects to.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use serde_json::json;
use std::path::PathBuf;

/// The uid the goldens' owner reference has (`crates/runtime-kubernetes/tests/common`).
const GOLDEN_UID: &str = "0b1f3c5e-1111-4222-8333-444455556666";

// ------------------------------------------------------------------------------------ render

/// What the goldens are: the objects of an example, as YAML documents joined by `---`. `render` prints that after one comment
/// line (the digest).
fn after_the_leading_comment(stdout: &str) -> &str {
    let (first, rest) = stdout.split_once('\n').unwrap();
    assert!(
        first.starts_with("# "),
        "the first line is a comment: {first}"
    );
    assert!(first.contains("sha256:"), "and holds the digest: {first}");
    rest
}

fn golden(name: &str) -> String {
    std::fs::read_to_string(
        root()
            .join("crates/runtime-kubernetes/tests/golden")
            .join(format!("{name}.yaml")),
    )
    .unwrap()
}

#[test]
fn render_matches_runtime_golden() {
    for (example_name, golden_name) in [("coder.yaml", "coder"), ("chat.yaml", "chat")] {
        let out = aapctl(
            &[
                "render",
                "-f",
                s(&example(example_name)),
                "--owner-uid",
                GOLDEN_UID,
            ],
            None,
        );
        assert_eq!(out.code, 0, "{example_name}: {}", out.stderr);
        assert!(
            after_the_leading_comment(&out.stdout) == golden(golden_name),
            "{example_name} differs from the golden {golden_name}:\n{}",
            out.stdout
        );
    }
}

#[test]
fn render_matches_the_run_pods_golden() {
    // The golden is the coder example with `coder.runPods.sizeClass: standard` and the operator chart's settings.
    let dir = scratch("run-pods-golden");
    let mut docs = read_docs(&example("coder.yaml"));
    for d in &mut docs {
        if d["kind"] == "AgentConfig" {
            set(
                d,
                "/spec/harness/adam/coder/runPods",
                json!({"sizeClass": "standard"}),
            );
        }
    }
    let file = dir.join("coder.yaml");
    write_docs(&file, &docs);
    let settings = chart_run_pod_settings(&dir);
    let out = aapctl(
        &[
            "render",
            "-f",
            s(&file),
            "--run-pods-file",
            s(&settings),
            "--owner-uid",
            GOLDEN_UID,
        ],
        None,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        after_the_leading_comment(&out.stdout) == golden("coder-run-pods"),
        "differs from the golden coder-run-pods:\n{}",
        out.stdout
    );
}

#[test]
fn render_prints_no_secret_value() {
    // Every example, rendered: the operator makes no Secret and holds no value of one, and nor does the CLI.
    let dir = scratch("no-secret");
    let settings = chart_run_pod_settings(&dir);
    let mut rendered = 0;
    for name in [
        "coder.yaml",
        "chat.yaml",
        "coder-me.yaml",
        "coder-run-pods.yaml",
    ] {
        let out = aapctl(
            &[
                "render",
                "-f",
                s(&example(name)),
                "--default-agent-image",
                IMAGE,
                "--run-pods-file",
                s(&settings),
            ],
            None,
        );
        assert_eq!(out.code, 0, "{name}: {}", out.stderr);
        for doc in documents(&out.stdout) {
            rendered += 1;
            assert_ne!(doc["kind"], "Secret", "{name}: a Secret was rendered");
            assert!(
                !out.stdout.contains("stringData"),
                "{name}: stringData in the output"
            );
        }
    }
    assert!(rendered > 10, "{rendered} objects were rendered");
}

#[test]
fn render_says_nothing_on_stdout_when_something_is_invalid() {
    let out = aapctl(
        &["render", "-f", s(&example("coder-me.yaml"))],
        None, // no default image: the example names none
    );
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
    assert!(out.stderr.contains("AgentConfig/coder"), "{}", out.stderr);
}

#[test]
fn render_takes_the_deployment_settings_of_the_operator() {
    // The default image fills what a config leaves out, and the ceiling of the access tier is the deployment's.
    let ok = aapctl(
        &[
            "render",
            "-f",
            s(&example("coder-me.yaml")),
            "--default-agent-image",
            IMAGE,
        ],
        None,
    );
    assert_eq!(ok.code, 0, "{}", ok.stderr);
    assert!(ok.stdout.contains(IMAGE));

    let dir = scratch("tier");
    let mut docs = read_docs(&example("coder.yaml"));
    for d in &mut docs {
        if d["kind"] == "AgentConfig" {
            set(d, "/spec/harness/adam/coder/maxAccessTier", json!("bypass"));
        }
    }
    let file = dir.join("coder.yaml");
    write_docs(&file, &docs);
    let refused = aapctl(
        &["render", "-f", s(&file), "--max-access-tier", "edits"],
        None,
    );
    assert_eq!(refused.code, 1);
    assert!(
        refused.stderr.contains("maxAccessTier"),
        "{}",
        refused.stderr
    );
    let allowed = aapctl(
        &["render", "-f", s(&file), "--max-access-tier", "bypass"],
        None,
    );
    assert_eq!(allowed.code, 0, "{}", allowed.stderr);
    let bad = aapctl(
        &["render", "-f", s(&file), "--max-access-tier", "root"],
        None,
    );
    assert_eq!(bad.code, 2);
}

// ------------------------------------------------------------------------------------ validate

#[test]
fn validate_accepts_every_example() {
    for name in [
        "coder.yaml",
        "chat.yaml",
        "coder-run-pods.yaml",
        "coder-me.yaml",
    ] {
        let dir = scratch("accepts");
        let settings = chart_run_pod_settings(&dir);
        let out = aapctl(
            &[
                "validate",
                "-f",
                s(&example(name)),
                "--default-agent-image",
                IMAGE,
                "--run-pods-file",
                s(&settings),
            ],
            None,
        );
        assert_eq!(out.code, 0, "{name}: {}", out.stderr);
        assert_eq!(out.stdout, "", "{name}");
    }
}

/// The invalid examples the CLI accepts: a bound of the CRD's schema, which only an API server reads, and an endpoint's own
/// rules when a config uses it (the operator judges every endpoint; the CLI judges only the endpoints no config reaches).
/// The CLI does not read a schema and does not evaluate CEL, so it cannot refuse them: it accepts them, and says so in its
/// README.
const REFUSED_BY_THE_API_SERVER_ONLY: [&str; 22] = [
    "audience-star-with-others.yaml",
    "audience-too-many.yaml",
    "audience-value-empty.yaml",
    "audience-value-too-long.yaml",
    "audience-value-with-space.yaml",
    "endpoint-reasoning-efforts-too-many.yaml",
    "endpoint-reasoning-too-many.yaml",
    "extra-body-too-many.yaml",
    "idle-timeout-not-a-duration.yaml",
    "idle-timeout-too-short.yaml",
    "mcp-servers-too-many.yaml",
    "model-endpoint-models-too-many.yaml",
    "model-endpoint-title-empty.yaml",
    "model-endpoint-title-too-long.yaml",
    "tool-provider-headers-too-many.yaml",
    "tool-provider-title-empty.yaml",
    "tool-provider-title-too-long.yaml",
    "tool-provider-tools-too-many.yaml",
    "tool-provider-url-too-long.yaml",
    "wake-ahead-too-long.yaml",
    "wake-timeout-too-long.yaml",
    "wake-timeout-too-short.yaml",
];

#[test]
fn validate_refuses_every_invalid_example() {
    let dir = scratch("invalid");
    let mut files: Vec<PathBuf> = std::fs::read_dir(root().join("examples/invalid"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    let mut refused = 0;
    let mut accepted = Vec::new();
    for file in files {
        let name = file.file_name().unwrap().to_str().unwrap().to_owned();
        let paired = invalid_with_partner(&file, &dir);
        let out = aapctl(&["validate", "-f", s(&paired)], None);
        if REFUSED_BY_THE_API_SERVER_ONLY.contains(&name.as_str()) {
            assert_eq!(
                out.code, 0,
                "{name} is refused by the API server only: {}",
                out.stderr
            );
            accepted.push(name);
            continue;
        }
        assert_eq!(out.code, 1, "{name} was not refused:\n{}", out.stderr);
        assert_eq!(out.stdout, "");
        // every line is `<file>: <Kind>/<name>: <field>: <message>`
        let prefix = format!("{}: ", s(&paired));
        let kind_and_name = out.stderr.lines().all(|l| {
            l.strip_prefix(&prefix).is_some_and(|rest| {
                [
                    "AgentService/",
                    "AgentConfig/",
                    "ModelEndpoint/",
                    "ToolProvider/",
                ]
                .iter()
                .any(|k| rest.starts_with(k))
            })
        });
        assert!(
            kind_and_name,
            "{name}: a line without its object:\n{}",
            out.stderr
        );
        assert!(
            out.stderr.contains(&name),
            "{name}: no file name:\n{}",
            out.stderr
        );
        refused += 1;
    }
    assert!(refused >= 34, "{refused} refused");
    assert_eq!(accepted, REFUSED_BY_THE_API_SERVER_ONLY);
}

#[test]
fn a_missing_referent_is_an_issue_naming_it() {
    let dir = scratch("missing");
    let mut docs = read_docs(&example("coder-me.yaml"));
    docs.retain(|d| d["kind"] != "ModelEndpoint");
    let file = dir.join("me.yaml");
    write_docs(&file, &docs);
    let out = aapctl(
        &["validate", "-f", s(&file), "--default-agent-image", IMAGE],
        None,
    );
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr.contains("ModelEndpoint") && out.stderr.contains("was not found"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("me.yaml"), "{}", out.stderr);
    assert!(out.stderr.contains("AgentConfig/"), "{}", out.stderr);

    // a service whose config is not among the documents
    let mut docs = read_docs(&example("coder.yaml"));
    docs.retain(|d| d["kind"] != "AgentConfig");
    write_docs(&file, &docs);
    let out = aapctl(&["validate", "-f", s(&file)], None);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("AgentService/coder"), "{}", out.stderr);
    assert!(out.stderr.contains("AgentConfig"), "{}", out.stderr);
}

#[test]
fn allow_missing_refs_downgrades_it() {
    let dir = scratch("allow-missing");
    let mut docs = read_docs(&example("coder-me.yaml"));
    docs.retain(|d| d["kind"] != "ModelEndpoint" && d["kind"] != "ToolProvider");
    let file = dir.join("me.yaml");
    write_docs(&file, &docs);
    let args = ["validate", "-f", s(&file), "--default-agent-image", IMAGE];
    assert_eq!(aapctl(&args, None).code, 1);
    let out = aapctl(
        &[
            "validate",
            "-f",
            s(&file),
            "--default-agent-image",
            IMAGE,
            "--allow-missing-refs",
        ],
        None,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("warning"), "{}", out.stderr);
    assert!(out.stderr.contains("ModelEndpoint"), "{}", out.stderr);

    // and it hides nothing else
    for d in &mut docs {
        if d["kind"] == "AgentService" {
            set(d, "/spec/store", json!({"postgres": {}}));
        }
    }
    write_docs(&file, &docs);
    let out = aapctl(
        &[
            "validate",
            "-f",
            s(&file),
            "--default-agent-image",
            IMAGE,
            "--allow-missing-refs",
        ],
        None,
    );
    assert_eq!(out.code, 1, "{}", out.stderr);
}

#[test]
fn unknown_fields_are_found_where_the_types_can_tell() {
    let dir = scratch("unknown");
    let mut docs = read_docs(&example("chat.yaml"));
    for d in &mut docs {
        if d["kind"] == "AgentService" {
            set(d, "/spec/scaling/wokers", json!(3));
        }
    }
    let file = dir.join("chat.yaml");
    write_docs(&file, &docs);
    let out = aapctl(
        &["validate", "-f", s(&file), "--default-agent-image", IMAGE],
        None,
    );
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains("AgentService/chat") && out.stderr.contains("spec.scaling.wokers"),
        "{}",
        out.stderr
    );
}

#[test]
fn duplicate_names_are_a_usage_error() {
    // the same kind, namespace and name in two files (not the same file twice: that is read once)
    let dir = scratch("duplicates");
    let copy = dir.join("coder-again.yaml");
    std::fs::copy(example("coder.yaml"), &copy).unwrap();
    let out = aapctl(
        &["validate", "-f", s(&example("coder.yaml")), "-f", s(&copy)],
        None,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("AgentService/coder"), "{}", out.stderr);
    assert!(out.stderr.contains("twice"), "{}", out.stderr);
}

#[test]
fn stdin_and_directories_are_read() {
    let text = std::fs::read_to_string(example("chat.yaml")).unwrap();
    let out = aapctl(&["validate", "-f", "-"], Some(&text));
    assert_eq!(out.code, 0, "{}", out.stderr);

    let bad = text.replace("topology: combined", "topology: sideways");
    let out = aapctl(&["validate", "-f", "-"], Some(&bad));
    // a value the type does not read is an issue, named by where it came from
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("<stdin>"), "{}", out.stderr);

    // a directory: the `*.yaml` and `*.yml` files of it, and nothing else
    let dir = scratch("directory");
    std::fs::copy(example("chat.yaml"), dir.join("a.yaml")).unwrap();
    let coder = std::fs::read_to_string(example("coder.yaml")).unwrap();
    std::fs::write(dir.join("b.yml"), coder).unwrap();
    std::fs::write(dir.join("notes.txt"), "not yaml: [").unwrap();
    let out = aapctl(&["validate", "-f", s(&dir)], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn exit_codes() {
    let ok = example("chat.yaml");
    assert_eq!(aapctl(&["validate", "-f", s(&ok)], None).code, 0);
    assert_eq!(aapctl(&["version"], None).code, 0);
    assert_eq!(aapctl(&["--help"], None).code, 0);
    assert_eq!(aapctl(&["render", "--help"], None).code, 0);

    // 1: an issue
    let invalid = root().join("examples/invalid/store-both.yaml");
    let paired = invalid_with_partner(&invalid, &scratch("exit-codes"));
    assert_eq!(aapctl(&["validate", "-f", s(&paired)], None).code, 1);

    // 2: usage and read errors
    let dir = scratch("exit-2");
    assert_eq!(aapctl(&[], None).code, 2);
    assert_eq!(aapctl(&["validate"], None).code, 2);
    assert_eq!(aapctl(&["bogus"], None).code, 2);
    assert_eq!(
        aapctl(&["validate", "-f", s(&dir.join("missing.yaml"))], None).code,
        2
    );
    let not_yaml = dir.join("broken.yaml");
    std::fs::write(&not_yaml, "kind: [unclosed").unwrap();
    assert_eq!(aapctl(&["validate", "-f", s(&not_yaml)], None).code, 2);
    let unknown = dir.join("unknown.yaml");
    std::fs::write(
        &unknown,
        "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\n",
    )
    .unwrap();
    let out = aapctl(&["validate", "-f", s(&unknown)], None);
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("ConfigMap"), "{}", out.stderr);
    let empty = dir.join("empty.yaml");
    std::fs::write(&empty, "").unwrap();
    assert_eq!(
        aapctl(&["validate", "-f", s(&empty)], None).code,
        2,
        "nothing to validate"
    );
}

#[test]
fn version_prints_the_version_and_nothing_else() {
    let out = aapctl(&["version"], None);
    assert_eq!(
        out.stdout.trim(),
        format!("aapctl {}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(out.stderr, "");
}
