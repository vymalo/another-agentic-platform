//! Reading the references of an `AgentConfig` into the inline form (§60a, AD-029).
//!
//! `materialize` turns a config that names a `ModelEndpoint`, `ToolProvider`s or no image into the
//! config whose fields say it: the endpoint's address and key where `model` had `endpointRef`, the
//! provider's URL and headers where a server had `providerRef`, the operator's default where there was
//! no image. Validation and resolution then run on **that** config, by the same code that reads an
//! inline one, which is what makes the two forms equal: an endpoint with the address and key of an
//! inline model gives the same variables, the same digest and the same pods. Nothing here knows an
//! environment variable.
//!
//! Two things have no inline field and travel beside the config: where a field came from ([`Origins`]),
//! so an issue about a provider's URL names the provider and not the server of the config, and a
//! provider's allow-list of tools.

use std::collections::{BTreeMap, BTreeSet};

use aap_api::{AgentConfig, Image};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

use crate::externals::Externals;
use crate::validate::ConfigIssue;

/// Where the fields of the materialized config came from, when not from the config itself.
#[derive(Clone, Debug, Default)]
pub(crate) struct Origins {
    /// The `ModelEndpoint` `model.baseUrl` and `model.apiKeySecretRef` were read from.
    pub endpoint: Option<String>,
    /// The `ToolProvider` each server of `tools.mcpServers` was read from, by server name.
    pub servers: BTreeMap<String, ProviderOrigin>,
}

/// A server that came from a `ToolProvider`.
#[derive(Clone, Debug)]
pub(crate) struct ProviderOrigin {
    /// The provider's name.
    pub provider: String,
    /// Its `mcp.tools`, as written. Empty: every tool.
    pub tools: Vec<String>,
}

/// A config with its references read, and what could not be.
#[derive(Clone, Debug)]
pub(crate) struct Materialized {
    /// The config with every reference that was found replaced by what it names. A reference that was
    /// not found stays (and `issues` says so); `resolve` never reads such a config, because it
    /// refuses first.
    pub config: AgentConfig,
    pub origins: Origins,
    /// The references that could not be read, and the image that is neither named nor defaulted.
    pub issues: Vec<ConfigIssue>,
}

/// The object `name` of `map`, or the sentence that says why there is none. When the cluster does not
/// serve the kind (`not_installed`) nobody can create the object: the sentence says what to install.
fn find<'a, T>(
    map: &'a BTreeMap<String, T>,
    kind: &str,
    not_installed: &BTreeSet<String>,
    name: &str,
    namespace: Option<&str>,
    meta: impl Fn(&T) -> &ObjectMeta,
    instead: &str,
) -> Result<&'a T, String> {
    let at = namespace.unwrap_or("(none)");
    if not_installed.contains(kind) {
        return Err(format!(
            "{kind} {name:?} cannot be read: this cluster does not serve the kind {kind}, because its CustomResourceDefinition is not installed (install the CRDs chart, deploy/operator-crds, which holds all four of agents.vymalo.com), or {instead}"
        ));
    }
    let Some(found) = map.get(name) else {
        return Err(format!(
            "{kind} {name:?} was not found in namespace {at:?}: create it, or {instead}"
        ));
    };
    match (meta(found).namespace.as_deref(), namespace) {
        (Some(there), Some(here)) if there != here => Err(format!(
            "{kind} {name:?} is in namespace {there:?} and the config in {here:?}: a reference is to the same namespace"
        )),
        _ => Ok(found),
    }
}

/// Read the references of `config` out of `externals`.
pub(crate) fn materialize(config: &AgentConfig, externals: &Externals) -> Materialized {
    let me = "AgentConfig";
    let namespace = config.metadata.namespace.as_deref();
    let mut out = config.clone();
    let mut origins = Origins::default();
    let mut issues = Vec::new();

    // The model. Only the valid shape is read: with an endpoint *and* an address, the exclusivity is
    // what `validate` reports, and nothing is guessed.
    let model = &config.spec.model;
    if let (Some(r), None, None) = (
        &model.endpoint_ref,
        &model.base_url,
        &model.api_key_secret_ref,
    ) && !r.name.trim().is_empty()
    {
        match find(
            &externals.model_endpoints,
            "ModelEndpoint",
            &externals.kinds_not_installed,
            &r.name,
            namespace,
            |e| &e.metadata,
            "set spec.model.baseUrl and spec.model.apiKeySecretRef inline",
        ) {
            Ok(endpoint) => {
                out.spec.model.endpoint_ref = None;
                out.spec.model.base_url = Some(endpoint.spec.base_url.clone());
                out.spec.model.api_key_secret_ref = Some(endpoint.spec.api_key_secret_ref.clone());
                origins.endpoint = Some(r.name.clone());
            }
            Err(message) => issues.push(ConfigIssue {
                field: format!("{me} spec.model.endpointRef"),
                message,
            }),
        }
    }

    // The servers. A server is read when it is a reference and nothing else.
    for (id, server) in &config.spec.tools.mcp_servers {
        let Some(r) = &server.provider_ref else {
            continue;
        };
        if server.url.is_some() || !server.headers.is_empty() || r.name.trim().is_empty() {
            continue;
        }
        match find(
            &externals.tool_providers,
            "ToolProvider",
            &externals.kinds_not_installed,
            &r.name,
            namespace,
            |p| &p.metadata,
            "give the server a url (and headers) inline",
        ) {
            Ok(provider) => {
                let mcp = &provider.spec.mcp;
                if let Some(s) = out.spec.tools.mcp_servers.get_mut(id) {
                    s.provider_ref = None;
                    s.url = Some(mcp.url.clone());
                    s.headers.clone_from(&mcp.headers);
                    // The server is optional when either the agent's entry or the provider says so:
                    // the agent's field has a default (false), so "absent" cannot be told from
                    // "false", and a rule that lets the agent's entry override the provider's would
                    // make the provider's `optional: true` unreachable from a plain `providerRef`.
                    s.optional = server.optional || mcp.optional;
                }
                origins.servers.insert(
                    id.clone(),
                    ProviderOrigin {
                        provider: r.name.clone(),
                        tools: mcp.tools.clone(),
                    },
                );
            }
            Err(message) => issues.push(ConfigIssue {
                field: format!("{me} spec.tools.mcpServers[{id}].providerRef"),
                message,
            }),
        }
    }

    // The image: the config's own, else the operator's default, else nothing to start.
    if config.spec.environment.image.is_none() {
        let default = externals
            .default_image
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty());
        match default {
            Some(d) if d.contains(char::is_whitespace) => issues.push(ConfigIssue {
                field: "operator --default-agent-image".to_owned(),
                message: format!(
                    "{d:?} has white space in it: the operator's default image is a reference such as repository:tag@sha256:…, and this config names no image of its own"
                ),
            }),
            Some(d) => {
                out.spec.environment.image = Some(Image {
                    reference: d.to_owned(),
                });
            }
            None => issues.push(ConfigIssue {
                field: format!("{me} spec.environment.image"),
                message: "no image, and this operator has no default agent image (--default-agent-image, DEFAULT_AGENT_IMAGE): name an image in the config, or give the operator a default".to_owned(),
            }),
        }
    }

    Materialized {
        config: out,
        origins,
        issues,
    }
}
