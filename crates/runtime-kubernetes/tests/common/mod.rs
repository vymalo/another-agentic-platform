//! Loading the examples and changing them, for the tests of `aap-runtime-kubernetes`.
#![allow(dead_code)] // each test file uses part of it

use std::path::{Path, PathBuf};

use aap_api::{AgentConfig, AgentService};
use aap_domain::ResolvedAgent;
use aap_runtime_kubernetes::owner_handle;
use serde::Deserialize;
use serde_json::Value;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// An example as the two JSON documents it holds: the service, then the config.
pub fn example(name: &str) -> (Value, Value) {
    let path = repo_root().join("examples").join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut service = None;
    let mut config = None;
    for d in serde_yaml::Deserializer::from_str(&text) {
        let d = Value::deserialize(d).unwrap();
        match d["kind"].as_str() {
            Some("AgentService") => service = Some(d),
            Some("AgentConfig") => config = Some(d),
            other => panic!("unexpected kind {other:?}"),
        }
    }
    (service.unwrap(), config.unwrap())
}

/// Set the value at a JSON pointer (`~1` is `/`), creating the objects on the way.
pub fn set(doc: &mut Value, pointer: &str, value: Value) {
    let mut at = doc;
    let parts: Vec<String> = pointer
        .split('/')
        .skip(1)
        .map(|p| p.replace("~1", "/").replace("~0", "~"))
        .collect();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        if let Value::Array(items) = at {
            let index: usize = part.parse().unwrap();
            if last {
                items[index] = value;
                return;
            }
            at = &mut items[index];
            continue;
        }
        if last {
            at[part.as_str()] = value;
            return;
        }
        if at.get(part.as_str()).is_none() {
            at[part.as_str()] = Value::Object(serde_json::Map::new());
        }
        at = &mut at[part.as_str()];
    }
}

/// An example with changes, resolved with an owner that has a uid.
pub fn resolved(name: &str, change: impl FnOnce(&mut Value, &mut Value)) -> ResolvedAgent {
    let (mut service, mut config) = example(name);
    change(&mut service, &mut config);
    let service: AgentService = serde_json::from_value(service).unwrap();
    let config: AgentConfig = serde_json::from_value(config).unwrap();
    let owner = owner_handle(
        "agents.vymalo.com/v1alpha1",
        "AgentService",
        service.metadata.name.clone().unwrap(),
        "0b1f3c5e-1111-4222-8333-444455556666",
    );
    aap_domain::resolve(&service, &config, &aap_domain::Externals::default(), owner).unwrap_or_else(
        |issues| {
            panic!(
                "the example does not resolve: {}",
                issues
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        },
    )
}

// ------------------------------------------------------------------------------------ run pods

/// `deploy/operator/values.yaml`, parsed.
pub fn operator_chart_values() -> Value {
    let path = repo_root().join("deploy/operator/values.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The settings the operator chart ships for run pods, as the file the chart renders for the operator, untyped.
pub fn chart_run_pod_settings_value() -> Value {
    let values = operator_chart_values();
    let mut settings = values["runPods"].clone();
    let object = settings.as_object_mut().expect("runPods is a map");
    object.remove("enabled");
    object.remove("priorityClassName");
    object.insert("classes".into(), values["runPodClasses"].clone());
    object.insert("priorityClass".into(), serde_json::json!("coder-run"));
    object.insert("role".into(), serde_json::json!("coder-run-pods"));
    settings
}

/// The settings the operator chart ships for run pods, as the file the chart renders for the operator.
pub fn chart_run_pod_settings() -> aap_domain::RunPodSettings {
    run_pod_settings(chart_run_pod_settings_value())
}

/// A settings file, read as the operator reads it and checked as it checks it.
pub fn run_pod_settings(settings: Value) -> aap_domain::RunPodSettings {
    let settings: aap_domain::RunPodSettings =
        serde_json::from_value(settings).expect("the chart's settings");
    settings.validate().expect("the chart's settings are valid");
    settings
}

/// The coder example with run pods on, the chart's settings changed by `settings` (as the file the chart renders), and the
/// example by `change`.
pub fn resolved_run_pods_with(
    settings: impl FnOnce(&mut Value),
    change: impl FnOnce(&mut Value, &mut Value),
) -> ResolvedAgent {
    let mut file = chart_run_pod_settings_value();
    settings(&mut file);
    resolved_with("coder", &run_pod_settings(file), |s, c| {
        set(
            c,
            "/spec/harness/adam/coder/runPods",
            serde_json::json!({"sizeClass": "standard"}),
        );
        change(s, c);
    })
}

/// The coder example with run pods on (the chart's settings and `standard`), changed by `change`.
pub fn resolved_run_pods(change: impl FnOnce(&mut Value, &mut Value)) -> ResolvedAgent {
    resolved_with("coder", &chart_run_pod_settings(), |s, c| {
        set(
            c,
            "/spec/harness/adam/coder/runPods",
            serde_json::json!({"sizeClass": "standard"}),
        );
        change(s, c);
    })
}

/// An example with changes, run pods settings given, resolved with an owner that has a uid.
pub fn resolved_with(
    name: &str,
    settings: &aap_domain::RunPodSettings,
    change: impl FnOnce(&mut Value, &mut Value),
) -> ResolvedAgent {
    let (mut service, mut config) = example(name);
    change(&mut service, &mut config);
    let service: AgentService = serde_json::from_value(service).unwrap();
    let config: AgentConfig = serde_json::from_value(config).unwrap();
    let owner = owner_handle(
        "agents.vymalo.com/v1alpha1",
        "AgentService",
        service.metadata.name.clone().unwrap(),
        "0b1f3c5e-1111-4222-8333-444455556666",
    );
    let externals = aap_domain::Externals::default().with_run_pods(settings.clone());
    aap_domain::resolve(&service, &config, &externals, owner).unwrap_or_else(|issues| {
        panic!(
            "the example does not resolve: {}",
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ")
        )
    })
}

/// A values file of `tools/adam-parity/cases`, parsed.
pub fn parity_case_values(case: &str) -> Value {
    let path = repo_root()
        .join("tools/adam-parity/cases")
        .join(format!("{case}.yaml"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Helm's merge of values: a map is merged key by key, `null` removes the key, anything else replaces.
pub fn helm_merge(base: &mut Value, over: &Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                if v.is_null() {
                    b.remove(k);
                } else if let Some(existing) = b.get_mut(k) {
                    helm_merge(existing, v);
                } else {
                    b.insert(k.clone(), v.clone());
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// The settings the operator side of a parity case runs with: the operator chart's defaults, changed by the `runPods` block of
/// the case's values for the adam-rs chart, whose keys the operator chart's keep (the same function as
/// `crates/domain/tests/common`'s). Returns the settings and the name of the size class the config picks.
pub fn run_pod_settings_of(case: &str) -> (aap_domain::RunPodSettings, String) {
    let case_values = parity_case_values(case);
    let rp = &case_values["runPods"];
    let mut settings = chart_run_pod_settings_value();
    let mut over = serde_json::Map::new();
    for key in [
        "image",
        "container",
        "cargoBuildJobs",
        "extraEnv",
        "quota",
        "idleSecs",
        "readyTimeoutSecs",
        "waitSecs",
        "nodeSelector",
        "tolerations",
        "nodeAffinity",
        "networkPolicy",
        "egressProxy",
    ] {
        if let Some(v) = rp.get(key) {
            over.insert(key.to_owned(), v.clone());
        }
    }
    if let Some(v) = rp["admissionPolicy"].get("extraAllowedImages") {
        over.insert("extraAllowedImages".into(), v.clone());
    }
    if let Some(v) = rp.get("priorityClassName") {
        over.insert("priorityClass".into(), v.clone());
    }
    let mut class = "standard".to_owned();
    if let Some(resources) = rp.get("resources") {
        over.insert("classes".into(), serde_json::json!({ "tuned": resources }));
        class = "tuned".into();
    }
    helm_merge(&mut settings, &Value::Object(over));
    (run_pod_settings(settings), class)
}
