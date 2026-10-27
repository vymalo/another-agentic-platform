//! The provider: every call that reaches the API server.

use std::collections::BTreeSet;
use std::fmt::Debug;

use aap_ports::{
    Capabilities, DeleteOutcome, DeletionPolicy, Endpoint, Issue, IssueReason, Role, RuntimeError,
    RuntimeId, RuntimeProvider, RuntimeSpec, RuntimeStatus, Surface,
};
use futures::stream::BoxStream;
use k8s_openapi::NamespaceResourceScope;
use k8s_openapi::api::apps::v1::{Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{
    ConfigMap, PersistentVolumeClaim, Pod, PodTemplateSpec, ResourceQuota, Service, ServiceAccount,
};
use k8s_openapi::api::networking::v1::NetworkPolicy;
use k8s_openapi::api::policy::v1::PodDisruptionBudget;
use k8s_openapi::api::rbac::v1::RoleBinding;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams};
use kube::{Api, Client, Resource};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::Error;
use crate::names::{
    self, DELETION_POLICY_ANNOTATION, FIELD_MANAGER, INSTANCE_LABEL, MANAGED_BY_LABEL,
    MANAGED_BY_VALUE, VOLUME_LABEL,
};
use crate::render::{self, Rendered, WorkloadObject, parse_policy};
use crate::status::{self, WorkloadObservation};

type Result<T, E = Error> = std::result::Result<T, E>;

/// A namespaced kind this provider reads and writes.
pub trait Kind:
    Resource<Scope = NamespaceResourceScope, DynamicType = ()>
    + Clone
    + Debug
    + Serialize
    + DeserializeOwned
    + Send
    + Sync
    + 'static
{
}

impl<K> Kind for K where
    K: Resource<Scope = NamespaceResourceScope, DynamicType = ()>
        + Clone
        + Debug
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + 'static
{
}

fn what<K: Kind>(name: &str) -> String {
    format!("{} {name}", K::kind(&()))
}

/// Whether an object is this service's and ours: the adoption guard's test.
fn is_ours(meta: &ObjectMeta, id: &RuntimeId) -> bool {
    let labels = meta.labels.as_ref();
    let label = |k: &str| labels.and_then(|l| l.get(k)).map(String::as_str);
    label(MANAGED_BY_LABEL) == Some(MANAGED_BY_VALUE) && label(INSTANCE_LABEL) == Some(id.name())
}

/// What [`KubernetesRuntime::revoke_run_pods`] leaves of a service's run pod objects: the names of the quotas and
/// the accounts it found, which go after the workloads no longer name the account.
struct Revoked {
    quotas: Vec<String>,
    accounts: Vec<String>,
}

/// A workload with a pod template: a StatefulSet or a Deployment.
trait PodTemplated {
    /// Whether the pod template runs as a named account, by either of the two fields it has for it
    /// (`serviceAccountName` and the deprecated `serviceAccount`).
    fn names_an_account(&self) -> bool;
}

fn template_names_an_account(template: Option<&PodTemplateSpec>) -> bool {
    template.and_then(|t| t.spec.as_ref()).is_some_and(|pod| {
        [&pod.service_account_name, &pod.service_account]
            .into_iter()
            .any(|n| n.as_deref().is_some_and(|n| !n.is_empty()))
    })
}

impl PodTemplated for StatefulSet {
    fn names_an_account(&self) -> bool {
        template_names_an_account(self.spec.as_ref().map(|s| &s.template))
    }
}

impl PodTemplated for Deployment {
    fn names_an_account(&self) -> bool {
        template_names_an_account(self.spec.as_ref().map(|s| &s.template))
    }
}

/// The runtime provider for native Kubernetes.
///
/// Cheap to clone: every clone shares one client. It needs the right to `get`, `list`, `watch`,
/// `patch` and `delete` the kinds it owns (StatefulSets, Deployments, Services, NetworkPolicies,
/// PodDisruptionBudgets, ConfigMaps, claims, and for run pods ServiceAccounts, ResourceQuotas and RoleBindings; the egress
/// proxy of run pods (D15c) is a Deployment, a Service, a ServiceAccount, a NetworkPolicy, a ConfigMap and a
/// PodDisruptionBudget, so it needs no right the others do not) and
/// to `list` and `watch` pods, and, only to clear the pods of a service that stops making run pods, to `delete`
/// them. For run pods it also needs the right to **bind** the one Role the deployment made for them: it holds no
/// right to make a pod or to `exec`, and Kubernetes refuses a binding of a role its maker could not use unless the
/// maker may bind it (*verified 2026-10-26* on a bare kube-apiserver v1.35.8). **It never touches a Secret**: a
/// Secret is a reference in a pod spec, and the kubelet is who reads it.
#[derive(Clone)]
pub struct KubernetesRuntime {
    client: Client,
    watch_namespace: Option<String>,
    clear_run_pods: bool,
}

impl KubernetesRuntime {
    /// A provider on this client. [`watch`](RuntimeProvider::watch) covers every namespace; see
    /// [`watching_namespace`](Self::watching_namespace).
    pub fn new(client: Client) -> Self {
        Self {
            client,
            watch_namespace: None,
            clear_run_pods: false,
        }
    }

    /// Look for the run pod objects of a service (its account, quota and binding, and the pods it made) and clear them when
    /// an [`ensure`](RuntimeProvider::ensure) finds that the spec has none and on [`delete`](RuntimeProvider::delete).
    ///
    /// **Off by default.** The look reads `serviceaccounts`, `resourcequotas` and `rolebindings`, which an operator that
    /// has no run pods has no right on (the chart gives them only with `runPods.enabled`): a provider that looked anyway
    /// would fail every `ensure` and every `delete` of every agent with `Forbidden`. Turn it on where run pods are (the
    /// operator does when it has settings for them). A spec that **has** run pods is applied whatever this says, and a
    /// service that made run pods under a provider that was not looking keeps their objects until the service goes (they
    /// carry its owner reference).
    #[must_use]
    pub fn clearing_run_pods(mut self, on: bool) -> Self {
        self.clear_run_pods = on;
        self
    }

    /// Whether this provider looks for run pod objects to clear ([`clearing_run_pods`](Self::clearing_run_pods)).
    #[must_use]
    pub fn is_clearing_run_pods(&self) -> bool {
        self.clear_run_pods
    }

    /// A provider on the client the environment gives: the in-cluster configuration, or the
    /// kubeconfig of `KUBECONFIG`.
    ///
    /// # Errors
    ///
    /// [`Error::Unavailable`] when no configuration can be inferred.
    pub async fn try_default() -> Result<Self> {
        let client = Client::try_default()
            .await
            .map_err(|e| Error::from_kube("the client configuration", e))?;
        Ok(Self::new(client))
    }

    /// Watch one namespace only, for a deployment whose operator has a namespaced `Role`.
    #[must_use]
    pub fn watching_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.watch_namespace = Some(namespace.into());
        self
    }

    fn api<K: Kind>(&self, namespace: &str) -> Api<K> {
        Api::namespaced(self.client.clone(), namespace)
    }

    async fn get_opt<K: Kind>(&self, namespace: &str, name: &str) -> Result<Option<K>> {
        self.api::<K>(namespace)
            .get_opt(name)
            .await
            .map_err(|e| Error::from_kube(what::<K>(name), e))
    }

    /// Server-side apply. `force`: the operator is the one writer of the fields it sets, and it
    /// also moves `replicas` itself (`suspend`), so a field another manager (or its own earlier
    /// patch) holds is taken back instead of stalling the reconcile. What protects objects that
    /// are not ours is the adoption guard, which runs before any apply.
    async fn apply<K: Kind>(&self, namespace: &str, object: &K) -> Result<K> {
        let name = object.meta().name.clone().unwrap_or_default();
        self.api::<K>(namespace)
            .patch(
                &name,
                &PatchParams::apply(FIELD_MANAGER).force(),
                &Patch::Apply(object),
            )
            .await
            .map_err(|e| Error::from_kube(what::<K>(&name), e))
    }

    /// The objects of a kind that are this service's and ours, not those already being deleted.
    async fn list_ours<K: Kind>(&self, id: &RuntimeId) -> Result<Vec<K>> {
        let list = self
            .api::<K>(id.scope())
            .list(&ListParams::default().labels(&names::ours_selector(id)))
            .await
            .map_err(|e| Error::from_kube(what::<K>("(list)"), e))?;
        Ok(list
            .items
            .into_iter()
            .filter(|o| o.meta().deletion_timestamp.is_none())
            .collect())
    }

    /// Delete by name, in the background (the object is gone at once, its pods are collected
    /// after). `false` when there was nothing to delete.
    async fn delete_named<K: Kind>(&self, namespace: &str, name: &str) -> Result<bool> {
        match self
            .api::<K>(namespace)
            .delete(name, &DeleteParams::background())
            .await
        {
            Ok(_) => Ok(true),
            Err(e) => {
                let e = Error::from_kube(what::<K>(name), e);
                if e.is_not_found() { Ok(false) } else { Err(e) }
            }
        }
    }

    /// The adoption guard for one object: an object of this name that is not ours.
    async fn foreign<K: Kind>(&self, id: &RuntimeId, name: &str) -> Result<Option<String>> {
        Ok(self
            .get_opt::<K>(id.scope(), name)
            .await?
            .filter(|o| !is_ours(o.meta(), id))
            .map(|_| what::<K>(name)))
    }

    // ------------------------------------------------------------ ensure

    async fn ensure_objects(&self, id: &RuntimeId, spec: &RuntimeSpec) -> Result<RuntimeStatus> {
        let rendered = render::render(id, spec)?;
        let conflicts = self.conflicts(id, &rendered).await?;
        if !conflicts.is_empty() {
            // Nothing is changed. What ours runs is reported as it is, with the reason it is not
            // being updated.
            let mut status = self.observe(id).await?;
            status.issues.extend(conflicts);
            return Ok(status);
        }
        // A service that made run pods and no longer asks: the right to make them goes before anything else is applied,
        // and then the pods; the account and the rest only after the workloads are applied without it, or the pod their
        // controller makes next names an account that is not there and is refused, for good.
        let revoked = if rendered.run_pods.is_none() && self.clear_run_pods {
            self.revoke_run_pods(id).await?
        } else {
            None
        };
        self.apply_all(id, &rendered).await?;
        self.remove_stale(id, &rendered).await?;
        if let Some(revoked) = &revoked {
            self.clear_run_pod_objects(id, revoked, true).await?;
        }
        let status = self.observe(id).await?;
        if matches!(
            status.phase,
            aap_ports::Phase::Ready | aap_ports::Phase::Suspended
        ) {
            // The rollout is done: no pod mounts a superseded file set any more.
            self.remove_stale_config_maps(id, &rendered).await?;
        }
        Ok(status)
    }

    /// Every object the spec needs whose name is held by something that is not ours, as a
    /// `NameConflict` issue each.
    async fn conflicts(&self, id: &RuntimeId, r: &Rendered) -> Result<Vec<Issue>> {
        let first_role = r.workloads.first().map_or(Role::All, |(role, _)| *role);
        let mut taken: Vec<(Role, String)> = Vec::new();
        let mut note = |role: Role, what: Option<String>| {
            if let Some(what) = what {
                taken.push((role, what));
            }
        };
        for c in &r.config_maps {
            let name = c.metadata.name.clone().unwrap_or_default();
            note(first_role, self.foreign::<ConfigMap>(id, &name).await?);
        }
        for c in &r.claims {
            let name = c.metadata.name.clone().unwrap_or_default();
            note(
                first_role,
                self.foreign::<PersistentVolumeClaim>(id, &name).await?,
            );
        }
        note(first_role, self.foreign::<Service>(id, id.name()).await?);
        for (role, w) in &r.workloads {
            let found = match w {
                WorkloadObject::StatefulSet(_) => self.foreign::<StatefulSet>(id, w.name()).await?,
                WorkloadObject::Deployment(_) => self.foreign::<Deployment>(id, w.name()).await?,
            };
            note(*role, found);
        }
        for b in &r.disruption_budgets {
            let name = b.metadata.name.clone().unwrap_or_default();
            note(
                first_role,
                self.foreign::<PodDisruptionBudget>(id, &name).await?,
            );
        }
        if r.network_policy.is_some() {
            note(
                first_role,
                self.foreign::<NetworkPolicy>(id, id.name()).await?,
            );
        }
        if let Some(run) = &r.run_pods {
            let name_of = |m: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta| {
                m.name.clone().unwrap_or_default()
            };
            note(
                first_role,
                self.foreign::<ConfigMap>(id, &name_of(&run.policy_params.metadata))
                    .await?,
            );
            note(
                first_role,
                self.foreign::<ServiceAccount>(id, &name_of(&run.service_account.metadata))
                    .await?,
            );
            note(
                first_role,
                self.foreign::<ResourceQuota>(id, &name_of(&run.quota.metadata))
                    .await?,
            );
            note(
                first_role,
                self.foreign::<NetworkPolicy>(id, &name_of(&run.network_policy.metadata))
                    .await?,
            );
            let proxy = &run.egress_proxy;
            note(
                first_role,
                self.foreign::<ConfigMap>(id, &name_of(&proxy.allow_list.metadata))
                    .await?,
            );
            note(
                first_role,
                self.foreign::<ServiceAccount>(id, &name_of(&proxy.service_account.metadata))
                    .await?,
            );
            note(
                first_role,
                self.foreign::<Deployment>(id, &name_of(&proxy.deployment.metadata))
                    .await?,
            );
            note(
                first_role,
                self.foreign::<Service>(id, &name_of(&proxy.service.metadata))
                    .await?,
            );
            if let Some(budget) = &proxy.disruption_budget {
                note(
                    first_role,
                    self.foreign::<PodDisruptionBudget>(id, &name_of(&budget.metadata))
                        .await?,
                );
            }
            note(
                first_role,
                self.foreign::<NetworkPolicy>(id, &name_of(&proxy.network_policy.metadata))
                    .await?,
            );
            note(
                first_role,
                self.foreign::<RoleBinding>(id, &name_of(&run.role_binding.metadata))
                    .await?,
            );
        }
        Ok(taken
            .into_iter()
            .map(|(role, what)| Issue {
                role,
                reason: IssueReason::NameConflict,
                message: format!(
                    "{what} exists and is not managed by {MANAGED_BY_VALUE} for this service; nothing was changed"
                ),
            })
            .collect())
    }

    async fn apply_all(&self, id: &RuntimeId, r: &Rendered) -> Result<()> {
        let ns = id.scope();
        for o in &r.config_maps {
            self.apply(ns, o).await?;
        }
        for o in &r.claims {
            self.apply(ns, o).await?;
        }
        if let Some(run) = &r.run_pods {
            // The guard's parameters and the account are there before the right to make pods is: the parameters
            // are what the admission policy reads, and a binding without them would let the account make any pod.
            self.apply(ns, &run.policy_params).await?;
            self.apply(ns, &run.service_account).await?;
            self.apply(ns, &run.quota).await?;
            // The proxy is there before the policy that makes it the one way out, and both before the right to make a
            // pod: a run pod is never made into a network that has a policy and no way out, or no policy at all.
            let proxy = &run.egress_proxy;
            self.apply(ns, &proxy.allow_list).await?;
            self.apply(ns, &proxy.service_account).await?;
            self.apply(ns, &proxy.deployment).await?;
            self.apply(ns, &proxy.service).await?;
            if let Some(budget) = &proxy.disruption_budget {
                self.apply(ns, budget).await?;
            }
            self.apply(ns, &proxy.network_policy).await?;
            self.apply(ns, &run.network_policy).await?;
            self.apply(ns, &run.role_binding).await?;
        }
        self.apply(ns, &r.service).await?;
        for (_, w) in &r.workloads {
            match w {
                WorkloadObject::StatefulSet(s) => {
                    let live = self.apply(ns, s.as_ref()).await?;
                    self.stop_naming_account(ns, s.as_ref(), &live).await?;
                }
                WorkloadObject::Deployment(d) => {
                    let live = self.apply(ns, d.as_ref()).await?;
                    self.stop_naming_account(ns, d.as_ref(), &live).await?;
                }
            }
        }
        for o in &r.disruption_budgets {
            self.apply(ns, o).await?;
        }
        if let Some(o) = &r.network_policy {
            self.apply(ns, o).await?;
        }
        Ok(())
    }

    /// Delete what an earlier spec made and this one does not: a workload that is gone or of the
    /// other kind, a budget, a policy. File sets wait for the rollout.
    async fn remove_stale(&self, id: &RuntimeId, r: &Rendered) -> Result<()> {
        let ns = id.scope();
        let want = |kind: &str| -> BTreeSet<&str> {
            r.workloads
                .iter()
                .filter(|(_, w)| w.kind() == kind)
                .map(|(_, w)| w.name())
                .collect()
        };
        let (want_sts, want_deploy) = (want("StatefulSet"), {
            // The egress proxy's Deployment is no workload of the spec: it is wanted as long as the spec has run pods.
            let mut names = want("Deployment");
            names.extend(
                r.run_pods
                    .iter()
                    .filter_map(|run| run.egress_proxy.deployment.metadata.name.as_deref()),
            );
            names
        });
        for s in self.list_ours::<StatefulSet>(id).await? {
            let name = s.metadata.name.unwrap_or_default();
            if !want_sts.contains(name.as_str()) {
                self.delete_named::<StatefulSet>(ns, &name).await?;
            }
        }
        for d in self.list_ours::<Deployment>(id).await? {
            let name = d.metadata.name.unwrap_or_default();
            if !want_deploy.contains(name.as_str()) {
                self.delete_named::<Deployment>(ns, &name).await?;
            }
        }
        let budgets: BTreeSet<&str> = r
            .disruption_budgets
            .iter()
            .chain(
                r.run_pods
                    .iter()
                    .filter_map(|run| run.egress_proxy.disruption_budget.as_ref()),
            )
            .filter_map(|b| b.metadata.name.as_deref())
            .collect();
        for b in self.list_ours::<PodDisruptionBudget>(id).await? {
            let name = b.metadata.name.unwrap_or_default();
            if !budgets.contains(name.as_str()) {
                self.delete_named::<PodDisruptionBudget>(ns, &name).await?;
            }
        }
        // The policy of the agent's ingress is `<svc>`; the run pods' is `<svc>-run-pods`; the egress proxy's is
        // `<svc>-egress`. Each goes when the spec no longer has it, and not for the other's sake.
        let wanted_policies: BTreeSet<&str> = r
            .network_policy
            .iter()
            .chain(
                r.run_pods
                    .iter()
                    .flat_map(|run| [&run.network_policy, &run.egress_proxy.network_policy]),
            )
            .filter_map(|p| p.metadata.name.as_deref())
            .collect();
        // The run pods' policy goes **last**: it is the one that denies, and nothing may leave a run pod under no policy while
        // the way out that the others describe is already gone (the order of the listing says nothing about that).
        let run_pods_policy = names::run_pods(id);
        let mut stale: Vec<String> = self
            .list_ours::<NetworkPolicy>(id)
            .await?
            .into_iter()
            .filter_map(|p| p.metadata.name)
            .filter(|name| !wanted_policies.contains(name.as_str()))
            .collect();
        order_run_pods_policy_last(&mut stale, &run_pods_policy);
        for name in stale {
            self.delete_named::<NetworkPolicy>(ns, &name).await?;
        }
        Ok(())
    }

    /// A workload that was applied without an account but still names one. A real API server keeps the deprecated
    /// `serviceAccount` of a pod template through an apply that leaves `serviceAccountName` out (server-side apply
    /// releases the field this manager owned, the deprecated one was never owned, and conversion fills the first from the
    /// second), so the template of a service that stopped asking for run pods would name an account for good, and the
    /// pod its controller makes after the account is deleted is refused. Only a merge patch that nulls **both** takes the
    /// name out (*verified 2026-10-27* on a bare kube-apiserver v1.35.8, StatefulSet and Deployment; a patch of only
    /// `serviceAccountName` changes nothing). Sent only when the object the apply answered with names an account that the
    /// render does not: a render without run pods costs nothing, and a workload that an earlier pass left naming an account
    /// is freed by the next.
    async fn stop_naming_account<K>(&self, namespace: &str, wanted: &K, live: &K) -> Result<()>
    where
        K: Kind + PodTemplated,
    {
        if wanted.names_an_account() || !live.names_an_account() {
            return Ok(());
        }
        let name = live.meta().name.clone().unwrap_or_default();
        self.merge_patch::<K>(
            namespace,
            &name,
            json!({"spec": {"template": {"spec": {"serviceAccountName": null, "serviceAccount": null}}}}),
        )
        .await
    }

    /// A JSON merge patch of one object, under our field manager. (A server-side apply of a partial object would drop
    /// every other field this manager owns.)
    async fn merge_patch<K: Kind>(&self, namespace: &str, name: &str, patch: Value) -> Result<()> {
        let params = PatchParams {
            field_manager: Some(FIELD_MANAGER.to_owned()),
            ..PatchParams::default()
        };
        self.api::<K>(namespace)
            .patch(name, &params, &Patch::Merge(patch))
            .await
            .map(drop)
            .map_err(|e| Error::from_kube(what::<K>(name), e))
    }

    /// A service that made run pods and does not any more: take the right to make them away first, then clear the
    /// pods that were made (nothing sweeps them now: the coder that did runs its commands in its own container). What
    /// is left is [`Revoked`], for [`clear_run_pod_objects`](Self::clear_run_pod_objects). `None` for a service that
    /// never had any, which costs three reads.
    async fn revoke_run_pods(&self, id: &RuntimeId) -> Result<Option<Revoked>> {
        let ns = id.scope();
        let bindings = self.list_ours::<RoleBinding>(id).await?;
        let accounts = self.list_ours::<ServiceAccount>(id).await?;
        let quotas = self.list_ours::<ResourceQuota>(id).await?;
        if bindings.is_empty() && accounts.is_empty() && quotas.is_empty() {
            return Ok(None);
        }
        for b in &bindings {
            self.delete_named::<RoleBinding>(ns, b.metadata.name.as_deref().unwrap_or_default())
                .await?;
        }
        self.delete_run_pods(id).await?;
        Ok(Some(Revoked {
            quotas: quotas.into_iter().filter_map(|q| q.metadata.name).collect(),
            accounts: accounts
                .into_iter()
                .filter_map(|a| a.metadata.name)
                .collect(),
        }))
    }

    /// The rest of a service's run pod objects, after [`revoke_run_pods`](Self::revoke_run_pods): the quota, **the egress
    /// proxy**, the network policy, the parameters of the guard and, with `template`, the run pod template (a file set that
    /// only the workers' pods mount, so it goes once they are applied without it), then **the accounts last**, so that an
    /// interrupted pass still finds one and comes back. By name, and only what is ours: a policy or a ConfigMap of the same
    /// name that someone else made is theirs. With `template` off (the service is going: its ConfigMaps all go with it) the
    /// template is left to that.
    ///
    /// The order is the one that never leaves a run pod with a network it should not have. By now the binding is gone
    /// and the run pods are deleted (`revoke_run_pods`); the **proxy goes before the run pods' policy**, so that a pod
    /// that was made in the instant before the binding went is cut off from the way out first (its policy denies the
    /// rest) and is never under no policy; the proxy's Deployment goes before its account and its policy, so that no pod
    /// of it outlives what it runs as; and the run pods' policy goes before the parameters of the guard, which find the
    /// pods.
    async fn clear_run_pod_objects(
        &self,
        id: &RuntimeId,
        revoked: &Revoked,
        template: bool,
    ) -> Result<()> {
        let ns = id.scope();
        for q in &revoked.quotas {
            self.delete_named::<ResourceQuota>(ns, q).await?;
        }
        self.clear_egress_proxy(id).await?;
        let (policy, params) = (names::run_pods(id), names::run_pods_policy_params(id));
        for p in self.list_ours::<NetworkPolicy>(id).await? {
            if p.metadata.name.as_deref() == Some(policy.as_str()) {
                self.delete_named::<NetworkPolicy>(ns, &policy).await?;
            }
        }
        let template_map = names::run_pod_template(id);
        for c in self.list_ours::<ConfigMap>(id).await? {
            let name = c.metadata.name.as_deref();
            if name == Some(params.as_str()) || (template && name == Some(template_map.as_str())) {
                self.delete_named::<ConfigMap>(ns, name.unwrap_or_default())
                    .await?;
            }
        }
        for a in &revoked.accounts {
            self.delete_named::<ServiceAccount>(ns, a).await?;
        }
        Ok(())
    }

    /// The egress proxy of a service, from the pods to the policy: its Deployment (and so its pods), what routes to it, its
    /// budget, its policy and its allow-list. Its account is in [`Revoked::accounts`] and goes last of all. Only what is
    /// ours, by name.
    async fn clear_egress_proxy(&self, id: &RuntimeId) -> Result<()> {
        let name = names::egress_proxy(id);
        self.delete_ours_named::<Deployment>(id, &name).await?;
        self.delete_ours_named::<Service>(id, &name).await?;
        self.delete_ours_named::<PodDisruptionBudget>(id, &name)
            .await?;
        self.delete_ours_named::<NetworkPolicy>(id, &name).await?;
        self.delete_ours_named::<ConfigMap>(id, &names::egress_allow(id))
            .await
    }

    /// Delete the object of this name if it is this service's and ours.
    async fn delete_ours_named<K: Kind>(&self, id: &RuntimeId, name: &str) -> Result<()> {
        for o in self.list_ours::<K>(id).await? {
            if o.meta().name.as_deref() == Some(name) {
                self.delete_named::<K>(id.scope(), name).await?;
            }
        }
        Ok(())
    }

    /// Delete the run pods of a service: the pods its coder labelled, found by the labels the parameters of its guard
    /// name (the operator wrote them from the spec; it knows nothing of adam's labels but through the spec). With the
    /// parameters gone there is nothing to find them by, and nothing is deleted.
    async fn delete_run_pods(&self, id: &RuntimeId) -> Result<()> {
        let ns = id.scope();
        let Some(params) = self
            .get_opt::<ConfigMap>(ns, &names::run_pods_policy_params(id))
            .await?
            .filter(|c| is_ours(&c.metadata, id))
        else {
            return Ok(());
        };
        let data = params.data.unwrap_or_default();
        let (Some(managed_by), Some(run_label)) = (data.get("managedBy"), data.get("runLabel"))
        else {
            return Ok(());
        };
        let selector = format!(
            "{MANAGED_BY_LABEL}={managed_by},{INSTANCE_LABEL}={},{run_label}",
            id.name()
        );
        let pods = self
            .api::<Pod>(ns)
            .list(&ListParams::default().labels(&selector))
            .await
            .map_err(|e| Error::from_kube(what::<Pod>("(list)"), e))?;
        for pod in pods.items {
            let name = pod.metadata.name.unwrap_or_default();
            self.delete_named::<Pod>(ns, &name).await?;
        }
        Ok(())
    }

    async fn remove_stale_config_maps(&self, id: &RuntimeId, r: &Rendered) -> Result<()> {
        let wanted: BTreeSet<&str> = r
            .config_maps
            .iter()
            .chain(
                r.run_pods
                    .iter()
                    .flat_map(|run| [&run.policy_params, &run.egress_proxy.allow_list]),
            )
            .filter_map(|c| c.metadata.name.as_deref())
            .collect();
        for c in self.list_ours::<ConfigMap>(id).await? {
            let name = c.metadata.name.unwrap_or_default();
            if !wanted.contains(name.as_str()) {
                self.delete_named::<ConfigMap>(id.scope(), &name).await?;
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------ status

    /// The workloads of the runtime that are ours, as observed.
    async fn workloads(&self, id: &RuntimeId) -> Result<Vec<WorkloadObservation>> {
        let sts = self.list_ours::<StatefulSet>(id).await?;
        let deploys = self.list_ours::<Deployment>(id).await?;
        Ok(sts
            .iter()
            .filter_map(status::observe_stateful_set)
            .chain(deploys.iter().filter_map(status::observe_deployment))
            .collect())
    }

    /// The status of what is ours. A runtime with nothing of ours is `Absent`, and says
    /// `NameConflict` when something that is not ours holds the name it would need.
    async fn observe(&self, id: &RuntimeId) -> Result<RuntimeStatus> {
        let workloads = self.workloads(id).await?;
        if workloads.is_empty() {
            let mut status = RuntimeStatus::absent();
            let taken = [
                self.foreign::<StatefulSet>(id, id.name()).await?,
                self.foreign::<Deployment>(id, id.name()).await?,
                self.foreign::<Service>(id, id.name()).await?,
            ];
            for what in taken.into_iter().flatten() {
                status.issues.push(Issue {
                    role: Role::All,
                    reason: IssueReason::NameConflict,
                    message: format!(
                        "{what} exists and is not managed by {MANAGED_BY_VALUE} for this service"
                    ),
                });
            }
            return Ok(status);
        }
        let pods = self.list_ours::<Pod>(id).await?;
        Ok(status::compute(&workloads, &pods))
    }

    // ------------------------------------------------------------ suspend

    async fn scale_to_zero(&self, id: &RuntimeId) -> Result<RuntimeStatus> {
        let ns = id.scope();
        let sts = self.list_ours::<StatefulSet>(id).await?;
        let deploys = self.list_ours::<Deployment>(id).await?;
        if sts.is_empty() && deploys.is_empty() {
            return Err(Error::NotFound {
                what: format!("runtime {id}"),
            });
        }
        // A merge patch of one field: a server-side apply of a partial object would drop every
        // other field this manager owns.
        let patch = json!({"spec": {"replicas": 0}});
        for s in sts {
            let name = s.metadata.name.unwrap_or_default();
            self.merge_patch::<StatefulSet>(ns, &name, patch.clone())
                .await?;
        }
        for d in deploys {
            let name = d.metadata.name.unwrap_or_default();
            self.merge_patch::<Deployment>(ns, &name, patch.clone())
                .await?;
        }
        self.observe(id).await
    }

    // ------------------------------------------------------------ delete

    async fn delete_objects(&self, id: &RuntimeId) -> Result<DeleteOutcome> {
        let ns = id.scope();
        let sts = self.list_ours::<StatefulSet>(id).await?;
        let deploys = self.list_ours::<Deployment>(id).await?;
        let claims = self.list_ours::<PersistentVolumeClaim>(id).await?;

        // What the last `ensure` asked for is on the workloads; the shared claims say it too.
        let policy = sts
            .iter()
            .map(|s| &s.metadata)
            .chain(deploys.iter().map(|d| &d.metadata))
            .chain(claims.iter().map(|c| &c.metadata))
            .find_map(|m| m.annotations.as_ref()?.get(DELETION_POLICY_ANNOTATION))
            .map_or(DeletionPolicy::Retain, |p| parse_policy(Some(p)));

        // The volumes that exist, and those a StatefulSet will make for its next replica.
        let mut volumes: BTreeSet<String> = claims
            .iter()
            .filter_map(|c| c.metadata.labels.as_ref()?.get(VOLUME_LABEL).cloned())
            .collect();
        for s in &sts {
            for t in s
                .spec
                .iter()
                .flat_map(|s| s.volume_claim_templates.iter().flatten())
            {
                volumes.extend(t.metadata.name.clone());
            }
        }

        let mut existed = !sts.is_empty() || !deploys.is_empty();
        // The right to make run pods first, then the pods they made (the guard's parameters are read to find them,
        // so they go with the rest below): nothing is left to make a pod again.
        if self.clear_run_pods
            && let Some(revoked) = self.revoke_run_pods(id).await?
        {
            self.clear_run_pod_objects(id, &revoked, false).await?;
            existed = true;
        }
        // Compute next: nothing is left to make a claim again.
        for s in &sts {
            self.delete_named::<StatefulSet>(ns, s.metadata.name.as_deref().unwrap_or_default())
                .await?;
        }
        for d in &deploys {
            self.delete_named::<Deployment>(ns, d.metadata.name.as_deref().unwrap_or_default())
                .await?;
        }
        existed |= self.delete_all::<PodDisruptionBudget>(id).await?;
        existed |= self.delete_all::<NetworkPolicy>(id).await?;
        existed |= self.delete_all::<Service>(id).await?;
        existed |= self.delete_all::<ConfigMap>(id).await?;

        // The claims are listed again: a StatefulSet may have made one since the first list.
        let claims = self.list_ours::<PersistentVolumeClaim>(id).await?;
        match policy {
            DeletionPolicy::Delete => {
                for c in &claims {
                    self.delete_named::<PersistentVolumeClaim>(
                        ns,
                        c.metadata.name.as_deref().unwrap_or_default(),
                    )
                    .await?;
                }
                Ok(DeleteOutcome {
                    existed,
                    retained_volumes: Vec::new(),
                })
            }
            DeletionPolicy::Retain => {
                for c in &claims {
                    self.strip_owners(ns, &c.metadata).await?;
                }
                volumes.extend(
                    claims
                        .iter()
                        .filter_map(|c| c.metadata.labels.as_ref()?.get(VOLUME_LABEL).cloned()),
                );
                Ok(DeleteOutcome {
                    existed,
                    // Nothing was retained if nothing was there.
                    retained_volumes: if existed {
                        volumes.into_iter().collect()
                    } else {
                        Vec::new()
                    },
                })
            }
        }
    }

    /// Delete every object of a kind that is this service's. `true` when there was one.
    async fn delete_all<K: Kind>(&self, id: &RuntimeId) -> Result<bool> {
        let mut any = false;
        for o in self.list_ours::<K>(id).await? {
            let name = o.meta().name.clone().unwrap_or_default();
            any |= self.delete_named::<K>(id.scope(), &name).await?;
        }
        Ok(any)
    }

    /// A kept claim keeps nothing that would let the garbage collector take it: the owner references
    /// of a workload or of the service are removed.
    async fn strip_owners(&self, namespace: &str, meta: &ObjectMeta) -> Result<()> {
        let Some(owners) = meta.owner_references.as_ref().filter(|o| !o.is_empty()) else {
            return Ok(());
        };
        let ours = |kind: &str, api_version: &str| {
            matches!(kind, "StatefulSet" | "Deployment")
                || api_version.split('/').next() == Some(names::OWNER_GROUP)
        };
        let kept: Vec<_> = owners
            .iter()
            .filter(|o| !ours(&o.kind, &o.api_version))
            .collect();
        if kept.len() == owners.len() {
            return Ok(());
        }
        let name = meta.name.clone().unwrap_or_default();
        let params = PatchParams {
            field_manager: Some(FIELD_MANAGER.to_owned()),
            ..PatchParams::default()
        };
        self.api::<PersistentVolumeClaim>(namespace)
            .patch(
                &name,
                &params,
                &Patch::Merge(json!({"metadata": {"ownerReferences": kept}})),
            )
            .await
            .map_err(|e| Error::from_kube(what::<PersistentVolumeClaim>(&name), e))?;
        Ok(())
    }

    // ------------------------------------------------------------ endpoint

    async fn endpoint_of(&self, id: &RuntimeId, surface: Surface) -> Result<Endpoint> {
        let service = self
            .get_opt::<Service>(id.scope(), id.name())
            .await?
            .filter(|s| is_ours(&s.metadata, id) && s.metadata.deletion_timestamp.is_none())
            .ok_or_else(|| Error::NotFound {
                what: format!("runtime {id}"),
            })?;
        let port = service
            .spec
            .and_then(|s| s.ports?.first().map(|p| p.port))
            .ok_or_else(|| Error::NotFound {
                what: format!("the port of runtime {id}"),
            })?;
        let base = format!("http://{}.{}.svc:{port}/", id.name(), id.scope());
        Ok(Endpoint {
            url: match surface {
                Surface::A2a => base,
                Surface::AgentCard => format!("{base}.well-known/agent-card.json"),
            },
        })
    }

    // ------------------------------------------------------------ audit

    /// Every plain-text value of the objects this provider applied for `id`, as the conformance
    /// suite reads them: names, labels, annotations, literal variables, commands, file contents.
    /// **A secret reference is how a secret travels and is not plain text**: `secretKeyRef` and
    /// the `secret` of a volume are left out, and so is `managedFields`.
    ///
    /// # Errors
    ///
    /// Any failure of the API server.
    pub async fn plain_text(&self, id: &RuntimeId) -> Result<Vec<String>> {
        let mut docs: Vec<Value> = Vec::new();
        fn push<K: Kind>(docs: &mut Vec<Value>, items: Vec<K>) {
            docs.extend(items.iter().filter_map(|o| serde_json::to_value(o).ok()));
        }
        push(&mut docs, self.list_ours::<ConfigMap>(id).await?);
        push(&mut docs, self.list_ours::<Service>(id).await?);
        push(&mut docs, self.list_ours::<StatefulSet>(id).await?);
        push(&mut docs, self.list_ours::<Deployment>(id).await?);
        push(&mut docs, self.list_ours::<NetworkPolicy>(id).await?);
        push(&mut docs, self.list_ours::<PodDisruptionBudget>(id).await?);
        push(&mut docs, self.list_ours::<ServiceAccount>(id).await?);
        push(&mut docs, self.list_ours::<ResourceQuota>(id).await?);
        push(&mut docs, self.list_ours::<RoleBinding>(id).await?);
        push(
            &mut docs,
            self.list_ours::<PersistentVolumeClaim>(id).await?,
        );
        let mut out = Vec::new();
        for mut doc in docs {
            strip_references(&mut doc);
            collect_strings(&doc, &mut out);
        }
        Ok(out)
    }
}

/// Put the run pods' policy last among the policies to delete, the others in name order.
fn order_run_pods_policy_last(names: &mut [String], run_pods_policy: &str) {
    names.sort_by_key(|name| (name == run_pods_policy, name.clone()));
}

/// Remove what is a reference to a Secret and not a value: `secretKeyRef`, the `secret` source of a
/// volume, and the server's bookkeeping.
fn strip_references(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.remove("secretKeyRef");
            map.remove("managedFields");
            // A volume's source is an object that names the volume next to it.
            if map.contains_key("name") {
                map.remove("secret");
            }
            for child in map.values_mut() {
                strip_references(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_references),
        _ => {}
    }
}

fn collect_strings(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|i| collect_strings(i, out)),
        Value::Object(map) => {
            for (k, child) in map {
                out.push(k.clone());
                collect_strings(child, out);
            }
        }
        _ => {}
    }
}

impl RuntimeProvider for KubernetesRuntime {
    fn name(&self) -> &'static str {
        "kubernetes"
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities { suspend: true }
    }

    async fn ensure(
        &self,
        id: &RuntimeId,
        spec: &RuntimeSpec,
    ) -> Result<RuntimeStatus, RuntimeError> {
        spec.check()?;
        self.ensure_objects(id, spec)
            .await
            .map_err(|e| e.into_runtime(id))
    }

    async fn suspend(&self, id: &RuntimeId) -> Result<RuntimeStatus, RuntimeError> {
        self.scale_to_zero(id).await.map_err(|e| e.into_runtime(id))
    }

    async fn delete(&self, id: &RuntimeId) -> Result<DeleteOutcome, RuntimeError> {
        self.delete_objects(id)
            .await
            .map_err(|e| e.into_runtime(id))
    }

    async fn status(&self, id: &RuntimeId) -> Result<RuntimeStatus, RuntimeError> {
        self.observe(id).await.map_err(|e| e.into_runtime(id))
    }

    async fn endpoint(&self, id: &RuntimeId, surface: Surface) -> Result<Endpoint, RuntimeError> {
        self.endpoint_of(id, surface)
            .await
            .map_err(|e| e.into_runtime(id))
    }

    fn watch(&self) -> BoxStream<'static, RuntimeId> {
        crate::watch::changes(self.client.clone(), self.watch_namespace.clone())
    }
}

#[cfg(feature = "testkit")]
impl aap_ports::testkit::RuntimeUnderTest for KubernetesRuntime {
    #[allow(clippy::panic)]
    async fn materialised(&self, id: &RuntimeId) -> Vec<String> {
        self.plain_text(id)
            .await
            .unwrap_or_else(|e| panic!("reading the objects of {id}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn references_to_secrets_are_not_plain_text() {
        let mut doc = json!({
            "metadata": {"name": "w", "managedFields": [{"manager": "x"}]},
            "spec": {"template": {"spec": {
                "containers": [{"name": "agent", "env": [
                    {"name": "K", "value": "literal"},
                    {"name": "S", "valueFrom": {"secretKeyRef": {"name": "the-secret", "key": "k"}}}
                ]}],
                "volumes": [
                    {"name": "key", "secret": {"secretName": "the-key"}},
                    {"name": "cm", "configMap": {"name": "files"}}
                ]
            }}}
        });
        strip_references(&mut doc);
        let mut text = Vec::new();
        collect_strings(&doc, &mut text);
        assert!(text.contains(&"literal".to_owned()));
        assert!(text.contains(&"files".to_owned()));
        for secret in ["the-secret", "the-key", "manager"] {
            assert!(!text.iter().any(|t| t == secret), "{secret}");
        }
    }

    #[test]
    fn the_run_pods_policy_is_deleted_after_every_other_policy_whatever_the_names() {
        let mut names: Vec<String> = ["svc-run-pods", "svc", "svc-egress", "a-late-comer"]
            .map(str::to_owned)
            .into();
        order_run_pods_policy_last(&mut names, "svc-run-pods");
        assert_eq!(names, ["a-late-comer", "svc", "svc-egress", "svc-run-pods"]);
        // Even when it would sort first.
        let mut names: Vec<String> = ["b", "a-run-pods"].map(str::to_owned).into();
        order_run_pods_policy_last(&mut names, "a-run-pods");
        assert_eq!(names, ["b", "a-run-pods"]);
    }

    #[test]
    fn an_object_is_ours_only_with_both_labels() {
        let id = RuntimeId::new("ns", "svc");
        let meta = |labels: &[(&str, &str)]| ObjectMeta {
            labels: Some(
                labels
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
            ),
            ..ObjectMeta::default()
        };
        assert!(is_ours(
            &meta(&[(MANAGED_BY_LABEL, "aap-operator"), (INSTANCE_LABEL, "svc")]),
            &id
        ));
        assert!(!is_ours(
            &meta(&[(MANAGED_BY_LABEL, "Helm"), (INSTANCE_LABEL, "svc")]),
            &id
        ));
        assert!(!is_ours(
            &meta(&[
                (MANAGED_BY_LABEL, "aap-operator"),
                (INSTANCE_LABEL, "other")
            ]),
            &id
        ));
        assert!(!is_ours(&meta(&[(MANAGED_BY_LABEL, "aap-operator")]), &id));
        assert!(!is_ours(&ObjectMeta::default(), &id));
    }
}
