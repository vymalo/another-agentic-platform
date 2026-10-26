//! Loading the examples and changing them, for the tests of `aap-domain`.
#![allow(dead_code)] // each test file uses part of it

use std::path::{Path, PathBuf};

use aap_api::{AgentConfig, AgentService};
use aap_domain::{ConfigIssue, Externals, ResolvedAgent, RunPodSettings};
use aap_ports::OwnerHandle;
use serde::Deserialize;
use serde_json::Value;

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The documents of a YAML file, as JSON.
pub fn documents(path: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_yaml::Deserializer::from_str(&text)
        .map(|d| Value::deserialize(d).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .collect()
}

/// An example as the two JSON documents it holds: the service, then the config.
pub fn example(name: &str) -> (Value, Value) {
    let docs = documents(&repo_root().join("examples").join(format!("{name}.yaml")));
    let mut service = None;
    let mut config = None;
    for d in docs {
        match d["kind"].as_str() {
            Some("AgentService") => service = Some(d),
            Some("AgentConfig") => config = Some(d),
            // The shared objects of the admin dashboard (§60a): `shared_objects` reads them.
            Some("ModelEndpoint" | "ToolProvider") => {}
            other => panic!("unexpected kind {other:?}"),
        }
    }
    (
        service.expect("an AgentService"),
        config.expect("an AgentConfig"),
    )
}

pub fn typed(service: &Value, config: &Value) -> (AgentService, AgentConfig) {
    (
        serde_json::from_value(service.clone()).expect("an AgentService"),
        serde_json::from_value(config.clone()).expect("an AgentConfig"),
    )
}

/// Set the value at a JSON pointer, creating the objects on the way.
pub fn set(doc: &mut Value, pointer: &str, value: Value) {
    let mut at = doc;
    let parts: Vec<&str> = pointer.split('/').skip(1).collect();
    for (i, part) in parts.iter().enumerate() {
        let part = part.replace("~1", "/").replace("~0", "~");
        if i + 1 == parts.len() {
            match at {
                Value::Array(a) => {
                    let i = part.parse::<usize>().unwrap();
                    if i == a.len() {
                        a.push(value);
                    } else {
                        a[i] = value;
                    }
                }
                _ => {
                    at[part.as_str()] = value;
                }
            }
            return;
        }
        if at.get(part.as_str()).is_none() && !at.is_array() {
            at[part.as_str()] = Value::Object(serde_json::Map::new());
        }
        at = match at {
            Value::Array(a) => &mut a[part.parse::<usize>().unwrap()],
            _ => &mut at[part.as_str()],
        };
    }
}

/// Remove the value at a JSON pointer, if it is there.
pub fn remove(doc: &mut Value, pointer: &str) {
    let (parent, last) = pointer.rsplit_once('/').expect("a pointer");
    if let Some(Value::Object(m)) = doc.pointer_mut(parent) {
        m.remove(&last.replace("~1", "/").replace("~0", "~"));
    }
}

/// A resolved example with changes: `change` edits the two documents before they are typed.
pub fn resolved(
    name: &str,
    change: impl FnOnce(&mut Value, &mut Value),
) -> Result<ResolvedAgent, Vec<ConfigIssue>> {
    resolved_with(name, &Externals::default(), change)
}

/// The same with the objects the config may reference, and the operator's default image.
pub fn resolved_with(
    name: &str,
    externals: &Externals,
    change: impl FnOnce(&mut Value, &mut Value),
) -> Result<ResolvedAgent, Vec<ConfigIssue>> {
    let (mut service, mut config) = example(name);
    change(&mut service, &mut config);
    let (service, config) = typed(&service, &config);
    aap_domain::resolve(
        &service,
        &config,
        externals,
        OwnerHandle::new("owner-token"),
    )
}

pub fn must_resolve(name: &str, change: impl FnOnce(&mut Value, &mut Value)) -> ResolvedAgent {
    resolved(name, change).unwrap_or_else(|issues| {
        panic!(
            "expected the example to resolve, got:\n{}",
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

pub fn must_resolve_with(
    name: &str,
    externals: &Externals,
    change: impl FnOnce(&mut Value, &mut Value),
) -> ResolvedAgent {
    resolved_with(name, externals, change).unwrap_or_else(|issues| {
        panic!(
            "expected the example to resolve, got:\n{}",
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        )
    })
}

/// The issues of an example with changes and some externals, which must be refused.
pub fn issues_with(
    name: &str,
    externals: &Externals,
    change: impl FnOnce(&mut Value, &mut Value),
) -> Vec<ConfigIssue> {
    match resolved_with(name, externals, change) {
        Ok(_) => panic!("expected the example to be refused"),
        Err(issues) => issues,
    }
}

/// The issues of an example with changes, which must be refused.
pub fn issues(name: &str, change: impl FnOnce(&mut Value, &mut Value)) -> Vec<ConfigIssue> {
    match resolved(name, change) {
        Ok(_) => panic!("expected the example to be refused"),
        Err(issues) => issues,
    }
}

/// Asserts that an issue is about `field_part` and says `message_part`.
pub fn assert_issue(issues: &[ConfigIssue], field_part: &str, message_part: &str) {
    assert!(
        issues
            .iter()
            .any(|i| i.field.contains(field_part) && i.message.contains(message_part)),
        "no issue about {field_part:?} saying {message_part:?} in:\n{}",
        issues
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// The `ModelEndpoint`s and `ToolProvider`s of an example file, as the `Externals` a composition root
/// would read for its config.
pub fn shared_objects(name: &str) -> Externals {
    let mut ext = Externals::default();
    for d in documents(&repo_root().join("examples").join(format!("{name}.yaml"))) {
        match d["kind"].as_str() {
            Some("ModelEndpoint") => {
                ext = ext.with_model_endpoint(serde_json::from_value(d).expect("a ModelEndpoint"));
            }
            Some("ToolProvider") => {
                ext = ext.with_tool_provider(serde_json::from_value(d).expect("a ToolProvider"));
            }
            _ => {}
        }
    }
    ext
}

// ------------------------------------------------------------------------------------ run pods

/// `deploy/operator/values.yaml`, parsed.
pub fn operator_chart_values() -> Value {
    let path = repo_root().join("deploy/operator/values.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_yaml::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
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

/// The settings the operator chart ships (its `runPods` block and `runPodClasses`, `deploy/operator/values.yaml`) as the
/// file the chart renders for the operator, with the names of the cluster-scoped objects the chart would make.
pub fn chart_run_pod_settings() -> Value {
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

/// The settings the operator side of a parity case runs with: the operator chart's defaults, changed by the
/// `runPods` block of the case's values for the adam-rs chart, whose keys the operator chart's keep (`resources` is
/// the class `tuned`; `admissionPolicy.extraAllowedImages` is `extraAllowedImages`; `priorityClassName` names the
/// class the chart is told to use). Returns the settings and the name of the size class the config picks.
pub fn run_pod_settings_of(case: &str) -> (RunPodSettings, String) {
    let case_values = parity_case_values(case);
    let rp = &case_values["runPods"];
    let mut settings = chart_run_pod_settings();
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
    let settings: RunPodSettings =
        serde_json::from_value(settings).expect("settings of the operator chart");
    if let Err(problems) = settings.validate() {
        panic!("the settings of {case} are refused: {problems:?}");
    }
    (settings, class)
}
