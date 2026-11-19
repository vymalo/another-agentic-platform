//! What `validate` and `resolve` read besides an `AgentService` and its `AgentConfig`: the objects the
//! config names, and the operator's own default (§60a, AD-029).
//!
//! The crate is pure and fetches nothing. A composition root finds the names with [`references`], reads
//! those objects however it reads objects, and gives them back in an [`Externals`]. An object that is not
//! in it is *missing*: `resolve` says so, naming it, and the controller reports `ConfigInvalid`.

use std::collections::{BTreeMap, BTreeSet};

use aap_api::{AccessTierCap, AgentConfig, ModelEndpoint, ToolProvider};

use crate::run_pods::RunPodSettings;
use crate::syntax::is_dns_subdomain;

/// The objects an `AgentConfig` may name, and the settings of the operator that fill what it leaves
/// out. [`Externals::default`] is the empty one: no object, no default image and no run pods, which is all an
/// inline config needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Externals {
    /// The `ModelEndpoint`s of the config's namespace, by name.
    pub model_endpoints: BTreeMap<String, ModelEndpoint>,
    /// The `ToolProvider`s of the config's namespace, by name.
    pub tool_providers: BTreeMap<String, ToolProvider>,
    /// The operator's default agent image (`--default-agent-image`), used by a config that names none.
    /// `None` (or only white space): the operator has no default, and such a config is invalid.
    pub default_image: Option<String>,
    /// The shared kinds the cluster does not serve, by kind (`ModelEndpoint`, `ToolProvider`): their
    /// CustomResourceDefinition is not installed. A reference to one is missing for a reason other than a
    /// forgotten object, and the issue says so: nobody can create the referent until the CRDs are there.
    pub kinds_not_installed: BTreeSet<String>,
    /// What the operator's deployment says about run pods (`RUN_PODS_FILE`, slice D15a). `None`: this operator
    /// has no run pods (the chart's `runPods.enabled` is false), and a config that asks for them is invalid.
    pub run_pods: Option<RunPodSettings>,
    /// The claims the deployment allows as a package cache (`CACHE_ALLOWED_CLAIMS`, K1): an `environment.cache` names one of
    /// these or is `ConfigInvalid`. **Empty (the default): no agent may have a cache**, because a claim of the namespace that
    /// is not the cache's (another agent's data, this agent's own work volume) must not be mountable by whoever writes a config.
    pub cache_claims: BTreeSet<String>,
    /// The highest access tier a config may ask for (`AGENT_MAX_ACCESS_TIER`, K1), `auto` by default, which is the coder's own.
    /// A config above it is `ConfigInvalid`; one that names none gets the ceiling when it is below `auto`.
    pub max_access_tier: AccessTierCap,
    /// The `RuntimeClass` of every agent's own pod (`AGENT_RUNTIME_CLASS`, K14a, AD-052). `None`: no class, and the spec
    /// serialises exactly as it did before the field existed. **Named, it is part of the digest**, so naming it rolls the
    /// agents once, as a new image would.
    pub agent_runtime_class: Option<String>,
}

impl Externals {
    /// The default image of the operator.
    #[must_use]
    pub fn with_default_image(mut self, image: impl Into<String>) -> Self {
        self.default_image = Some(image.into());
        self
    }

    /// The claims the deployment allows as a package cache.
    #[must_use]
    pub fn with_cache_claims<I, S>(mut self, claims: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.cache_claims = claims.into_iter().map(Into::into).collect();
        self
    }

    /// The deployment's ceiling of the access tier.
    #[must_use]
    pub fn with_max_access_tier(mut self, ceiling: AccessTierCap) -> Self {
        self.max_access_tier = ceiling;
        self
    }

    /// The deployment's settings for run pods.
    #[must_use]
    pub fn with_run_pods(mut self, settings: RunPodSettings) -> Self {
        self.run_pods = Some(settings);
        self
    }

    /// The `RuntimeClass` of every agent's own pod (K14a, AD-052). Empty: none, as before the setting existed.
    #[must_use]
    pub fn with_agent_runtime_class(mut self, class: impl Into<String>) -> Self {
        let class = class.into();
        let class = class.trim();
        self.agent_runtime_class = (!class.is_empty()).then(|| class.to_owned());
        self
    }

    /// A shared kind (`ModelEndpoint` or `ToolProvider`) whose CRD the cluster does not have.
    #[must_use]
    pub fn with_kind_not_installed(mut self, kind: impl Into<String>) -> Self {
        self.kinds_not_installed.insert(kind.into());
        self
    }

    /// A `ModelEndpoint`, kept under its name.
    #[must_use]
    pub fn with_model_endpoint(mut self, endpoint: ModelEndpoint) -> Self {
        let name = endpoint.metadata.name.clone().unwrap_or_default();
        self.model_endpoints.insert(name, endpoint);
        self
    }

    /// A `ToolProvider`, kept under its name.
    #[must_use]
    pub fn with_tool_provider(mut self, provider: ToolProvider) -> Self {
        let name = provider.metadata.name.clone().unwrap_or_default();
        self.tool_providers.insert(name, provider);
        self
    }
}

/// The names of the objects an `AgentConfig` references, in its own namespace.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct References {
    /// `spec.model.endpointRef.name`.
    pub model_endpoint: Option<String>,
    /// Every `spec.tools.mcpServers.<n>.providerRef.name`, once each, in name order.
    pub tool_providers: BTreeSet<String>,
}

impl References {
    /// Whether the config references nothing.
    pub fn is_empty(&self) -> bool {
        self.model_endpoint.is_none() && self.tool_providers.is_empty()
    }
}

/// What `config` references. A composition root reads these, and a controller that watches the kinds
/// uses it to find the configs a changed object concerns. A reference whose name is empty, or is not the
/// name of an object (a DNS-1123 subdomain), is not one: `validate` reports it, and nothing is ever
/// requested of the API server under such a name.
pub fn references(config: &AgentConfig) -> References {
    let spec = &config.spec;
    References {
        model_endpoint: spec
            .model
            .as_ref()
            .and_then(|m| m.endpoint_ref.as_ref())
            .map(|r| r.name.clone())
            .filter(|n| is_dns_subdomain(n)),
        tool_providers: spec
            .tools
            .mcp_servers
            .values()
            .filter_map(|s| s.provider_ref.as_ref())
            .map(|r| r.name.clone())
            .filter(|n| is_dns_subdomain(n))
            .collect(),
    }
}
