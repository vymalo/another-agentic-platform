//! Which services a changed `ModelEndpoint` or `ToolProvider` concerns (§60a, AD-029).
//!
//! A service names its config and a config names its referents, so the way from a referent to the
//! services is two hops through the caches the controllers already keep: the configs of the
//! namespace that reference it, then the services of the namespace that run those configs. Nothing is
//! asked of the API server. A deleted referent maps the same way, which is what makes a service whose
//! `ModelEndpoint` was deleted `ConfigInvalid` at once and not at its next timer.

use std::collections::BTreeSet;

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
        .map(|s| ObjectRef::from_obj(&**s))
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
