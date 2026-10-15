//! What `validate` and `resolve` read besides an `AgentService` and its `AgentConfig`: the objects the
//! config names, and the operator's own default (§60a, AD-029).
//!
//! The crate is pure and fetches nothing. A composition root finds the names with [`references`], reads
//! those objects however it reads objects, and gives them back in an [`Externals`]. An object that is not
//! in it is *missing*: `resolve` says so, naming it, and the controller reports `ConfigInvalid`.

use std::collections::{BTreeMap, BTreeSet};

use aap_api::{AgentConfig, ModelEndpoint, ToolProvider};

/// The objects an `AgentConfig` may name, and the settings of the operator that fill what it leaves
/// out. [`Externals::default`] is the empty one: no object and no default image, which is all an
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
}

impl Externals {
    /// The default image of the operator.
    #[must_use]
    pub fn with_default_image(mut self, image: impl Into<String>) -> Self {
        self.default_image = Some(image.into());
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
/// uses it to find the configs a changed object concerns. A reference with an empty name is not one:
/// `validate` reports it.
pub fn references(config: &AgentConfig) -> References {
    let spec = &config.spec;
    References {
        model_endpoint: spec
            .model
            .endpoint_ref
            .as_ref()
            .map(|r| r.name.clone())
            .filter(|n| !n.trim().is_empty()),
        tool_providers: spec
            .tools
            .mcp_servers
            .values()
            .filter_map(|s| s.provider_ref.as_ref())
            .map(|r| r.name.clone())
            .filter(|n| !n.trim().is_empty())
            .collect(),
    }
}
