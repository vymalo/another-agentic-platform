//! `AgentService`: the stable, addressable agent (§59a, "The v0 CRDs").

use k8s_openapi::api::core::v1::ResourceRequirements;
use k8s_openapi::api::networking::v1::NetworkPolicyPeer;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::{CustomResource, KubeSchema};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{NameRef, SecretKeyRef};
use crate::status::AgentServiceStatus;

/// The desired state of an agent as a service: which config it runs, which interfaces it
/// serves, how it scales and where its ledger lives.
// kube-derive's repeated `printcolumn(type_ = …)` trips this lint (a false positive).
#[allow(clippy::duplicated_attributes)]
#[derive(CustomResource, KubeSchema, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[kube(
    group = "agents.vymalo.com",
    version = "v1alpha1",
    kind = "AgentService",
    plural = "agentservices",
    namespaced,
    status = "AgentServiceStatus",
    derive = "PartialEq",
    doc = "A durable, addressable adam-rs agent service. It names an AgentConfig and the operator makes the workload, Service, NetworkPolicy and storage for it.",
    printcolumn(name = "State", type_ = "string", json_path = ".status.state"),
    printcolumn(name = "Config", type_ = "string", json_path = ".spec.configRef.name"),
    printcolumn(
        name = "Replicas",
        type_ = "integer",
        json_path = ".status.runtime.replicas"
    ),
    printcolumn(
        name = "Age",
        type_ = "date",
        json_path = ".metadata.creationTimestamp"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct AgentServiceSpec {
    /// What the agent does, in a sentence. Listed in the agent registry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// The person or account this agent belongs to, as the orchestrator names them: 1 to 63 characters of
    /// lower-case letters, digits, `-` and `_`. Absent: nobody's, and no owner-scoped route of the Platform API
    /// can touch it. Changing it never restarts the agent (it is not in the digest). It is **not** the Kubernetes
    /// owner reference of a pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 63), regex(pattern = r"^[a-z0-9][a-z0-9_-]*$"))]
    pub owner: Option<String>,

    /// The account this agent's own pods' minutes are for: the person's opaque id from the identity provider (the `sub`,
    /// trimmed and lower-cased), 1 to 128 bytes of `[A-Za-z0-9._:/-]`; an `@` is never one, so an e-mail is not an account.
    /// The Platform API writes it only for a caller that holds `platform:agents.own`; write access to the `AgentService`
    /// itself is the trust boundary. Absent: nobody's, a shared agent.
    /// Changing it never restarts the agent (it is not in the digest). It is **not** `owner`, the orchestrator's pseudonym.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1, max = 128), regex(pattern = r"^[A-Za-z0-9._:/-]+$"))]
    pub account: Option<String>,

    /// The AgentConfig, in the same namespace, this service runs.
    pub config_ref: NameRef,

    /// The protocol surfaces the service serves. Nothing is exposed by default.
    #[serde(default)]
    pub interfaces: Interfaces,

    /// How many processes run, and in which topology.
    #[serde(default)]
    pub scaling: Scaling,

    /// Scale the workload to zero replicas. The only scale-to-zero of v0.
    #[serde(default)]
    pub suspend: bool,

    /// Where the agent's durable run ledger lives.
    pub store: Store,

    /// Who may reach the service.
    #[serde(default)]
    pub access: Access,

    /// How the agent is listed in the agent registry.
    #[serde(default)]
    pub registry: Registry,

    /// What happens to data (work volumes, an operator-owned database) when the service is deleted.
    #[serde(default)]
    pub deletion_policy: DeletionPolicy,

    /// The release channels of the service (AD-059, §10a). Absent: one revision, replaced in place, and the
    /// workload keeps the name `<service>`. Present: a channel is a pointer to an `AgentConfig`, and the
    /// operator names a workload per live revision (`<service>-r<n>`). The dashboard's forms cannot say it:
    /// an object that carries it is YAML only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<Release>,
}

/// The release channels of a service (AD-059, §10a): `defaultChannel` and the channels, each a pointer to an
/// `AgentConfig`. A channel is a name that points at a revision; moving it is editing one line, and a rollback
/// is moving it back. The platform is the only owner of channel state (§12a).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("self.channels.all(k, k.size() <= 32 && k.matches('^[a-z0-9]([a-z0-9-]*[a-z0-9])?$'))").message("release.channels: a channel name is at most 32 characters of lower-case letters, digits and -, starting and ending with a letter or a digit"))]
#[x_kube(validation = Rule::new("self.defaultChannel in self.channels").message("release.defaultChannel names one of release.channels"))]
pub struct Release {
    /// The channel a request that names none gets. One of `channels`.
    #[schemars(length(min = 1, max = 32))]
    pub default_channel: String,

    /// Channel name to what it points at: at most 8 channels.
    #[schemars(extend("maxProperties" = 8))]
    pub channels: std::collections::BTreeMap<String, ChannelTarget>,
}

/// What a channel points at: the `AgentConfig` whose revision serves it. The same shape as `configRef`, so a
/// channel's image, model and tools are what that config says.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChannelTarget {
    /// The `AgentConfig`, in the service's namespace, the channel points at.
    pub config_ref: NameRef,
}

/// An agent's owner (`AgentService.spec.owner`): 1 to 63 characters of lower-case letters, digits, `-` and `_`, starting
/// with a letter or a digit. Never a path separator or a space: it is a segment of a route.
pub fn is_owner_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    s.len() <= 63
        && bytes
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// The protocol surfaces of a service (§11). Only A2A is served in v0.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Interfaces {
    /// The A2A surface.
    #[serde(default)]
    pub a2a: A2aInterface,
    /// The Responses surface. Exists for the target design; v0 refuses `true`.
    #[serde(default)]
    pub responses: UnsupportedInterface,
    /// The MCP surface. Exists for the target design; v0 refuses `true`.
    #[serde(default)]
    pub mcp: UnsupportedInterface,
}

/// The A2A surface of a service.
// K1: the rule and the default of `enabled` are adam-rs's (its `A2aInterface`), so a document it accepts is read the same.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("(has(self.enabled) && !self.enabled) || has(self.bearerTokensSecretRef)").message("interfaces.a2a needs bearerTokensSecretRef: no token, no server"))]
pub struct A2aInterface {
    /// Serve A2A. Without it the agent is not listed in the registry. Off when omitted, as before K1 (adam-rs's
    /// operator defaults it to on: a manifest that writes `a2a` with only a token reference must not turn a
    /// disabled agent on at the next apply, so this operator does not, and says so in the superset test).
    #[serde(default)]
    pub enabled: bool,

    /// The Secret key that holds the bearer tokens (`A2A_BEARER_TOKENS`). No token, no server:
    /// the agent fails closed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bearer_tokens_secret_ref: Option<SecretKeyRef>,

    /// The URL clients reach the agent at (`PUBLIC_URL`). Empty: `http://<name>.<ns>.svc:8080/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
}

/// A surface the v0 operator does not serve.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct UnsupportedInterface {
    /// Must be false in v0.
    #[serde(default)]
    #[x_kube(validation = Rule::new("self == false").message("v0 serves A2A only: this interface cannot be enabled"))]
    pub enabled: bool,
}

/// How many processes run, and in which topology.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("!has(self.front) || self.topology == 'split'").message("scaling.front is only allowed with topology: split"))]
#[x_kube(validation = Rule::new("has(self.idleTimeout) || (!has(self.wakeAhead) && !has(self.wakeTimeout))").message("scaling.wakeAhead and scaling.wakeTimeout need scaling.idleTimeout"))]
pub struct Scaling {
    /// `combined`: one workload runs every role. `split`: a control plane (`<name>-front`) and workers.
    #[serde(default)]
    pub topology: Topology,

    /// Replicas of the workload `<name>` (the adam workers as pods).
    #[serde(default = "one")]
    #[schemars(range(min = 1))]
    pub workers: i32,

    /// The front of a `split` topology.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub front: Option<Front>,

    /// How long the agent must report no work before it sleeps (scaled to zero). A duration such as
    /// `15m`, from `1m` to `24h`. Absent: the agent never sleeps (AD-058).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[x_kube(validation = Rule::new("self.matches('^[1-9][0-9]*[smh]$') && duration(self) >= duration('1m') && duration(self) <= duration('24h')").message("scaling.idleTimeout is a duration from 1m to 24h, such as 15m"))]
    pub idle_timeout: Option<String>,

    /// How long before a scheduled firing a sleeping agent is woken. A duration from `10s` to `1h`.
    /// Only with `idleTimeout`; absent: 2m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[x_kube(validation = Rule::new("self.matches('^[1-9][0-9]*[smh]$') && duration(self) >= duration('10s') && duration(self) <= duration('1h')").message("scaling.wakeAhead is a duration from 10s to 1h, such as 2m"))]
    pub wake_ahead: Option<String>,

    /// How long a request waits for a sleeping agent to wake before the caller is told to retry.
    /// A duration from `5s` to `5m`. Only with `idleTimeout`; absent: 60s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[x_kube(validation = Rule::new("self.matches('^[1-9][0-9]*[smh]$') && duration(self) >= duration('5s') && duration(self) <= duration('5m')").message("scaling.wakeTimeout is a duration from 5s to 5m, such as 60s"))]
    pub wake_timeout: Option<String>,
}

fn one() -> i32 {
    1
}

impl Default for Scaling {
    fn default() -> Self {
        Self {
            topology: Topology::default(),
            workers: one(),
            front: None,
            idle_timeout: None,
            wake_ahead: None,
            wake_timeout: None,
        }
    }
}

/// The topology of a service's processes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Topology {
    /// One workload runs every role.
    #[default]
    Combined,
    /// A control plane (`<name>-front`) and the workers (`<name>`).
    Split,
}

/// The front (control plane) of a `split` topology.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Front {
    /// Replicas of `<name>-front`. A PodDisruptionBudget is made when there is more than one.
    #[serde(default = "one")]
    #[schemars(range(min = 1))]
    pub replicas: i32,
}

impl Default for Front {
    fn default() -> Self {
        Self { replicas: one() }
    }
}

/// Where the agent's durable run ledger lives.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Store {
    /// A PostgreSQL ledger.
    pub postgres: PostgresStore,
}

/// A PostgreSQL ledger: a Secret someone else owns, or an operator-owned CloudNativePG cluster.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("has(self.secretRef) != has(self.cnpg)").message("exactly one of store.postgres.secretRef and store.postgres.cnpg"))]
pub struct PostgresStore {
    /// The Secret key that holds the connection string (`DATABASE_URL`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretKeyRef>,

    /// An operator-owned CloudNativePG `Cluster` `<name>-db`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cnpg: Option<Cnpg>,
}

/// An operator-owned CloudNativePG cluster.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Cnpg {
    /// Instances of the cluster.
    #[serde(default = "one")]
    #[schemars(range(min = 1))]
    pub instances: i32,

    /// Storage of each instance.
    pub storage: CnpgStorage,

    /// CPU and memory requests and limits of each instance. Unset: none (the pod is BestEffort).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,

    /// A volume of its own for the write-ahead log. Unset: the log stays on the data volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal_storage: Option<CnpgStorage>,

    /// PostgreSQL parameters: only `shared_buffers`, `max_connections` and `work_mem`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub postgresql: Option<CnpgPostgresql>,
}

/// The PostgreSQL settings of a CloudNativePG cluster the operator makes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CnpgPostgresql {
    /// Parameter name to value. The names allowed are `shared_buffers`, `max_connections` and
    /// `work_mem` (the operator refuses any other); each value is at most 32 characters.
    #[schemars(length(max = 3), inner(length(min = 1, max = 32)))]
    pub parameters: std::collections::BTreeMap<String, String>,
}

/// The parameters a service's database may set.
pub const CNPG_PARAMETERS: [&str; 3] = ["shared_buffers", "max_connections", "work_mem"];

/// Storage of a CloudNativePG instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CnpgStorage {
    /// Size of the volume, e.g. `5Gi`.
    pub size: Quantity,

    /// Storage class. Unset: the cluster's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_class: Option<String>,
}

/// Who may reach the service, and who may use the agent.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct Access {
    /// Peers allowed to reach port 8080: the `from` of a NetworkPolicy ingress rule. There is no
    /// egress rule: the agent needs git hosts, the model gateway and registries.
    #[serde(default)]
    pub allow_from: Vec<NetworkPolicyPeer>,

    /// Who may use the agent in a client (AD-028): the values of the person's roles claim that
    /// admit them. By convention a value is the agent's use permission, `agent.use:<agent-name>`
    /// (10 characters and the name, so a name of at most 54 fits the 64 of a value; the dashboard
    /// makes names of at most 40); `*` admits everyone. Absent or empty: administrators only. The
    /// CRD checks only the shape; what the values mean is the consumer's rule. It reaches no pod: it
    /// goes to the registry item as `audience` (slice D3: each value once, in byte order), and
    /// nothing enforces it before slice D8.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 32), inner(length(min = 1, max = 64)))]
    #[x_kube(validation = Rule::new("!self.exists(a, a == '*') || self.size() == 1").message("access.audience: the value * is only allowed alone"))]
    #[x_kube(validation = Rule::new("self.all(a, a.matches('^[!-~]+$'))").message("access.audience: a value is visible ASCII characters, with no space or control character"))]
    pub audience: Option<Vec<String>>,
}

/// How the agent is listed in the agent registry.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Registry {
    /// Display title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Free-form tags.
    #[serde(default)]
    pub tags: Vec<String>,
}

/// What happens to data when an AgentService is deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum DeletionPolicy {
    /// The compute is removed; the work volumes and an operator-owned database stay.
    #[default]
    Retain,
    /// The data goes too.
    Delete,
}
