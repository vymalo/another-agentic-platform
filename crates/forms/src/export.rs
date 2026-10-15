//! The YAML export (§60a, `GET /v1/agents/{name}/yaml`): View YAML and Export YAML.
//!
//! "Both objects as YAML, without `status`, `managedFields` and server-set metadata." The export is a
//! **whitelist**, not a list of what to take out: the object is rebuilt from `apiVersion`, `kind`, a few
//! members of `metadata` and the `spec`, so a member that a future Kubernetes adds to `metadata`, and the
//! whole of `status`, cannot reach it. Moving an agent to GitOps is "Export YAML, commit it, sync" (AD-031).
//!
//! What stays of `metadata`: `name`, `namespace`, the labels and the annotations a person put. What goes:
//! `uid`, `resourceVersion`, `generation`, `creationTimestamp`, `deletionTimestamp`, `managedFields`,
//! `ownerReferences`, `finalizers` and the rest, and, of the annotations, the ones a tool writes
//! (`kubectl.kubernetes.io/…`, which holds a copy of the last apply, and Argo CD's tracking id, which belongs to
//! the application that synced it). The `app.kubernetes.io/managed-by` label stays: an exported object is shown
//! as it is, and Argo CD adds its annotation, which makes the dashboard stop writing it (AD-031).

use std::collections::BTreeMap;

use aap_ports::{ARGO_TRACKING_ANNOTATION, Agent, Stored};
use serde::Serialize;
use serde_json::Value;

/// The export failed. It cannot, for a `Stored` the types of this workspace made; it is a `Result` so that
/// nothing is hidden if it ever does.
#[derive(Debug, thiserror::Error)]
#[error("cannot write YAML: {0}")]
pub struct ExportError(String);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Document<'a> {
    api_version: String,
    kind: &'static str,
    metadata: Metadata<'a>,
    spec: &'a Value,
}

#[derive(Serialize)]
struct Metadata<'a> {
    name: &'a str,
    namespace: &'a str,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    labels: &'a BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    annotations: BTreeMap<&'a str, &'a str>,
}

/// Whether an annotation is written by a tool and not by a person.
fn by_a_tool(key: &str) -> bool {
    key.starts_with("kubectl.kubernetes.io/") || key == ARGO_TRACKING_ANNOTATION
}

/// One object as one YAML document: `apiVersion`, `kind`, `metadata` and `spec`.
///
/// # Errors
///
/// [`ExportError`], which a `Stored` made of JSON does not give.
pub fn export_object(object: &Stored) -> Result<String, ExportError> {
    let document = Document {
        api_version: format!("{}/{}", aap_api::GROUP, aap_api::VERSION),
        kind: object.kind.as_str(),
        metadata: Metadata {
            name: &object.name,
            namespace: &object.namespace,
            labels: &object.labels,
            annotations: object
                .annotations
                .iter()
                .filter(|(k, _)| !by_a_tool(k))
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect(),
        },
        spec: &object.spec,
    };
    serde_yaml::to_string(&document).map_err(|e| ExportError(e.to_string()))
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
