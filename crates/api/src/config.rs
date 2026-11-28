//! `AgentConfig`: what an agent is made of (§59a, "The v0 CRDs").

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::ResourceRequirements;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::{CustomResource, KubeSchema};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::{Empty, NameRef, SecretKeyRef};
use crate::model_endpoint::ReasoningEffort;
use crate::status::AgentConfigStatus;

/// The configuration of an agent: its harness, model, tools, environment and security. A
/// secret is never a value here: a field that needs one names a Secret and a key.
// kube-derive's repeated `printcolumn(type_ = …)` trips this lint (a false positive).
#[allow(clippy::duplicated_attributes)]
#[derive(CustomResource, KubeSchema, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[kube(
    group = "agents.vymalo.com",
    version = "v1alpha1",
    kind = "AgentConfig",
    plural = "agentconfigs",
    namespaced,
    status = "AgentConfigStatus",
    derive = "PartialEq",
    doc = "What an agent is made of. Referenced by an AgentService; the operator validates it and resolves it into a runtime.",
    printcolumn(
        name = "Binary",
        type_ = "string",
        json_path = ".spec.harness.adam.binary"
    ),
    printcolumn(
        name = "Image",
        type_ = "string",
        json_path = ".spec.environment.image.ref",
        priority = 1
    ),
    printcolumn(
        name = "Age",
        type_ = "date",
        json_path = ".metadata.creationTimestamp"
    )
)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("self.harness.type != 'adam-rs' || has(self.model)").message("harness.type adam-rs needs spec.model"))]
pub struct AgentConfigSpec {
    /// The agent framework that runs.
    pub harness: Harness,

    /// The model the agent talks to. Required for `adam-rs`; an `a2a` image gets none from the
    /// platform (AD-054).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<Model>,

    /// The tools the agent may use.
    #[serde(default)]
    pub tools: Tools,

    /// The image, resources and volumes of the agent's pods.
    pub environment: Environment,

    /// The pod's security context. Everything else of it is fixed by the operator.
    #[serde(default)]
    pub security: Security,

    /// Literal environment variables. Names only the adam binaries do not own; never a secret.
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
}

// ---------------------------------------------------------------- harness

/// The agent framework: `adam-rs`, the default (AD-022), or a bring-your-own A2A image (AD-054).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("self.type != 'adam-rs' || (has(self.adam) && !has(self.a2a))").message("harness.type adam-rs needs harness.adam and no harness.a2a"))]
#[x_kube(validation = Rule::new("self.type != 'a2a' || (has(self.a2a) && !has(self.adam))").message("harness.type a2a needs harness.a2a and no harness.adam"))]
pub struct Harness {
    /// The harness type.
    #[serde(rename = "type")]
    pub kind: HarnessType,

    /// The adam-rs settings. Only with `type: adam-rs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adam: Option<Adam>,

    /// The settings of an image that serves A2A itself. Only with `type: a2a`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a2a: Option<A2a>,
}

/// The harness types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum HarnessType {
    /// adam-rs: `adam-coder` or `adam-agent`, one image.
    #[serde(rename = "adam-rs")]
    AdamRs,
    /// An image that serves A2A itself: the operator runs it and invents no variable (AD-054).
    #[serde(rename = "a2a")]
    A2a,
}

/// An image that serves A2A itself (AD-054): the operator runs it with the pod hardening, store and
/// registry entry of an adam-rs agent, and sets only the variables listed here.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("self.cardPath.startsWith('/')").message("harness.a2a.cardPath is an absolute path: it starts with `/`"))]
#[x_kube(validation = Rule::new("self.healthPath.startsWith('/')").message("harness.a2a.healthPath is an absolute path: it starts with `/`"))]
pub struct A2a {
    /// The image. The harness's own: `environment.image` is refused with it.
    pub image: Image,

    /// The port the image serves on.
    #[serde(default = "a2a_port")]
    #[schemars(range(min = 1, max = 65535))]
    pub port: i32,

    /// Where the image serves its A2A agent card, as a path.
    #[serde(default = "a2a_card_path")]
    pub card_path: String,

    /// The path that answers 200 while the image serves: its startup and readiness probes.
    #[serde(default = "a2a_health_path")]
    pub health_path: String,

    /// The variables the image gets, each a plain value or a Secret key. The operator invents none.
    #[serde(default)]
    #[schemars(extend("maxItems" = MAX_A2A_ENV))]
    pub env: Vec<A2aEnv>,
}

/// The default port of an A2A image.
pub const A2A_PORT: i32 = 8080;
/// Where an A2A image serves its card unless it says otherwise.
pub const A2A_CARD_PATH: &str = "/.well-known/agent-card.json";
/// The path an A2A image answers 200 on unless it says otherwise.
pub const A2A_HEALTH_PATH: &str = "/healthz";
/// The most variables an A2A image may be given (a schema bound: one environment of a pod is limited).
pub const MAX_A2A_ENV: u64 = 64;

fn a2a_port() -> i32 {
    A2A_PORT
}

fn a2a_card_path() -> String {
    A2A_CARD_PATH.to_owned()
}

fn a2a_health_path() -> String {
    A2A_HEALTH_PATH.to_owned()
}

/// One variable of an A2A image: a plain value, or a Secret key, never both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("has(self.value) != has(self.secretKeyRef)").message("an a2a environment variable has exactly one of value and secretKeyRef"))]
pub struct A2aEnv {
    /// The variable's name.
    #[schemars(length(min = 1))]
    pub name: String,

    /// A plain value. Never a secret.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,

    /// The Secret key that holds the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_key_ref: Option<SecretKeyRef>,
}

/// The adam-rs settings: which binary runs, where the agent comes from, and the coder's tunables.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("self.binary != 'adam-coder' || (has(self.agent.embedded) && has(self.coder))").message("binary adam-coder needs agent.embedded and the coder block"))]
#[x_kube(validation = Rule::new("self.binary != 'adam-agent' || (has(self.agent.folder) && !has(self.coder))").message("binary adam-agent needs agent.folder and no coder block"))]
pub struct Adam {
    /// The binary the container runs.
    pub binary: Binary,

    /// Where the agent comes from.
    pub agent: AgentSource,

    /// The coder's settings. Only with `adam-coder`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coder: Option<Coder>,
}

/// The two binaries of the adam-rs image.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Binary {
    /// A coding task to a verified pull request.
    #[serde(rename = "adam-coder")]
    AdamCoder,
    /// Serves any agent folder.
    #[serde(rename = "adam-agent")]
    AdamAgent,
}

/// Where the agent comes from: a folder, or the one compiled into the binary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("has(self.folder) != has(self.embedded)").message("exactly one of agent.folder and agent.embedded"))]
pub struct AgentSource {
    /// An agent that is only a folder, served by `adam-agent`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<Folder>,

    /// The agent compiled into the binary (`adam-coder`). No settings: `{}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded: Option<Empty>,
}

/// An agent folder, mounted read-only at `/etc/adam/agent` (`ADAM_AGENT_DIR`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("has(self.files) != has(self.configMapRef)").message("exactly one of agent.folder.files and agent.folder.configMapRef"))]
pub struct Folder {
    /// The whole folder inline: relative path to content. A ConfigMap holds 1 MiB, which is the
    /// folder's size limit in v0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<BTreeMap<String, String>>,

    /// A ConfigMap of the same namespace that holds the folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_map_ref: Option<NameRef>,
}

/// The settings of `adam-coder`. A field left out is not set, so the binary's own default applies.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Coder {
    /// Runs advanced at once, per process (`WORKERS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub workers: Option<i32>,

    /// `MAX_CHECK_CYCLES`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0))]
    pub max_check_cycles: Option<i32>,

    /// `CHECK_TIMEOUT_SECS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub check_timeout_secs: Option<i64>,

    /// `WORKSPACE_SWEEP_SECS`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub workspace_sweep_secs: Option<i64>,

    /// `WORKSPACE_PLACEMENT`: `""`, `shared`, `affinity` or `isolated`. Checked by the reconciler,
    /// which mirrors the adam-rs chart's rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_placement: Option<String>,

    /// `ALLOWED_REPO_HOSTS`.
    #[serde(default)]
    pub allowed_repo_hosts: Vec<String>,

    /// `GITHUB_API_URL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_api_url: Option<String>,

    /// `PR_DRAFT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr_draft: Option<bool>,

    /// `GIT_AUTHOR_NAME` and `GIT_AUTHOR_EMAIL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_author: Option<GitAuthor>,

    /// `CREATE_REPO_OWNERS`. Empty: the tool stays off.
    #[serde(default)]
    pub create_repo_owners: Vec<String>,

    /// `OPENCODE_MODEL`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opencode_model: Option<String>,

    /// `MAX_ACCESS_TIER`: the highest access tier a person may pick for a message, on every role.
    /// Unset: `auto`. `bypass` (renaming and deleting repositories) is the deployment's opt-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_access_tier: Option<AccessTierCap>,

    /// How the coder authenticates to GitHub.
    pub github: Github,

    /// Run pods (D15, adam-rs ADR 0019 and 0041): the commands of a run, `run_command`, `run_checks` and
    /// OpenCode, run in a pod of their own, which sees only its own run and none of the coder's secrets,
    /// and not in the coder's container, where they could read the GitHub App's private key. Absent: no
    /// run pods, and the objects of an agent are exactly what they were. Present: the operator makes what
    /// the coder's chart makes for them (the pod template, a service account that may make and exec in
    /// pods, a quota, a network policy and the parameters of the admission policy that guards the pods
    /// it may make) and sets `RUN_ENVIRONMENT=kubernetes` and the `RUN_POD_*` variables. Needs an operator
    /// whose chart has run pods on (`runPods.enabled`, `runPodClasses`): with none, the config is
    /// `ConfigInvalid`, saying so.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_pods: Option<RunPods>,
}

/// Run pods for the coder (`coder.runPods`). The only field is the size of a pod; everything else
/// (the image, the quota, the network policy, the timings) is the deployment's, in the operator
/// chart's `runPods` block.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunPods {
    /// A class of the operator chart's `runPodClasses`, by name (`standard`: 250m CPU and 512Mi of
    /// memory requested, 2Gi of memory at most). It is the resources of the run container. A name the
    /// operator does not have is `ConfigInvalid`, saying which it has.
    #[serde(default = "standard_class")]
    #[schemars(length(min = 1))]
    pub size_class: String,
}

/// The highest access tier a deployment lets a person pick (`access-tier/v1`).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum AccessTierCap {
    /// Read and plan only.
    Plan,
    /// Also change files, commit, push and open pull requests.
    Edits,
    /// Also merge pull requests and create repositories (the coder's default).
    #[default]
    Auto,
    /// Also rename and delete repositories, with no consent form for a repository.
    Bypass,
}

impl AccessTierCap {
    /// The value of `MAX_ACCESS_TIER`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Edits => "edits",
            Self::Auto => "auto",
            Self::Bypass => "bypass",
        }
    }
}

/// The size class of a `runPods` that names none.
pub const STANDARD_CLASS: &str = "standard";

fn standard_class() -> String {
    STANDARD_CLASS.to_owned()
}

/// The author of the coder's commits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GitAuthor {
    /// `GIT_AUTHOR_NAME`.
    pub name: String,
    /// `GIT_AUTHOR_EMAIL`.
    pub email: String,
}

/// The coder's GitHub credential: a GitHub App or a token, never both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("has(self.app) != has(self.token)").message("exactly one of github.app and github.token"))]
pub struct Github {
    /// A GitHub App.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<GithubApp>,

    /// A token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<GithubToken>,
}

/// A GitHub App. The private key is mounted as a file, never an environment variable.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("has(self.installationId) != has(self.owners)").message("exactly one of github.app.installationId and github.app.owners"))]
pub struct GithubApp {
    /// The App's client ID: public, not a secret (`GITHUB_APP_ID`).
    #[schemars(length(min = 1))]
    pub id: String,

    /// The owners whose installation is looked up (`GITHUB_APP_OWNERS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(length(min = 1))]
    pub owners: Option<Vec<String>>,

    /// One pinned installation (`GITHUB_APP_INSTALLATION_ID`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_id: Option<i64>,

    /// The Secret key that holds the private key (a PEM file).
    pub private_key_secret_ref: SecretKeyRef,
}

/// A GitHub token (`GITHUB_TOKEN`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubToken {
    /// The Secret key that holds the token.
    pub secret_ref: SecretKeyRef,
}

// ------------------------------------------------------------------ model

/// The model the agent talks to, over an OpenAI-compatible endpoint: a `ModelEndpoint` by name
/// (`endpointRef`), or the address and key inline (`baseUrl` and `apiKeySecretRef`), never both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("!has(self.endpointRef) || (!has(self.baseUrl) && !has(self.apiKeySecretRef))").message("model.endpointRef is exclusive with model.baseUrl and model.apiKeySecretRef"))]
#[x_kube(validation = Rule::new("has(self.endpointRef) || (has(self.baseUrl) && has(self.apiKeySecretRef))").message("model needs an endpointRef, or both a baseUrl and an apiKeySecretRef"))]
pub struct Model {
    /// The model alias (`MODEL`).
    #[schemars(length(min = 1))]
    pub model: String,

    /// A `ModelEndpoint` of the same namespace that holds the address and the key (AD-029).
    /// Exclusive with `baseUrl` and `apiKeySecretRef`. The operator reads the endpoint into the same
    /// variables the inline form gives (§60a), so an edit of the endpoint rolls every agent that uses
    /// it. An endpoint that is not there makes the agent `ConfigReferentMissing` (ANR-1159), naming it, until it is applied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<NameRef>,

    /// The endpoint (`MODEL_BASE_URL`): a value or a Secret key. With `apiKeySecretRef`, unless
    /// `endpointRef` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<BaseUrl>,

    /// The Secret key that holds the API key (`MODEL_API_KEY`). With `baseUrl`, unless
    /// `endpointRef` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_secret_ref: Option<SecretKeyRef>,

    /// A JSON object merged into every chat-completions request (`MODEL_EXTRA_BODY`), to make a gateway
    /// or a model emit its reasoning, for example `{reasoning_effort: medium}`. Not a secret: it shows in
    /// the pod's environment. It may not set `model`, `messages`, `tools`, `tool_choice` or `stream`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "free_form_object")]
    pub extra_body: Option<serde_json::Map<String, serde_json::Value>>,

    /// Send the reasoning of earlier turns back to the model, under this member name
    /// (`MODEL_ECHO_REASONING`). Unset: never sent, which is what almost every model wants.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub echo_reasoning: Option<EchoReasoning>,

    /// The context window of the model `model` names, in tokens (`MODEL_CONTEXT_WINDOW`): what each
    /// usage report of a call on that alias says as its window, so a screen shows how full the context
    /// is. Unset: reports carry no window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1, max = MAX_CONTEXT_WINDOW), extend("format" = "int64"))]
    pub context_window: Option<u64>,

    /// The reasoning effort of this agent (`model.reasoningEffort`, ADR 0063): one of the levels the
    /// `ModelEndpoint`'s entry for `model` declares. Unset sends nothing. **The operator writes no variable
    /// for it yet**: the coder's seat is the card's levels (from `MODEL_ALIASES_FILE`) and the
    /// orchestrator's per-message `reasoning/v1`, which follow; until then the field is stored and
    /// validated, and reaches no model (§60a).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
}

/// The largest `model.contextWindow`: 2^53 - 1, the largest count a usage report carries (what
/// `adam_service::MAX_CONTEXT_WINDOW` accepts).
pub const MAX_CONTEXT_WINDOW: u64 = (1 << 53) - 1;

/// The most members of `model.extraBody` (a schema bound), and the most bytes of its canonical JSON (`validate`):
/// one environment string is limited, and a config that passes must not be a pod that cannot start.
pub const MAX_EXTRA_BODY_MEMBERS: u64 = 64;
/// See [`MAX_EXTRA_BODY_MEMBERS`].
pub const MAX_EXTRA_BODY_BYTES: usize = 16 * 1024;

/// The member a model's earlier reasoning is sent back under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EchoReasoning {
    /// `reasoning_content`.
    ReasoningContent,
    /// `reasoning`.
    Reasoning,
}

impl EchoReasoning {
    /// The value of `MODEL_ECHO_REASONING`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReasoningContent => "reasoning_content",
            Self::Reasoning => "reasoning",
        }
    }
}

/// An object whose members the schema does not describe.
fn free_form_object(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    // `nullable` as the sibling optional fields are; `maxProperties` is a bound on what a user can make the pod's
    // environment carry (`validate` also caps the serialised size).
    schemars::json_schema!({
        "type": "object",
        "nullable": true,
        "maxProperties": MAX_EXTRA_BODY_MEMBERS,
        "x-kubernetes-preserve-unknown-fields": true
    })
}

/// The model endpoint: a plain value, or a Secret key when the deployment keeps it out of git.
/// Used by `AgentConfig.spec.model.baseUrl` and `ModelEndpoint.spec.baseUrl`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("has(self.value) != has(self.secretRef)").message("exactly one of baseUrl.value and baseUrl.secretRef"))]
pub struct BaseUrl {
    /// The URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,

    /// The Secret key that holds the URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<SecretKeyRef>,
}

// ------------------------------------------------------------------ tools

/// The tools the agent may use.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Tools {
    /// The read-only GitHub MCP server, as a native sidecar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_mcp: Option<GithubMcp>,

    /// Extra MCP servers, by name, at most 32 (the bound lets an API server price the CEL rules of
    /// a server). Written to a ConfigMap that holds `${VAR}` references only.
    #[serde(default)]
    #[schemars(extend("maxProperties" = 32))]
    pub mcp_servers: BTreeMap<String, McpServer>,

    /// Allow a plain `http://` MCP server on another machine (`MCP_ALLOW_INSECURE`). Never automatic.
    #[serde(default)]
    pub allow_insecure_http: bool,
}

/// The `github-mcp` sidecar. It holds no credential: the coder sends the credentials of each call.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GithubMcp {
    /// Run the sidecar.
    #[serde(default = "yes")]
    pub sidecar: bool,

    /// Its loopback port. Checked again by the reconciler.
    #[serde(default = "github_mcp_port")]
    #[schemars(range(min = 1, max = 65535))]
    pub port: i32,

    /// `GITHUB_HOST` of the sidecar, for GitHub Enterprise. Empty or unset: github.com.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

fn yes() -> bool {
    true
}

fn github_mcp_port() -> i32 {
    8082
}

impl Default for GithubMcp {
    fn default() -> Self {
        Self {
            sidecar: yes(),
            port: github_mcp_port(),
            host: None,
        }
    }
}

/// One extra MCP server: a `ToolProvider` by name (`providerRef`), or the URL and headers inline,
/// never both.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(validation = Rule::new("!has(self.providerRef) || (!has(self.url) && (!has(self.headers) || self.headers.size() == 0))").message("mcpServers.providerRef is exclusive with url and headers"))]
#[x_kube(validation = Rule::new("has(self.providerRef) || has(self.url)").message("an MCP server needs a url, or a providerRef"))]
pub struct McpServer {
    /// A `ToolProvider` of the same namespace that holds the URL and the headers (AD-029).
    /// Exclusive with `url` and `headers`. The operator reads the provider into the same entry of the
    /// extra MCP file the inline form gives (§60a), so an edit of the provider rolls every agent that
    /// uses it. A provider that is not there makes the agent `ConfigInvalid`, naming it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<NameRef>,

    /// Its `http` or `https` URL, with no user name, password or `${…}` in it (checked by the
    /// reconciler). Required unless `providerRef` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// Request headers, by header name. Each value is a Secret key. Not with `providerRef`.
    #[serde(default)]
    pub headers: BTreeMap<String, McpHeader>,

    /// The agent starts without it when it is unreachable. Beside a `providerRef` the provider has an
    /// `optional` of its own, and the server is optional when either says so.
    #[serde(default)]
    pub optional: bool,
}

/// A header whose value is a Secret key.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpHeader {
    /// Plain text put in front of the secret, e.g. `Bearer `.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,

    /// The Secret key that holds the value. It is named by the variable that carries it.
    pub secret_ref: SecretKeyRef,
}

// ------------------------------------------------------------ environment

/// The image, resources and volumes of the agent's pods. It has the shape of the spec of the
/// later `AgentEnvironment` (§8), so a `*Ref` can join it additively.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    /// The image. adam-rs ships one image for both binaries. Absent: the operator's default coder
    /// image (AD-029, the operator's file key `defaults.agentImage`; with no default configured, a config
    /// with no image is `ConfigInvalid`). An agent may still pin its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<Image>,

    /// CPU and memory of the agent container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,

    /// The shared package cache: a claim you manage, mounted at `/cache` in every pod that runs
    /// toolchains (adam-rs ADR 0040). Unset: `/cache` is a directory of the container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<Cache>,

    /// Volumes. None: the operator renders a Deployment.
    #[serde(default)]
    pub volumes: Vec<Volume>,

    /// The pod's `terminationGracePeriodSeconds`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 0))]
    pub termination_grace_period_seconds: Option<i64>,
}

/// The package cache of the agent's pods: npm's, pnpm's and cargo's caches, which the image keeps under
/// `/cache`. One trust domain: whatever a run writes there every run reads, so share the claim only with
/// agents you trust.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Cache {
    /// The name of a PersistentVolumeClaim in the service's namespace, `ReadWriteMany`, that you manage and
    /// whose root is writable by uid and gid 10001. The operator creates and deletes nothing; a missing
    /// claim keeps the pod `Pending`.
    #[schemars(length(min = 1))]
    pub existing_claim: String,
}

/// An image.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Image {
    /// The reference: a tag, or a tag and a digest.
    #[serde(rename = "ref")]
    #[schemars(length(min = 1))]
    pub reference: String,
}

/// A volume of the agent's pods (§8).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    /// The volume's name; the claim template of a StatefulSet is named after it.
    #[schemars(length(min = 1))]
    pub name: String,

    /// Who shares the volume (§28). v0 uses `agent`; the reconciler refuses the others.
    pub scope: VolumeScope,

    /// Where it is mounted.
    #[schemars(length(min = 1))]
    pub mount_path: String,

    /// What backs it.
    pub source: VolumeSource,
}

/// Who shares a volume (§28).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum VolumeScope {
    /// Destroyed after a run.
    Run,
    /// Shared between the runs of one AgentService.
    Agent,
    /// Shared by the agents of a project.
    Project,
}

/// What backs a volume. Only `persistent` exists in v0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VolumeSource {
    /// A persistent volume claim.
    pub persistent: PersistentVolume,
}

/// A persistent volume claim.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolume {
    /// Requested size, e.g. `20Gi`.
    pub size: Quantity,

    /// Storage class. Unset: the cluster's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_class: Option<String>,

    /// One claim per replica (a StatefulSet's `volumeClaimTemplate`), or a single shared claim.
    #[serde(default)]
    pub per_replica: bool,
}

// --------------------------------------------------------------- security

/// The pod's security context. The rest of it is fixed: no service-account token, `runAsNonRoot`,
/// `seccompProfile: RuntimeDefault`, no capabilities, no privilege escalation.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Security {
    /// `runAsUser`. Unset: 10001, the adam image's user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub run_as_user: Option<i64>,

    /// `runAsGroup`. Unset: 10001.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub run_as_group: Option<i64>,

    /// `fsGroup`. Unset: 10001.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(range(min = 1))]
    pub fs_group: Option<i64>,

    /// `fsGroupChangePolicy`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fs_group_change_policy: Option<FsGroupChangePolicy>,
}

/// `fsGroupChangePolicy` of a pod.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum FsGroupChangePolicy {
    /// Only when the root of the volume has the wrong owner.
    OnRootMismatch,
    /// Always.
    Always,
}
