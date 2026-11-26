//! The binary's contract: `crdgen` prints exactly the checked-in CRDs, and `run` reads one configuration file (ANR-962).
//!
//! `operator run --check` and `operator run` exit **78** (`EX_CONFIG`) when the file is refused, naming the setting, and 1
//! for anything else. No secret is ever quoted: a value that could be a token never appears in a message.

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use std::path::{Path, PathBuf};
use std::process::Command;

fn operator() -> Command {
    Command::new(env!("CARGO_BIN_EXE_operator"))
}

/// `deploy/operator-crds` reads its own copy (a Helm chart reads only its directory); it is the checked-in file, byte for byte
/// (render-check.sh checks it too, with helm).
#[test]
fn the_crds_chart_holds_the_checked_in_crds() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let checked_in =
        std::fs::read_to_string(root.join("deploy/crds/agents.vymalo.com.yaml")).unwrap();
    let copy =
        std::fs::read_to_string(root.join("deploy/operator-crds/files/agents.vymalo.com.yaml"))
            .unwrap_or_default();
    assert!(
        copy == checked_in,
        "deploy/operator-crds/files/agents.vymalo.com.yaml differs from deploy/crds/agents.vymalo.com.yaml. Copy it:\n  cp deploy/crds/agents.vymalo.com.yaml deploy/operator-crds/files/"
    );
}

#[test]
fn crdgen_matches_the_checked_in_crds() {
    let out = operator()
        .arg("crdgen")
        .output()
        .expect("runs the operator");
    assert!(
        out.status.success(),
        "crdgen failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let generated = String::from_utf8(out.stdout).expect("utf-8");

    let golden =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/crds/agents.vymalo.com.yaml");
    let checked_in = std::fs::read_to_string(&golden).unwrap_or_default();
    assert!(
        generated == checked_in,
        "{} differs from the output of `crdgen`. Regenerate it:\n  cargo run -q -p aap-operator -- crdgen > deploy/crds/agents.vymalo.com.yaml",
        golden.display()
    );
}

#[test]
fn schema_matches_the_checked_in_schema() {
    let out = operator()
        .arg("schema")
        .output()
        .expect("runs the operator");
    assert!(
        out.status.success(),
        "schema failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let generated = String::from_utf8(out.stdout).expect("utf-8");

    let golden = Path::new(env!("CARGO_MANIFEST_DIR")).join("config.schema.json");
    let checked_in = std::fs::read_to_string(&golden).unwrap_or_default();
    assert!(
        generated == checked_in,
        "{} differs from the output of `schema`. Regenerate it:\n  cargo run -q -p aap-operator -- schema > bin/operator/config.schema.json",
        golden.display()
    );
}

/// Every object the file names its keys of refuses any other key (`deny_unknown_fields`), so the schema refuses a typo as the
/// binary does; the count guards against a schema that has no objects at all.
#[test]
fn schema_refuses_unknown_keys_at_every_object_with_properties() {
    fn walk(value: &serde_json::Value, path: &str, with_properties: &mut usize) {
        match value {
            serde_json::Value::Object(map) => {
                if map.contains_key("properties") {
                    *with_properties += 1;
                    assert_eq!(
                        map.get("additionalProperties"),
                        Some(&serde_json::Value::Bool(false)),
                        "{path} accepts unknown keys"
                    );
                }
                for (key, child) in map {
                    walk(child, &format!("{path}/{key}"), with_properties);
                }
            }
            serde_json::Value::Array(items) => {
                for (i, child) in items.iter().enumerate() {
                    walk(child, &format!("{path}/{i}"), with_properties);
                }
            }
            _ => {}
        }
    }
    let text =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("config.schema.json"))
            .expect("the schema is checked in");
    let schema: serde_json::Value = serde_json::from_str(&text).expect("JSON");
    let mut with_properties = 0;
    walk(&schema, "#", &mut with_properties);
    assert!(with_properties >= 10, "only {with_properties} objects");
}

#[test]
fn crdgen_prints_one_document_per_kind() {
    let out = operator()
        .arg("crdgen")
        .output()
        .expect("runs the operator");
    let text = String::from_utf8(out.stdout).expect("utf-8");
    assert_eq!(
        text.matches("\n---\n").count(),
        4,
        "one `---` before each of the four CRDs"
    );
    for name in [
        "agentconfigs",
        "agentservices",
        "modelendpoints",
        "toolproviders",
    ] {
        assert!(text.contains(&format!("name: {name}.agents.vymalo.com")));
    }
}

/// A directory of files this test made.
fn dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aap-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    dir
}

/// A configuration file with this text.
fn file(name: &str, text: &str) -> PathBuf {
    let path = dir().join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

/// `run` on a file, with no cluster to reach: `KUBERNETES_SERVICE_HOST` unset and a kubeconfig that is not there, so it
/// stops at `connecting to the cluster` unless something before that stopped it.
fn run(config: &Path) -> (Option<i32>, String, String) {
    let out = operator()
        .arg("run")
        .arg("--config")
        .arg(config)
        .env_remove("KUBERNETES_SERVICE_HOST")
        .env("KUBECONFIG", "/nonexistent/kubeconfig")
        .env("HOME", "/nonexistent")
        .output()
        .expect("runs the operator");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `run --check` on a file.
fn check(config: &Path) -> (Option<i32>, String, String) {
    let out = operator()
        .arg("run")
        .arg("--check")
        .arg("--config")
        .arg(config)
        .output()
        .expect("runs the operator");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The least a deployment must say: a version, and nothing else.
const MINIMAL: &str = "version: 1\n";

#[test]
fn run_lists_its_two_settings() {
    let out = operator()
        .args(["run", "--help"])
        .output()
        .expect("runs the operator");
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for setting in ["--config", "--check", "/etc/aap-operator/config.yaml"] {
        assert!(help.contains(setting), "{setting} is not in:\n{help}");
    }
    // No setting is an environment variable any more: the file is the only way in.
    assert!(!help.contains("env:"), "{help}");
}

#[test]
fn the_example_is_a_file_the_operator_accepts() {
    let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("config.example.yaml");
    let (code, stdout, stderr) = check(&example);
    assert_eq!(code, Some(0), "{stderr}");
    assert!(stdout.contains(": ok"), "{stdout}");
    // And `run` takes it too, and goes on to look for the cluster it does not have.
    let (code, _, stderr) = run(&example);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("connecting to the cluster"), "{stderr}");
}

#[test]
fn a_file_that_is_not_there_stops_the_operator_with_ex_config() {
    let absent = dir().join("absent.yaml");
    for (code, _, stderr) in [
        run(&absent),
        check(&absent),
        // The default path is the chart's, and nothing is mounted here.
        {
            let out = operator()
                .arg("run")
                .env_remove("KUBERNETES_SERVICE_HOST")
                .output()
                .expect("runs the operator");
            (
                out.status.code(),
                String::new(),
                String::from_utf8_lossy(&out.stderr).into_owned(),
            )
        },
    ] {
        assert_eq!(code, Some(78), "{stderr}");
        assert!(stderr.contains("cannot be read"), "{stderr}");
        assert!(!stderr.contains("connecting to the cluster"), "{stderr}");
    }
}

#[test]
fn a_key_the_file_does_not_have_is_refused_by_both_with_ex_config() {
    for text in [
        format!("{MINIMAL}typo: x\n"),
        format!("{MINIMAL}runPods: {{ typo: 1 }}\n"),
        format!("{MINIMAL}watch: {{ namespaces: [], typo: x }}\n"),
        format!("{MINIMAL}telemetry: {{ otlp: {{ typo: x }} }}\n"),
    ] {
        let path = file("typo.yaml", &text);
        for (code, _, stderr) in [run(&path), check(&path)] {
            assert_eq!(code, Some(78), "{text}: {stderr}");
            assert!(stderr.contains("typo"), "{text}: {stderr}");
            assert!(!stderr.contains("connecting to the cluster"), "{stderr}");
        }
    }
}

#[test]
fn a_secret_that_is_a_value_and_not_a_reference_is_refused_without_quoting_it() {
    let path = file(
        "token.yaml",
        &format!("{MINIMAL}registry: {{ token: s3cret-token-value }}\n"),
    );
    let (code, _, stderr) = check(&path);
    assert_eq!(code, Some(78), "{stderr}");
    assert!(stderr.contains("token"), "{stderr}");
    assert!(
        !stderr.contains("s3cret-token-value"),
        "a value that may be the token is in the message: {stderr}"
    );
}

#[test]
fn a_version_that_is_not_one_is_refused() {
    for text in [
        "listen: { health: 0.0.0.0:1 }\n".to_owned(),
        "version: 2\n".to_owned(),
    ] {
        let path = file("version.yaml", &text);
        let (code, _, stderr) = check(&path);
        assert_eq!(code, Some(78), "{text}: {stderr}");
        assert!(stderr.contains("version"), "{text}: {stderr}");
    }
}

#[test]
fn the_settings_the_file_refuses_stop_the_operator_before_it_looks_for_a_cluster() {
    // Each is a file the operator could not work with, and the message names the setting.
    for (text, named) in [
        (
            "version: 1\ndefaults: { agentImage: 'ghcr.io/x/y:tag with space' }\n",
            "defaults.agentImage",
        ),
        (
            "version: 1\ndefaults: { agentRuntimeClass: 'Not A Class' }\n",
            "defaults.agentRuntimeClass",
        ),
        (
            "version: 1\nwatch: { namespaces: [Bad_Name] }\n",
            "watch.namespaces",
        ),
        ("version: 1\nwatch: { namespaces: [a, a] }\n", "twice"),
        (
            "version: 1\nreconcile: { concurrency: 0 }\n",
            "reconcile.concurrency",
        ),
        (
            "version: 1\nlisten: { health: 0.0.0.0:9090 }\n",
            "listen.metrics",
        ),
        (
            "version: 1\ntoolProbe: { timeoutSecs: 61 }\n",
            "toolProbe.timeoutSecs",
        ),
        (
            "version: 1\nleaderElection: { enabled: true }\n",
            "leaderElection.namespace",
        ),
        (
            "version: 1\nwatch: { namespaces: [agents] }\nleaderElection: { enabled: true, renewSecs: 15 }\n",
            "retry < renew < lease",
        ),
        ("version: 1\nlog: { filter: '=' }\n", "log.filter"),
        (
            "version: 1\ntelemetry: { otlp: { endpoint: 'unix:///tmp/x' } }\n",
            "telemetry.otlp.endpoint",
        ),
        (
            "version: 1\ntelemetry: { otlp: { endpoint: 'http://c:4318/v1/traces', headers: { authorization: { file: /h } } } }\n",
            "telemetry.otlp.headers",
        ),
    ] {
        let path = file("refused.yaml", text);
        for (code, _, stderr) in [run(&path), check(&path)] {
            assert_eq!(code, Some(78), "{text}: {stderr}");
            assert!(stderr.contains(named), "{text}: {stderr}");
            assert!(!stderr.contains("connecting to the cluster"), "{stderr}");
        }
    }
}

#[test]
fn the_run_pods_settings_of_the_chart_are_the_files_and_their_checks_are_named() {
    // The chart's `runPods` and `runPodClasses`, as the chart renders them, are a block this file takes.
    let values: serde_json::Value = serde_yaml::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/operator/values.yaml"
        ))
        .expect("the chart's values"),
    )
    .expect("YAML");
    let mut run_pods = values["runPods"].clone();
    let object = run_pods.as_object_mut().expect("a map");
    object.remove("enabled");
    object.remove("priorityClassName");
    object.insert("classes".into(), values["runPodClasses"].clone());
    object.insert(
        "priorityClass".into(),
        serde_json::json!("aap-operator-ns-run"),
    );
    object.insert("role".into(), serde_json::json!("aap-operator-run-pods"));

    let mut document: serde_json::Value = serde_yaml::from_str(MINIMAL).expect("YAML");
    document["runPods"] = run_pods.clone();
    let valid = file("run-pods.yaml", &serde_yaml::to_string(&document).unwrap());
    let (code, _, stderr) = run(&valid);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("connecting to the cluster"), "{stderr}");

    // A mistake in them names the key: the operator does not start with half the guard.
    let mut broken = run_pods;
    broken["quota"]["pods"] = serde_json::json!(0);
    document["runPods"] = broken;
    let path = file(
        "run-pods-broken.yaml",
        &serde_yaml::to_string(&document).unwrap(),
    );
    let (code, _, stderr) = run(&path);
    assert_eq!(code, Some(78), "{stderr}");
    assert!(stderr.contains("runPods.quota.pods"), "{stderr}");
    assert!(!stderr.contains("connecting to the cluster"), "{stderr}");
}

/// The variables `run` used to read. The file is the only way in now: a shell cannot change what the operator does, and a
/// chart cannot either.
#[test]
fn the_environment_variables_of_the_old_settings_are_not_read() {
    let path = file("minimal.yaml", MINIMAL);
    let out = operator()
        .arg("run")
        .arg("--config")
        .arg(&path)
        .env_remove("KUBERNETES_SERVICE_HOST")
        .env("KUBECONFIG", "/nonexistent/kubeconfig")
        .env("HOME", "/nonexistent")
        // Each was a setting, and each is a value the file would refuse.
        .env("DEFAULT_AGENT_IMAGE", "ghcr.io/x/y:tag with space")
        .env("WATCH_NAMESPACE", "Bad_Name")
        .env("WATCH_NAMESPACES", "Bad_Name")
        .env("RUN_PODS_FILE", "/nonexistent/run-pods.yaml")
        .env("HEALTH_ADDR", "not-an-address")
        .env("AGENT_MAX_ACCESS_TIER", "root")
        .env("TOOL_PROBE_TIMEOUT_SECS", "0")
        .env("LEADER_ELECTION_ENABLED", "true")
        .env("RUST_LOG", "=")
        .output()
        .expect("runs the operator");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("connecting to the cluster"),
        "an old variable changed what the operator did: {stderr}"
    );
}

#[cfg(not(feature = "runtime-kubernetes"))]
#[test]
fn run_without_a_runtime_provider_says_what_to_rebuild_with() {
    let path = file("no-runtime.yaml", MINIMAL);
    let (code, _, stderr) = run(&path);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("no runtime provider"), "{stderr}");
}
