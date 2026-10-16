//! The YAML export (§60a, `GET /v1/agents/{name}/yaml`): View YAML and Export YAML.
//!
//! "Both objects as YAML, without `status`, `managedFields` and server-set metadata." The export is a
//! **whitelist**, not a list of what to take out: the object is rebuilt from `apiVersion`, `kind`, a few
//! members of `metadata` and the `spec`, so a member that a future Kubernetes adds to `metadata`, and the
//! whole of `status`, cannot reach it. Moving an agent to GitOps is "Export YAML, commit it, sync" (AD-031).
//!
//! What stays of `metadata`: `name`, `namespace`, the labels the person put (a key with no prefix: the convention of
//! Kubernetes is that a tool must prefix its own) and the platform's (`agents.vymalo.com/…`), and the
//! `app.kubernetes.io/managed-by` label when it says the dashboard; of the annotations, only the platform's. What
//! goes: `uid`, `resourceVersion`, `generation`, `creationTimestamp`, `deletionTimestamp`, `managedFields`,
//! `ownerReferences`, `finalizers` and the rest, and every label and annotation a tool wrote
//! (`kubectl.kubernetes.io/…`, which holds a copy of the last apply, Argo CD's tracking id and `instance` label, Flux's
//! and Helm's). **Labels and annotations are filtered by an allow-list, not by a list of what to take out**: there is no
//! blacklist in this crate, so a tool that arrives next year leaks nothing. The `managed-by` label stays: an exported
//! object is shown as it is, and Argo CD adds its annotation, which makes the dashboard stop writing it (AD-031).
//!
//! The YAML is written by `src/yaml.rs`, not by a library: a string that YAML 1.1 reads as a boolean, a number or a
//! timestamp (`yes`, `no`, `on`, `off`, `y`, `n`, `12:30`, `1_000`, `2026-10-16`) is quoted.

use std::collections::BTreeMap;

use aap_ports::{Agent, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, Stored};
use serde_json::{Value, json};

use crate::yaml;

/// The export failed. It cannot, for a `Stored` the types of this workspace made; it is a `Result` so that
/// nothing is hidden if it ever does.
#[derive(Debug, thiserror::Error)]
#[error("cannot write YAML: {0}")]
pub struct ExportError(String);

/// The prefix of the annotations and labels the platform itself puts on its objects: the one thing besides
/// the person's own labels that an export keeps.
const PLATFORM_PREFIX: &str = "agents.vymalo.com/";

/// Whether the export keeps a label. **An allow-list**: a label with a prefix is some tool's by the convention of
/// Kubernetes (an unprefixed key is "private to the user"; "automated system components … which add labels to
/// end-user objects must specify a prefix"), so what is kept is the person's own labels (no prefix), the
/// platform's, and `app.kubernetes.io/managed-by` when it says the dashboard. Every other label (Argo CD's and
/// Flux's tracking, Helm's, `app.kubernetes.io/instance`, whatever the next tool adds) is out.
fn keeps_label(key: &str, value: &str) -> bool {
    !key.contains('/')
        || key.starts_with(PLATFORM_PREFIX)
        || (key == MANAGED_BY_LABEL && value == MANAGED_BY_DASHBOARD)
}

/// Whether the export keeps an annotation. **An allow-list**, and a short one: the platform's own. An annotation
/// is where tools keep copies of the object (`kubectl.kubernetes.io/last-applied-configuration`), their
/// tracking ids and their state, and a person's note has no prefix a tool could not also use, so none of
/// them is exported: a GitOps export needs the spec, the name and the labels, and a note is added in the
/// repository where the object goes.
fn keeps_annotation(key: &str) -> bool {
    key.starts_with(PLATFORM_PREFIX)
}

fn kept<'a>(
    map: &'a BTreeMap<String, String>,
    keeps: impl Fn(&str, &str) -> bool,
) -> BTreeMap<&'a str, &'a str> {
    map.iter()
        .filter(|(k, v)| keeps(k, v))
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect()
}

/// One object as one YAML document: `apiVersion`, `kind`, `metadata` and `spec`.
///
/// # Errors
///
/// [`ExportError`], which a `Stored` made of JSON does not give.
pub fn export_object(object: &Stored) -> Result<String, ExportError> {
    let mut metadata = serde_json::Map::new();
    metadata.insert("name".into(), json!(object.name));
    metadata.insert("namespace".into(), json!(object.namespace));
    let labels = kept(&object.labels, keeps_label);
    if !labels.is_empty() {
        metadata.insert("labels".into(), json!(labels));
    }
    let annotations = kept(&object.annotations, |key, _| keeps_annotation(key));
    if !annotations.is_empty() {
        metadata.insert("annotations".into(), json!(annotations));
    }
    let document: Value = json!({
        "apiVersion": format!("{}/{}", aap_api::GROUP, aap_api::VERSION),
        "kind": object.kind.as_str(),
        "metadata": metadata,
        "spec": object.spec,
    });
    Ok(yaml::to_string(&document))
}

/// The two objects of an agent as YAML: the `AgentConfig`, then the `AgentService`, in the order the Platform API
/// applies them, as two documents. The config is left out when the service names one that is not there.
///
/// # Errors
///
/// As [`export_object`].
pub fn export_agent(agent: &Agent) -> Result<String, ExportError> {
    let mut out = String::new();
    if let Some(config) = &agent.config {
        out.push_str(&export_object(config)?);
        out.push_str("---\n");
    }
    out.push_str(&export_object(&agent.service)?);
    Ok(out)
}
