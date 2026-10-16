//! The permissions of the Platform API (§60a, AD-032) as compile-time constants.
//!
//! A permission is a Keycloak client role of the client `another-agentic`; Keycloak expands the
//! composite roles into the roles claim of the token, so the claim holds the permissions a person has
//! however they came by them. **The API checks one of these strings, never a role name**, and its
//! configuration lists no role: a deployment that wants another bundle changes a composite in
//! Keycloak and nothing else. The names are *proposed, not final* (§93): a name is in tokens and in
//! every `audience`, so the owner confirms them before the API is built (D7).
//!
//! `agent.use:<agent-name>` is **not** one of them: it is what an agent's `audience` lists
//! (AD-028), the orchestrator reads it, and the dashboard only writes it ([`use_permission`]).

/// What every dashboard permission starts with, and what `GET /v1/me` filters the roles claim by.
pub const PREFIX: &str = "platform:";

/// Read the dashboard: agents, models, tool servers, the deployment's defaults and View YAML. Draws
/// `/admin`. Guards every `GET` of `/v1/info`, `/v1/agents`, `/v1/models` and `/v1/tool-servers`, and
/// `GET /v1/agents/{name}/yaml`.
pub const AGENTS_READ: &str = "platform:agents.read";

/// Create, edit, suspend, resume and delete agents, audience included. Guards `PUT` and `DELETE` of
/// `/v1/agents/{name}`.
pub const AGENTS_WRITE: &str = "platform:agents.write";

/// Create, edit and delete `ModelEndpoint`s. Guards `PUT` and `DELETE` of `/v1/models/{name}`.
pub const MODELS_WRITE: &str = "platform:models.write";

/// Create, edit and delete `ToolProvider`s. Guards `PUT` and `DELETE` of `/v1/tool-servers/{name}`.
pub const TOOLPROVIDERS_WRITE: &str = "platform:toolproviders.write";

/// See the offered Secret keys and name one in a write (AD-030). Guards `GET /v1/secret-keys`, **and
/// any write whose body names a secret key**, in addition to the write's own permission.
pub const SECRETS_PICK: &str = "platform:secrets.pick";

/// Every permission of the dashboard, in the order of the table of AD-032.
pub const ALL: [&str; 5] = [
    AGENTS_READ,
    AGENTS_WRITE,
    MODELS_WRITE,
    TOOLPROVIDERS_WRITE,
    SECRETS_PICK,
];

/// The convention of AD-028: the use permission of an agent, which its `audience` lists. At most 50
/// characters for a name of at most 40.
#[must_use]
pub fn use_permission(agent: &str) -> String {
    format!("agent.use:{agent}")
}
