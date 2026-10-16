//! `AgentObjects` on the Kubernetes API server (§60a, slice D5, AD-025, AD-030, AD-031).
//!
//! The dashboard's four kinds, `AgentService`, `AgentConfig`, `ModelEndpoint` and `ToolProvider`, are written by
//! **server-side apply** under the field manager [`FIELD_MANAGER`], with `force` off, each carrying the label
//! `app.kubernetes.io/managed-by: dashboard.agents.vymalo.com`. The `status` is never written. The ExternalSecrets
//! that carry the offer label are read for the keys the forms may name; **no Secret is read**.
//!
//! # An agent is two objects, and the API server writes one at a time
//!
//! A replace reads both objects, checks everything that can be checked from what it read, **dry-runs both applies**
//! (`dryRun=All`, each with the `resourceVersion` and the `uid` it read) and only then applies the `AgentConfig`
//! and the `AgentService` for real. So a stale token on either object, a rule of the CRDs on either body, and a
//! field another manager owns are all refused while nothing has changed. What remains is the window between the two
//! real applies, and **every way out of it leaves the config written**, says why, and is logged at `warn` with the
//! names and the kinds and no value:
//!
//! | The service, when its apply comes | The answer | A retry |
//! |---|---|---|
//! | only its `resourceVersion` moved (the operator wrote its `status`, or put its finalizer on) | none: the apply is repeated **once** with the version it has now, and the save succeeds | not needed |
//! | its `spec`, labels or annotations changed, or it was made again (another `uid`), or it is gone | `PreconditionFailed` (`NotFound` for a replace of a service that is gone, `AlreadyExists` for a create over one) | reads both again, succeeds |
//! | it began to be deleted | `Deleting` | after it is gone, as a create |
//! | a field the apply sets is owned by another manager | `Conflict`, with the server's words | refused by the dry run, with nothing written, until they let go of the field |
//! | the API server fails the apply (`5xx`, `429`, a timeout) | `Unavailable` | reads both again, succeeds when it is back |
//!
//! The config that a create leaves is a dashboard-owned config that no service names, which the next create takes over
//! and which [`list_orphans`](aap_ports::AgentObjects::list_orphans) lists. The same repeat-once applies to the config
//! apply and to a shared object's apply (nothing is half-written there: only the version moved).
//!
//! # A delete waits for the operator, and the config goes last
//!
//! The operator puts a finalizer on every service it sees. Its cleanup, when the finalizer runs, **reads the
//! `AgentConfig` to honour `deletionPolicy`** (a person who saved `Retain` over `Delete` and deleted at once has had no
//! reconcile in between), and a config it cannot read leaves the policy it remembers standing: the volumes and the
//! database go. So a delete of an agent never deletes the config while the service is there. It deletes both as a dry run,
//! then the service, then **waits for the service to go, for at most [`DELETE_WAIT`]** (5 s: long enough for the operator's
//! reconcile, which the watch starts at once, short enough for an HTTP request that a person waits for), and only then
//! lists the services once more and deletes the config, unless a service made meanwhile names it (a window of one request
//! remains: a service made between that list and the config's delete loses its config, and the operator reports it
//! `ConfigNotFound`). When the operator is not done in time the delete **returns anyway**: a delete returns once the service
//! is gone or is being deleted, and the config goes when the service is gone. What is left is a dashboard-owned config that no
//! service names, an *orphan*: `list_orphans` lists it with its token and a delete of its name removes it. A config that
//! is GitOps's, or that a service names, is never deleted that way.
//!
//! # What a `resourceVersion` does in an apply (*verified 2026-10-16*, kube-apiserver v1.35.8)
//!
//! Kubernetes documents that a client that needs to detect lost updates should make its request conditional on the
//! `resourceVersion`, "either HTTP PUT or HTTP PATCH"
//! (<https://kubernetes.io/docs/reference/using-api/api-concepts/>, "Updates to existing resources", *verified
//! 2026-10-16*: the page was fetched and read). It does not say what an *apply* does with one (the server-side apply page
//! only says that a controller "doesn't have to" specify it, *verified* the same day), so it was observed, on the
//! v1.35.8 only, by `tests/cluster.rs` and by hand:
//!
//! * an apply whose `metadata.resourceVersion` is not the object's is a `409 Conflict` (`the object has been
//!   modified; please apply your changes to the latest version and try again`), **also when the apply would
//!   change nothing**, and also under `dryRun=All`;
//! * an apply whose `metadata.uid` is not the object's is a `422` (`field is immutable`), and, when the object is
//!   not there, a `409` (`uid mismatch ... and no existing object was found`): **a write that names the `uid` it
//!   read can never create the object again after somebody deleted it**;
//! * an apply that names a `resourceVersion` for an object that is **not there** creates it (the version is
//!   ignored). A create therefore names a version that no object has, [`NEVER`], so that an object made in the
//!   meantime is a `409` and is never merged into. **That `"1"` is pinned on v1.35.8 only**: the API server writes
//!   its own objects first, so an object never has version `1` there, and `tests/cluster.rs`
//!   (`a_create_names_a_version_no_object_has_and_never_merges_into_one`) fails the day a server gives one;
//! * a delete takes `preconditions.resourceVersion` and `preconditions.uid` in its body (`409` when they differ),
//!   also with `dryRun: ["All"]` in the body; **the query parameter `dryRun` is ignored when the delete has a body**;
//! * a delete of an object that has finalizers answers the object, with a `deletionTimestamp`, and keeps it until the
//!   last one is removed; an apply of such an object is accepted.
//!
//! # What the roles must be
//!
//! A server-side apply needs `patch`, and `create` as well to make an object (*verified 2026-10-16*: with `patch`
//! alone the answer is `403` for the create, dry-run or not; and the Kubernetes documentation says the same, "a principal
//! ... requires the patch permission to edit existing resources, and also needs the create verb permission in order to
//! create new resources with Server-Side Apply", <https://kubernetes.io/docs/reference/using-api/server-side-apply/>,
//! "Access control and permissions", *verified 2026-10-16*: the page was fetched and read). The verbs this crate uses are
//! in its README and in `tests/rbac.rs`, which holds them equal to the calls it makes.
//!
//! No Kubernetes type is in a signature the port defines.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod external;
mod failure;
mod stored;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use aap_api::{AgentConfig, AgentService, ModelEndpoint, ToolProvider};
use aap_ports::{
    Agent, AgentDraft, AgentObjects, ETag, FieldError, Kind, MANAGED_BY_DASHBOARD,
    MANAGED_BY_LABEL, OFFER_LABEL, ObjectsError, OfferedSecret, Precondition, SharedDraft,
    SharedKind, Stored, config_name, config_references,
};
pub use external::EXTERNAL_SECRET_API_VERSION;
use failure::{Failure, classify};
use kube::Client;
use kube::api::{
    Api, ApiResource, DeleteParams, DynamicObject, GroupVersionKind, ListParams, Patch,
    PatchParams, Preconditions, PropagationPolicy,
};
use serde_json::{Value, json};
use stored::Live;

/// The field manager of every apply (§60a, *Applying*). It owns the fields the dashboard sets, and no other
/// manager's: the operator writes `status` and a finalizer, and GitOps tools write their own objects.
pub const FIELD_MANAGER: &str = "agents.vymalo.com/dashboard";

/// How long a delete waits, at most, for the operator to finish with the service ([`KubernetesObjects::with_delete_wait`]).
///
/// The operator's finalizer releases the compute and the store in one pass of its reconciler, which the watch starts at
/// once: under `deletionPolicy: Retain` that is usually a second or two. A delete is an HTTP request that a person
/// waits for, behind proxies that cut a request off after some tens of seconds, and the page asks again every 2 seconds
/// (§60a): a few seconds let the common delete answer with the agent gone, and hold a worker for no longer when it is
/// not. The figure is a choice, not a measurement.
pub const DELETE_WAIT: Duration = Duration::from_secs(5);

/// The `resourceVersion` a create names: the API server ignores the version of an object that is not there, and
/// refuses it with a `409` for one that is, so that a create never merges into an object made in the meantime.
/// *Verified 2026-10-16* (see the crate documentation). An object never has version `1`: the API server writes
/// its own objects first.
pub const NEVER: &str = "1";

/// The dashboard's objects of one namespace, on the API server of `client`.
///
/// Cheap to clone: every clone shares one client. It needs the verbs of the README on the four kinds and `list`
/// on `externalsecrets.external-secrets.io` in its namespace, and **no right on Secrets**.
#[derive(Clone)]
pub struct KubernetesObjects {
    client: Client,
    namespace: String,
    offer_label: String,
    delete_wait: Duration,
}

impl KubernetesObjects {
    /// The objects of `namespace`, offering the keys of the ExternalSecrets that carry [`OFFER_LABEL`]
    /// (`agents.vymalo.com/offer: "true"`).
    pub fn new(client: Client, namespace: impl Into<String>) -> Self {
        Self {
            client,
            namespace: namespace.into(),
            offer_label: OFFER_LABEL.to_owned(),
            delete_wait: DELETE_WAIT,
        }
    }

    /// Offer the keys of the ExternalSecrets that carry this label with the value `true` instead (the
    /// deployment's `offer_label`).
    #[must_use]
    pub fn with_offer_label(mut self, label: impl Into<String>) -> Self {
        self.offer_label = label.into();
        self
    }

    /// Wait this long, at most, for the service of a deleted agent to go before a delete answers ([`DELETE_WAIT`] by
    /// default). When the operator is done in time the config is deleted too and the agent is gone; when it is not, the
    /// delete is accepted all the same and the config stays until the service has gone (see `delete_agent`).
    #[must_use]
    pub fn with_delete_wait(mut self, wait: Duration) -> Self {
        self.delete_wait = wait;
        self
    }

    /// The namespace this serves.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    fn api(&self, kind: Kind) -> Api<DynamicObject> {
        Api::namespaced_with(self.client.clone(), &self.namespace, &resource(kind))
    }

    /// One object, or `None` when there is none. A name that no object can have is not asked for: it goes into
    /// the path of the request, where `x?watch=true` would be a query.
    async fn get(&self, kind: Kind, name: &str) -> Result<Option<Live>, ObjectsError> {
        if !is_name(name) {
            return Ok(None);
        }
        match self.api(kind).get(name).await {
            Ok(object) => Ok(Some(Live::from_object(kind, &self.namespace, object))),
            Err(error) => match classify(error) {
                Failure::NotFound(_) => Ok(None),
                other => Err(failed(&resource(kind), other)),
            },
        }
    }

    /// Every object of a kind, by name.
    async fn list(&self, kind: Kind) -> Result<Vec<Live>, ObjectsError> {
        let list = self
            .api(kind)
            .list(&ListParams::default())
            .await
            .map_err(|e| failed(&resource(kind), classify(e)))?;
        let mut live: Vec<Live> = list
            .items
            .into_iter()
            .map(|o| Live::from_object(kind, &self.namespace, o))
            .collect();
        live.sort_by(|a, b| a.stored.name.cmp(&b.stored.name));
        Ok(live)
    }

    /// The agent whose service is `service`: the config it names.
    async fn agent_of(&self, service: Live) -> Result<Agent, ObjectsError> {
        let config = match config_name(&service.stored.spec) {
            Some(name) => self.get(Kind::AgentConfig, name).await?,
            None => None,
        };
        Ok(Agent {
            service: service.stored,
            config: config.map(|c| c.stored),
        })
    }

    /// The services other than `except` that name the config `config`, in byte order.
    fn naming(services: &[Live], config: &str, except: &str) -> Vec<String> {
        services
            .iter()
            .filter(|s| s.stored.name != except && config_name(&s.stored.spec) == Some(config))
            .map(|s| s.stored.name.clone())
            .collect()
    }

    /// One server-side apply.
    async fn apply(&self, write: &Write<'_>, dry_run: bool) -> Result<Live, Failure> {
        let mut params = PatchParams::apply(FIELD_MANAGER);
        if dry_run {
            params = params.dry_run();
        }
        let body = write.body();
        match self
            .api(write.kind)
            .patch(write.name, &params, &Patch::Apply(&body))
            .await
        {
            Ok(object) => Ok(Live::from_object(write.kind, &self.namespace, object)),
            // An apply that finds no namespace is a deployment's mistake, not a missing object.
            Err(error) => Err(match classify(error) {
                Failure::NotFound(e) => Failure::Internal(e),
                other => other,
            }),
        }
    }

    /// Apply `writes` in order: all of them as a dry run first, then for real.
    ///
    /// A dry run is refused for the same reasons a real apply is, so a refusal of any of them is made while
    /// nothing has changed. Mistakes in a body are all reported, the first other failure is returned at once.
    async fn write(&self, writes: &[Write<'_>]) -> Result<Vec<Live>, ObjectsError> {
        let mut mistakes = Vec::new();
        for write in writes {
            match self.apply(write, true).await {
                Ok(_) => {}
                Err(Failure::Invalid(problems)) => {
                    mistakes.extend(problems.into_iter().map(|p| p.into_field(write.kind)));
                }
                Err(other) => return Err(self.refused(write, other).await),
            }
        }
        if !mistakes.is_empty() {
            return Err(ObjectsError::Invalid(mistakes));
        }
        let mut written: Vec<Live> = Vec::with_capacity(writes.len());
        for write in writes {
            match self.apply_for_real(write).await {
                Ok(live) => written.push(live),
                Err(error) => {
                    if let Some(first) = written.first() {
                        // The one window: the first object is written and the second is not. Names and kinds, and a
                        // fixed word for what went wrong; never a value, never the server's own words.
                        tracing::warn!(
                            namespace = %self.namespace,
                            name = write.name,
                            written = %first.stored.kind,
                            failed = %write.kind,
                            outcome = outcome(&error),
                            "an agent was written halfway: its config is written and its service is not; a retry reads both again"
                        );
                    }
                    return Err(error);
                }
            }
        }
        Ok(written)
    }

    /// One real apply of `write`.
    ///
    /// A `409` about the version of an object that was read is looked into **once**: the operator writes the `status`
    /// of a service when its config changes (and the status of the config when it has resolved it), which gives the
    /// object a new `resourceVersion` between the dry run and this apply without changing anything the person's save is
    /// about. When the object is the one that was read, with the same `spec`, labels and annotations, and is not being
    /// deleted, the apply is repeated once with the version it has now. Anything else is a lost race.
    async fn apply_for_real(&self, write: &Write<'_>) -> Result<Live, ObjectsError> {
        let failure = match self.apply(write, false).await {
            Ok(live) => return Ok(live),
            Err(failure) => failure,
        };
        let (Failure::Version, Guard::Exactly(read)) = (&failure, &write.guard) else {
            return Err(self.refused(write, failure).await);
        };
        let now = self.get(write.kind, write.name).await?;
        match now {
            Some(now) if now.only_moved_since(read) => {
                let again = Write::new(write.kind, write.name, write.spec, Guard::of(Some(&now)));
                match self.apply(&again, false).await {
                    Ok(live) => Ok(live),
                    Err(failure) => Err(self.refused(&again, failure).await),
                }
            }
            now => Err(write_race_outcome(
                write.kind,
                write.name,
                &write.guard,
                now.as_ref(),
            )),
        }
    }

    /// What a refused apply of `write` is, for the person.
    async fn refused(&self, write: &Write<'_>, failure: Failure) -> ObjectsError {
        match failure {
            Failure::Version => self.lost_race(write.kind, write.name, &write.guard).await,
            Failure::FieldConflict(message) => {
                ObjectsError::Conflict(format!("{} {}: {message}", write.kind, write.name))
            }
            Failure::Invalid(problems) => ObjectsError::Invalid(
                problems
                    .into_iter()
                    .map(|p| p.into_field(write.kind))
                    .collect(),
            ),
            other => failed(&resource(write.kind), other),
        }
    }

    /// A write found the object changed, or made or deleted, since it was read: what that is.
    async fn lost_race(&self, kind: Kind, name: &str, guard: &Guard) -> ObjectsError {
        match self.get(kind, name).await {
            Ok(now) => write_race_outcome(kind, name, guard, now.as_ref()),
            Err(error) => error,
        }
    }

    /// Delete `live`, bound to the object that was read. An object that has finalizers is not gone when the API server
    /// has answered: it is [`Deleted::Terminating`] until they have run.
    async fn delete_one(
        &self,
        live: &Live,
        strict: bool,
        dry_run: bool,
    ) -> Result<Deleted, ObjectsError> {
        let params = DeleteParams {
            dry_run,
            propagation_policy: Some(PropagationPolicy::Background),
            preconditions: Some(Preconditions {
                // The version of the object is a precondition where the token covered it. The `uid` always is:
                // a name that came back is another object.
                resource_version: strict.then(|| live.version().to_owned()),
                uid: Some(live.uid.clone()).filter(|u| !u.is_empty()),
            }),
            ..DeleteParams::default()
        };
        let kind = live.stored.kind;
        let name = live.stored.name.as_str();
        match self.api(kind).delete(name, &params).await {
            // The API server answers the object, with a `deletionTimestamp`, while finalizers are to run, and a
            // `Status` once it is gone (*observed* on a v1.35.8).
            Ok(answer) if answer.is_left() => Ok(Deleted::Terminating),
            Ok(_) => Ok(Deleted::Gone),
            Err(error) => Err(match classify(error) {
                Failure::NotFound(_) => ObjectsError::NotFound {
                    kind,
                    name: name.to_owned(),
                },
                Failure::Version => match self.get(kind, name).await {
                    Ok(now) => race_outcome(kind, name, &Guard::of(Some(live)), now.as_ref()),
                    Err(error) => error,
                },
                other => failed(&resource(kind), other),
            }),
        }
    }

    /// Whether the service has gone, asked again every few milliseconds for at most the delete wait. A service of
    /// the name that is another object (made again) is the old one gone. An API server that cannot be asked ends the
    /// wait: the delete was accepted, and the operator finishes it.
    async fn wait_until_gone(&self, service: &Live) -> bool {
        let name = service.stored.name.as_str();
        let poll =
            (self.delete_wait / 20).clamp(Duration::from_millis(10), Duration::from_millis(250));
        let deadline = Instant::now() + self.delete_wait;
        loop {
            match self.get(Kind::AgentService, name).await {
                Ok(None) => return true,
                Ok(Some(now)) if now.uid != service.uid => return true,
                Ok(Some(_)) => {}
                Err(error) => {
                    tracing::warn!(
                        service = name,
                        outcome = outcome(&error),
                        "could not ask whether a deleted service has gone; the config stays"
                    );
                    return false;
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            tokio::time::sleep(poll.min(left)).await;
        }
    }

    /// The service of an agent has gone: its config goes with it, unless a service has been made meanwhile that names it
    /// (the services are listed again right before the delete, so that one keeps its config; **a window of one request
    /// remains**). Bound to the config's `uid` and not to its version: the operator writes the status of a config, and
    /// a version read earlier would strand a config whose service is gone. A config that is gone, or made again, is left.
    async fn delete_the_config_of_a_gone_service(&self, config: &Live) -> Result<(), ObjectsError> {
        let services = self.list(Kind::AgentService).await?;
        if !Self::naming(&services, &config.stored.name, "").is_empty() {
            return Ok(());
        }
        match self.delete_one(config, false, false).await {
            Ok(_) | Err(ObjectsError::NotFound { .. } | ObjectsError::PreconditionFailed) => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// A delete of a name whose service is not there: the orphan config of that name, on its own token, or nothing.
    async fn delete_orphan(
        &self,
        services: &[Live],
        name: &str,
        when: &ETag,
    ) -> Result<(), ObjectsError> {
        let not_found = || ObjectsError::NotFound {
            kind: Kind::AgentService,
            name: name.to_owned(),
        };
        let Some(config) = self.get(Kind::AgentConfig, name).await? else {
            return Err(not_found());
        };
        if !config.stored.owner().writable() {
            return Err(not_owned(&config));
        }
        let by = Self::naming(services, name, "");
        if !by.is_empty() {
            return Err(ObjectsError::Referenced {
                kind: Kind::AgentConfig,
                name: name.to_owned(),
                by,
            });
        }
        if *when != config.stored.etag() {
            return Err(ObjectsError::PreconditionFailed);
        }
        match self.delete_one(&config, true, false).await {
            Ok(_) => Ok(()),
            Err(ObjectsError::NotFound { .. }) => Err(not_found()),
            Err(error) => Err(error),
        }
    }

    /// The agents that name the shared object `name` of `kind`, in byte order, once each.
    async fn referenced_by(
        &self,
        kind: SharedKind,
        name: &str,
    ) -> Result<Vec<String>, ObjectsError> {
        let services = self.list(Kind::AgentService).await?;
        let configs = self.list(Kind::AgentConfig).await?;
        let mut by = BTreeSet::new();
        for service in &services {
            let named = config_name(&service.stored.spec)
                .and_then(|n| configs.iter().find(|c| c.stored.name == n));
            if named.is_some_and(|c| config_references(&c.stored.spec).names(kind, name)) {
                by.insert(service.stored.name.clone());
            }
        }
        Ok(by.into_iter().collect())
    }

    /// The mistakes of a draft that the API server also finds in its bodies: asked of a dry run, and only for
    /// the sake of the answer (a draft with a mistake of ours is not written).
    async fn mistakes_of_the_server(&self, draft: &AgentDraft) -> Vec<FieldError> {
        let writes = [
            Write::new(
                Kind::AgentConfig,
                &draft.name,
                &draft.config,
                Guard::Unbound,
            ),
            Write::new(
                Kind::AgentService,
                &draft.name,
                &draft.service,
                Guard::Unbound,
            ),
        ];
        let mut out = Vec::new();
        for write in &writes {
            if let Err(Failure::Invalid(problems)) = self.apply(write, true).await {
                out.extend(problems.into_iter().map(|p| p.into_field(write.kind)));
            }
        }
        out
    }
}

/// What a delete did to an object.
enum Deleted {
    /// It is gone.
    Gone,
    /// It has finalizers that have not run: it stays, with a `deletionTimestamp`, until they have.
    Terminating,
}

/// A fixed word for what an error was, for a log: never the error's own words, which can carry a server's.
fn outcome(error: &ObjectsError) -> &'static str {
    match error {
        ObjectsError::AlreadyExists { .. } => "already exists",
        ObjectsError::NotFound { .. } => "not found",
        ObjectsError::PreconditionFailed => "the object changed",
        ObjectsError::NotOwned { .. } => "not owned",
        ObjectsError::Referenced { .. } => "referenced",
        ObjectsError::Deleting { .. } => "being deleted",
        ObjectsError::Conflict(_) => "a field conflict",
        ObjectsError::Invalid(_) => "invalid",
        ObjectsError::Unavailable(_) => "unavailable",
        ObjectsError::Internal(_) => "internal",
    }
}

/// What a write that lost a race is, from the object as it is now (`None`: it is not there).
///
/// **Only the primary kind of a write is told as `AlreadyExists` and `NotFound`**: the agent is its service, and a shared
/// object is itself, while the config is one half of the agent's token, so that a config made or deleted between the read
/// and the apply is a token that went stale (`PreconditionFailed`) and not a missing agent.
fn race_outcome(kind: Kind, name: &str, guard: &Guard, now: Option<&Live>) -> ObjectsError {
    let primary = kind != Kind::AgentConfig;
    match (guard, now) {
        (Guard::Absent, Some(_)) if primary => ObjectsError::AlreadyExists {
            kind,
            name: name.to_owned(),
        },
        (Guard::Exactly(_), None) if primary => ObjectsError::NotFound {
            kind,
            name: name.to_owned(),
        },
        _ => ObjectsError::PreconditionFailed,
    }
}

/// [`race_outcome`] of a write: an object that began to be deleted in the meantime is told as such.
fn write_race_outcome(kind: Kind, name: &str, guard: &Guard, now: Option<&Live>) -> ObjectsError {
    match now {
        Some(live) if live.stored.deleting => deleting(live),
        now => race_outcome(kind, name, guard, now),
    }
}

/// What a write needs of the object it finds.
enum Guard {
    /// Nothing: the dry run that only collects mistakes.
    Unbound,
    /// The object must not be there: the create.
    Absent,
    /// The object must be the one that was read, at the version it was read at.
    Exactly(Box<Live>),
}

impl Guard {
    fn of(found: Option<&Live>) -> Self {
        found.map_or(Self::Absent, |live| Self::Exactly(Box::new(live.clone())))
    }
}

/// One server-side apply: an object of `kind` named `name`, with this `spec`.
struct Write<'a> {
    kind: Kind,
    name: &'a str,
    spec: &'a Value,
    guard: Guard,
}

impl<'a> Write<'a> {
    fn new(kind: Kind, name: &'a str, spec: &'a Value, guard: Guard) -> Self {
        Self {
            kind,
            name,
            spec,
            guard,
        }
    }

    /// The configuration that is applied: the label of the dashboard and the `spec`, and nothing else. Never a
    /// `status`, never a field of another manager.
    fn body(&self) -> Value {
        let resource = resource(self.kind);
        let mut metadata = json!({
            "name": self.name,
            "labels": { MANAGED_BY_LABEL: MANAGED_BY_DASHBOARD },
        });
        match &self.guard {
            Guard::Unbound => {}
            Guard::Absent => metadata["resourceVersion"] = json!(NEVER),
            Guard::Exactly(read) => {
                metadata["resourceVersion"] = json!(read.version());
                if !read.uid.is_empty() {
                    metadata["uid"] = json!(read.uid);
                }
            }
        }
        json!({
            "apiVersion": resource.api_version,
            "kind": resource.kind,
            "metadata": metadata,
            "spec": self.spec,
        })
    }
}

/// How the API server addresses a kind. The group, version, kind and plural are those of the types of `aap-api`,
/// so they cannot drift from the CRDs.
fn resource(kind: Kind) -> ApiResource {
    match kind {
        Kind::AgentService => ApiResource::erase::<AgentService>(&()),
        Kind::AgentConfig => ApiResource::erase::<AgentConfig>(&()),
        Kind::ModelEndpoint => ApiResource::erase::<ModelEndpoint>(&()),
        Kind::ToolProvider => ApiResource::erase::<ToolProvider>(&()),
    }
}

/// How the API server addresses an ExternalSecret.
fn external_secret_resource() -> ApiResource {
    ApiResource::from_gvk_with_plural(
        &GroupVersionKind::gvk("external-secrets.io", "v1", "ExternalSecret"),
        "externalsecrets",
    )
}

/// The API server could not do what a read or an unexpected failure asked.
#[derive(Debug, thiserror::Error)]
enum Unexpected {
    /// The kind is not served: the CRDs of `deploy/operator-crds` are not installed.
    #[error("the API server does not serve {0}: the CRDs are not installed")]
    NotServed(String),
    /// A refusal that a write of the dashboard cannot have.
    #[error("unexpected answer of the API server for {0}: {1}")]
    Answer(String, String),
}

/// A failure that is neither a refusal nor a lost race: the API server cannot be reached, or something is wrong.
fn failed(resource: &ApiResource, failure: Failure) -> ObjectsError {
    let what = format!("{}.{}", resource.plural, resource.group);
    match failure {
        Failure::NotServed => ObjectsError::Unavailable(Box::new(Unexpected::NotServed(what))),
        Failure::Unavailable(e) => ObjectsError::Unavailable(Box::new(e)),
        Failure::Internal(e) | Failure::NotFound(e) => ObjectsError::Internal(Box::new(e)),
        Failure::AlreadyExists => {
            ObjectsError::Internal(Box::new(Unexpected::Answer(what, "AlreadyExists".into())))
        }
        Failure::Version => ObjectsError::Internal(Box::new(Unexpected::Answer(
            what,
            "a version conflict".into(),
        ))),
        Failure::FieldConflict(m) => ObjectsError::Internal(Box::new(Unexpected::Answer(what, m))),
        Failure::Invalid(p) => ObjectsError::Internal(Box::new(Unexpected::Answer(
            what,
            p.iter()
                .map(|p| format!("{}: {}", p.path, p.message))
                .collect::<Vec<_>>()
                .join("; "),
        ))),
    }
}

/// Whether `name` is a DNS-1123 subdomain, the names of these objects: lower case letters, digits, `-` and `.`,
/// each dot-separated label beginning and ending with a letter or a digit, at most 253 characters. The API
/// server checks it too, with a `422` that names `metadata.name`; this is what keeps a name that is not one
/// out of a request path.
fn is_name(name: &str) -> bool {
    let alnum = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
    !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.starts_with(alnum)
                && label.ends_with(alnum)
                && label.chars().all(|c| alnum(c) || c == '-')
        })
}

fn bad_name(kind: Kind) -> FieldError {
    FieldError::new(
        kind,
        "metadata.name",
        "must be a DNS-1123 subdomain: lower case letters, digits, '-' and '.', each part beginning and ending with a letter or a digit",
    )
}

fn not_an_object(kind: Kind) -> FieldError {
    FieldError::new(kind, "spec", "a spec is an object")
}

/// The mistakes of a draft that need no API server.
fn check(draft: &AgentDraft) -> Vec<FieldError> {
    let mut out = Vec::new();
    if !is_name(&draft.name) {
        out.push(bad_name(Kind::AgentService));
    }
    if !draft.service.is_object() {
        out.push(not_an_object(Kind::AgentService));
    }
    if !draft.config.is_object() {
        out.push(not_an_object(Kind::AgentConfig));
    }
    if draft.service.is_object() && config_name(&draft.service) != Some(draft.name.as_str()) {
        out.push(FieldError::new(
            Kind::AgentService,
            "spec.configRef.name",
            format!(
                "must be {:?}: the config of an agent has the agent's name",
                draft.name
            ),
        ));
    }
    out
}

fn deleting(live: &Live) -> ObjectsError {
    ObjectsError::Deleting {
        kind: live.stored.kind,
        name: live.stored.name.clone(),
    }
}

fn not_owned(live: &Live) -> ObjectsError {
    ObjectsError::NotOwned {
        kind: live.stored.kind,
        name: live.stored.name.clone(),
    }
}

impl AgentObjects for KubernetesObjects {
    async fn list_agents(&self) -> Result<Vec<Agent>, ObjectsError> {
        let services = self.list(Kind::AgentService).await?;
        let configs = self.list(Kind::AgentConfig).await?;
        Ok(services
            .into_iter()
            .map(|service| {
                let config = config_name(&service.stored.spec)
                    .and_then(|n| configs.iter().find(|c| c.stored.name == n))
                    .map(|c| c.stored.clone());
                Agent {
                    service: service.stored,
                    config,
                }
            })
            .collect())
    }

    async fn get_agent(&self, name: &str) -> Result<Option<Agent>, ObjectsError> {
        match self.get(Kind::AgentService, name).await? {
            Some(service) => Ok(Some(self.agent_of(service).await?)),
            None => Ok(None),
        }
    }

    async fn put_agent(
        &self,
        draft: &AgentDraft,
        when: &Precondition,
    ) -> Result<Agent, ObjectsError> {
        let name = draft.name.as_str();
        let mut mistakes = check(draft);
        if !mistakes.is_empty() {
            // Nothing is read or written. The API server is asked only to find the mistakes of the bodies that
            // are ours to report too, so that one answer lists all of them.
            if is_name(name) && draft.service.is_object() && draft.config.is_object() {
                mistakes.extend(self.mistakes_of_the_server(draft).await);
            }
            return Err(ObjectsError::Invalid(mistakes));
        }

        // Everything that can be refused from what is read is refused here, before the first write.
        let services = self.list(Kind::AgentService).await?;
        let service = services.iter().find(|s| s.stored.name == name);
        let config = self.get(Kind::AgentConfig, name).await?;
        match (when, service) {
            // A service that is being deleted is refused below, as such.
            (Precondition::DoesNotExist, Some(s)) if !s.stored.deleting => {
                return Err(ObjectsError::AlreadyExists {
                    kind: Kind::AgentService,
                    name: name.to_owned(),
                });
            }
            (Precondition::Matches(_), None) => {
                return Err(ObjectsError::NotFound {
                    kind: Kind::AgentService,
                    name: name.to_owned(),
                });
            }
            _ => {}
        }
        if let Some(gitops) = service
            .into_iter()
            .chain(config.iter())
            .find(|o| !o.stored.owner().writable())
        {
            return Err(not_owned(gitops));
        }
        // Nothing is written to an object whose finalizers are running: the operator reads the config while it
        // releases the service, and a name that is going is free once it has gone.
        if let Some(dying) = service
            .into_iter()
            .chain(config.iter())
            .find(|o| o.stored.deleting)
        {
            return Err(deleting(dying));
        }
        let by = Self::naming(&services, name, name);
        if !by.is_empty() {
            return Err(ObjectsError::Referenced {
                kind: Kind::AgentConfig,
                name: name.to_owned(),
                by,
            });
        }
        if let (Precondition::Matches(etag), Some(service)) = (when, service) {
            // The token is the service and the config it names, which is the one this write is about unless
            // somebody pointed the service elsewhere.
            let named = match config_name(&service.stored.spec) {
                Some(n) if n == name => config.clone(),
                Some(n) => self.get(Kind::AgentConfig, n).await?,
                None => None,
            };
            let current = Agent {
                service: service.stored.clone(),
                config: named.map(|c| c.stored),
            };
            if current.etag() != *etag {
                return Err(ObjectsError::PreconditionFailed);
            }
        }

        let writes = [
            Write::new(
                Kind::AgentConfig,
                name,
                &draft.config,
                Guard::of(config.as_ref()),
            ),
            Write::new(Kind::AgentService, name, &draft.service, Guard::of(service)),
        ];
        let mut written = self.write(&writes).await?.into_iter();
        match (written.next(), written.next()) {
            (Some(config), Some(service)) => Ok(Agent {
                service: service.stored,
                config: Some(config.stored),
            }),
            _ => Err(ObjectsError::Internal(
                "an agent just written is not there".into(),
            )),
        }
    }

    async fn delete_agent(&self, name: &str, when: &ETag) -> Result<(), ObjectsError> {
        if !is_name(name) {
            return Err(ObjectsError::NotFound {
                kind: Kind::AgentService,
                name: name.to_owned(),
            });
        }
        let services = self.list(Kind::AgentService).await?;
        let Some(service) = services.iter().find(|s| s.stored.name == name) else {
            return self.delete_orphan(&services, name, when).await;
        };
        let config = match config_name(&service.stored.spec) {
            Some(config) => self.get(Kind::AgentConfig, config).await?,
            None => None,
        };
        if let Some(gitops) = std::iter::once(service)
            .chain(config.iter())
            .find(|o| !o.stored.owner().writable())
        {
            return Err(not_owned(gitops));
        }
        if let Some(config) = &config {
            let by = Self::naming(&services, &config.stored.name, name);
            if !by.is_empty() {
                return Err(ObjectsError::Referenced {
                    kind: Kind::AgentConfig,
                    name: config.stored.name.clone(),
                    by,
                });
            }
        }
        let current = Agent {
            service: service.stored.clone(),
            config: config.as_ref().map(|c| c.stored.clone()),
        };
        if current.etag() != *when {
            return Err(ObjectsError::PreconditionFailed);
        }

        // Both deletes as a dry run first, each bound to the version it was read at: a refusal of either leaves
        // both objects.
        for live in config.iter().chain(std::iter::once(service)) {
            self.delete_one(live, true, true).await?;
        }
        // Then the service. **The config stays until the service is gone.** The operator's finalizer on the service
        // reads the config to honour `deletionPolicy`, and a config it cannot find is a policy it cannot honour:
        // a person who saved `Retain` and deleted at once would lose the data the old `Delete` takes.
        let gone = match self.delete_one(service, true, false).await? {
            Deleted::Gone => true,
            Deleted::Terminating => self.wait_until_gone(service).await,
        };
        match (&config, gone) {
            (Some(config), true) => self.delete_the_config_of_a_gone_service(config).await,
            // The operator is not done. The delete is accepted: the service goes when it is, and its config stays
            // as an orphan that a later delete of the name removes.
            _ => Ok(()),
        }
    }

    async fn list_orphans(&self) -> Result<Vec<Stored>, ObjectsError> {
        let services = self.list(Kind::AgentService).await?;
        Ok(self
            .list(Kind::AgentConfig)
            .await?
            .into_iter()
            .filter(|c| c.stored.owner().writable())
            .filter(|c| Self::naming(&services, &c.stored.name, "").is_empty())
            .map(|c| c.stored)
            .collect())
    }

    async fn list_shared(&self, kind: SharedKind) -> Result<Vec<Stored>, ObjectsError> {
        Ok(self
            .list(kind.kind())
            .await?
            .into_iter()
            .map(|l| l.stored)
            .collect())
    }

    async fn get_shared(
        &self,
        kind: SharedKind,
        name: &str,
    ) -> Result<Option<Stored>, ObjectsError> {
        Ok(self.get(kind.kind(), name).await?.map(|l| l.stored))
    }

    async fn put_shared(
        &self,
        kind: SharedKind,
        draft: &SharedDraft,
        when: &Precondition,
    ) -> Result<Stored, ObjectsError> {
        let kind = kind.kind();
        let name = draft.name.as_str();
        let mut mistakes = Vec::new();
        if !is_name(name) {
            mistakes.push(bad_name(kind));
        }
        if !draft.spec.is_object() {
            mistakes.push(not_an_object(kind));
        }
        if !mistakes.is_empty() {
            return Err(ObjectsError::Invalid(mistakes));
        }
        let found = self.get(kind, name).await?;
        match (when, &found) {
            (Precondition::DoesNotExist, Some(live)) if !live.stored.deleting => {
                return Err(ObjectsError::AlreadyExists {
                    kind,
                    name: name.to_owned(),
                });
            }
            (Precondition::Matches(_), None) => {
                return Err(ObjectsError::NotFound {
                    kind,
                    name: name.to_owned(),
                });
            }
            _ => {}
        }
        if let Some(live) = &found {
            if !live.stored.owner().writable() {
                return Err(not_owned(live));
            }
            if live.stored.deleting {
                return Err(deleting(live));
            }
            if let Precondition::Matches(etag) = when
                && *etag != live.stored.etag()
            {
                return Err(ObjectsError::PreconditionFailed);
            }
        }
        let writes = [Write::new(
            kind,
            name,
            &draft.spec,
            Guard::of(found.as_ref()),
        )];
        self.write(&writes)
            .await?
            .pop()
            .map(|l| l.stored)
            .ok_or_else(|| ObjectsError::Internal("an object just written is not there".into()))
    }

    async fn delete_shared(
        &self,
        kind: SharedKind,
        name: &str,
        when: &ETag,
    ) -> Result<(), ObjectsError> {
        let Some(live) = self.get(kind.kind(), name).await? else {
            return Err(ObjectsError::NotFound {
                kind: kind.kind(),
                name: name.to_owned(),
            });
        };
        if !live.stored.owner().writable() {
            return Err(not_owned(&live));
        }
        if *when != live.stored.etag() {
            return Err(ObjectsError::PreconditionFailed);
        }
        let by = self.referenced_by(kind, name).await?;
        if !by.is_empty() {
            return Err(ObjectsError::Referenced {
                kind: kind.kind(),
                name: name.to_owned(),
                by,
            });
        }
        self.delete_one(&live, true, false).await.map(|_| ())
    }

    async fn offered_secrets(&self) -> Result<Vec<OfferedSecret>, ObjectsError> {
        let resource = external_secret_resource();
        let api: Api<DynamicObject> =
            Api::namespaced_with(self.client.clone(), &self.namespace, &resource);
        let params = ListParams::default().labels(&format!("{}=true", self.offer_label));
        let list = match api.list(&params).await {
            Ok(list) => list,
            Err(error) => {
                return match classify(error) {
                    // An API server without the External Secrets Operator has nothing to offer: the offer is
                    // opt-in by label, and nothing can have opted in.
                    Failure::NotServed => Ok(Vec::new()),
                    other => Err(failed(&resource, other)),
                };
            }
        };
        let offers: BTreeSet<OfferedSecret> =
            list.items.iter().flat_map(external::offers_of).collect();
        Ok(offers.into_iter().collect())
    }
}
