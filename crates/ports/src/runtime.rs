//! `RuntimeProvider`: make an agent's compute exist, and say how it is doing.

use std::future::Future;

use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};

use crate::{Role, RuntimeError, RuntimeId, RuntimeSpec};

/// The phase of a runtime (§18).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Phase {
    /// Nothing exists.
    Absent,
    /// A rollout is in progress.
    Provisioning,
    /// Every replica is ready.
    Ready,
    /// Scaled to zero on purpose.
    Suspended,
    /// The runtime cannot get ready without a change.
    Failed,
}

/// Why a workload is not well. The controller turns each into a condition reason (§59a, "Status").
/// Closed on purpose: a new reason must be handled everywhere it is read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum IssueReason {
    /// The agent process refused its configuration (exit code 78).
    ConfigRejected,
    /// A dependency of the agent is unreachable: the database, an MCP server (exit code 69).
    DependencyUnavailable,
    /// A referenced Secret or key does not exist (`CreateContainerConfigError`). The provider
    /// cannot read Secrets, so the backend's answer is how it learns.
    MissingSecret {
        /// The Secret that is missing.
        name: String,
    },
    /// The image cannot be pulled.
    ImagePull,
    /// The container keeps exiting for another reason.
    CrashLoop,
    /// An object with a name the runtime needs exists and is not ours (the adoption guard). The
    /// provider changed nothing.
    NameConflict,
}

/// One thing wrong with one workload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    /// The workload's role.
    pub role: Role,
    /// What is wrong.
    pub reason: IssueReason,
    /// A sentence for a person. Never a secret's value: the provider has none.
    pub message: String,
}

/// How a runtime is doing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    /// Where it is in its life.
    pub phase: Phase,
    /// Ready replicas of the workers (`RuntimeSpec::worker_replicas` when everything is up).
    pub replicas: u32,
    /// What is wrong, if anything.
    pub issues: Vec<Issue>,
}

impl RuntimeStatus {
    /// Nothing exists.
    pub fn absent() -> Self {
        Self {
            phase: Phase::Absent,
            replicas: 0,
            issues: Vec::new(),
        }
    }

    /// Whether an issue says another object has the name the runtime needs.
    pub fn is_name_conflict(&self) -> bool {
        self.issues
            .iter()
            .any(|i| i.reason == IssueReason::NameConflict)
    }
}

/// What a provider can do. A call for something it cannot is [`RuntimeError::Unsupported`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// [`RuntimeProvider::suspend`] works, and so does `RuntimeSpec::suspend`.
    pub suspend: bool,
}

/// A protocol surface of a runtime.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Surface {
    /// The A2A endpoint: where a client sends JSON-RPC.
    A2a,
    /// The agent card.
    AgentCard,
}

/// Where a surface can be reached.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// An absolute `http` or `https` URL.
    pub url: String,
}

/// What a delete left behind.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteOutcome {
    /// There was a runtime to delete.
    pub existed: bool,
    /// Names of the persistent volumes that stay, because the deletion policy is `Retain`. A
    /// service of the same name that comes back finds them again.
    pub retained_volumes: Vec<String>,
}

/// The volumes of a deleted runtime that were kept (`Retain`), with when it was deleted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedVolumes {
    /// Whose volumes they were.
    pub id: RuntimeId,
    /// When the delete happened, in seconds since the Unix epoch.
    pub retained_at: i64,
}

/// Makes the compute of an agent exist.
///
/// A provider is a build-time choice (AD-020): the controller is generic over it. The methods are
/// native `async fn`s in the trait with a `Send` bound on the future, so no `async-trait` box is
/// paid and a controller can spawn them; the price is that the trait is not object safe, and the
/// controller does not need it to be.
///
/// Every method is **idempotent** and the id is the only identity: calling `ensure` twice with the
/// same spec is one runtime. The deletion policy of the spec of the last `ensure` is what `delete`
/// honours, so `delete(id)` needs no spec: a provider remembers it on the objects it made.
pub trait RuntimeProvider: Send + Sync {
    /// The provider's name, for `status.runtime.provider`: `kubernetes`.
    fn name(&self) -> &'static str;

    /// What this provider can do.
    fn capabilities(&self) -> Capabilities;

    /// Make the runtime match `spec`: create it, change it, or wake it (there is no separate
    /// `activate`). A runtime blocked by a name that is not ours is **not** an error: the status
    /// carries the issue and nothing was changed.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidSpec`] when [`RuntimeSpec::check`] fails, `Unsupported` when the
    /// spec asks for something [`capabilities`](Self::capabilities) denies, `Unavailable` when the
    /// backend cannot be reached.
    fn ensure(
        &self,
        id: &RuntimeId,
        spec: &RuntimeSpec,
    ) -> impl Future<Output = Result<RuntimeStatus, RuntimeError>> + Send;

    /// Scale to zero and keep everything else. Ensuring a suspended runtime again wakes it.
    ///
    /// # Errors
    ///
    /// `NotFound` when there is no runtime, `Unsupported` without the capability.
    fn suspend(
        &self,
        id: &RuntimeId,
    ) -> impl Future<Output = Result<RuntimeStatus, RuntimeError>> + Send;

    /// Remove the runtime, and its data when the deletion policy is `Delete`. Deleting what is not
    /// there is a success with `existed: false`.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the backend cannot be reached.
    fn delete(
        &self,
        id: &RuntimeId,
    ) -> impl Future<Output = Result<DeleteOutcome, RuntimeError>> + Send;

    /// The runtimes of `scope` whose volumes a `delete` with `Retain` kept and nobody claimed since, with the time
    /// of that delete. A running runtime is never in it. The default is none.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the backend cannot be reached.
    fn retained_volumes(
        &self,
        scope: &str,
    ) -> impl Future<Output = Result<Vec<RetainedVolumes>, RuntimeError>> + Send {
        let _ = scope;
        async { Ok(Vec::new()) }
    }

    /// Delete the kept volumes of `id` (the retention period ran out). A runtime that is running, or whose
    /// volumes were not kept, is left alone and the answer is `existed: false`.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the backend cannot be reached.
    fn purge_volumes(
        &self,
        id: &RuntimeId,
    ) -> impl Future<Output = Result<DeleteOutcome, RuntimeError>> + Send {
        let _ = id;
        async { Ok(DeleteOutcome::default()) }
    }

    /// How the runtime is doing. A runtime that does not exist is `Phase::Absent`, not an error.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the backend cannot be reached.
    fn status(
        &self,
        id: &RuntimeId,
    ) -> impl Future<Output = Result<RuntimeStatus, RuntimeError>> + Send;

    /// Where a surface of the runtime can be reached from inside the cluster.
    ///
    /// # Errors
    ///
    /// `NotFound` when there is no runtime.
    fn endpoint(
        &self,
        id: &RuntimeId,
        surface: Surface,
    ) -> impl Future<Output = Result<Endpoint, RuntimeError>> + Send;

    /// Ids of runtimes whose state changed since the stream was taken, so the controller never
    /// watches workloads itself. **A signal is never the truth** (the same rule as adam-rs): it makes
    /// the controller look sooner, it is at most once, and the controller still reconciles on a
    /// timer and reads [`status`](Self::status).
    fn watch(&self) -> BoxStream<'static, RuntimeId>;
}

/// An `Arc` of a provider is a provider: every call is the one it wraps. It is how a composition root hands the
/// same provider to several controllers (the operator holds one shard per namespace, K6b) when the provider is not
/// `Clone` itself, and it changes nothing of what a call does.
impl<T: RuntimeProvider + ?Sized> RuntimeProvider for std::sync::Arc<T> {
    fn name(&self) -> &'static str {
        (**self).name()
    }

    fn capabilities(&self) -> Capabilities {
        (**self).capabilities()
    }

    fn ensure(
        &self,
        id: &RuntimeId,
        spec: &RuntimeSpec,
    ) -> impl Future<Output = Result<RuntimeStatus, RuntimeError>> + Send {
        (**self).ensure(id, spec)
    }

    fn suspend(
        &self,
        id: &RuntimeId,
    ) -> impl Future<Output = Result<RuntimeStatus, RuntimeError>> + Send {
        (**self).suspend(id)
    }

    fn delete(
        &self,
        id: &RuntimeId,
    ) -> impl Future<Output = Result<DeleteOutcome, RuntimeError>> + Send {
        (**self).delete(id)
    }

    fn retained_volumes(
        &self,
        scope: &str,
    ) -> impl Future<Output = Result<Vec<RetainedVolumes>, RuntimeError>> + Send {
        (**self).retained_volumes(scope)
    }

    fn purge_volumes(
        &self,
        id: &RuntimeId,
    ) -> impl Future<Output = Result<DeleteOutcome, RuntimeError>> + Send {
        (**self).purge_volumes(id)
    }

    fn status(
        &self,
        id: &RuntimeId,
    ) -> impl Future<Output = Result<RuntimeStatus, RuntimeError>> + Send {
        (**self).status(id)
    }

    fn endpoint(
        &self,
        id: &RuntimeId,
        surface: Surface,
    ) -> impl Future<Output = Result<Endpoint, RuntimeError>> + Send {
        (**self).endpoint(id, surface)
    }

    fn watch(&self) -> BoxStream<'static, RuntimeId> {
        (**self).watch()
    }
}
