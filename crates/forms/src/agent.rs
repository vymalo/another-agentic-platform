//! The agent form (§60a, "Screens": *Coder* and *Folder agent*).
//!
//! One form with two variants of what is particular to a kind. **Invalid states that cost nothing to
//! forbid are not representable**: a coder has a GitHub App and no instructions, a folder agent has
//! files and no GitHub App, and a secret is a [`SecretPick`] and never a string. A body with a member the
//! form does not have is refused whole (`deny_unknown_fields`: a `400`, "a body that is not a form"); a
//! *value* that is wrong (a name that is too long, a count out of range) is the `422` of
//! [`validate_agent`](crate::validate_agent), with the field.
//!
//! The form mirrors what the custom resources say, field by field, **including whether a field may be
//! absent** (`description`, `audience`, `workers`, `prDraft`, `opencodeModel`, `image` are `Option`s, as in
//! the objects): a form that wrote an absent field as an empty one would change an object the person did
//! not edit. Defaults of the *screen* (the audience `agent.use:<name>`, the OpenCode model "the same as the
//! model", `optional` on) are the client's to fill in; defaults of the *deployment* are
//! [`Defaults`](crate::Defaults)', and are in no form.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::secret::SecretPick;

/// The two kinds of agent the dashboard makes (AD-033, §60a): a coder (`adam-coder`) and a folder agent
/// (`adam-agent`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    /// A coding task to a verified pull request: `adam-coder`, the agent compiled into the binary.
    Coder,
    /// An agent that is only a folder, served by `adam-agent`.
    Folder,
}

/// The agent form: what an administrator edits about a coder or a folder agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentForm {
    /// The agent's name, and the name of both its objects. `^[a-z][a-z0-9-]{0,38}[a-z0-9]$`, unique, fixed
    /// after the create.
    pub name: String,
    /// The title the agent is listed with (`registry.title`), 1 to 80 characters.
    pub title: String,
    /// What the agent does, in a sentence (`description`), at most 500 characters.
    #[serde(default)]
    pub description: Option<String>,
    /// Tags (`registry.tags`): at most 16, each 1 to 64 characters of lower-case words and dashes.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Scale to zero (`spec.suspend`): the Suspend and Resume actions.
    #[serde(default)]
    pub suspend: bool,
    /// The model: an endpoint and an alias.
    pub model: ModelChoice,
    /// The tool servers (`ToolProvider`s) the agent uses, by name. A set: the objects keep them by name, and
    /// a name said twice is said once.
    #[serde(default)]
    pub tool_servers: BTreeSet<String>,
    /// Allow a plain `http://` tool server on another host (`tools.allowInsecureHttp`). It covers **every**
    /// server of the agent. Never automatic.
    #[serde(default)]
    pub allow_insecure_http: bool,
    /// The agent's own image, under *Advanced*. Absent: the operator's default coder image (AD-029).
    #[serde(default)]
    pub image: Option<String>,
    /// Who may use the agent (`access.audience`, AD-028): the values of a person's roles claim that admit
    /// them, by convention `agent.use:<name>`; `*` admits everyone. Absent or empty: administrators only.
    #[serde(default)]
    pub audience: Option<Vec<String>>,
    /// What is particular to the kind.
    pub agent: AgentKindForm,
}

/// The model of an agent: a `ModelEndpoint` by name and one of its aliases, or one typed by hand.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelChoice {
    /// The `ModelEndpoint` (`model.endpointRef.name`).
    pub endpoint: String,
    /// The alias (`model.model`). The endpoint's `models` are offers: nothing checks that it serves this one.
    pub model: String,
}

/// What is particular to a kind of agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AgentKindForm {
    /// A coder.
    Coder(CoderSettings),
    /// A folder agent.
    Folder(FolderSettings),
}

/// What a coder has of its own (§60a, *Coder*).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoderSettings {
    /// The GitHub App, one per coder (AD-033).
    pub github: GithubAppForm,
    /// Owners who may get new repositories (`coder.createRepoOwners`): a subset of the owners. Default none.
    #[serde(default)]
    pub create_repo_owners: Vec<String>,
    /// Runs advanced at once (`coder.workers`), 1 to 16. Absent: the binary's own default.
    #[serde(default)]
    pub workers: Option<i32>,
    /// Pull requests as drafts (`coder.prDraft`). Absent: the binary's own default.
    #[serde(default)]
    pub pr_draft: Option<bool>,
    /// OpenCode's model (`coder.opencodeModel`). Absent: the binary's own default (the screen offers "the
    /// same as the model").
    #[serde(default)]
    pub opencode_model: Option<String>,
    /// The commands of a run in pods of their own (`coder.runPods.sizeClass`, slice D15): a size class of the
    /// operator's chart, offered by `GET /v1/info` (`runPodSizeClasses`). Absent: no run pods, and a GitHub App coder
    /// runs its commands next to the App's private key (the access tiers are no boundary against a command).
    /// Absent on the wire when absent, so a form with no run pods is the wire form it was before the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_pod_size_class: Option<String>,
}

/// The GitHub App of a coder: its id, its private key as an offered secret key, and the owners it acts on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubAppForm {
    /// The App's client ID: public, not a secret (`github.app.id`).
    pub app_id: String,
    /// Where the App's private key is: **a reference** to an offered key (`github.app.privateKeySecretRef`),
    /// which needs the permission `platform:secrets.pick`.
    pub private_key: SecretPick,
    /// The GitHub logins the coder may act on (`github.app.owners`, `GITHUB_APP_OWNERS`): at least one,
    /// `^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$`, and never `*`.
    pub owners: Vec<String>,
}

/// What a folder agent has of its own (§60a, *Folder agent*): its folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderSettings {
    /// The folder, inline: relative path to content. `instructions.md` is in it, with its front matter;
    /// more files by relative path (`skills/…`); no `mcp.json` (the tools come from the Tool servers
    /// screen); at most 1 MiB in all.
    pub files: BTreeMap<String, String>,
}

impl AgentForm {
    /// Which kind of agent this is.
    pub fn kind(&self) -> AgentKind {
        match self.agent {
            AgentKindForm::Coder(_) => AgentKind::Coder,
            AgentKindForm::Folder(_) => AgentKind::Folder,
        }
    }

    /// The secret keys the form names, each with the form field that names it. **A write whose body names
    /// one needs `platform:secrets.pick` besides the write's own permission** (AD-032, §60a): the router
    /// asks [`names_a_secret`](Self::names_a_secret).
    pub fn secret_picks(&self) -> Vec<(String, &SecretPick)> {
        match &self.agent {
            AgentKindForm::Coder(c) => {
                vec![("agent.github.privateKey".to_owned(), &c.github.private_key)]
            }
            AgentKindForm::Folder(_) => Vec::new(),
        }
    }

    /// Whether the form names a secret key.
    pub fn names_a_secret(&self) -> bool {
        !self.secret_picks().is_empty()
    }
}
