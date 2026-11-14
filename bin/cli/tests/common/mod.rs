//! Helpers of the tests of `aapctl`: run the binary, read and write documents.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;
use serde_json::{Value, json};

pub struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn example(name: &str) -> PathBuf {
    root().join("examples").join(name)
}

pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("aapctl")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn aapctl(args: &[&str], stdin: Option<&str>) -> Out {
    let mut child = Command::new(env!("CARGO_BIN_EXE_aapctl"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    Out {
        code: out.status.code().unwrap(),
        stdout: String::from_utf8(out.stdout).unwrap(),
        stderr: String::from_utf8(out.stderr).unwrap(),
    }
}

pub fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

pub fn documents(text: &str) -> Vec<Value> {
    serde_yaml::Deserializer::from_str(text)
        .map(|d| Value::deserialize(d).unwrap())
        .filter(|v| !v.is_null())
        .collect()
}

pub fn read_docs(path: &Path) -> Vec<Value> {
    documents(&std::fs::read_to_string(path).unwrap())
}

pub fn write_docs(path: &Path, docs: &[Value]) {
    let text = docs
        .iter()
        .map(|d| serde_yaml::to_string(d).unwrap())
        .collect::<Vec<_>>()
        .join("---\n");
    std::fs::write(path, text).unwrap();
}

pub fn set(doc: &mut Value, pointer: &str, value: Value) {
    fn go(at: &mut Value, parts: &[&str], value: Value) {
        let (first, rest) = parts.split_first().unwrap();
        if let Value::Array(items) = at {
            let index: usize = first.parse().unwrap();
            if rest.is_empty() {
                items[index] = value;
            } else {
                go(&mut items[index], rest, value);
            }
            return;
        }
        if rest.is_empty() {
            at[*first] = value;
            return;
        }
        if at.get(*first).is_none() {
            at[*first] = json!({});
        }
        go(&mut at[*first], rest, value);
    }
    let parts: Vec<&str> = pointer.split('/').skip(1).collect();
    go(doc, &parts, value);
}

pub const IMAGE: &str = "ghcr.io/example/coder:sha-0000000";

/// The settings the operator chart ships for run pods, as the file the chart renders for the operator
/// (`crates/runtime-kubernetes/tests/common::chart_run_pod_settings_value`).
pub fn chart_run_pod_settings(dir: &Path) -> PathBuf {
    let text = std::fs::read_to_string(root().join("deploy/operator/values.yaml")).unwrap();
    let values: Value = serde_yaml::from_str(&text).unwrap();
    let mut settings = values["runPods"].clone();
    let object = settings.as_object_mut().unwrap();
    object.remove("enabled");
    object.remove("priorityClassName");
    object.insert("classes".into(), values["runPodClasses"].clone());
    object.insert("priorityClass".into(), json!("coder-run"));
    object.insert("role".into(), json!("coder-run-pods"));
    let path = dir.join("run-pods.yaml");
    std::fs::write(&path, serde_yaml::to_string(&settings).unwrap()).unwrap();
    path
}

/// The invalid example with the valid objects it needs, as the domain's own test pairs them: the coder's service or config
/// around it, or a config that names a shared object.
pub fn invalid_with_partner(invalid: &Path, dir: &Path) -> PathBuf {
    let mut docs = read_docs(&example("coder.yaml"));
    let mut doc = read_docs(invalid).remove(0);
    let kind = doc["kind"].as_str().unwrap().to_owned();
    let ns = "another-agentic-system";
    let mut shared = None;
    match kind.as_str() {
        "AgentService" => {
            set(&mut doc, "/metadata/name", json!("coder"));
            set(&mut doc, "/metadata/namespace", json!(ns));
            set(&mut doc, "/spec/configRef/name", json!("coder"));
            docs.retain(|d| d["kind"] != "AgentService");
        }
        "AgentConfig" => {
            set(&mut doc, "/metadata/name", json!("coder"));
            set(&mut doc, "/metadata/namespace", json!(ns));
            docs.retain(|d| d["kind"] != "AgentConfig");
        }
        "ModelEndpoint" | "ToolProvider" => {
            set(&mut doc, "/metadata/name", json!("invalid"));
            set(&mut doc, "/metadata/namespace", json!(ns));
            shared = Some(kind.clone());
        }
        other => panic!("{other}"),
    }
    if let Some(shared) = shared {
        for d in &mut docs {
            if d["kind"] != "AgentConfig" {
                continue;
            }
            if shared == "ModelEndpoint" {
                d["spec"]["model"]
                    .as_object_mut()
                    .unwrap()
                    .remove("baseUrl");
                d["spec"]["model"]
                    .as_object_mut()
                    .unwrap()
                    .remove("apiKeySecretRef");
                set(d, "/spec/model/endpointRef", json!({"name": "invalid"}));
            } else {
                set(
                    d,
                    "/spec/tools/mcpServers",
                    json!({"search": {"providerRef": {"name": "invalid"}}}),
                );
            }
        }
    }
    docs.push(doc);
    let path = dir.join(invalid.file_name().unwrap());
    write_docs(&path, &docs);
    path
}
