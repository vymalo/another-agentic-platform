//! From the documents of a file to the forms the Platform API takes (§60b, `apply`).
//!
//! The API takes **forms**, not custom resources, and fills what no form says (the deployment's part: the store, the security
//! context, the network peers...) from its own defaults on a create and keeps what the object holds on a replace. So a file is read
//! the way the dashboard reads an object: each document becomes an [`aap_ports::Stored`], and `aap_forms::load_agent` and the
//! `load` of the model and tool server forms say whether a form holds all of it. **An object a form cannot say is refused with one
//! line per field**, as the dashboard refuses it (`yaml-only`), never sent: sending its form would drop the rest.
//!
//! What the API would drop silently is refused too: a label other than the dashboard's, **any** annotation (the API writes none),
//! a member of `metadata` the API does not write. `aapctl export` leaves out exactly what `apply` refuses ([`strip_what_apply_refuses`]),
//! so that an export is read back unchanged.

use std::collections::BTreeMap;

use aap_forms::{AgentKind, ModelEndpointForm, Representation, ToolServerForm, load_agent};
use aap_platform_client::{Collection, clean};
use aap_ports::{
    Agent, Kind, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, ResourceVersion, Stored, config_name,
};
use serde_json::Value;

use crate::failure::Failure;
use crate::load::Object;

/// Whether the API writes this label: the dashboard's own, and no other. What it does not write it would drop from the object.
fn label_is_written(key: &str, value: Option<&str>) -> bool {
    key == MANAGED_BY_LABEL && value == Some(MANAGED_BY_DASHBOARD)
}

/// The labels and annotations a document of an export holds that `apply` would refuse (the API writes the dashboard's label and no
/// annotation), taken out of it; their keys, never their values, are returned. For `export`.
pub fn strip_what_apply_refuses(doc: &mut Value) -> Vec<String> {
    let mut removed = Vec::new();
    let Some(meta) = doc.get_mut("metadata").and_then(Value::as_object_mut) else {
        return removed;
    };
    if let Some(labels) = meta.get_mut("labels").and_then(Value::as_object_mut) {
        let drop: Vec<String> = labels
            .iter()
            .filter(|(k, v)| !label_is_written(k, v.as_str()))
            .map(|(k, _)| k.clone())
            .collect();
        for key in drop {
            labels.remove(&key);
            removed.push(format!("label {key}"));
        }
        if labels.is_empty() {
            meta.remove("labels");
        }
    }
    if let Some(annotations) = meta.get("annotations").and_then(Value::as_object) {
        removed.extend(annotations.keys().map(|k| format!("annotation {k}")));
        meta.remove("annotations");
    }
    removed
}

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

/// The namespace one document of a file says.
#[derive(Clone, Debug)]
pub struct DocNamespace {
    /// `AgentConfig/chat`.
    pub document: String,
    /// `metadata.namespace`.
    pub namespace: String,
    /// The file it came from.
    pub file: String,
}

/// One thing to apply: a form, and for an agent the deployment's part its objects hold.
#[derive(Clone, Debug)]
pub struct Item {
    /// What it is.
    pub resource: Resource,
    /// Its name.
    pub name: String,
    /// The `metadata.namespace` of each document that says one (an agent has two documents). Each must be the API's.
    pub namespaces: Vec<DocNamespace>,
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
                    if !label_is_written(label, v.as_str()) {
                        found.push(format!(
                            "metadata.labels: label {} would be dropped: the API writes only {MANAGED_BY_LABEL}: {MANAGED_BY_DASHBOARD}; remove it",
                            clean(label)
                        ));
                    }
                }
            }
            "annotations" => {
                for (annotation, _) in value.as_object().into_iter().flatten() {
                    found.push(format!(
                        "metadata.annotations: annotation {} would be dropped: the API writes none; remove it",
                        clean(annotation)
                    ));
                }
            }
            other => found.push(format!(
                "metadata.{}: would be dropped: the API writes only the name, the namespace and its own label; remove it",
                clean(other)
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
    // The service that named each config: a config is one agent's.
    let mut used: Vec<Option<String>> = vec![None; configs.len()];

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

        let mut paired: Option<&Object> = None;
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
                let named: Vec<(usize, &&Object)> = wanted
                    .as_deref()
                    .map(|w| {
                        configs
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| c.name == w)
                            .collect()
                    })
                    .unwrap_or_default();
                // The same namespace first; a document that says none is in the API's, and is checked against it later.
                let found = named
                    .iter()
                    .find(|(_, c)| c.namespace == object.namespace)
                    .or_else(|| {
                        named
                            .iter()
                            .find(|(_, c)| c.namespace.is_none() || object.namespace.is_none())
                    });
                let Some(&(index, config)) = found else {
                    if let Some((_, other)) = named.first() {
                        return Err(Failure::usage(format!(
                            "{label}: its AgentService is in the namespace {:?} and its AgentConfig {:?} in {:?}: an agent's two objects are in one namespace ({})",
                            object.namespace.as_deref().map(clean).unwrap_or_default(),
                            clean(&other.name),
                            other.namespace.as_deref().map(clean).unwrap_or_default(),
                            object.file
                        )));
                    }
                    return Err(Failure::usage(format!(
                        "{label}: names the AgentConfig {:?}, which is not among the documents ({})",
                        wanted.unwrap_or_default(),
                        object.file
                    )));
                };
                if let Some(first) = &used[index] {
                    return Err(Failure::usage(format!(
                        "{label} and {first} both name the AgentConfig {:?}: a config is one agent's ({})",
                        clean(&config.name),
                        object.file
                    )));
                }
                used[index] = Some(label.clone());
                here.extend(dropped_metadata(config));
                paired = Some(*config);
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
            // A usage error, not a refusal of content: the file says something the API has no place for.
            let mut failure = Failure::usage(format!(
                "{label}: metadata the API would drop ({})",
                object.file
            ));
            for line in here {
                failure = failure.also(line);
            }
            refusals.push(failure);
        }
        if let Some(form) = form {
            let mut namespaces = Vec::new();
            if let Some(namespace) = &object.namespace {
                namespaces.push(DocNamespace {
                    document: format!("{}/{}", object.kind, object.name),
                    namespace: namespace.clone(),
                    file: object.file.clone(),
                });
            }
            if let Some(config) = paired.filter(|c| c.namespace.is_some()) {
                namespaces.push(DocNamespace {
                    document: format!("AgentConfig/{}", config.name),
                    namespace: config.namespace.clone().unwrap_or_default(),
                    file: config.file.clone(),
                });
            }
            items.push(Item {
                resource,
                name: object.name.clone(),
                namespaces,
                form,
                deployment,
                agent_kind,
            });
        }
    }
    for (config, used) in configs.iter().zip(&used) {
        if used.is_none() {
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
