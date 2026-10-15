//! `AgentObjects`: the custom resources the dashboard writes, as the Platform API reads and writes
//! them (§60a, AD-025, AD-030, AD-031).
//!
//! The dashboard owns four kinds: an **agent** is an `AgentService` and the `AgentConfig` it runs (one
//! unit here: they are saved, read and deleted together and share one [`ETag`]), and the shared
//! `ModelEndpoint` and `ToolProvider` objects the agents name (AD-029). The custom resources stay the
//! only store of agent configuration (AD-016): an implementation keeps no database of its own.
//!
//! **No Kubernetes type crosses this seam**, and neither does a type of `aap-api`: a body is the JSON
//! an API server holds as `spec` (and `status`), a name is a string, and what the API server adds
//! (a `resourceVersion`, a `generation`, labels and annotations) is a neutral field of [`Stored`]. The
//! typed side of a body is `aap-forms`', which builds a draft from a form and reads a form back.
//!
//! **A secret value cannot cross it either.** [`OfferedSecret`] has a name, a key and the name of the
//! ExternalSecret that makes it, and no field for a value (AD-024, AD-030); an implementation has no
//! right on Secrets.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{BoxError, Classify, ErrorClass};

/// The label every object the dashboard writes carries (§60a, "Applying").
pub const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";

/// The value of [`MANAGED_BY_LABEL`] on the dashboard's objects.
pub const MANAGED_BY_DASHBOARD: &str = "dashboard.agents.vymalo.com";

/// The annotation Argo CD puts on what it tracks (its default tracking method; *verified 2026-10-05*,
/// §60a, "GitOps and the dashboard"). An object that has it is GitOps's, whatever else it carries.
pub const ARGO_TRACKING_ANNOTATION: &str = "argocd.argoproj.io/tracking-id";

/// The label that offers the keys of an ExternalSecret to the forms (AD-030): `agents.vymalo.com/offer:
/// "true"`. The default of the deployment's `offer_label`; an implementation is told which to read.
pub const OFFER_LABEL: &str = "agents.vymalo.com/offer";

/// The kinds the dashboard reads and writes. Closed: a new one must fail to compile in every
/// implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Kind {
    /// The addressable agent.
    AgentService,
    /// What the agent is made of.
    AgentConfig,
    /// A shared model endpoint.
    ModelEndpoint,
    /// A shared tool server.
    ToolProvider,
}

impl Kind {
    /// The kind as the API server spells it (`AgentService`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AgentService => "AgentService",
            Self::AgentConfig => "AgentConfig",
            Self::ModelEndpoint => "ModelEndpoint",
            Self::ToolProvider => "ToolProvider",
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The two kinds of shared object, in the words of the routes: `/v1/models` and `/v1/tool-servers`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SharedKind {
    /// A `ModelEndpoint`.
    Model,
    /// A `ToolProvider`.
    ToolServer,
}

impl SharedKind {
    /// The custom resource kind.
    pub const fn kind(self) -> Kind {
        match self {
            Self::Model => Kind::ModelEndpoint,
            Self::ToolServer => Kind::ToolProvider,
        }
    }
}

/// The `resourceVersion` of one object, **opaque**: only equality means anything. An implementation
/// that has none makes one (a counter).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResourceVersion(String);

impl ResourceVersion {
    /// A version from the backend's own token.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The optimistic-concurrency token of what a caller read (§60a: "the `resourceVersion` of both objects
/// as an `ETag`"). It is made from the versions of **every object** of the unit, so a change of either
/// the `AgentService` or the `AgentConfig` makes the old token stale. Opaque to a caller, who echoes it
/// back in `If-Match` and never looks inside; the Platform API quotes it as a strong `ETag`.
///
/// It is derived, never stored, by [`Stored::etag`] and [`Agent::etag`]: the encoding is one definition
/// here, so every implementation agrees on what "the same" means.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ETag(String);

impl ETag {
    /// A token the caller sent back (the header's value without its quotes).
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// The token, for the header.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Length-prefixed, so no pair of versions can spell the token of another pair; an object that is not
    /// there is `-`, which no length-prefixed version is.
    fn of(versions: &[Option<&ResourceVersion>]) -> Self {
        let mut token = String::new();
        for (i, v) in versions.iter().enumerate() {
            if i > 0 {
                token.push('.');
            }
            match v {
                Some(v) => {
                    token.push_str(&v.as_str().len().to_string());
                    token.push(':');
                    token.push_str(v.as_str());
                }
                None => token.push('-'),
            }
        }
        Self(token)
    }
}

/// Who may write an object (AD-031): **one owner per object**.
///
/// The dashboard owns an object when it carries the label [`MANAGED_BY_LABEL`] with the value
/// [`MANAGED_BY_DASHBOARD`] **and** no Argo CD tracking annotation. Everything else is GitOps's for the
/// dashboard, whoever really applied it (Argo CD, Flux, `kubectl`): read-only, with View YAML.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Owner {
    /// The dashboard wrote it and may write it again.
    Dashboard,
    /// Not the dashboard's: shown read-only, never written, never deleted.
    GitOps,
}

impl Owner {
    /// The rule of AD-031, in one place for every implementation.
    pub fn of(labels: &BTreeMap<String, String>, annotations: &BTreeMap<String, String>) -> Self {
        let labelled =
            labels.get(MANAGED_BY_LABEL).map(String::as_str) == Some(MANAGED_BY_DASHBOARD);
        if labelled && !annotations.contains_key(ARGO_TRACKING_ANNOTATION) {
            Self::Dashboard
        } else {
            Self::GitOps
        }
    }

    /// Whether the dashboard may write the object.
    pub const fn writable(self) -> bool {
        matches!(self, Self::Dashboard)
    }
}

/// One object of the dashboard's kinds, as an API server holds it, reduced to what the dashboard needs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stored {
    /// Its kind.
    pub kind: Kind,
    /// Its namespace (the Platform API's one namespace).
    pub namespace: String,
    /// Its name.
    pub name: String,
    /// Its `resourceVersion`.
    pub version: ResourceVersion,
    /// Its `metadata.generation`: ahead of `status.observedGeneration` while the operator has not seen the
    /// last change (the state *Saving* of §60a).
    pub generation: i64,
    /// Its labels, all of them.
    pub labels: BTreeMap<String, String>,
    /// Its annotations, all of them.
    pub annotations: BTreeMap<String, String>,
    /// Its `spec`, as JSON.
    pub spec: Value,
    /// Its `status`, as JSON: what the operator observed. `None` for a kind that has none, and before
    /// the operator has written any. The dashboard never writes it.
    pub status: Option<Value>,
}

impl Stored {
    /// Who may write it.
    pub fn owner(&self) -> Owner {
        Owner::of(&self.labels, &self.annotations)
    }

    /// The token of this object alone (a shared object's).
    pub fn etag(&self) -> ETag {
        ETag::of(&[Some(&self.version)])
    }
}

/// An agent: its `AgentService` and the `AgentConfig` that service names (`spec.configRef.name`). The
/// config is `None` when the service names one that does not exist; the operator then marks the service
/// `Blocked`, and the dashboard shows that.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Agent {
    /// The addressable agent. Its name is the agent's.
    pub service: Stored,
    /// What it runs.
    pub config: Option<Stored>,
}

impl Agent {
    /// The agent's name.
    pub fn name(&self) -> &str {
        &self.service.name
    }

    /// Who may write it: the dashboard only when it owns **both** objects.
    pub fn owner(&self) -> Owner {
        let config_owner = self.config.as_ref().map_or(Owner::Dashboard, Stored::owner);
        if self.service.owner().writable() && config_owner.writable() {
            Owner::Dashboard
        } else {
            Owner::GitOps
        }
    }

    /// The token of the two objects: stale when either changed.
    pub fn etag(&self) -> ETag {
        ETag::of(&[
            Some(&self.service.version),
            self.config.as_ref().map(|c| &c.version),
        ])
    }
}

/// What the dashboard writes for an agent: the `spec` of each of its two objects. Both objects are
/// named `name`, and `service.configRef.name` must say so; the implementation adds the labels.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentDraft {
    /// The agent's name, and the name of both objects.
    pub name: String,
    /// The `AgentService`'s `spec`.
    pub service: Value,
    /// The `AgentConfig`'s `spec`.
    pub config: Value,
}

/// What the dashboard writes for a shared object: its `spec`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SharedDraft {
    /// The object's name.
    pub name: String,
    /// Its `spec`.
    pub spec: Value,
}

/// What a write requires of the object it finds (`PUT`'s `If-None-Match` and `If-Match`). There is no
/// unconditional write: two administrators must never overwrite each other silently (§60a, *Revisions*).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Precondition {
    /// Create: the object must not exist (`If-None-Match: *`).
    DoesNotExist,
    /// Replace: the object must be the one that was read (`If-Match`).
    Matches(ETag),
}

/// A key of a Secret an agent may name (AD-030): what `GET /v1/secret-keys` lists. **No value**: the
/// type has no field for one, so nothing that holds an `OfferedSecret` can leak a secret.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferedSecret {
    /// The Secret's name (the `target.name` of the ExternalSecret).
    pub secret: String,
    /// The key inside it (the `secretKey` of one of its `data` entries).
    pub key: String,
    /// The ExternalSecret that makes the Secret.
    pub external_secret: String,
}

/// A mistake in a body, with the field it is about, as an API server's validation reports it. The field
/// is a path from the object in the style of `aap-domain`'s issues: `AgentService spec.access.audience`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldError {
    /// The field, `<Kind> <path>`.
    pub field: String,
    /// What is wrong.
    pub message: String,
}

impl fmt::Display for FieldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.field, self.message)
    }
}

/// The names an `AgentConfig` references (the dashboard's two shared kinds), read from its `spec` as JSON:
/// `model.endpointRef.name` and each `tools.mcpServers.<n>.providerRef.name`. This is the rule of
/// `aap_domain::references` without the check that a name is one: `aap-forms` holds the two equal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct References {
    /// The `ModelEndpoint` the model names.
    pub model_endpoint: Option<String>,
    /// The `ToolProvider`s the servers name, once each.
    pub tool_providers: BTreeSet<String>,
}

impl References {
    /// Whether `name` of `kind` is among them. Only the shared kinds can be referenced.
    pub fn names(&self, kind: SharedKind, name: &str) -> bool {
        match kind {
            SharedKind::Model => self.model_endpoint.as_deref() == Some(name),
            SharedKind::ToolServer => self.tool_providers.contains(name),
        }
    }
}

/// What the `AgentConfig` spec `config` references.
pub fn config_references(config: &Value) -> References {
    let name_of = |v: &Value| {
        v.pointer("/name")
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    References {
        model_endpoint: config.pointer("/model/endpointRef").and_then(name_of),
        tool_providers: config
            .pointer("/tools/mcpServers")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|servers| servers.values())
            .filter_map(|server| server.get("providerRef").and_then(name_of))
            .collect(),
    }
}

/// The `AgentConfig` an `AgentService` spec names: `configRef.name`.
pub fn config_name(service: &Value) -> Option<&str> {
    service.pointer("/configRef/name").and_then(Value::as_str)
}

/// Why a call of [`AgentObjects`] failed. **Each variant has one status in the Platform API** (the table
/// is in the README of this crate, and `tests/contract.rs` holds it): the router maps by variant.
#[derive(Debug, thiserror::Error)]
pub enum ObjectsError {
    /// A create found the object (or, for an agent, its `AgentService`). `409`.
    #[error("{kind} {name} already exists")]
    AlreadyExists {
        /// The kind that exists.
        kind: Kind,
        /// Its name.
        name: String,
    },
    /// A replace or a delete found nothing. `404`.
    #[error("{kind} {name} was not found")]
    NotFound {
        /// The kind that is missing.
        kind: Kind,
        /// Its name.
        name: String,
    },
    /// The [`ETag`] of a replace or a delete is not the object's: somebody saved first. Read again.
    /// `412`.
    #[error("the object changed since it was read")]
    PreconditionFailed,
    /// The object is not the dashboard's ([`Owner::GitOps`]), so it is neither written nor deleted
    /// (AD-031). `409`.
    #[error("{kind} {name} is managed by GitOps, not by the dashboard")]
    NotOwned {
        /// The kind that is not ours.
        kind: Kind,
        /// Its name.
        name: String,
    },
    /// A delete of a shared object that agents still name. `409`, with `by`, the names of those agents
    /// (their `AgentService`s), in byte order and once each.
    #[error("{kind} {name} is still used by {}", by.join(", "))]
    Referenced {
        /// The shared kind.
        kind: Kind,
        /// Its name.
        name: String,
        /// The agents that name it.
        by: Vec<String>,
    },
    /// Another manager owns a field of the object (a server-side apply conflict). `409`. Read again.
    #[error("conflict: {0}")]
    Conflict(String),
    /// The backend refused the body, field by field (a CEL rule of a CRD, an unreadable `spec`). `422`.
    #[error("the body is invalid: {}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
    Invalid(Vec<FieldError>),
    /// The backend cannot be reached (the API server). `503`.
    #[error("the object store is unavailable")]
    Unavailable(#[source] BoxError),
    /// A bug. `500`.
    #[error("internal error")]
    Internal(#[source] BoxError),
}

impl Classify for ObjectsError {
    fn class(&self) -> ErrorClass {
        match self {
            // The same call never succeeds until something else changes.
            Self::AlreadyExists { .. }
            | Self::NotOwned { .. }
            | Self::Referenced { .. }
            | Self::Invalid(_) => ErrorClass::Invalid,
            Self::NotFound { .. } => ErrorClass::NotFound,
            // Lost a race: read again, retry.
            Self::PreconditionFailed | Self::Conflict(_) => ErrorClass::Conflict,
            Self::Unavailable(_) => ErrorClass::Transient,
            Self::Internal(_) => ErrorClass::Internal,
        }
    }
}

/// The dashboard's custom resources. Implemented on the API server by `objects-kubernetes` (D5, server-side
/// apply with the field manager `agents.vymalo.com/dashboard`); `MemoryObjects` (feature `testkit`) is the
/// reference. An implementation is bound to one namespace.
///
/// What every implementation does, and `agent_objects_conformance!` holds:
///
/// * **Ownership (AD-031).** A write or a delete of an object that is not [`Owner::Dashboard`] is
///   [`ObjectsError::NotOwned`], before anything is changed. A write adds [`MANAGED_BY_LABEL`] to what it
///   writes. A read returns every object, the dashboard's or not.
/// * **No lost update.** Every write has a [`Precondition`]; a stale [`ETag`] is
///   [`ObjectsError::PreconditionFailed`] and nothing is changed.
/// * **The status is the operator's.** A write never touches it.
/// * **References are not checked on a write** (the operator marks an agent `ConfigInvalid` naming a
///   missing referent, and the API validates before it writes); they are checked on a **delete**: a shared
///   object some agent names is [`ObjectsError::Referenced`].
/// * **An agent is two objects, written in order**: the `AgentConfig`, then the `AgentService` (the
///   operator never sees a service whose config is not there yet); deleted in the other order. A write
///   that fails between the two does not leave a config that a retry of the create cannot take over.
pub trait AgentObjects: Send + Sync {
    /// Every agent of the namespace, ordered by name.
    ///
    /// # Errors
    ///
    /// [`ObjectsError::Unavailable`].
    fn list_agents(&self) -> impl Future<Output = Result<Vec<Agent>, ObjectsError>> + Send;

    /// One agent, or `None`.
    ///
    /// # Errors
    ///
    /// As [`list_agents`](Self::list_agents).
    fn get_agent(
        &self,
        name: &str,
    ) -> impl Future<Output = Result<Option<Agent>, ObjectsError>> + Send;

    /// Create or replace both objects of an agent and return what was written, with the new tokens.
    ///
    /// A create ([`Precondition::DoesNotExist`]) needs **the `AgentService` to be absent**; a
    /// dashboard-owned `AgentConfig` of that name, left by a create that failed halfway, is taken over.
    /// A replace ([`Precondition::Matches`]) needs the agent to exist with that token.
    ///
    /// # Errors
    ///
    /// [`ObjectsError::AlreadyExists`] (create), [`ObjectsError::NotFound`] (replace),
    /// [`ObjectsError::PreconditionFailed`], [`ObjectsError::NotOwned`], [`ObjectsError::Invalid`] (a body
    /// that is not an object, a `configRef` that is not the agent's name, a rule the backend states),
    /// [`ObjectsError::Conflict`], [`ObjectsError::Unavailable`].
    fn put_agent(
        &self,
        draft: &AgentDraft,
        when: &Precondition,
    ) -> impl Future<Output = Result<Agent, ObjectsError>> + Send;

    /// Delete the `AgentService`, then its `AgentConfig`. `deletionPolicy` decides what the operator does
    /// with the data (§59a). With `Some(etag)` the agent must still be the one that was read.
    ///
    /// # Errors
    ///
    /// [`ObjectsError::NotFound`], [`ObjectsError::PreconditionFailed`], [`ObjectsError::NotOwned`],
    /// [`ObjectsError::Unavailable`].
    fn delete_agent(
        &self,
        name: &str,
        when: Option<&ETag>,
    ) -> impl Future<Output = Result<(), ObjectsError>> + Send;

    /// Every shared object of the kind, ordered by name.
    ///
    /// # Errors
    ///
    /// [`ObjectsError::Unavailable`].
    fn list_shared(
        &self,
        kind: SharedKind,
    ) -> impl Future<Output = Result<Vec<Stored>, ObjectsError>> + Send;

    /// One shared object, or `None`.
    ///
    /// # Errors
    ///
    /// As [`list_shared`](Self::list_shared).
    fn get_shared(
        &self,
        kind: SharedKind,
        name: &str,
    ) -> impl Future<Output = Result<Option<Stored>, ObjectsError>> + Send;

    /// Create or replace a shared object and return what was written.
    ///
    /// # Errors
    ///
    /// As [`put_agent`](Self::put_agent), without the `configRef`.
    fn put_shared(
        &self,
        kind: SharedKind,
        draft: &SharedDraft,
        when: &Precondition,
    ) -> impl Future<Output = Result<Stored, ObjectsError>> + Send;

    /// Delete a shared object nobody names.
    ///
    /// # Errors
    ///
    /// [`ObjectsError::Referenced`] while an agent names it (GitOps's agents count), then as
    /// [`delete_agent`](Self::delete_agent).
    fn delete_shared(
        &self,
        kind: SharedKind,
        name: &str,
        when: Option<&ETag>,
    ) -> impl Future<Output = Result<(), ObjectsError>> + Send;

    /// The Secret keys the forms may name (AD-030), ordered by `(secret, key, external_secret)`: the
    /// `data[].secretKey` of every ExternalSecret that carries the offer label, under its `target.name`.
    /// An ExternalSecret holds no value, and this reads no Secret.
    ///
    /// # Errors
    ///
    /// [`ObjectsError::Unavailable`].
    fn offered_secrets(
        &self,
    ) -> impl Future<Output = Result<Vec<OfferedSecret>, ObjectsError>> + Send;
}
