//! From the documents of a file to the forms the Platform API takes (§60b, `apply`).
//!
//! The API takes **forms**, not custom resources, and fills what no form says (the deployment's part: the store, the security
//! context, the network peers...) from its own defaults on a create and keeps what the object holds on a replace. So a file is read
//! the way the dashboard reads an object: each document becomes an [`aap_ports::Stored`], and `aap_forms::load_agent` and the
//! `load` of the model and tool server forms say whether a form holds all of it. **An object a form cannot say is refused with one
//! line per field**, as the dashboard refuses it (`yaml-only`), never sent: sending its form would drop the rest.
//!
//! What the API would drop silently is refused too: a label other than the dashboard's, an annotation that is not the platform's,
//! a member of `metadata` the API does not write.

use std::collections::BTreeMap;

use aap_forms::{AgentKind, ModelEndpointForm, Representation, ToolServerForm, load_agent};
use aap_platform_client::Collection;
use aap_ports::{
    Agent, Kind, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, ResourceVersion, Stored, config_name,
};
use serde_json::Value;

use crate::failure::Failure;
use crate::load::Object;

/// The prefix of the annotations the platform itself writes: the one kind of annotation a file may carry (an export keeps them).
const PLATFORM_PREFIX: &str = "agents.vymalo.com/";

/// What `apply` writes, in the order it writes: what is named comes before what names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Resource {
    /// A `ModelEndpoint`.
    Model,
    /// A `ToolProvider`.
    ToolServer,
    /// An `AgentService` with its `AgentConfig`.
    Agent,
}

impl Resource {
    /// The noun a person types and reads: `agent`, `model`, `tool-server`.
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Model => "model",
            Self::ToolServer => "tool-server",
        }
    }

    /// The API's collection.
    pub const fn collection(self) -> Collection {
        match self {
            Self::Agent => Collection::Agents,
            Self::Model => Collection::Models,
            Self::ToolServer => Collection::ToolServers,
        }
    }

    /// The kind of the object whose name an item of this resource has.
    pub const fn kind_name(self) -> &'static str {
        match self {
            Self::Agent => "AgentService",
            Self::Model => "ModelEndpoint",
            Self::ToolServer => "ToolProvider",
        }
    }
}

/// One thing to apply: a form, and for an agent the deployment's part its objects hold.
#[derive(Clone, Debug)]
pub struct Item {
    /// What it is.
    pub resource: Resource,
    /// Its name.
    pub name: String,
    /// The file it came from.
    pub file: String,
    /// `metadata.namespace`, if the file says.
    pub namespace: Option<String>,
    /// The form, as the API's body.
    pub form: Value,
    /// An agent's deployment part, as the API's detail says it. `None` for a model or a tool server.
    pub deployment: Option<Value>,
    /// An agent's kind, which picks the defaults of a create.
    pub agent_kind: Option<AgentKind>,
}

impl Item {
    /// `agent/chat`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.resource.noun(), self.name)
    }
}

fn stored(object: &Object, kind: Kind) -> Stored {
    let strings = |pointer: &str| -> BTreeMap<String, String> {
        object
            .doc
            .pointer(pointer)
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                    .collect()
            })
            .unwrap_or_default()
    };
    Stored {
        kind,
        namespace: object.namespace.clone().unwrap_or_default(),
        name: object.name.clone(),
        version: ResourceVersion::new(""),
        uid: String::new(),
        generation: 1,
        labels: strings("/metadata/labels"),
        annotations: strings("/metadata/annotations"),
        spec: object.doc.get("spec").cloned().unwrap_or(Value::Null),
        status: None,
        deleting: false,
    }
}

/// What the API would not write of the object's `metadata`, one sentence each.
fn dropped_metadata(object: &Object) -> Vec<String> {
    let mut found = Vec::new();
    let Some(meta) = object.doc.get("metadata").and_then(Value::as_object) else {
        return found;
    };
    for (key, value) in meta {
        match key.as_str() {
            "name" | "namespace" => {}
            "labels" => {
                for (label, v) in value.as_object().into_iter().flatten() {
                    let dashboard = label == MANAGED_BY_LABEL && v.as_str() == Some(MANAGED_BY_DASHBOARD);
                    if !dashboard {
                        found.push(format!(
                            "metadata.labels: label {label} would be dropped: the API writes only {MANAGED_BY_LABEL}: {MANAGED_BY_DASHBOARD}; remove it"
                        ));
                    }
                }
            }
            "annotations" => {
                for (annotation, _) in value.as_object().into_iter().flatten() {
                    if !annotation.starts_with(PLATFORM_PREFIX) {
                        found.push(format!(
                            "metadata.annotations: annotation {annotation} would be dropped: the API writes none; remove it"
                        ));
                    }
                }
            }
            other => found.push(format!(
                "metadata.{other}: would be dropped: the API writes only the name, the namespace and its own label; remove it"
            )),
        }
    }
    found
}

fn lines_of_yaml_only(label: &str, file: &str, reasons: &[aap_forms::Unrepresentable]) -> Failure {
    let mut failure = Failure::new(
        1,
        format!(
            "{label}: yaml-only: no form says all of this object, so sending its form would lose the rest; change it as YAML ({file})"
        ),
    );
    for reason in reasons {
        failure = failure.also(reason.to_string());
    }
    failure
}

/// The forms of everything in `objects`, in the order they are written: models, tool servers, agents (each in the order given).
///
/// # Errors
///
/// Exit 2 when a service names a config that is not among the documents or a config no service names; exit 1, with every refusal
/// at once, for an object that no form says or whose metadata the API would drop.
pub fn prepare(objects: &[Object]) -> Result<Vec<Item>, Failure> {
    let mut items: Vec<Item> = Vec::new();
    let mut refusals: Vec<Failure> = Vec::new();

    let configs: Vec<&Object> = objects.iter().filter(|o| o.kind == "AgentConfig").collect();
    let mut used = vec![false; configs.len()];

    for object in objects {
        let (resource, kind) = match object.kind {
            "ModelEndpoint" => (Resource::Model, Kind::ModelEndpoint),
            "ToolProvider" => (Resource::ToolServer, Kind::ToolProvider),
            "AgentService" => (Resource::Agent, Kind::AgentService),
            _ => continue,
        };
        let label = format!("{}/{}", resource.noun(), object.name);
        let mut here: Vec<String> = dropped_metadata(object);
        let service = stored(object, kind);

        let (form, deployment, agent_kind) = match resource {
            Resource::Model | Resource::ToolServer => {
                let (representation, reasons) = if resource == Resource::Model {
                    let loaded = ModelEndpointForm::load(&service);
                    match loaded.representation {
                        Representation::Form(f) => (
                            Some(serde_json::to_value(f).unwrap_or(Value::Null)),
                            Vec::new(),
                        ),
                        Representation::YamlOnly(r) => (None, r),
                    }
                } else {
                    let loaded = ToolServerForm::load(&service);
                    match loaded.representation {
                        Representation::Form(f) => (
                            Some(serde_json::to_value(f).unwrap_or(Value::Null)),
                            Vec::new(),
                        ),
                        Representation::YamlOnly(r) => (None, r),
                    }
                };
                if let Some(form) = representation {
                    (Some(form), None, None)
                } else {
                    refusals.push(lines_of_yaml_only(&label, &object.file, &reasons));
                    (None, None, None)
                }
            }
            Resource::Agent => {
                let wanted = config_name(&service.spec).map(str::to_owned);
                let found = wanted.as_deref().and_then(|w| {
                    configs.iter().enumerate().find(|(_, c)| {
                        c.name == w
                            && (c.namespace == object.namespace
                                || c.namespace.is_none()
                                || object.namespace.is_none())
                    })
                });
                let Some((index, config)) = found else {
                    return Err(Failure::usage(format!(
                        "{label}: names the AgentConfig {:?}, which is not among the documents ({})",
                        wanted.unwrap_or_default(),
                        object.file
                    )));
                };
                used[index] = true;
                here.extend(dropped_metadata(config));
                let agent = Agent {
                    service: service.clone(),
                    config: Some(stored(config, Kind::AgentConfig)),
                };
                match load_agent(&agent).representation {
                    Representation::Form(view) => {
                        let kind = view.kind();
                        (
                            Some(serde_json::to_value(&view.form).unwrap_or(Value::Null)),
                            Some(serde_json::to_value(&view.deployment).unwrap_or(Value::Null)),
                            Some(kind),
                        )
                    }
                    Representation::YamlOnly(reasons) => {
                        refusals.push(lines_of_yaml_only(&label, &object.file, &reasons));
                        (None, None, None)
                    }
                }
            }
        };
        if !here.is_empty() {
            let mut failure = Failure::new(
                1,
                format!("{label}: metadata the API would drop ({})", object.file),
            );
            for line in here {
                failure = failure.also(line);
            }
            refusals.push(failure);
        }
        if let Some(form) = form {
            items.push(Item {
                resource,
                name: object.name.clone(),
                file: object.file.clone(),
                namespace: object.namespace.clone(),
                form,
                deployment,
                agent_kind,
            });
        }
    }
    for (config, used) in configs.iter().zip(&used) {
        if !used {
            return Err(Failure::usage(format!(
                "AgentConfig/{}: no AgentService among the documents names it ({}); an agent is a service and its config",
                config.name, config.file
            )));
        }
    }
    if let Some(all) = Failure::all(refusals) {
        return Err(all);
    }
    items.sort_by_key(|i| i.resource);
    Ok(items)
}

/// The paths at which two JSON documents differ, `a.b.c`: objects are walked, and a list or a scalar that differs is one path.
pub fn differing_paths(a: &Value, b: &Value) -> Vec<String> {
    fn walk(a: &Value, b: &Value, at: &str, out: &mut Vec<String>) {
        match (a, b) {
            (Value::Object(x), Value::Object(y)) => {
                let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
                for key in keys {
                    let next = if at.is_empty() {
                        key.clone()
                    } else {
                        format!("{at}.{key}")
                    };
                    match (x.get(key), y.get(key)) {
                        (Some(p), Some(q)) => walk(p, q, &next, out),
                        _ => out.push(next),
                    }
                }
            }
            _ => {
                if a != b {
                    out.push(at.to_owned());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(a, b, "", &mut out);
    out
}
