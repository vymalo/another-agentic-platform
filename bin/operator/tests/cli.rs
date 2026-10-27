//! The binary's contract: `crdgen` prints exactly the checked-in CRDs, and `run` says what it needs.

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use std::path::Path;
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

#[cfg(not(feature = "runtime-kubernetes"))]
#[test]
fn run_without_a_runtime_provider_says_what_to_rebuild_with() {
    let out = operator().arg("run").output().expect("runs the operator");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("no runtime provider"), "{stderr}");
}

#[cfg(feature = "runtime-kubernetes")]
#[test]
fn run_without_a_cluster_fails_and_says_where_it_looked() {
    // No in-cluster environment, and a kubeconfig that is not there: nothing to connect to.
    let out = operator()
        .arg("run")
        .env_remove("KUBERNETES_SERVICE_HOST")
        .env("KUBECONFIG", "/nonexistent/kubeconfig")
        .env("HOME", "/nonexistent")
        .env("HEALTH_ADDR", "127.0.0.1:0")
        .env("METRICS_ADDR", "127.0.0.1:0")
        .output()
        .expect("runs the operator");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("connecting to the cluster"), "{stderr}");
}

#[test]
fn run_lists_its_settings_and_the_environment_variables_that_set_them() {
    let out = operator()
        .args(["run", "--help"])
        .output()
        .expect("runs the operator");
    assert!(out.status.success());
    let help = String::from_utf8_lossy(&out.stdout);
    for variable in [
        "WATCH_NAMESPACE",
        "HEALTH_ADDR",
        "METRICS_ADDR",
        "POD_NAME",
        "AAP_CONCURRENCY",
        "AAP_RESYNC_SECS",
        "AAP_RESYNC_PENDING_SECS",
        "AAP_RECONCILE_TIMEOUT_SECS",
        "DEFAULT_AGENT_IMAGE",
        "--default-agent-image",
        "RUN_PODS_FILE",
        "--run-pods-file",
    ] {
        assert!(help.contains(variable), "{variable} is not in:\n{help}");
    }
    assert!(
        help.contains("0.0.0.0:8081") && help.contains("0.0.0.0:9090"),
        "{help}"
    );
}

/// `run` with no cluster to reach: `KUBERNETES_SERVICE_HOST` unset and no kubeconfig, so it stops at
/// `connecting to the cluster` unless something before that stopped it.
#[cfg(feature = "runtime-kubernetes")]
fn run_without_a_cluster(default_image: Option<&str>) -> (bool, String) {
    let mut command = operator();
    command
        .arg("run")
        .env_remove("KUBERNETES_SERVICE_HOST")
        .env_remove("DEFAULT_AGENT_IMAGE")
        .env("KUBECONFIG", "/nonexistent/kubeconfig")
        .env("HOME", "/nonexistent")
        .env("HEALTH_ADDR", "127.0.0.1:0")
        .env("METRICS_ADDR", "127.0.0.1:0");
    if let Some(image) = default_image {
        command.env("DEFAULT_AGENT_IMAGE", image);
    }
    let out = command.output().expect("runs the operator");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[cfg(feature = "runtime-kubernetes")]
#[test]
fn a_default_image_with_white_space_stops_the_operator_before_it_looks_for_a_cluster() {
    let (ok, stderr) = run_without_a_cluster(Some("ghcr.io/x/y:tag with space"));
    assert!(!ok);
    assert!(
        stderr.contains("DEFAULT_AGENT_IMAGE") && stderr.contains("white space"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("connecting to the cluster"),
        "it never got that far: {stderr}"
    );
}

#[cfg(feature = "runtime-kubernetes")]
#[test]
fn a_default_image_that_is_empty_or_a_reference_goes_on_to_look_for_a_cluster() {
    // An empty value is a chart value left blank: no default, not a mistake.
    for value in [
        None,
        Some(""),
        Some("  "),
        Some("ghcr.io/x/y:sha-1@sha256:abc"),
    ] {
        let (ok, stderr) = run_without_a_cluster(value);
        assert!(!ok);
        assert!(
            stderr.contains("connecting to the cluster"),
            "{value:?}: {stderr}"
        );
    }
}

/// `run` with a `RUN_PODS_FILE` and no cluster to reach.
#[cfg(feature = "runtime-kubernetes")]
fn run_with_run_pods_file(file: &std::path::Path) -> (bool, String) {
    let mut command = operator();
    command
        .arg("run")
        .env_remove("KUBERNETES_SERVICE_HOST")
        .env_remove("DEFAULT_AGENT_IMAGE")
        .env("RUN_PODS_FILE", file)
        .env("KUBECONFIG", "/nonexistent/kubeconfig")
        .env("HOME", "/nonexistent")
        .env("HEALTH_ADDR", "127.0.0.1:0")
        .env("METRICS_ADDR", "127.0.0.1:0");
    let out = command.output().expect("runs the operator");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The settings the operator chart ships, as the file the chart renders for the operator.
#[cfg(feature = "runtime-kubernetes")]
fn chart_settings_file(
    dir: &std::path::Path,
    change: impl FnOnce(&mut serde_json::Value),
) -> std::path::PathBuf {
    let values: serde_json::Value = serde_yaml::from_str(
        &std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../deploy/operator/values.yaml"
        ))
        .expect("the chart's values"),
    )
    .expect("YAML");
    let mut settings = values["runPods"].clone();
    let object = settings.as_object_mut().expect("a map");
    object.remove("enabled");
    object.remove("priorityClassName");
    object.insert("classes".into(), values["runPodClasses"].clone());
    object.insert(
        "priorityClass".into(),
        serde_json::json!("aap-operator-ns-run"),
    );
    object.insert("role".into(), serde_json::json!("aap-operator-run-pods"));
    change(&mut settings);
    let path = dir.join("run-pods.yaml");
    std::fs::write(&path, serde_yaml::to_string(&settings).expect("YAML")).expect("writes");
    path
}

#[cfg(feature = "runtime-kubernetes")]
#[test]
fn settings_for_run_pods_that_are_not_valid_stop_the_operator_before_it_looks_for_a_cluster() {
    let dir = std::env::temp_dir().join(format!("aap-cli-run-pods-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    // A file that is not there, one that is not the settings, and settings that fail their checks: each says what and where.
    let (ok, stderr) = run_with_run_pods_file(&dir.join("absent.yaml"));
    assert!(
        !ok && stderr.contains("RUN_PODS_FILE") && stderr.contains("cannot be read"),
        "{stderr}"
    );
    let junk = dir.join("junk.yaml");
    std::fs::write(&junk, "classes: 3\n").expect("writes");
    let (ok, stderr) = run_with_run_pods_file(&junk);
    assert!(
        !ok && stderr.contains("not the settings of run pods"),
        "{stderr}"
    );
    let unknown = chart_settings_file(&dir, |s| {
        s["admissionPolicy"] = serde_json::json!({"enabled": false});
    });
    let (ok, stderr) = run_with_run_pods_file(&unknown);
    assert!(
        !ok && stderr.contains("not the settings of run pods"),
        "{stderr:?}"
    );
    // The egress proxy (D15c): a file from before it (no `egressProxy`) and one that still has the switch the policy lost
    // are not the settings of run pods; a list that would be empty, an entry that is a bare suffix, and a port that is none are named.
    let without_proxy = chart_settings_file(&dir, |s| {
        s.as_object_mut().expect("a map").remove("egressProxy");
    });
    let (ok, stderr) = run_with_run_pods_file(&without_proxy);
    assert!(
        !ok && stderr.contains("not the settings of run pods"),
        "{stderr:?}"
    );
    let switch = chart_settings_file(&dir, |s| {
        s["networkPolicy"]["enabled"] = serde_json::json!(false);
    });
    let (ok, stderr) = run_with_run_pods_file(&switch);
    assert!(
        !ok && stderr.contains("not the settings of run pods"),
        "no way to run pods without the policy: {stderr:?}"
    );
    let bad_proxy = chart_settings_file(&dir, |s| {
        s["egressProxy"]["allow"] = serde_json::json!([".com"]);
        s["egressProxy"]["extraAllow"] = serde_json::json!([]);
        s["egressProxy"]["ports"] = serde_json::json!([0]);
    });
    let (ok, stderr) = run_with_run_pods_file(&bad_proxy);
    assert!(
        !ok && stderr.contains("egressProxy.allow[0]")
            && stderr.contains("bare suffix")
            && stderr.contains("egressProxy.ports"),
        "every mistake is named: {stderr}"
    );
    let invalid = chart_settings_file(&dir, |s| {
        s["quota"]["pods"] = serde_json::json!(0);
        s["image"]["digest"] = serde_json::json!("sha256:short");
    });
    let (ok, stderr) = run_with_run_pods_file(&invalid);
    assert!(
        !ok && stderr.contains("quota.pods") && stderr.contains("image.digest"),
        "every mistake is named: {stderr}"
    );
    assert!(!stderr.contains("connecting to the cluster"), "{stderr}");
    // Valid settings go on to look for a cluster; so does an empty name, which is no run pods.
    let valid = chart_settings_file(&dir, |_| {});
    let (ok, stderr) = run_with_run_pods_file(&valid);
    assert!(
        !ok && stderr.contains("connecting to the cluster"),
        "{stderr}"
    );
    let (ok, stderr) = run_with_run_pods_file(std::path::Path::new(""));
    assert!(
        !ok && stderr.contains("connecting to the cluster"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
