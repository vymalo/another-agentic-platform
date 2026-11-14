//! Which services a changed `ModelEndpoint` or `ToolProvider` concerns (§60a, AD-029).
//!
//! A service names its config and a config names its referents, so the way from a referent to the
//! services is two hops through the caches the controllers already keep: the configs of the
//! namespace that reference it, then the services of the namespace that run those configs. Nothing is
//! asked of the API server. A deleted referent maps the same way, which is what makes a service whose
//! `ModelEndpoint` was deleted `ConfigInvalid` at once and not at its next timer.

use std::collections::BTreeSet;
use std::sync::Arc;

use aap_api::{AgentConfig, AgentService, ModelEndpoint, ToolProvider};
use aap_domain::{References, references};
use kube::ResourceExt;
use kube::runtime::reflector::{ObjectRef, Store};

/// The services of `namespace` that run a config whose references satisfy `uses`.
fn services_using(
    services: &Store<AgentService>,
    configs: &Store<AgentConfig>,
    namespace: Option<&str>,
    uses: impl Fn(&References) -> bool,
) -> Vec<ObjectRef<AgentService>> {
    users(services, configs, namespace, uses)
        .iter()
        .map(|s| ObjectRef::from_obj(&**s))
        .collect()
}

/// The services themselves, for the status of a shared object (K2): the same two hops.
fn users(
    services: &Store<AgentService>,
    configs: &Store<AgentConfig>,
    namespace: Option<&str>,
    uses: impl Fn(&References) -> bool,
) -> Vec<Arc<AgentService>> {
    let Some(namespace) = namespace else {
        return Vec::new();
    };
    let names: BTreeSet<String> = configs
        .state()
        .iter()
        .filter(|c| c.namespace().as_deref() == Some(namespace) && uses(&references(c)))
        .map(|c| c.name_any())
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    services
        .state()
        .iter()
        .filter(|s| {
            s.namespace().as_deref() == Some(namespace) && names.contains(&s.spec.config_ref.name)
        })
        .cloned()
        .collect()
}

/// The services to reconcile when `endpoint` changes or is deleted: those whose config has it as
/// `spec.model.endpointRef`.
pub fn services_of_endpoint(
    services: &Store<AgentService>,
    configs: &Store<AgentConfig>,
    endpoint: &ModelEndpoint,
) -> Vec<ObjectRef<AgentService>> {
    let name = endpoint.name_any();
    services_using(services, configs, endpoint.namespace().as_deref(), |r| {
        r.model_endpoint.as_deref() == Some(name.as_str())
    })
}

/// The services to reconcile when `provider` changes or is deleted: those whose config has it as the
/// `providerRef` of any server.
pub fn services_of_provider(
    services: &Store<AgentService>,
    configs: &Store<AgentConfig>,
    provider: &ToolProvider,
) -> Vec<ObjectRef<AgentService>> {
    let name = provider.name_any();
    services_using(services, configs, provider.namespace().as_deref(), |r| {
        r.tool_providers.contains(&name)
    })
}

/// The services that run a config naming `endpoint`, as the caches have them (K2: the users of a `ModelEndpoint`).
pub(crate) fn users_of_endpoint(
    services: &Store<AgentService>,
    configs: &Store<AgentConfig>,
    endpoint: &ModelEndpoint,
) -> Vec<Arc<AgentService>> {
    let name = endpoint.name_any();
    users(services, configs, endpoint.namespace().as_deref(), |r| {
        r.model_endpoint.as_deref() == Some(name.as_str())
    })
}

/// The services that run a config naming `provider` (K2: the users of a `ToolProvider`).
pub(crate) fn users_of_provider(
    services: &Store<AgentService>,
    configs: &Store<AgentConfig>,
    provider: &ToolProvider,
) -> Vec<Arc<AgentService>> {
    let name = provider.name_any();
    users(services, configs, provider.namespace().as_deref(), |r| {
        r.tool_providers.contains(&name)
    })
}

/// The shared objects of a namespace that `config` names: the endpoint, then the providers. What a changed or
/// deleted config or service triggers (K2).
pub(crate) fn endpoint_of_config(config: &AgentConfig) -> Vec<ObjectRef<ModelEndpoint>> {
    let (Some(ns), Some(name)) = (config.namespace(), references(config).model_endpoint) else {
        return Vec::new();
    };
    vec![ObjectRef::new(&name).within(&ns)]
}

/// The tool providers of a namespace that `config` names.
pub(crate) fn providers_of_config(config: &AgentConfig) -> Vec<ObjectRef<ToolProvider>> {
    let Some(ns) = config.namespace() else {
        return Vec::new();
    };
    references(config)
        .tool_providers
        .iter()
        .map(|name| ObjectRef::new(name).within(&ns))
        .collect()
}
