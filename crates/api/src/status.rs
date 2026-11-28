//! The status of `AgentService` and `AgentConfig`, and the condition types and reasons the controller writes.

use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What the controller observed of an AgentService.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentServiceStatus {
    /// The `metadata.generation` the controller last reconciled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    /// The state derived from the conditions and the runtime phase (§88).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ServiceState>,

    /// The AgentConfig this status was resolved from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigStatus>,

    /// The runtime, as the provider reports it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<RuntimeStatus>,

    /// Where the agent can be reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoints: Option<Endpoints>,

    /// Present only while the agent is asleep or waking (AD-058, P3a-1). The operator writes it (the idle loop only
    /// proposes; the reconcile is its one writer) and **reads it back after a restart**, so an operator that is
    /// restarted, or a leader that fails over, does not wake a sleeping agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sleep: Option<SleepStatus>,

    /// The service's revisions, when `spec.release` is set (AD-059, §10a): one per resolved digest the service has
    /// run, newest first, with the epochs its image serves and the runs still open in each. Absent: no
    /// `spec.release`, so there is one revision and it is the workload the service names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revisions: Vec<RevisionStatus>,

    /// `ConfigResolved`, `StoreReady`, `RuntimeReady`, `Listed` and `Ready`.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// One revision of a service (AD-059, §10a): what its pods run, which epochs they can replay, and what they still
/// have open. A revision is immutable; only its replica count moves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RevisionStatus {
    /// `<service>-r<n>`, `n` counting the service's digests in order.
    pub name: String,

    /// `sha256:<hex>` of the resolved runtime, as `status.config.digest` gives it.
    pub digest: String,

    /// The epochs its image serves.
    pub epochs: Epochs,

    /// Where it is in the drain lifecycle.
    pub phase: RevisionPhase,

    /// Pods of its workload that are ready.
    #[serde(default)]
    pub ready_replicas: i32,

    /// Open runs per epoch, keyed by the epoch's decimal text, as the agent's activity answer reports them.
    /// **Absent is not zero**: the agent did not answer, and a revision is never retired on a missing answer
    /// (fail closed).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub open_runs: std::collections::BTreeMap<String, i64>,
}

/// The epochs a revision's image serves (adam's ADR 0076, through its activity answer): a worker steps a run whose
/// epoch is in `min..=max` and refuses the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Epochs {
    /// The oldest epoch a worker of this revision can replay.
    pub min: i64,
    /// The newest epoch this revision's image serves.
    pub max: i64,
}

/// Where a revision is in the drain lifecycle (AD-059, §10a): the one new runs go to, the one that still serves
/// runs started on it, and the one nothing serves any more.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum RevisionPhase {
    /// New runs go to it.
    Current,
    /// A newer revision is `Current` and this one still serves open runs.
    Draining,
    /// No open run needs it. Its workload is scaled to zero, then deleted.
    Retired,
}

/// The annotation that wakes a sleeping agent (AD-058, ANR-Q134). Its value is an RFC 3339 instant, written by
/// whatever receives a request for the agent. The decision never parses it: **any value that differs from
/// `status.sleep.wakeAtSeen`** (the value when the agent fell asleep) is a wake request. It is level-triggered, the
/// operator never removes it, and an awake agent ignores it.
pub const WAKE_AT_ANNOTATION: &str = "agents.vymalo.com/wake-at";

/// Why an agent is at zero, and, once it is waking, why (AD-058, P3a-1; the field names accepted by the owner,
/// ANR-Q140, 2026-11-27).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SleepStatus {
    /// When the agent was scaled to zero.
    pub since: Time,
    /// When the agent it asked has work due, as the idle loop's confirmed poll said: the operator wakes it
    /// `scaling.wakeAhead` earlier.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_due_at: Option<Time>,
    /// The value of the `agents.vymalo.com/wake-at` annotation when the agent fell asleep. A value that differs
    /// from it is a request to wake. At most 64 bytes of the value as it was; a longer one is stored as
    /// `sha256:<hex>` (71 characters), so an annotation of any size cannot grow the status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(max = 71))]
    pub wake_at_seen: Option<String>,
    /// Present from the moment the agent is woken until its runtime is ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waking: Option<WakingStatus>,
}

/// A wake in progress.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WakingStatus {
    /// When the operator lifted the scale to zero.
    pub since: Time,
    /// What woke it.
    pub cause: WakeCause,
}

/// What woke a sleeping agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum WakeCause {
    /// The `agents.vymalo.com/wake-at` annotation changed.
    Request,
    /// `nextDueAt` minus `scaling.wakeAhead` came.
    Schedule,
}

/// The state of a service. `Suspended` is healthy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ServiceState {
    /// The runtime is ready.
    Ready,
    /// The desired state is applied and the runtime is not ready yet, or has an issue.
    Degraded,
    /// `spec.suspend` is set and the runtime is suspended.
    Suspended,
    /// The operator did not apply the desired state, and left what runs untouched.
    Blocked,
}

/// The AgentConfig a service was resolved from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ConfigStatus {
    /// Name of the AgentConfig.
    pub name: String,
    /// Its `metadata.generation` when it was resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,
    /// `sha256:<hex>` of the resolved runtime: the seed of AgentRevision (§9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// The runtime of a service, in the neutral terms of `RuntimeStatus` (§59a).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    /// The provider that made it: `kubernetes`.
    pub provider: String,
    /// The phase of §18.
    pub phase: RuntimePhase,
    /// Replicas that are ready.
    #[serde(default)]
    pub replicas: i32,
}

/// The phase of a runtime (§18).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum RuntimePhase {
    /// Nothing exists.
    Absent,
    /// A rollout is in progress.
    Provisioning,
    /// Ready.
    Ready,
    /// Scaled to zero on purpose.
    Suspended,
    /// Scaled to zero because the agent reported no work (AD-058); a request or a due schedule wakes it.
    /// Distinct from `Suspended`, which is a person's choice.
    Sleeping,
    /// Woken (AD-058, P3a-1): the scale to zero is lifted and the runtime is not ready yet.
    Waking,
    /// Failed.
    Failed,
}

/// Where the agent can be reached.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Endpoints {
    /// The A2A endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a2a: Option<String>,
    /// The agent card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_card: Option<String>,
}

/// What the controller observed of an AgentConfig.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AgentConfigStatus {
    /// The `metadata.generation` the controller last validated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    /// `Valid`.
    #[serde(default)]
    pub conditions: Vec<Condition>,
}

/// What the controller observed of a `ModelEndpoint` or a `ToolProvider` (§60a, K2): who uses it, whether it is valid and
/// whether the agents that use it are `Ready`. **No secret is ever in it**: no header value, no URL with credentials, no
/// token; the names are those of `AgentService`s of the object's own namespace.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SharedStatus {
    /// The `metadata.generation` the controller last judged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_generation: Option<i64>,

    /// The `AgentService`s of the namespace that run a config naming this object, sorted, each once, **at most
    /// [`MAX_USED_BY`]** (the operator's bound: the schema has none, because only the operator writes it): the rest are counted
    /// in `usedByMore`.
    #[serde(default)]
    pub used_by: Vec<String>,

    /// How many users are not in `usedBy` (absent when there are at most [`MAX_USED_BY`]).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub used_by_more: i32,

    /// `Accepted`, `InUse` and `AgentsReady`; on a `ToolProvider` the deployment probes, also `Reachable` (K3b).
    #[serde(default)]
    pub conditions: Vec<Condition>,

    /// The MCP server's tools, on a `ToolProvider` the deployment probes (K3b, AD-053). Absent when probing is off, or
    /// when the object was not probed (invalid, a header is needed, or the server is unreachable). The reachability is the
    /// `Reachable` condition; **no tool description, schema or URL is here**.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<ProbeStatus>,
}

/// What the operator observed of a `ToolProvider`'s MCP server (K3b): the tool names it listed, at most 64. The reachability
/// is the `Reachable` condition of the [`SharedStatus`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProbeStatus {
    /// The tool names the server listed, sorted, each once, **at most 64** (the operator's bound: the schema has none,
    /// because only the operator writes it): the rest are counted in `toolsMore`.
    #[serde(default)]
    pub tools: Vec<String>,

    /// How many tool names are not in `tools` (absent when there are at most 64).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tools_more: i32,

    /// The server still named a `nextCursor` after the operator read its five pages: there may be more tools than those
    /// listed, so the count is a lower bound (absent when false).
    #[serde(default, skip_serializing_if = "is_false")]
    pub incomplete: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero(n: &i32) -> bool {
    *n == 0
}

/// The most users a [`SharedStatus`] lists by name. The status of an object that a whole fleet names must stay
/// a bounded size (every change of it is sent to every watcher).
pub const MAX_USED_BY: usize = 64;

/// The most tool names a [`ProbeStatus`] lists. The status of a server that lists hundreds must stay a bounded size
/// (every change of it is sent to every watcher); the rest are counted.
pub const MAX_PROBED_TOOLS: usize = 64;

/// Condition types (§59a, "Status").
pub mod condition_type {
    /// AgentConfig: it passed the validation of `aap-domain`.
    pub const VALID: &str = "Valid";
    /// AgentService: the config exists and resolved.
    pub const CONFIG_RESOLVED: &str = "ConfigResolved";
    /// AgentService: the store is ready.
    pub const STORE_READY: &str = "StoreReady";
    /// AgentService: the runtime is ready.
    pub const RUNTIME_READY: &str = "RuntimeReady";
    /// AgentService: listed in the agent registry. Informs, never gates `Ready`.
    pub const LISTED: &str = "Listed";
    /// AgentService: the first three are true.
    pub const READY: &str = "Ready";
    /// ModelEndpoint, ToolProvider: the object passed the validation of `aap-domain` on its own (K2).
    pub const ACCEPTED: &str = "Accepted";
    /// ModelEndpoint, ToolProvider: whether an agent names it (K2). Informs.
    pub const IN_USE: &str = "InUse";
    /// ModelEndpoint, ToolProvider: whether the agents that name it are `Ready` (K2). Informs, gates nothing.
    pub const AGENTS_READY: &str = "AgentsReady";
    /// ToolProvider: the MCP server answered the probe (K3b, AD-053). Absent when probing is off.
    pub const REACHABLE: &str = "Reachable";
}

/// Condition reasons (§59a, "Status").
pub mod reason {
    /// `ConfigResolved` true.
    pub const RESOLVED: &str = "Resolved";
    /// `ConfigResolved` false: the named AgentConfig does not exist.
    pub const CONFIG_NOT_FOUND: &str = "ConfigNotFound";
    /// `ConfigResolved` false: the config failed validation, and waiting will not fix it.
    pub const CONFIG_INVALID: &str = "ConfigInvalid";
    /// `ConfigResolved` false: a `ModelEndpoint` or a `ToolProvider` the config names is not there yet (ANR-1159). The
    /// operator looks again soon and the service unblocks by itself, so a client waits instead of treating the agent as
    /// refused. `ConfigInvalid` stays for a config that is really wrong.
    pub const CONFIG_REFERENT_MISSING: &str = "ConfigReferentMissing";
    /// `StoreReady` true: a Secret is referenced.
    pub const SECRET_REFERENCED: &str = "SecretReferenced";
    /// `StoreReady` true: the CloudNativePG cluster is ready.
    pub const CLUSTER_READY: &str = "ClusterReady";
    /// `StoreReady` false: the CloudNativePG API is absent.
    pub const CNPG_NOT_INSTALLED: &str = "CNPGNotInstalled";
    /// `StoreReady` false: the cluster is not ready.
    pub const CLUSTER_NOT_READY: &str = "ClusterNotReady";
    /// `RuntimeReady` and `Ready` true.
    pub const READY: &str = "Ready";
    /// `RuntimeReady` false: a rollout is in progress.
    pub const PROVISIONING: &str = "Provisioning";
    /// `RuntimeReady` false: suspended on purpose.
    pub const SUSPENDED: &str = "Suspended";
    /// `RuntimeReady` false: scaled to zero because the agent reported no work (AD-058).
    pub const SLEEPING: &str = "Sleeping";
    /// `RuntimeReady` false: woken, the runtime is scaling back up (AD-058, P3a-1).
    pub const WAKING: &str = "Waking";
    /// `RuntimeReady` false: still waking after `scaling.wakeTimeout`. The operator keeps waking and says so (ANR-Q141).
    pub const WAKE_TIMED_OUT: &str = "WakeTimedOut";
    /// `RuntimeReady` false: a referenced Secret or key does not exist.
    pub const MISSING_SECRET: &str = "MissingSecret";
    /// `RuntimeReady` false: the agent process refused its configuration (exit 78).
    pub const CONFIG_REJECTED: &str = "ConfigRejected";
    /// `RuntimeReady` false: a dependency is unreachable (exit 69).
    pub const DEPENDENCY_UNAVAILABLE: &str = "DependencyUnavailable";
    /// `RuntimeReady` false: the image cannot be pulled.
    pub const IMAGE_PULL: &str = "ImagePull";
    /// `RuntimeReady` false: the container keeps exiting.
    pub const CRASH_LOOP: &str = "CrashLoop";
    /// `RuntimeReady` false: an object the runtime needs exists and is not ours.
    pub const NAME_CONFLICT: &str = "NameConflict";
    /// `Listed` true.
    pub const LISTED: &str = "Listed";
    /// `Listed` false: A2A is not enabled.
    pub const A2A_DISABLED: &str = "A2ADisabled";
    /// `Listed` false: the service is `Blocked`.
    pub const SERVICE_BLOCKED: &str = "ServiceBlocked";
    /// `Listed` false: the registry is past its limits.
    pub const REGISTRY_FULL: &str = "RegistryFull";
    /// `Listed` false: the registry is not served.
    pub const REGISTRY_DISABLED: &str = "RegistryDisabled";
    /// `Ready` true.
    pub const RECONCILED: &str = "Reconciled";
    /// `Valid` true, and `Accepted` true.
    pub const VALID: &str = "Valid";
    /// `Accepted` false, and `Reachable` unknown for an invalid `ToolProvider`: the object fails the validation of
    /// `aap-domain`; the message names each field.
    pub const INVALID: &str = "Invalid";
    /// `InUse` true.
    pub const REFERENCED: &str = "Referenced";
    /// `InUse` false.
    pub const UNREFERENCED: &str = "Unreferenced";
    /// `AgentsReady` true: every agent that names it is `Ready` or `Suspended` (healthy).
    pub const ALL_READY: &str = "AllReady";
    /// `AgentsReady` false: some agent that names it is not; the message names them.
    pub const SOME_NOT_READY: &str = "SomeNotReady";
    /// `AgentsReady` unknown: no agent names it, so there is nothing to be ready.
    pub const NO_AGENTS: &str = "NoAgents";
    /// `Reachable` true: `initialize` and `tools/list` answered.
    pub const PROBED: &str = "Probed";
    /// `Reachable` unknown: the server names a header, which the operator does not read (AD-024).
    pub const NEEDS_HEADER: &str = "NeedsHeader";
    /// `Reachable` false: the server did not answer.
    pub const UNREACHABLE: &str = "Unreachable";
    /// `Reachable` false: the answer is not MCP.
    pub const NOT_MCP: &str = "NotMcp";
    /// `Reachable` false: the server did not answer within the deployment's deadline.
    pub const TIMEOUT: &str = "Timeout";
}
