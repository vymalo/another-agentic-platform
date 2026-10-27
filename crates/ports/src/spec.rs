//! The neutral description of a runtime: what a provider is asked to make. Nothing in it is a
//! Kubernetes type, and nothing in it can hold a secret's value (AD-020, AD-024).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{OwnerHandle, RuntimeError, SecretRef};

/// What happens to data when the service is deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeletionPolicy {
    /// The compute goes; work volumes and an operator-owned database stay.
    #[default]
    Retain,
    /// The data goes too.
    Delete,
}

/// Which part of an adam process a workload runs: adam's `ROLE`. Closed on purpose: a new role
/// must fail to compile in every provider.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Role {
    /// The A2A server and the workers in one process (`ROLE` unset).
    All,
    /// The A2A server over a runtime that only starts, delivers to and cancels runs.
    ControlPlane,
    /// The workers.
    Worker,
}

impl Role {
    /// Whether this role steps runs (and so is the workload `replicas` of a status counts).
    pub const fn runs_workers(self) -> bool {
        matches!(self, Self::All | Self::Worker)
    }
}

/// Everything a provider needs to make one agent's runtime. It is the same on every `ensure` of
/// the same configuration, and it is what the [`digest`](Self::digest) is of.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSpec {
    /// Whose runtime it is. Not part of the digest.
    pub owner: OwnerHandle,
    /// What a delete does to data. Not part of the digest.
    pub deletion: DeletionPolicy,
    /// `sha256:<hex>` of what the pods run (see the digest rules in `aap-domain`), which a provider
    /// writes on the pods (`agents.vymalo.com/config-digest`) so that a changed folder, file, image
    /// or variable is a rollout and nothing else is. Empty when the spec was not resolved.
    pub digest: String,
    /// Scale to zero. Not part of the digest: scale is applied, it does not roll pods.
    pub suspend: bool,
    /// The workloads: one for `combined`, two for `split` (the workers, then the front). The
    /// first is the one `replicas` of a [`RuntimeStatus`](crate::RuntimeStatus) counts.
    pub workloads: Vec<Workload>,
    /// Files a provider materialises by name (on Kubernetes: ConfigMaps).
    pub file_sets: Vec<FileSet>,
    /// How the agent is reached.
    pub network: Network,
    /// Run pods (slice D15): what the provider makes besides the workloads so that the workers can make
    /// pods of their own for the commands of a run, and what guards the pods they make. `None`: none, and
    /// the spec serialises exactly as it did before the field existed. Not part of the digest: it is
    /// *applied*, and what a pod reads of it is already in the workloads (`service_account`, the
    /// variables, the template's file set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_pods: Option<RunPods>,
}

impl RuntimeSpec {
    /// The replicas that step runs when everything runs: the sum over the workloads whose role
    /// [`runs_workers`](Role::runs_workers).
    pub fn worker_replicas(&self) -> u32 {
        self.workloads
            .iter()
            .filter(|w| w.role.runs_workers())
            .map(|w| w.replicas)
            .sum()
    }

    /// The invariants every provider relies on, so each rejects the same malformed spec the same
    /// way: at least one workload, unique names, every mount names a volume of its workload, every
    /// file volume names a file set of the spec, the network selects a workload, no empty names.
    ///
    /// # Errors
    ///
    /// [`RuntimeError::InvalidSpec`] naming the first broken invariant.
    pub fn check(&self) -> Result<(), RuntimeError> {
        let bad = |what: String| Err(RuntimeError::InvalidSpec(what));
        if self.workloads.is_empty() {
            return bad("no workload".into());
        }
        let mut workloads = BTreeSet::new();
        for w in &self.workloads {
            if w.name.is_empty() {
                return bad("a workload has no name".into());
            }
            if !workloads.insert(w.name.as_str()) {
                return bad(format!("workload {} appears twice", w.name));
            }
            if w.container.image.is_empty() {
                return bad(format!("workload {}: the container has no image", w.name));
            }
            let mut volumes = BTreeSet::new();
            for v in &w.volumes {
                if v.name.is_empty() || !volumes.insert(v.name.as_str()) {
                    return bad(format!(
                        "workload {}: volume {:?} is empty or repeated",
                        w.name, v.name
                    ));
                }
                match &v.source {
                    VolumeSource::Files { file_set, .. } => {
                        if !self.file_sets.iter().any(|f| &f.name == file_set) {
                            return bad(format!(
                                "workload {}: volume {} names the file set {file_set}, which the spec lacks",
                                w.name, v.name
                            ));
                        }
                    }
                    VolumeSource::SecretFile { secret, file, .. } => {
                        if secret.name.is_empty() || secret.key.is_empty() || file.is_empty() {
                            return bad(format!(
                                "workload {}: volume {} has an empty secret reference",
                                w.name, v.name
                            ));
                        }
                    }
                    VolumeSource::Persistent(p) => {
                        if p.size.is_empty() {
                            return bad(format!(
                                "workload {}: volume {} has no size",
                                w.name, v.name
                            ));
                        }
                    }
                    VolumeSource::ExternalFiles { name, .. } => {
                        if name.is_empty() {
                            return bad(format!(
                                "workload {}: volume {} names no file set",
                                w.name, v.name
                            ));
                        }
                    }
                    VolumeSource::Scratch(s) => {
                        if s.size_limit.as_deref().is_some_and(|l| l.trim().is_empty()) {
                            return bad(format!(
                                "workload {}: volume {} has an empty size limit",
                                w.name, v.name
                            ));
                        }
                    }
                    VolumeSource::AccountToken => {
                        if w.service_account.is_none() {
                            return bad(format!(
                                "workload {}: volume {} is the credentials of the workload's service account, and it runs as none",
                                w.name, v.name
                            ));
                        }
                    }
                }
            }
            // The account's token is for the agent container alone: a sidecar that holds it is a sidecar that can act as
            // the account (the git sandbox exists because one container must not).
            let token_volumes: BTreeSet<&str> = w
                .volumes
                .iter()
                .filter(|v| matches!(v.source, VolumeSource::AccountToken))
                .map(|v| v.name.as_str())
                .collect();
            for c in &w.sidecars {
                if let Some(m) = c
                    .mounts
                    .iter()
                    .find(|m| token_volumes.contains(m.volume.as_str()))
                {
                    return bad(format!(
                        "workload {}: the sidecar {} mounts {}, the credentials of the service account, which only the agent container may hold",
                        w.name, c.name, m.volume
                    ));
                }
            }
            for c in std::iter::once(&w.container).chain(&w.sidecars) {
                if c.name.is_empty() {
                    return bad(format!("workload {}: a container has no name", w.name));
                }
                for m in &c.mounts {
                    if !volumes.contains(m.volume.as_str()) {
                        return bad(format!(
                            "workload {}: container {} mounts {}, which is no volume of the workload",
                            w.name, c.name, m.volume
                        ));
                    }
                }
                let mut names = BTreeSet::new();
                for e in &c.env {
                    if e.name.is_empty() || !names.insert(e.name.as_str()) {
                        return bad(format!(
                            "workload {}: container {} sets the variable {:?} twice or with no name",
                            w.name, c.name, e.name
                        ));
                    }
                    if let EnvValue::Secret(s) = &e.value
                        && (s.name.is_empty() || s.key.is_empty())
                    {
                        return bad(format!(
                            "workload {}: container {}: {} has an empty secret reference",
                            w.name, c.name, e.name
                        ));
                    }
                }
            }
        }
        let mut sets = BTreeSet::new();
        for f in &self.file_sets {
            if f.name.is_empty() || !sets.insert(f.name.as_str()) {
                return bad(format!("file set {:?} is empty or repeated", f.name));
            }
        }
        if !workloads.contains(self.network.selects.as_str()) {
            return bad(format!(
                "the network selects {:?}, which is no workload",
                self.network.selects
            ));
        }
        if self.network.port == 0 {
            return bad("the network port is 0".into());
        }
        self.check_run_pods()
    }

    /// The invariants of [`run_pods`](Self::run_pods): a workload runs as the account only if the spec has
    /// run pods and names that account, at least one workload does when it has, and nothing the provider
    /// writes a name or a number from is empty or zero.
    fn check_run_pods(&self) -> Result<(), RuntimeError> {
        let bad = |what: String| Err(RuntimeError::InvalidSpec(what));
        let Some(rp) = &self.run_pods else {
            return match self.workloads.iter().find(|w| w.service_account.is_some()) {
                Some(w) => bad(format!(
                    "workload {} runs as an account, and the spec has no run pods",
                    w.name
                )),
                None => Ok(()),
            };
        };
        let empty = |name: &str, value: &str| -> Result<(), RuntimeError> {
            if value.trim().is_empty() {
                bad(format!("run pods: {name} is empty"))
            } else {
                Ok(())
            }
        };
        empty("the service account", &rp.service_account)?;
        empty("the role", &rp.role)?;
        empty("the priority class", &rp.priority_class)?;
        empty("the memory limit of the quota", &rp.quota.limits_memory)?;
        empty("the label that marks a pod managed", &rp.pods.managed_by)?;
        empty("the label that carries the run", &rp.pods.run_label)?;
        empty("the work claim", &rp.guard.claim)?;
        empty("the name of the model key's Secret", &rp.guard.secret.name)?;
        empty("the key of the model key's Secret", &rp.guard.secret.key)?;
        if rp.quota.pods == 0 {
            return bad("run pods: the quota allows no pod".into());
        }
        let proxy = &rp.network.proxy;
        empty("the image of the egress proxy", &proxy.image)?;
        empty(
            "the allow-list directory of the egress proxy",
            &proxy.allow_dir,
        )?;
        empty(
            "the name of the egress proxy's allow-list",
            &proxy.allow.name,
        )?;
        if proxy.command.is_empty() {
            return bad("run pods: the egress proxy has no command".into());
        }
        if proxy.port == 0 {
            return bad("run pods: the egress proxy's port is 0".into());
        }
        if !proxy.health_path.starts_with('/') {
            return bad("run pods: the egress proxy's health path is not a path".into());
        }
        if proxy.replicas == 0 {
            return bad("run pods: the egress proxy has no replica".into());
        }
        if proxy.egress_ports.is_empty() || proxy.egress_ports.contains(&0) {
            return bad("run pods: the egress proxy may reach no port, or port 0".into());
        }
        if proxy.allow.files.is_empty() {
            return bad("run pods: the egress proxy's allow-list has no file".into());
        }
        if proxy.run_as == 0 {
            return bad("run pods: the egress proxy would run as root".into());
        }
        if rp.guard.images.is_empty() || rp.guard.images.iter().any(|i| i.trim().is_empty()) {
            return bad("run pods: the guard allows no image, or an empty one".into());
        }
        for w in &self.workloads {
            if let Some(account) = &w.service_account
                && account != &rp.service_account
            {
                return bad(format!(
                    "workload {} runs as {account}, and the run pods are for {}",
                    w.name, rp.service_account
                ));
            }
        }
        if !self
            .workloads
            .iter()
            .any(|w| w.service_account.as_ref() == Some(&rp.service_account))
        {
            return bad(format!(
                "run pods are for {}, and no workload runs as it",
                rp.service_account
            ));
        }
        Ok(())
    }
}

/// One set of identical pods.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workload {
    /// The object name: `<svc>`, or `<svc>-front`.
    pub name: String,
    /// The part of adam it runs.
    pub role: Role,
    /// Pods when running. Not part of the digest.
    pub replicas: u32,
    /// The pods keep their name across restarts and each owns its volumes (a StatefulSet on
    /// Kubernetes). True when a volume is per replica, and when a pod name is a worker's identity
    /// (`EnvValue::PodName`: runs are pinned to a worker by it, so a new name strands them).
    pub stable_identity: bool,
    /// The agent container.
    pub container: Container,
    /// Containers that start before the agent, run beside it and stop after it (native sidecars).
    pub sidecars: Vec<Container>,
    /// The volumes of the pods.
    pub volumes: Vec<VolumeSpec>,
    /// The security context the pod asks for. Everything else is the provider's fixed hardening:
    /// no service-account token, non-root, the runtime's default seccomp profile, no capabilities,
    /// no privilege escalation.
    pub security: Security,
    /// Seconds a pod has to stop.
    pub termination_grace_secs: Option<u32>,
    /// Pods that must stay up through a voluntary disruption (a PodDisruptionBudget); `None`: no
    /// budget. Not part of the digest.
    pub min_available: Option<u32>,
    /// The account the pods run as: the one that may make run pods ([`RuntimeSpec::run_pods`]). `None`: the
    /// namespace's default account and no token, as ever. The pod never mounts the account's token by itself: it
    /// is a volume of the workload ([`VolumeSource::AccountToken`]) that **the agent container alone** mounts
    /// (`check` refuses a sidecar that does). Part of the digest only when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account: Option<String>,
}

/// One container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Container {
    /// Its name in the pod.
    pub name: String,
    /// The image reference, a tag or a tag and a digest.
    pub image: String,
    /// Replaces the image's entrypoint. Empty: the image's own.
    pub command: Vec<String>,
    /// Arguments.
    pub args: Vec<String>,
    /// The environment, in order. Unique names.
    pub env: Vec<EnvVar>,
    /// The port it serves, named `http`. `None`: it serves none.
    pub port: Option<u16>,
    /// Where the volumes are mounted.
    pub mounts: Vec<Mount>,
    /// The probes.
    pub probes: Probes,
    /// CPU and memory.
    pub resources: Resources,
    /// The container's root file system is read-only: it writes to its volumes and nothing else. Off unless set, and
    /// then not in the serialised spec, so the digest of a spec that never sets it is what it was.
    #[serde(default, skip_serializing_if = "is_false")]
    pub read_only_root_filesystem: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One environment variable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    /// Its name.
    pub name: String,
    /// Where its value comes from.
    pub value: EnvValue,
}

impl EnvVar {
    /// A variable with a literal value.
    pub fn literal(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: EnvValue::Literal(value.into()),
        }
    }

    /// A variable whose value is a key of a Secret.
    pub fn secret(name: impl Into<String>, secret: SecretRef) -> Self {
        Self {
            name: name.into(),
            value: EnvValue::Secret(secret),
        }
    }
}

/// Where the value of a variable comes from. Closed on purpose: a new source must fail to compile
/// in every provider, and a secret is a reference and never a value (AD-024).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnvValue {
    /// A value written in the spec. Never a secret.
    Literal(String),
    /// A key of a Secret the provider hands to the process without reading it.
    Secret(SecretRef),
    /// The name of the pod itself (the downward API's `metadata.name`).
    PodName,
    /// The namespace of the pod itself (the downward API's `metadata.namespace`).
    PodNamespace,
}

/// A volume mounted into a container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mount {
    /// The volume's name, one of the workload's.
    pub volume: String,
    /// Where it is mounted.
    pub path: String,
    /// Mounted read-only.
    pub read_only: bool,
}

/// A probe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probe {
    /// What it does.
    pub action: ProbeAction,
    /// Seconds between probes.
    pub period_secs: u32,
    /// Seconds a probe may take. `None`: the provider's default.
    pub timeout_secs: Option<u32>,
    /// Failures before it counts as failed. `None`: the provider's default.
    pub failure_threshold: Option<u32>,
}

/// What a probe does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeAction {
    /// `GET` this path on the container's port.
    Http {
        /// The path.
        path: String,
    },
    /// Run this command in the container; success is exit code 0.
    Exec {
        /// The command and its arguments.
        command: Vec<String>,
    },
}

/// The probes of a container.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Probes {
    /// Ready to be probed for liveness: the process has started.
    pub startup: Option<Probe>,
    /// Alive: restart it when this fails.
    pub liveness: Option<Probe>,
    /// Ready: take it out of the Service when this fails.
    pub readiness: Option<Probe>,
}

/// CPU and memory, as quantity strings (`500m`, `1Gi`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resources {
    /// What the container is guaranteed.
    pub requests: BTreeMap<String, String>,
    /// What it may not exceed.
    pub limits: BTreeMap<String, String>,
}

/// The pod's security context. The ids are applied as given.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Security {
    /// `runAsUser`.
    pub run_as_user: i64,
    /// `runAsGroup`.
    pub run_as_group: i64,
    /// `fsGroup`.
    pub fs_group: i64,
    /// `fsGroupChangePolicy`. `None`: the provider's default.
    pub fs_group_change_policy: Option<FsGroupChangePolicy>,
}

/// When the group ownership of a volume is changed to `fsGroup`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FsGroupChangePolicy {
    /// Only when the root of the volume has the wrong owner.
    OnRootMismatch,
    /// Always.
    Always,
}

/// A volume of a workload's pods.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VolumeSpec {
    /// Its name, unique in the workload. A persistent volume's claim is named after it.
    pub name: String,
    /// What backs it.
    pub source: VolumeSource,
}

/// What backs a volume. Closed on purpose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum VolumeSource {
    /// Durable storage. **Data**: not removed with the service unless the deletion policy says so.
    Persistent(PersistentVolume),
    /// A file set of the spec, mounted read-only, each file at its path.
    Files {
        /// The name of one of [`RuntimeSpec::file_sets`].
        file_set: String,
        /// The permission bits of the files (`0o444`).
        mode: u32,
    },
    /// A file set that someone else owns (on Kubernetes: a ConfigMap the user made), mounted as it
    /// is. The operator cannot read it, so a change of its content is not a rollout.
    ExternalFiles {
        /// Its name.
        name: String,
        /// The permission bits of the files.
        mode: u32,
    },
    /// One key of a Secret, mounted as one file: a private key is a file and never a variable.
    SecretFile {
        /// The Secret key.
        secret: SecretRef,
        /// The file's name in the mount.
        file: String,
        /// The permission bits (`0o440`).
        mode: u32,
    },
    /// Space that lives and dies with the pod (on Kubernetes an `emptyDir`): a socket two containers share, a
    /// `/tmp` of one. It is **not** data: nothing of it survives the pod.
    Scratch(ScratchVolume),
    /// The credentials of the workload's service account ([`Workload::service_account`]) as an in-cluster client
    /// reads them: a short-lived token, the cluster's certificate authority and the namespace, in one directory
    /// (on Kubernetes the projected `kube-api-access` volume the kubelet would otherwise mount into **every**
    /// container). Mounted into the agent container alone, so that a sidecar holds no credential for the cluster:
    /// `RuntimeSpec::check` refuses a sidecar that mounts it, and a workload with no account.
    AccountToken,
}

/// A [`VolumeSource::Scratch`] volume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScratchVolume {
    /// In memory (a tmpfs) and not on the node's disk: what is written counts against the memory limit of the
    /// containers that write it.
    pub in_memory: bool,
    /// The most it may hold, a quantity (`64Mi`). `None`: no limit of its own. A program that fills a limited
    /// volume gets "no space left on device"; one that fills an unlimited volume on disk gets the pod evicted.
    pub size_limit: Option<String>,
}

/// A persistent volume.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistentVolume {
    /// Requested size, a quantity string (`20Gi`).
    pub size: String,
    /// Storage class. `None`: the cluster's default.
    pub storage_class: Option<String>,
    /// Who shares it.
    pub sharing: Sharing,
}

/// Who shares a persistent volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sharing {
    /// One volume per replica (a `volumeClaimTemplate`), mounted by that replica alone
    /// (ReadWriteOnce).
    PerReplica,
    /// One volume for the service, mounted by every replica at once (ReadWriteMany). Named
    /// `<service>-<volume name>`.
    Shared,
}

/// Files a provider materialises by name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSet {
    /// Its name, unique in the spec.
    pub name: String,
    /// Relative path to content. Paths may contain `/`; a provider whose storage cannot
    /// (ConfigMap keys) maps them and mounts each file at its path.
    pub files: BTreeMap<String, String>,
    /// The name carries the content, so the set is never edited: a new content is a new set, and a
    /// provider removes the superseded ones once the rollout is done.
    pub immutable: bool,
}

/// How the agent is reached: a Service, and when there are peers, who may reach it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Network {
    /// The port the Service listens on and the container serves.
    pub port: u16,
    /// The name of the workload the Service sends to (the front, with `split`).
    pub selects: String,
    /// Who may reach the port. Empty: nobody is restricted and no policy is made. Ingress only: the
    /// agent needs git hosts, the model gateway and registries, so there is no egress rule.
    pub allow_from: Vec<Peer>,
}

/// One peer of an ingress rule: what a Kubernetes `NetworkPolicyPeer` says, in neutral terms. At
/// least one field is set, and `cidr` is not combined with the selectors.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    /// Peers in the namespaces these labels select.
    pub namespaces: Option<Selector>,
    /// Peers among the pods these labels select.
    pub pods: Option<Selector>,
    /// Peers at these addresses.
    pub cidr: Option<IpBlock>,
}

/// A label selector.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selector {
    /// Labels that must equal.
    pub match_labels: BTreeMap<String, String>,
    /// Further requirements.
    pub match_expressions: Vec<Requirement>,
}

/// One requirement of a [`Selector`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirement {
    /// The label key.
    pub key: String,
    /// How it is compared.
    pub operator: SelectorOperator,
    /// The values, for `In` and `NotIn`.
    pub values: Vec<String>,
}

/// How a [`Requirement`] compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectorOperator {
    /// The label's value is one of `values`.
    In,
    /// The label's value is none of `values`.
    NotIn,
    /// The label exists.
    Exists,
    /// The label does not exist.
    DoesNotExist,
}

/// A range of addresses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpBlock {
    /// A CIDR, e.g. `10.0.0.0/8`.
    pub cidr: String,
    /// Sub-ranges left out.
    pub except: Vec<String>,
}

// ------------------------------------------------------------------------------------ run pods

/// Run pods (slice D15, adam-rs ADR 0019 and 0041): the pods the workers make for the commands of a
/// run, and what the provider makes so that they may, and may only, make those. Nothing in it is a
/// Kubernetes type; a provider that cannot make pods refuses a spec that has it.
///
/// The pod template the workers read is a file set of the spec and a volume of the workloads
/// (`aap-domain` makes both); what is here is what is applied beside the workloads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPods {
    /// The account the workers run as ([`Workload::service_account`]): the one account that may make
    /// the pods, and the one the guard is for. The provider makes it.
    pub service_account: String,
    /// The set of rights the account is given (on Kubernetes: a `Role` of the namespace that lets it
    /// make, find, watch and delete pods and `exec` in them, which the deployment made and the provider
    /// binds the account to: the provider holds the right to bind that role and no right of its own to
    /// make a pod or `exec`).
    pub role: String,
    /// The class the pods have, and the quota is scoped to (a quota cannot select pods by label, but it
    /// can by priority class). The deployment made it.
    pub priority_class: String,
    /// What the pods may use in all.
    pub quota: RunPodQuota,
    /// What the pods may reach: deny by default, and the egress proxy. Always present: a spec with run pods and no
    /// network policy for them is not one.
    pub network: RunPodNetwork,
    /// How the workers label what they make.
    pub pods: RunPodLabels,
    /// What the guard lets a pod made by the account be.
    pub guard: RunPodGuard,
}

/// What the pods of one agent may use in all. A pod beyond it is refused with a quota error, which the
/// workers wait out.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPodQuota {
    /// At most this many pods.
    pub pods: u32,
    /// At most this much memory, as a quantity (`8Gi`), counted on the limits of the pods.
    pub limits_memory: String,
}

/// How the workers label the pods they make: what a policy that selects them reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPodLabels {
    /// The value of `app.kubernetes.io/managed-by` on a run pod (adam's `adam-coder`).
    pub managed_by: String,
    /// The label whose presence marks a pod as a run's (adam's `adam.vymalo.com/run`).
    pub run_label: String,
}

/// What a run pod may reach, **deny by default** (adam-rs ADR 0042): nothing connects to it (the workers `exec` in it
/// through the API server), and it reaches names, the egress proxy of its service, and what `extra_egress` adds, and
/// nothing else. There is no value that opens the internet to it, and no spec without this: run pods and their network
/// go together.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPodNetwork {
    /// Where names are resolved: the peer, on port 53 over UDP and TCP.
    pub dns: Peer,
    /// The one way out to the internet: the proxy the provider makes beside the run pods.
    pub proxy: EgressProxy,
    /// Further egress rules, for what the proxy cannot serve (an in-cluster target such as a model gateway).
    pub extra_egress: Vec<EgressRule>,
}

/// The HTTPS-only egress proxy of a service's run pods (adam-rs ADR 0042): a small workload of its own, from the
/// agent's image, that accepts `CONNECT host:port` to the hosts of an allow-list and nothing else, and the one
/// destination the network policy of the run pods names besides DNS. The provider makes its Deployment, Service,
/// ServiceAccount (no token), allow-list file, PodDisruptionBudget (more than one replica) and NetworkPolicy; what
/// the process is and how it is told its settings (command, variables) are the spec's, so that the provider knows
/// nothing of adam.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressProxy {
    /// The image, a tag or a tag and a digest: the agent's own.
    pub image: String,
    /// The command (the image's entrypoint is replaced).
    pub command: Vec<String>,
    /// The environment: literals only.
    pub env: Vec<EnvVar>,
    /// The port it serves: the Service, the container and the run pods' policy name it.
    pub port: u16,
    /// The path that answers 200 on [`port`](Self::port) while it serves: its readiness and liveness probes.
    pub health_path: String,
    /// Pods. With more than one a budget keeps one up through a voluntary disruption.
    pub replicas: u32,
    /// The allow-list, a file set of one file. Mounted read-only at [`allow_dir`](Self::allow_dir); the process reads it at
    /// start only, so a changed list is a rollout of the proxy.
    pub allow: FileSet,
    /// Where the allow-list's directory is mounted.
    pub allow_dir: String,
    /// The TCP ports its pods may reach, besides DNS: the only ones the proxy's policy opens.
    pub egress_ports: Vec<u16>,
    /// Ranges its pods never reach: left out of the internet its policy opens. IPv4 only.
    pub deny_cidrs: Vec<String>,
    /// The user its pods run as (non-root).
    pub run_as: i64,
    /// CPU and memory.
    pub resources: Resources,
}

/// One egress rule: to these peers (empty: to anywhere), on these ports (empty: on any).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressRule {
    /// The peers.
    pub to: Vec<Peer>,
    /// The ports.
    pub ports: Vec<EgressPort>,
}

/// One port of an [`EgressRule`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressPort {
    /// The protocol.
    pub protocol: Protocol,
    /// The port, a number. `None`: every port of the protocol.
    pub port: Option<u16>,
    /// The last port of a range that starts at `port`.
    pub end_port: Option<u16>,
}

/// A transport protocol of a network rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Protocol {
    /// TCP.
    Tcp,
    /// UDP.
    Udp,
    /// SCTP.
    Sctp,
}

/// What the guard lets a pod made by the run-pods account be: the guard refuses any other. The images,
/// the claim and the one Secret key are the agent's; the rest of the guard (not privileged, not root, no
/// host namespace, no `hostPath`, no Secret volume, no token) is the same for every agent and is the
/// provider's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPodGuard {
    /// The images a container of a pod may have, spelled as the pod spells them.
    pub images: Vec<String>,
    /// The one persistent claim a pod may mount (besides empty directories): the work volume.
    pub claim: String,
    /// The one Secret key a pod may reference, as one variable: the model key.
    pub secret: SecretRef,
}
