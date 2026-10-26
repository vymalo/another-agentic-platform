//! The shared objects an agent names, as `aap-domain` reads them, and what a change of one of them does to the
//! agents that name it.

use std::collections::BTreeSet;

use aap_api::{AgentConfig, AgentService, ModelEndpoint, ToolProvider};
use aap_domain::{Externals, RunPodSettings};
use aap_ports::{Agent, Kind, SharedKind, Stored, config_references};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::de::DeserializeOwned;

use crate::error::FormError;
use crate::validate::DEFAULT_IMAGE_STANDIN;

/// What `aap-domain::validate` reads besides the agent: the `ModelEndpoint` and `ToolProvider` objects the Platform
/// API listed, and the operator's default image, and the objects that **exist and cannot be read**.
///
/// An object whose `spec` is not the shape of its kind (a GitOps author's, or a newer version's) is in
/// [`unreadable`](Self::unreadable), not left out silently: an agent that names it is told so, and not that it is
/// missing, which would send a person to create an object that is there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Referents {
    /// The objects that were read, and the operator's default image.
    pub externals: Externals,
    /// The stored objects this version cannot read as their kind, by kind and name.
    pub unreadable: BTreeSet<(Kind, String)>,
}

fn object_meta(s: &Stored) -> ObjectMeta {
    ObjectMeta {
        name: Some(s.name.clone()),
        namespace: Some(s.namespace.clone()),
        ..ObjectMeta::default()
    }
}

fn spec<T: DeserializeOwned>(s: &Stored) -> Option<T> {
    serde_json::from_value(s.spec.clone()).ok()
}

/// The referents for a list of the store's shared objects, the operator's default image (`None`:
/// [`DEFAULT_IMAGE_STANDIN`]) and the size classes of run pods the operator has (empty: it has no run pods, and
/// an agent that asks for them is refused; otherwise [`RunPodSettings::offering`] stands for its settings, which
/// the API does not read).
pub fn externals(
    models: &[Stored],
    tool_servers: &[Stored],
    default_image: Option<&str>,
    run_pod_size_classes: &[String],
) -> Referents {
    let mut externals =
        Externals::default().with_default_image(default_image.unwrap_or(DEFAULT_IMAGE_STANDIN));
    if !run_pod_size_classes.is_empty() {
        externals = externals.with_run_pods(RunPodSettings::offering(
            run_pod_size_classes.iter().map(String::as_str),
        ));
    }
    let mut unreadable = BTreeSet::new();
    for stored in models {
        if let Some(spec) = spec(stored) {
            let mut object = ModelEndpoint::new(&stored.name, spec);
            object.metadata = object_meta(stored);
            externals = externals.with_model_endpoint(object);
        } else {
            unreadable.insert((Kind::ModelEndpoint, stored.name.clone()));
        }
    }
    for stored in tool_servers {
        if let Some(spec) = spec(stored) {
            let mut object = ToolProvider::new(&stored.name, spec);
            object.metadata = object_meta(stored);
            externals = externals.with_tool_provider(object);
        } else {
            unreadable.insert((Kind::ToolProvider, stored.name.clone()));
        }
    }
    Referents {
        externals,
        unreadable,
    }
}

/// A shared object as it would be once a write has been made: what [`dependents_after`] is asked about. It is
/// what `validate_model` and `validate_tool_server` give.
#[derive(Clone, Copy, Debug)]
pub enum SharedChange<'a> {
    /// A `ModelEndpoint`.
    Model(&'a ModelEndpoint),
    /// A `ToolProvider`.
    ToolServer(&'a ToolProvider),
}

/// The two typed objects of a stored agent, or `None` when either is not the shape of its kind.
fn typed(agent: &Agent) -> Option<(AgentService, AgentConfig)> {
    let config = agent.config.as_ref()?;
    let mut service = AgentService::new(agent.name(), spec(&agent.service)?);
    service.metadata = object_meta(&agent.service);
    let mut cfg = AgentConfig::new(&config.name, spec(config)?);
    cfg.metadata = object_meta(config);
    Some((service, cfg))
}

/// What editing a shared object would break in the agents that name it.
///
/// The validation of a model endpoint or a tool server is asked through a probe agent (`validate_model`,
/// `validate_tool_server`), which has `tools.allowInsecureHttp` on because that rule is the agent's, not the
/// object's: so a tool server changed to a plain `http://` address passes, and every agent that names it and does
/// not allow plain http stops being valid (`ConfigInvalid`) the moment the write is made, unseen. This is the
/// check for it: for each of `agents` that names the changed object, the operator's rules (`aap_domain::validate`) are
/// asked **twice**, with the object as it is in `referents` and as `change` makes it, and the issues the change
/// **adds** are returned, one error each (on no field: the form as a whole), naming the agent. An agent that was
/// invalid already is not blamed on the change, and one that does not name the object is not asked.
///
/// The Platform API (D7) calls it before a shared write, with the agents of `list_agents`, and decides whether
/// the answer is a `422` or a warning the person confirms.
pub fn dependents_after(
    change: SharedChange<'_>,
    agents: &[Agent],
    referents: &Referents,
) -> Vec<FormError> {
    let (kind, name, after) = match change {
        SharedChange::Model(m) => (
            SharedKind::Model,
            m.metadata.name.clone().unwrap_or_default(),
            referents.externals.clone().with_model_endpoint(m.clone()),
        ),
        SharedChange::ToolServer(t) => (
            SharedKind::ToolServer,
            t.metadata.name.clone().unwrap_or_default(),
            referents.externals.clone().with_tool_provider(t.clone()),
        ),
    };
    let mut errors = Vec::new();
    for agent in agents {
        let Some(config) = &agent.config else {
            continue;
        };
        if !config_references(&config.spec).names(kind, &name) {
            continue;
        }
        let Some((service, config)) = typed(agent) else {
            errors.push(FormError::new(
                "",
                format!(
                    "the agent {} names {name} and cannot be read by this version, so what the change does to it is not known",
                    agent.name()
                ),
            ));
            continue;
        };
        let issues = |externals: &Externals| {
            aap_domain::validate(&service, &config, externals)
                .err()
                .unwrap_or_default()
        };
        let before = issues(&referents.externals);
        for issue in issues(&after).into_iter().filter(|i| !before.contains(i)) {
            errors.push(FormError::new(
                "",
                format!(
                    "the agent {} would be refused by the operator after this change ({}): {}",
                    agent.name(),
                    issue.field,
                    issue.message
                ),
            ));
        }
    }
    errors
}
