//! What the API answers: the objects of the ports as the dashboard's screens read them.
//!
//! Every answer is an object, never a bare list (a list at the top of a JSON document can be extended by nothing), with
//! camelCase members. The forms are `aap-forms`' own types on the wire; this module adds the state of an agent, which is the
//! page's lifecycle (§60a, *An agent's lifecycle*) read from the objects, and the agents that use a shared object.

use std::collections::BTreeMap;

use aap_api::{AgentServiceStatus, ServiceState};
use aap_forms::{AgentKind, AgentView, Loaded, Representation, load_agent};
use aap_ports::{Agent, ObjectsError, Owner, SharedKind, Stored, config_references};
use serde::Serialize;
use serde_json::Value;

/// Where an agent is in the lifecycle the Agents screen draws. `InChat` is the page's own check (the orchestrator's list)
/// and is not here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    /// A change is saved and the operator has not seen it yet: the generation is ahead of `observedGeneration`.
    Saving,
    /// The operator did not apply the desired state (a referent is missing, the config is invalid).
    Blocked,
    /// Applied, and the runtime is not ready or has an issue.
    Degraded,
    /// The runtime is ready.
    Ready,
    /// `spec.suspend` is set and the runtime is suspended.
    Suspended,
    /// Being deleted: the operator has not finished with the finalizer.
    Deleting,
}

fn status_of(stored: &Stored) -> Option<AgentServiceStatus> {
    stored
        .status
        .as_ref()
        .and_then(|s| serde_json::from_value(s.clone()).ok())
}

/// The state of an agent, from its two objects.
pub fn state_of(agent: &Agent) -> AgentState {
    if agent.deleting() {
        return AgentState::Deleting;
    }
    let Some(status) = status_of(&agent.service) else {
        return AgentState::Saving;
    };
    let service_seen = status.observed_generation.unwrap_or(0) >= agent.service.generation;
    // A config edit does not move the service's generation; the operator says which one it resolved.
    let config_seen = match (
        &agent.config,
        status.config.as_ref().and_then(|c| c.observed_generation),
    ) {
        (Some(config), Some(seen)) => seen >= config.generation,
        _ => true,
    };
    if !(service_seen && config_seen) {
        return AgentState::Saving;
    }
    match status.state {
        Some(ServiceState::Ready) => AgentState::Ready,
        Some(ServiceState::Degraded) => AgentState::Degraded,
        Some(ServiceState::Suspended) => AgentState::Suspended,
        Some(ServiceState::Blocked) => AgentState::Blocked,
        None => AgentState::Saving,
    }
}

fn kind_of(agent: &Agent) -> Option<AgentKind> {
    let binary = agent
        .config
        .as_ref()?
        .spec
        .pointer("/harness/adam/binary")?
        .as_str()?;
    match binary {
        "adam-coder" => Some(AgentKind::Coder),
        "adam-agent" => Some(AgentKind::Folder),
        _ => None,
    }
}

fn text(v: &Value, pointer: &str) -> Option<String> {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn texts(v: &Value, pointer: &str) -> Option<Vec<String>> {
    v.pointer(pointer).and_then(Value::as_array).map(|a| {
        a.iter()
            .filter_map(|e| e.as_str().map(str::to_owned))
            .collect()
    })
}

/// A row of the Agents screen.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    /// The agent's name.
    pub name: String,
    /// `registry.title`.
    pub title: Option<String>,
    /// `description`.
    pub description: Option<String>,
    /// A coder or a folder agent; `None` when the config is missing or is neither.
    pub kind: Option<AgentKind>,
    /// Where the agent is in its lifecycle.
    pub state: AgentState,
    /// The conditions the operator wrote (`ConfigResolved`, `StoreReady`, `RuntimeReady`, `Listed`, `Ready`), as they are:
    /// the first false one is the reason to show.
    pub conditions: Vec<Value>,
    /// `spec.suspend`.
    pub suspend: bool,
    /// `registry.tags`.
    pub tags: Vec<String>,
    /// `access.audience`: absent is administrators only.
    pub audience: Option<Vec<String>>,
    /// Who may write it.
    pub managed_by: Owner,
    /// Whether the operator is deleting it.
    pub deleting: bool,
    /// `status.config.digest`.
    pub config_digest: Option<String>,
}

/// The row of `agent`.
pub fn summary(agent: &Agent) -> AgentSummary {
    let spec = &agent.service.spec;
    let status = agent.service.status.as_ref();
    AgentSummary {
        name: agent.name().to_owned(),
        title: text(spec, "/registry/title"),
        description: text(spec, "/description"),
        kind: kind_of(agent),
        state: state_of(agent),
        conditions: status
            .and_then(|s| s.get("conditions"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        suspend: spec
            .pointer("/suspend")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        tags: texts(spec, "/registry/tags").unwrap_or_default(),
        audience: texts(spec, "/access/audience"),
        managed_by: agent.owner(),
        deleting: agent.deleting(),
        config_digest: status.and_then(|s| text(s, "/config/digest")),
    }
}

/// One agent: its row, the form or the reasons there is none, and the status as the operator wrote it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDetail {
    /// The row.
    #[serde(flatten)]
    pub summary: AgentSummary,
    /// The form and the deployment's part its objects hold, or why no form says the objects.
    pub representation: Representation<AgentView>,
    /// `AgentService.status`, whole.
    pub status: Option<Value>,
    /// `AgentConfig.status`, whole.
    pub config_status: Option<Value>,
}

/// The detail of `agent`.
pub fn detail(agent: &Agent) -> AgentDetail {
    let Loaded { representation, .. } = load_agent(agent);
    AgentDetail {
        summary: summary(agent),
        representation,
        status: agent.service.status.clone(),
        config_status: agent.config.as_ref().and_then(|c| c.status.clone()),
    }
}

/// A model or a tool server, with the agents that use it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedDetail<F: Serialize> {
    /// The object's name.
    pub name: String,
    /// `spec.title`.
    pub title: Option<String>,
    /// Who may write it.
    pub managed_by: Owner,
    /// Whether it is being deleted.
    pub deleting: bool,
    /// The agents that name it, in byte order: GitOps's agents count.
    pub used_by: Vec<String>,
    /// The form, or why there is none.
    pub representation: Representation<F>,
}

/// For every shared object of `kind`, the agents that name it.
pub fn users(
    agents: &[Agent],
    kind: SharedKind,
    objects: &[Stored],
) -> BTreeMap<String, Vec<String>> {
    let mut by: BTreeMap<String, Vec<String>> = objects
        .iter()
        .map(|o| (o.name.clone(), Vec::new()))
        .collect();
    for agent in agents {
        let Some(config) = &agent.config else {
            continue;
        };
        let refs = config_references(&config.spec);
        for (name, users) in &mut by {
            if refs.names(kind, name) {
                users.push(agent.name().to_owned());
            }
        }
    }
    for users in by.values_mut() {
        users.sort();
        users.dedup();
    }
    by
}

/// The agents that name one shared object.
pub fn users_of(agents: &[Agent], kind: SharedKind, name: &str) -> Vec<String> {
    let mut names: Vec<String> = agents
        .iter()
        .filter(|a| {
            a.config
                .as_ref()
                .is_some_and(|c| config_references(&c.spec).names(kind, name))
        })
        .map(|a| a.name().to_owned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The `NotOwned` or `Deleting` the port would give for a write to `agent`, found from a read: a write that is
/// refused for these reasons is refused before its form is looked at, as the port would refuse it.
pub fn refusal_of(agent: &Agent) -> Option<ObjectsError> {
    for object in std::iter::once(&agent.service).chain(&agent.config) {
        if !object.owner().writable() {
            return Some(ObjectsError::NotOwned {
                kind: object.kind,
                name: object.name.clone(),
            });
        }
    }
    for object in std::iter::once(&agent.service).chain(&agent.config) {
        if object.deleting {
            return Some(ObjectsError::Deleting {
                kind: object.kind,
                name: object.name.clone(),
            });
        }
    }
    None
}

/// The same for a shared object.
pub fn refusal_of_shared(object: &Stored) -> Option<ObjectsError> {
    if !object.owner().writable() {
        return Some(ObjectsError::NotOwned {
            kind: object.kind,
            name: object.name.clone(),
        });
    }
    if object.deleting {
        return Some(ObjectsError::Deleting {
            kind: object.kind,
            name: object.name.clone(),
        });
    }
    None
}

/// The title of a shared object.
pub fn title_of(object: &Stored) -> Option<String> {
    text(&object.spec, "/title")
}
