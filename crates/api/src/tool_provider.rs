//! `ToolProvider`, the v0 subset of §32: one remote MCP server set once and named by the agents
//! that use it (AD-029, §60a "What the custom resources gain").

use std::collections::BTreeMap;

use kube::{CustomResource, KubeSchema};
use serde::{Deserialize, Serialize};

use crate::config::McpHeader;

/// Where some tools come from: one remote MCP server. An `AgentConfig` names it with
/// `spec.tools.mcpServers.<name>.providerRef`; the operator resolves the reference into the same
/// extra-MCP entry the inline form gives (slice D2 of the admin dashboard). A secret is never a
/// value here (AD-024): a header names a Secret key.
// kube-derive's repeated `printcolumn(type_ = …)` trips this lint (a false positive).
#[allow(clippy::duplicated_attributes)]
#[derive(CustomResource, KubeSchema, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[kube(
    group = "agents.vymalo.com",
    version = "v1alpha1",
    kind = "ToolProvider",
    plural = "toolproviders",
    namespaced,
    derive = "PartialEq",
    doc = "Where tools come from, set once and named by the AgentConfigs that use it (spec.tools.mcpServers.<name>.providerRef). v0: one remote MCP server.",
    printcolumn(name = "Title", type_ = "string", json_path = ".spec.title"),
    printcolumn(
        name = "URL",
        type_ = "string",
        json_path = ".spec.mcp.url",
        priority = 1
    ),
    printcolumn(
        name = "Age",
        type_ = "date",
        json_path = ".metadata.creationTimestamp"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct ToolProviderSpec {
    /// What the dashboard shows.
    #[schemars(length(min = 1))]
    pub title: String,

    /// The remote MCP server. The only kind of provider in v0 (§32 lists more).
    pub mcp: ProviderMcp,
}

/// One remote MCP server, in the shape of an inline `AgentConfig` `mcpServers` entry, plus the
/// allow-list of its tools.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderMcp {
    /// Its `http` or `https` URL, with no user name, password or `${…}` in it (checked by the
    /// reconciler, as for an inline server).
    #[schemars(length(min = 1))]
    pub url: String,

    /// Request headers, by header name. Each value is a Secret key.
    #[serde(default)]
    pub headers: BTreeMap<String, McpHeader>,

    /// The tools of the server the agent may call. Empty: every tool of the server. Nothing in the
    /// inline form says this, so slice D2 decides what the operator does with it.
    #[serde(default)]
    pub tools: Vec<String>,

    /// The agent starts without it when it is unreachable.
    #[serde(default)]
    pub optional: bool,
}
