//! The deployment's defaults (§60a, "Deployment defaults"), and the part of an agent they fill.
//!
//! The forms show what matters to an administrator; the rest of an agent comes from the Platform
//! API's configuration file (Helm values in home-os, so GitOps owns it). [`Defaults`] is that file's
//! section: the API's configuration deserialises it (D7) and refuses a typo (`deny_unknown_fields`).
//!
//! **A default is never a field of a form**, and it is never presented as if a person had set it. It
//! enters the custom resources as a [`Deployment`], the part of an agent's objects that a form does not
//! edit, shown on the form read-only under *Deployment defaults*:
//!
//! * a **new** agent gets [`Defaults::deployment_for`], the defaults filled in;
//! * an **existing** agent has the `Deployment` its objects hold ([`crate::AgentView`]), which a save
//!   writes back unchanged: **a save never moves what the person did not edit**, and a change of the
//!   deployment's defaults reaches an agent only when the caller chooses to give it
//!   `deployment_for` again. The store is the case that decides this: an agent whose ledger moved to
//!   another database because an unrelated field was saved would lose its runs.

use aap_api::{Cnpg, GitAuthor, GithubMcp, PostgresStore, SecretKeyRef, Security, Store, Volume};
use k8s_openapi::api::core::v1::ResourceRequirements;
use k8s_openapi::api::networking::v1::NetworkPolicyPeer;
use serde::{Deserialize, Serialize};

use crate::agent::AgentKind;
use crate::image::ImagePolicy;

/// What the Platform API's configuration file says every agent gets, apart from what its form sets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Defaults {
    /// The namespace the dashboard's agents live in (AD-031: `another-agentic-system`).
    pub namespace: String,

    /// The label that offers the keys of an ExternalSecret to the forms (AD-030): `agents.vymalo.com/offer:
    /// "true"`. Shown on `GET /v1/info`, and named in the refusal of a key that is not offered.
    #[serde(default = "default_offer_label")]
    pub offer_label: String,

    /// Where an agent's durable run ledger lives.
    pub store: StoreDefault,

    /// The Secret key that holds the A2A bearer tokens (`interfaces.a2a.bearerTokensSecretRef`). It must
    /// hold the orchestrator's `AGENT_REGISTRY_AGENT_TOKEN` (§59a): without it the orchestrator lists the
    /// agent and cannot call it.
    pub a2a_bearer: SecretKeyRef,

    /// `access.allowFrom`: the peers that may reach the agent (the orchestrator's namespace).
    #[serde(default)]
    pub allow_from: Vec<NetworkPolicyPeer>,

    /// The pod's `security` settings.
    #[serde(default)]
    pub security: Security,

    /// What a coder gets.
    #[serde(default)]
    pub coder: CoderDefaults,

    /// What a folder agent gets.
    #[serde(default)]
    pub folder: FolderDefaults,

    /// The registries an agent may pin its own image from (*Advanced*, §60a), as hosts (`ghcr.io`,
    /// `registry.example:5000`; an image with no registry is `docker.io`'s). **Absent: any registry**, the
    /// tag or digest still required. **A list: only those. An empty list: none**, so no agent may pin its
    /// own image (it fails closed: somebody wrote it). See [`ImagePolicy`].
    #[serde(default)]
    pub allowed_image_registries: Option<Vec<String>>,
}

fn default_offer_label() -> String {
    aap_ports::OFFER_LABEL.to_owned()
}

/// Where a new agent's ledger lives: exactly one of an operator-owned CloudNativePG cluster per agent
/// (`cnpg`: instances and size), and a Secret key per agent named by a pattern (`secretRef`). In the
/// configuration file it is the map the CRD has (`store: { cnpg: { … } }`); a file that says both or neither is
/// refused with that sentence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawStoreDefault", into = "RawStoreDefault")]
pub enum StoreDefault {
    /// An operator-owned CloudNativePG cluster per agent.
    Cnpg(Cnpg),
    /// A Secret key per agent, named by a pattern.
    SecretRef(SecretPattern),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawStoreDefault {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cnpg: Option<Cnpg>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret_ref: Option<SecretPattern>,
}

impl TryFrom<RawStoreDefault> for StoreDefault {
    type Error = &'static str;

    fn try_from(raw: RawStoreDefault) -> Result<Self, Self::Error> {
        match (raw.cnpg, raw.secret_ref) {
            (Some(c), None) => Ok(Self::Cnpg(c)),
            (None, Some(s)) => Ok(Self::SecretRef(s)),
            _ => Err("exactly one of store.cnpg and store.secretRef"),
        }
    }
}

impl From<StoreDefault> for RawStoreDefault {
    fn from(store: StoreDefault) -> Self {
        match store {
            StoreDefault::Cnpg(c) => Self {
                cnpg: Some(c),
                secret_ref: None,
            },
            StoreDefault::SecretRef(s) => Self {
                cnpg: None,
                secret_ref: Some(s),
            },
        }
    }
}

/// A Secret key whose name is made from the agent's: `{name}-db-app` is `coder-me-db-app` for `coder-me`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretPattern {
    /// The Secret's name, `{name}` standing for the agent's name.
    pub name: String,
    /// The key inside it.
    pub key: String,
}

/// The defaults of a coder (`adam-coder`). Not the GitHub App: there is one App per coder (AD-033), so its
/// id and its private key's reference are fields of each coder.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoderDefaults {
    /// `GIT_AUTHOR_NAME` and `GIT_AUTHOR_EMAIL`.
    #[serde(default)]
    pub git_author: Option<GitAuthor>,
    /// `ALLOWED_REPO_HOSTS`.
    #[serde(default)]
    pub allowed_repo_hosts: Vec<String>,
    /// `GITHUB_API_URL`.
    #[serde(default)]
    pub github_api_url: Option<String>,
    /// The read-only GitHub MCP server, as a sidecar. Absent: none.
    #[serde(default)]
    pub github_mcp: Option<GithubMcp>,
    /// The coder's work volume (`work`, §59a). Absent: none.
    #[serde(default)]
    pub work_volume: Option<Volume>,
    /// CPU and memory of the agent container.
    #[serde(default)]
    pub resources: Option<ResourceRequirements>,
}

/// The defaults of a folder agent (`adam-agent`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderDefaults {
    /// CPU and memory of the agent container.
    #[serde(default)]
    pub resources: Option<ResourceRequirements>,
}

impl Defaults {
    /// The rule about the image an agent pins for itself.
    pub fn image_policy(&self) -> ImagePolicy {
        match &self.allowed_image_registries {
            None => ImagePolicy::any_registry(),
            Some(registries) => ImagePolicy::only(registries),
        }
    }

    /// What a **new** agent of `kind` named `name` gets: the defaults, filled in for it.
    pub fn deployment_for(&self, kind: AgentKind, name: &str) -> Deployment {
        let store = match &self.store {
            StoreDefault::Cnpg(cnpg) => Store {
                postgres: PostgresStore {
                    secret_ref: None,
                    cnpg: Some(cnpg.clone()),
                },
            },
            StoreDefault::SecretRef(pattern) => Store {
                postgres: PostgresStore {
                    secret_ref: Some(SecretKeyRef {
                        name: pattern.name.replace("{name}", name),
                        key: pattern.key.clone(),
                    }),
                    cnpg: None,
                },
            },
        };
        let mut deployment = Deployment {
            store,
            a2a_bearer: Some(self.a2a_bearer.clone()),
            allow_from: self.allow_from.clone(),
            security: self.security.clone(),
            resources: None,
            volumes: Vec::new(),
            github_mcp: None,
            coder: CoderDeployment::default(),
        };
        match kind {
            AgentKind::Coder => {
                let c = &self.coder;
                deployment.resources.clone_from(&c.resources);
                deployment.volumes = c.work_volume.iter().cloned().collect();
                deployment.github_mcp.clone_from(&c.github_mcp);
                deployment.coder = CoderDeployment {
                    git_author: c.git_author.clone(),
                    allowed_repo_hosts: c.allowed_repo_hosts.clone(),
                    github_api_url: c.github_api_url.clone(),
                };
            }
            AgentKind::Folder => {
                deployment.resources.clone_from(&self.folder.resources);
            }
        }
        deployment
    }
}

/// The part of an agent's objects that no form edits: the deployment's defaults as the agent holds them.
///
/// One struct for both kinds. For a folder agent the `coder` part is empty and the `github_mcp` and
/// `volumes` are whatever its objects hold (nothing, from the defaults). Every field is a type of
/// `aap-api`, so the objects of an agent that was made somewhere else are read whole.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Deployment {
    /// `AgentService.spec.store`.
    pub store: Store,
    /// `AgentService.spec.interfaces.a2a.bearerTokensSecretRef`.
    pub a2a_bearer: Option<SecretKeyRef>,
    /// `AgentService.spec.access.allowFrom`.
    pub allow_from: Vec<NetworkPolicyPeer>,
    /// `AgentConfig.spec.security`.
    pub security: Security,
    /// `AgentConfig.spec.environment.resources`.
    pub resources: Option<ResourceRequirements>,
    /// `AgentConfig.spec.environment.volumes`.
    pub volumes: Vec<Volume>,
    /// `AgentConfig.spec.tools.githubMcp`.
    pub github_mcp: Option<GithubMcp>,
    /// The coder's block: what is not the form's.
    pub coder: CoderDeployment,
}

/// The coder-only part of a [`Deployment`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoderDeployment {
    /// `coder.gitAuthor`.
    pub git_author: Option<GitAuthor>,
    /// `coder.allowedRepoHosts`.
    pub allowed_repo_hosts: Vec<String>,
    /// `coder.githubApiUrl`.
    pub github_api_url: Option<String>,
}
