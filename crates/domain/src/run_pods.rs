//! Run pods (slice D15a, §60a): the deployment's settings for them, the Pod adam reads as its template
//! (`RUN_POD_TEMPLATE_FILE`), and what an `AgentConfig` with `coder.runPods` makes of both.
//!
//! The settings are the operator chart's (`runPods`, `runPodClasses`), handed in a file the composition root
//! reads once: this crate is pure and reads no file. The pod template is adam's input like the extra MCP
//! file is, so it is made here, as the chart's `_runpods.tpl` makes it, and the parity goldens
//! (`tests/golden/coder-run-pods*.json`) hold the two equal.

use std::collections::{BTreeMap, BTreeSet};

use aap_ports::{
    Container, EgressPort, EgressProxy, EgressRule, EnvVar, FileSet, Mount, Peer, Probe,
    ProbeAction, Probes, Protocol, Resources, RunPodGuard, RunPodLabels, RunPodNetwork,
    RunPodNodeType, RunPodQuota, RunPods, ScratchVolume, Security, Selector, VolumeSource,
    VolumeSpec,
};
use k8s_openapi::api::core::v1::{NodeAffinity, Toleration};
use k8s_openapi::api::networking::v1::NetworkPolicyEgressRule;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::contract::{self, run_pods as rp};
use crate::syntax::{is_dns_label, is_dns_subdomain, is_env_name, is_quantity, quantity_nanos};

/// What the deployment says about run pods: the operator chart's `runPods` block, its `runPodClasses`, the
/// names of the cluster-scoped objects it made, as the `runPods` block of the operator's configuration file
/// (`bin/operator/README.md`). An
/// unknown key is refused. Everything the file does not default is required: the chart writes it all, so the
/// defaults are in one place.
///
/// A `RunPodSettings` is **checked** by [`validate`](Self::validate); the operator refuses to start with one
/// that fails, as it refuses a default image with white space in it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunPodSettings {
    /// The `PriorityClass` of run pods, which the quota is scoped to and the admission policy requires. The
    /// chart made it (or the deployment manages it); the operator makes no cluster-scoped object.
    pub priority_class: String,
    /// The `RuntimeClass` of run pods (K14a, AD-052), when the deployment names one: the object is the administrator's,
    /// and the admission policy requires this class once it is set. Empty or absent: none, and the template is what it was.
    #[serde(default)]
    pub runtime_class_name: Option<String>,
    /// The `Role` that lets an account make, find, watch and delete pods and `exec` in them. The chart made it;
    /// the operator binds each coder's account to it and holds the right to bind that role alone.
    pub role: String,
    /// The size classes, by name (AD-031): the resources of the run container.
    pub classes: BTreeMap<String, RunPodClass>,
    /// The run container's image: the workspace image of another-agentic-images.
    pub image: RunImage,
    /// The container of the template the commands run in (`RUN_POD_CONTAINER`).
    pub container: String,
    /// `CARGO_BUILD_JOBS` of the run container.
    pub cargo_build_jobs: u32,
    /// More variables of the run container (never a secret).
    #[serde(default)]
    pub extra_env: BTreeMap<String, String>,
    /// What the run pods of one agent may use in all.
    pub quota: RunPodQuotaSettings,
    /// Idle time before a pod is deleted (`RUN_POD_IDLE_SECS`); 0 keeps it until its run ends.
    pub idle_secs: u32,
    /// How long a pod may take to be ready (`RUN_POD_READY_TIMEOUT_SECS`).
    pub ready_timeout_secs: u32,
    /// How long a run retries for a pod the cluster cannot give now (`RUN_POD_WAIT_SECS`).
    pub wait_secs: u32,
    /// `nodeSelector` of the template.
    #[serde(default)]
    pub node_selector: BTreeMap<String, String>,
    /// The node types the deployment admits for run pods (ANR-660, AD-066). Empty (the default) is no restriction, and then the
    /// field is not written, so no file, template or digest moves.
    #[serde(default, skip_serializing_if = "NodeTypeSettings::is_empty")]
    pub node_type: NodeTypeSettings,
    /// `tolerations` of the template, as written.
    #[serde(default)]
    pub tolerations: Vec<Value>,
    /// `affinity.nodeAffinity` of the template, as written. Absent or `{}`: none.
    #[serde(default)]
    pub node_affinity: Option<Value>,
    /// Images a run pod may use besides the run image and the coder's own (the init container's), spelled as the
    /// pod spells them.
    #[serde(default)]
    pub extra_allowed_images: Vec<String>,
    /// What a run pod may reach: names, the egress proxy and `extraEgress`, and nothing else.
    pub network_policy: RunPodNetworkSettings,
    /// The one way out of a run pod to the internet (slice D15c; adam-rs ADR 0042).
    pub egress_proxy: EgressProxySettings,
    /// The git sandbox of the coder's pod (slice D15d; adam-rs ADR 0041, open decision 6): the sidecar the coder's local git
    /// runs in. There is no switch that turns it off while run pods are on.
    pub git_sandbox: GitSandboxSettings,
}

/// One size class: the resources of the run container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunPodClass {
    /// What the container is guaranteed.
    #[serde(default)]
    pub requests: BTreeMap<String, String>,
    /// What it may not exceed. `memory` is required: the quota counts it.
    #[serde(default)]
    pub limits: BTreeMap<String, String>,
    /// The `RuntimeClass` of the pods of THIS class (ANR-402, AD-052): it replaces the deployment's `runtimeClassName` for them,
    /// so one class can be sandboxed with another runtime handler than the rest, or sandboxed while the deployment names none.
    /// Empty or absent: the deployment's, if it names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_class_name: Option<String>,
}

/// The node types a run pod may be scheduled on (ANR-660, AD-066): the key of a node label the deployment keeps honest and the values
/// of it that it admits. The template is pinned to them by a required node affinity and the admission policy refuses a run pod that
/// is not. Both parts or neither.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
pub struct NodeTypeSettings {
    /// The node label's key (`example.com/node-type`). Empty: off.
    pub label_key: String,
    /// The label values admitted.
    pub admitted: Vec<String>,
}

impl NodeTypeSettings {
    /// Whether no node type is named: the restriction is off.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.label_key.is_empty() && self.admitted.is_empty()
    }
}

/// The size weight of a run-pod class (ANR-660, AD-066): the larger of `requests.cpu` / 1 vCPU and `requests.memory` / 2 GiB, which
/// must be one of the grid 0.25, 0.5, 1, 2, 4, 8. Held as **quarter-units** (1, 2, 4, 8, 16, 32), so that it is exact: no float is
/// ever made of a quantity, and a class that is off the grid is refused and never rounded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SizeWeight(u8);

impl SizeWeight {
    /// The grid, in quarter-units.
    pub const GRID: [u8; 6] = [1, 2, 4, 8, 16, 32];

    /// The weight in quarter-units: 1 is 0.25, 32 is 8.
    #[must_use]
    pub fn quarters(self) -> u8 {
        self.0
    }

    /// The weight of a class, from its `requests` alone (the run container's).
    ///
    /// # Errors
    ///
    /// A sentence that names the key: a request missing or not a plain quantity, or a weight that is not on the grid.
    pub fn of(class: &RunPodClass) -> Result<Self, String> {
        // One vCPU and two GiB (binary: `2G` is not `2Gi`), in the nano-units `quantity_nanos` counts in.
        const CPU_UNIT: u128 = 1_000_000_000;
        const MEMORY_UNIT: u128 = 2 * (1 << 30) * 1_000_000_000;
        // `quantity_nanos` drops digits finer than a nano-unit, which would weigh `1.0000000001` CPU as exactly 1: the weight is made
        // of an EXACT value, and a quantity that does not fit one is refused. Memory must also be a whole number of bytes.
        let ask = |key: &str, bytes: bool| -> Result<u128, String> {
            let value = class.requests.get(key).ok_or_else(|| {
                format!("requests.{key} is required: the size weight is made of it")
            })?;
            exact_nanos(value)
                .filter(|n| !bytes || n % CPU_UNIT == 0)
                .ok_or_else(|| {
                    format!("requests.{key}: {value:?} is not an exact plain quantity (no exponent, no digit finer than the unit's resolution) that fits")
                })
        };
        let cpu = ask("cpu", false)?;
        let memory = ask("memory", true)?;
        // The larger ratio, by cross-multiplication: cpu/CPU_UNIT against memory/MEMORY_UNIT.
        let overflow = || "requests are too large to weigh".to_owned();
        let cpu_scaled = cpu.checked_mul(MEMORY_UNIT).ok_or_else(overflow)?;
        let memory_scaled = memory.checked_mul(CPU_UNIT).ok_or_else(overflow)?;
        let (numerator, denominator) = if cpu_scaled >= memory_scaled {
            (cpu, CPU_UNIT)
        } else {
            (memory, MEMORY_UNIT)
        };
        // quarters = 4 * numerator / denominator, which must be a whole number on the grid.
        let quarters = numerator.checked_mul(4).ok_or_else(overflow)?;
        for step in Self::GRID {
            if quarters
                == u128::from(step)
                    .checked_mul(denominator)
                    .ok_or_else(overflow)?
            {
                return Ok(Self(step));
            }
        }
        Err(format!(
            "requests (cpu {:?}, memory {:?}) weigh nothing on the grid 0.25, 0.5, 1, 2, 4, 8 (1 vCPU or 2 GiB is 1; the larger of the two counts, and it must be exactly one of them)",
            class.requests.get("cpu").map_or("", String::as_str),
            class.requests.get("memory").map_or("", String::as_str),
        ))
    }
}

impl std::fmt::Display for SizeWeight {
    /// The decimal grid value: `0.25`, `0.5`, `1`, `2`, `4`, `8`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            1 => f.write_str("0.25"),
            2 => f.write_str("0.5"),
            q => write!(f, "{}", q / 4),
        }
    }
}

impl RunPodClass {
    /// The class's own `RuntimeClass`, when it names one (an empty value is none).
    #[must_use]
    pub fn runtime_class_name(&self) -> Option<&str> {
        self.runtime_class_name
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
    }
}

/// The run container's image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunImage {
    /// The repository.
    pub repository: String,
    /// The tag.
    pub tag: String,
    /// `sha256:<64 hex>`, pinned. Empty: none.
    #[serde(default)]
    pub digest: String,
    /// `IfNotPresent`, `Always` or `Never`.
    #[serde(default = "if_not_present")]
    pub pull_policy: String,
}

fn if_not_present() -> String {
    "IfNotPresent".to_owned()
}

impl RunImage {
    /// `repository:tag`, and `@digest` when there is one, as the pod spells it.
    #[must_use]
    pub fn reference(&self) -> String {
        if self.digest.is_empty() {
            format!("{}:{}", self.repository, self.tag)
        } else {
            format!("{}:{}@{}", self.repository, self.tag, self.digest)
        }
    }
}

/// The quota of one agent's run pods.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunPodQuotaSettings {
    /// A quantity (`8Gi`), counted on the limits of the pods.
    pub limits_memory: String,
    /// At least 1.
    pub pods: u32,
}

/// What a run pod may reach, deny by default (adam-rs ADR 0042): no ingress; egress to DNS, to the egress proxy and to
/// `extraEgress`. There is no setting that opens the internet to a run pod, and none that turns the policy off: the
/// operator chart refuses `networkPolicy.enabled: false`, and a file that still has the key is refused here (an unknown key).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunPodNetworkSettings {
    /// The pod and service CIDRs of the cluster, besides the ranges below: what the proxy never connects to. IPv4.
    #[serde(default, rename = "clusterCIDRs")]
    pub cluster_cidrs: Vec<String>,
    /// Never reachable through the proxy: RFC 1918, the link-local range, the shared address space. IPv4.
    #[serde(rename = "blockedCIDRs")]
    pub blocked_cidrs: Vec<String>,
    /// The namespace DNS is in.
    pub dns_namespace: String,
    /// The labels of the DNS pods.
    pub dns_pod_labels: BTreeMap<String, String>,
    /// More egress rules for a run pod, as `NetworkPolicy` `egress` entries: `to` (an `ipBlock`, a `namespaceSelector` or a
    /// `podSelector`) and `ports` (numbers). An in-cluster target the proxy cannot serve (it connects to public IPv4
    /// addresses only), such as a model gateway: its host goes in `egressProxy.extraNoProxy` too.
    #[serde(default)]
    pub extra_egress: Vec<Value>,
}

/// The egress proxy of the run pods of one agent (slice D15c; adam-rs ADR 0042, `runPods.egressProxy`): `adam-egress`, an
/// HTTPS-only forward proxy, from the agent's own coder image. It accepts `CONNECT host:port` to the hosts of the allow-list
/// and nothing else, resolves the name itself and connects to public IPv4 addresses only. The allow-list is the
/// **deployment's**: an `AgentConfig` cannot extend it (§60a, D15c, decision 1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressProxySettings {
    /// Pods of the proxy, at least 1. With more than one, a PodDisruptionBudget keeps one up through a drain.
    pub replicas: u32,
    /// The ports a `CONNECT` may name, and the only TCP ports the proxy's pods may reach (443 is HTTPS).
    pub ports: Vec<u16>,
    /// The hosts a run pod may reach: `example.com` is that host, `.example.com` every subdomain of it and not the apex.
    pub allow: Vec<String>,
    /// More hosts, added to `allow` (lower-cased, once each).
    #[serde(default)]
    pub extra_allow: Vec<String>,
    /// More names the tools reach without the proxy (`NO_PROXY`), after `localhost,127.0.0.1`.
    #[serde(default)]
    pub extra_no_proxy: Vec<String>,
    /// CPU and memory of the proxy's container.
    pub resources: EgressResources,
}

/// The resources of a container the operator makes beside the agent's: the egress proxy's, the git sandbox's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContainerResources {
    /// What the container is guaranteed.
    #[serde(default)]
    pub requests: BTreeMap<String, String>,
    /// What it may not exceed.
    #[serde(default)]
    pub limits: BTreeMap<String, String>,
}

/// [`ContainerResources`], by the name D15c gave them.
pub type EgressResources = ContainerResources;

/// The git sandbox of a coder with run pods (slice D15d; adam-rs ADR 0041, open decision 6, `runPods.gitSandbox` of the
/// coder chart): `adam-git-sandbox`, from **the agent's own coder image**, a native sidecar of the coder's pod in which the
/// coder's local `git` runs. A repository's configuration can name a program (a `filter.x.clean`, a `core.fsmonitor`) that
/// git runs, and some repositories are the run's own, so that program has to run where nothing secret is readable: the
/// sidecar has no Secret volume, no variable from a Secret, no service-account token (the token is a volume of the coder
/// container alone), a pid namespace of its own, a read-only root file system and a `/tmp` of its own. It mounts the work
/// volume at the same path as the coder and runs as the same user. It is on with run pods and has no switch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GitSandboxSettings {
    /// CPU and memory of the sidecar. The memory limit is required: a program that a filter starts is stopped by this limit
    /// and not by the node's, and the sidecar's `/tmp` counts against it.
    pub resources: ContainerResources,
    /// The size of the sidecar's `/tmp`, in memory (`64Mi`): a program that fills it gets "no space left on device"; a
    /// disk-backed volume would get the whole coder pod evicted.
    pub tmp_size: String,
    /// `GIT_SANDBOX_MAX_COMMANDS`: git commands run at once, 1 to 4096; the next waits.
    pub max_commands: u32,
}

/// The most commands a sandbox may run at once (the server's own bound).
pub const MAX_SANDBOX_COMMANDS: u32 = 4096;

impl EgressProxySettings {
    /// The hosts the proxy allows, as its allow-list file says them: `allow`, then `extraAllow`, trimmed, lower case, once
    /// each, in order.
    #[must_use]
    pub fn allow_list(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        self.allow
            .iter()
            .chain(&self.extra_allow)
            .map(|h| h.trim().to_ascii_lowercase())
            .filter(|h| seen.insert(h.clone()))
            .collect()
    }

    /// `NO_PROXY` of a run pod: the loopback names, then `extraNoProxy`.
    #[must_use]
    pub fn no_proxy(&self) -> String {
        rp::egress::NO_PROXY
            .iter()
            .map(|n| (*n).to_owned())
            .chain(self.extra_no_proxy.iter().cloned())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The text of the allow-list file: a header line, then one host per line.
    #[must_use]
    pub fn allow_file(&self) -> String {
        let mut text = format!("{}\n", rp::egress::ALLOW_HEADER);
        for host in self.allow_list() {
            text.push_str(&host);
            text.push('\n');
        }
        text
    }
}

/// Why an entry of the allow-list is not one, as the coder chart's `_validate.tpl` says it: a host name (no scheme, port,
/// path, wildcard or IP address), not a bare suffix. `None` when it is fine.
fn allow_entry_problem(entry: &str) -> Option<&'static str> {
    let host = entry.trim().to_ascii_lowercase();
    let body = host.strip_prefix('.').unwrap_or(&host);
    if body.is_empty() || !body.split('.').all(is_dns_label) {
        return Some(
            "is not a host name: write example.com for one host and .example.com for every subdomain of it (no scheme, port, path, wildcard or IP address)",
        );
    }
    let last = body.rsplit('.').next().unwrap_or(body);
    if last.bytes().all(|b| b.is_ascii_digit()) {
        return Some(
            "ends in digits only, which reads as an IP address, and the proxy refuses IP literals",
        );
    }
    if host.starts_with('.') && !body.contains('.') {
        return Some(
            "is a bare suffix, which would allow every host of a top-level domain: name the registrable domain, such as .example.com",
        );
    }
    None
}

/// A `NO_PROXY` entry: a name or a suffix, with no comma and no space.
fn is_no_proxy_entry(entry: &str) -> bool {
    !entry.is_empty()
        && entry
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".*:/_-".contains(&b))
}

impl RunPodSettings {
    /// Settings that offer these size classes and say nothing else true: what a client that knows the names of the
    /// classes (the dashboard's Platform API: the operator's chart tells it the names) asks `validate` with, so that
    /// an agent is not refused for the operator having no run pods when it has, and a class it lacks is. The
    /// resources, the image and the rest are placeholders no check of an agent reads; the operator's own settings
    /// are the real ones.
    #[must_use]
    pub fn offering<'a>(classes: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            priority_class: "stand-in".to_owned(),
            runtime_class_name: None,
            role: "stand-in".to_owned(),
            classes: classes
                .into_iter()
                .map(|name| {
                    (
                        name.to_owned(),
                        RunPodClass {
                            requests: BTreeMap::new(),
                            limits: BTreeMap::from([("memory".to_owned(), "1Gi".to_owned())]),
                            runtime_class_name: None,
                        },
                    )
                })
                .collect(),
            image: RunImage {
                repository: "stand-in.invalid/workspace".to_owned(),
                tag: "unknown".to_owned(),
                digest: String::new(),
                pull_policy: if_not_present(),
            },
            container: "run".to_owned(),
            cargo_build_jobs: 2,
            extra_env: BTreeMap::new(),
            quota: RunPodQuotaSettings {
                limits_memory: "8Gi".to_owned(),
                pods: 4,
            },
            idle_secs: 900,
            ready_timeout_secs: 600,
            wait_secs: 1020,
            node_selector: BTreeMap::new(),
            node_type: NodeTypeSettings::default(),
            tolerations: Vec::new(),
            node_affinity: None,
            extra_allowed_images: Vec::new(),
            network_policy: RunPodNetworkSettings {
                cluster_cidrs: Vec::new(),
                blocked_cidrs: Vec::new(),
                dns_namespace: "kube-system".to_owned(),
                dns_pod_labels: BTreeMap::from([("k8s-app".to_owned(), "kube-dns".to_owned())]),
                extra_egress: Vec::new(),
            },
            egress_proxy: EgressProxySettings {
                replicas: 1,
                ports: vec![443],
                allow: vec!["stand-in.invalid".to_owned()],
                extra_allow: Vec::new(),
                extra_no_proxy: Vec::new(),
                resources: ContainerResources {
                    requests: BTreeMap::new(),
                    limits: BTreeMap::new(),
                },
            },
            git_sandbox: GitSandboxSettings {
                resources: ContainerResources {
                    requests: BTreeMap::new(),
                    limits: BTreeMap::from([("memory".to_owned(), "1Gi".to_owned())]),
                },
                tmp_size: "64Mi".to_owned(),
                max_commands: 64,
            },
        }
    }

    /// The names of the size classes, in order: what `GET /v1/info` offers.
    #[must_use]
    pub fn class_names(&self) -> Vec<String> {
        self.classes.keys().cloned().collect()
    }

    /// The `RuntimeClass` of run pods when the deployment names one: an empty value is none, as the chart leaves the key out.
    #[must_use]
    pub fn runtime_class_name(&self) -> Option<&str> {
        self.runtime_class_name
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
    }

    /// Every mistake of the settings, each as a sentence that names the key. `Ok` when there is none.
    ///
    /// # Errors
    ///
    /// The sentences.
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut problems = Vec::new();
        let mut bad = |what: String| problems.push(what);

        for (key, name) in [
            ("priorityClass", &self.priority_class),
            ("role", &self.role),
        ] {
            if !is_dns_subdomain(name) {
                bad(format!(
                    "{key}: {name:?} is not the name of an object (a DNS-1123 subdomain)"
                ));
            }
        }
        if let Some(class) = self.runtime_class_name()
            && !is_dns_subdomain(class)
        {
            bad(format!(
                "runtimeClassName: {class:?} is not the name of a RuntimeClass (a DNS-1123 subdomain)"
            ));
        }
        if self.classes.is_empty() {
            bad(
                "classes: at least one size class (the operator has no run pods without one)"
                    .into(),
            );
        }
        let quota_memory = quantity_nanos(&self.quota.limits_memory);
        if !is_quantity(&self.quota.limits_memory) || quota_memory.is_none_or(|q| q == 0) {
            bad(format!(
                "quota.limitsMemory: {:?} is not a quantity such as 8Gi",
                self.quota.limits_memory
            ));
        }
        if self.quota.pods == 0 {
            bad("quota.pods: at least 1".into());
        }
        for (name, class) in &self.classes {
            let at = format!("classes.{name}");
            if !is_dns_label(name) {
                bad(format!("{at}: a class's name is a DNS label"));
            }
            for (kind, map) in [("requests", &class.requests), ("limits", &class.limits)] {
                for (resource, value) in map {
                    if !is_quantity(value) {
                        bad(format!(
                            "{at}.{kind}.{resource}: {value:?} is not a quantity"
                        ));
                    }
                }
            }
            // ANR-660 (AD-066): a class is weighed from its requests, and one that is off the grid is refused, not rounded.
            if let Err(why) = SizeWeight::of(class) {
                bad(format!("{at}.{why}"));
            }
            if let Some(runtime) = class.runtime_class_name()
                && !is_dns_subdomain(runtime)
            {
                bad(format!(
                    "{at}.runtimeClassName: {runtime:?} is not the name of a RuntimeClass (a DNS-1123 subdomain)"
                ));
            }
            match class.limits.get("memory") {
                None => bad(format!(
                    "{at}.limits.memory is required: the quota counts it, and a pod without a memory limit is refused by it"
                )),
                Some(limit) => {
                    if let (Some(limit), Some(quota)) = (
                        quantity_nanos(limit),
                        quantity_nanos(&self.quota.limits_memory),
                    ) && limit > quota
                    {
                        bad(format!(
                            "{at}.limits.memory ({}) is above quota.limitsMemory ({}): no pod of this class could ever be made",
                            class.limits.get("memory").map_or("", String::as_str),
                            self.quota.limits_memory
                        ));
                    }
                }
            }
        }
        if self.image.repository.trim().is_empty()
            || self.image.repository.contains(char::is_whitespace)
            || self.image.tag.trim().is_empty()
            || self.image.tag.contains(char::is_whitespace)
        {
            bad("image.repository and image.tag are required, with no space".into());
        }
        if !self.image.digest.is_empty() && !is_sha256(&self.image.digest) {
            bad(format!(
                "image.digest: {:?} is not sha256:<64 hex digits>",
                self.image.digest
            ));
        }
        if !matches!(
            self.image.pull_policy.as_str(),
            "IfNotPresent" | "Always" | "Never"
        ) {
            bad(format!(
                "image.pullPolicy: {:?} is not IfNotPresent, Always or Never",
                self.image.pull_policy
            ));
        }
        if !is_dns_label(&self.container) {
            bad(format!(
                "container: {:?} is not a container name (lower case letters, digits and -)",
                self.container
            ));
        }
        if self.ready_timeout_secs == 0 {
            bad("readyTimeoutSecs: at least 1".into());
        }
        for (name, value) in &self.extra_env {
            if !is_env_name(name) {
                bad(format!("extraEnv.{name}: not a variable name"));
            } else if rp::egress::PROXY_ENV.contains(&name.as_str()) {
                bad(format!(
                    "extraEnv.{name}: the operator sets the proxy variables of a run pod from egressProxy (the egress proxy is the only way out); to add names that bypass it, use egressProxy.extraNoProxy"
                ));
            } else if name == contract::env::MODEL_API_KEY {
                // adam-rs ADR 0038: a run pod reads no Secret, so it holds no model key, and a secret is never a setting.
                bad(format!(
                    "extraEnv.{name}: a run pod holds no model key (adam-rs ADR 0038), and a secret is never a setting"
                ));
            } else if name == "CARGO_BUILD_JOBS" {
                bad(format!(
                    "extraEnv.{name}: the operator sets this variable itself (cargoBuildJobs)"
                ));
            }
            let _ = value;
        }
        for (i, t) in self.tolerations.iter().enumerate() {
            if let Err(why) = only_known_toleration(t, &format!("tolerations[{i}]")) {
                bad(why);
            }
            if serde_json::from_value::<Toleration>(t.clone()).is_err() {
                bad(format!("tolerations[{i}]: not a Kubernetes toleration"));
            }
        }
        if let Some(a) = self.node_affinity_value() {
            if let Err(why) = only_known_node_affinity(a, "nodeAffinity") {
                bad(why);
            }
            if serde_json::from_value::<NodeAffinity>(a.clone()).is_err() {
                bad("nodeAffinity: not a Kubernetes nodeAffinity".into());
            }
        }
        for (i, image) in self.extra_allowed_images.iter().enumerate() {
            if image.trim().is_empty() || image.contains(char::is_whitespace) {
                bad(format!(
                    "extraAllowedImages[{i}]: an image reference with no space"
                ));
            }
        }
        let node_type = &self.node_type;
        if !node_type.is_empty() {
            if node_type.label_key.is_empty() || node_type.admitted.is_empty() {
                bad("nodeType.labelKey and nodeType.admitted go together: name the node label's key and the values of it that run pods may run on, or neither".into());
            }
            if !node_type.label_key.is_empty() && !is_label_key(&node_type.label_key) {
                bad(format!(
                    "nodeType.labelKey: {:?} is not a label key",
                    node_type.label_key
                ));
            }
            for (i, value) in node_type.admitted.iter().enumerate() {
                if value.is_empty() || !is_label_value(value) {
                    bad(format!(
                        "nodeType.admitted[{i}]: {value:?} is not a label value (non-empty here)"
                    ));
                }
            }
            if let Err(why) = self.node_affinity_for_template() {
                bad(format!("nodeAffinity: {why}"));
            }
            if self.node_affinity_names_key(&node_type.label_key) {
                bad(format!(
                    "nodeAffinity: a required term already has a matchExpressions entry on {:?}, the key of nodeType.labelKey: the operator adds its own requirement to every term, and one of the deployment's on the same label would be ANDed with it. Remove it from nodeAffinity",
                    node_type.label_key
                ));
            }
            if self.node_selector.contains_key(&node_type.label_key) {
                bad(format!(
                    "nodeSelector.{}: the key is the node type's (nodeType.labelKey): the operator pins run pods to the admitted values, and a selector on it would fight that",
                    node_type.label_key
                ));
            }
        }
        let net = &self.network_policy;
        for cidr in net.blocked_cidrs.iter().chain(&net.cluster_cidrs) {
            if !is_ipv4_cidr(cidr) {
                bad(format!(
                    "networkPolicy: {cidr:?} is not an IPv4 CIDR such as 10.0.0.0/8 (IPv6 is denied, not allowed)"
                ));
            }
        }
        if !is_dns_label(&net.dns_namespace) {
            bad(format!(
                "networkPolicy.dnsNamespace: {:?} is not a namespace name",
                net.dns_namespace
            ));
        }
        if net.dns_pod_labels.is_empty() {
            bad("networkPolicy.dnsPodLabels: at least one label (the DNS pods)".into());
        }
        for (i, rule) in net.extra_egress.iter().enumerate() {
            if let Err(why) = egress_rule(rule) {
                bad(format!("networkPolicy.extraEgress[{i}]: {why}"));
            }
        }
        let proxy = &self.egress_proxy;
        if proxy.replicas == 0 {
            bad("egressProxy.replicas: at least 1".into());
        }
        if proxy.ports.is_empty() || proxy.ports.contains(&0) {
            bad("egressProxy.ports: a non-empty list of port numbers from 1 to 65535 (443 by default)".into());
        }
        for (key, list) in [("allow", &proxy.allow), ("extraAllow", &proxy.extra_allow)] {
            for (i, entry) in list.iter().enumerate() {
                if let Some(why) = allow_entry_problem(entry) {
                    bad(format!("egressProxy.{key}[{i}]: {entry:?} {why}"));
                }
            }
        }
        if proxy.allow_list().is_empty() {
            bad("egressProxy.allow and extraAllow hold no host: the proxy refuses an empty allow-list (exit 78) and a run pod would reach nothing. Keep the defaults of allow, or list the hosts it needs".into());
        }
        for (i, entry) in proxy.extra_no_proxy.iter().enumerate() {
            if !is_no_proxy_entry(entry) {
                bad(format!(
                    "egressProxy.extraNoProxy[{i}]: {entry:?} is not a name for NO_PROXY (no comma or space)"
                ));
            }
        }
        for (kind, map) in [
            ("requests", &proxy.resources.requests),
            ("limits", &proxy.resources.limits),
        ] {
            for (resource, value) in map {
                if !is_quantity(value) {
                    bad(format!(
                        "egressProxy.resources.{kind}.{resource}: {value:?} is not a quantity"
                    ));
                }
            }
        }
        let sandbox = &self.git_sandbox;
        for (kind, map) in [
            ("requests", &sandbox.resources.requests),
            ("limits", &sandbox.resources.limits),
        ] {
            for (resource, value) in map {
                if !is_quantity(value) {
                    bad(format!(
                        "gitSandbox.resources.{kind}.{resource}: {value:?} is not a quantity"
                    ));
                }
            }
        }
        if !sandbox.resources.limits.contains_key("memory") {
            bad("gitSandbox.resources.limits.memory is required: a program that a repository's configuration makes git run in the sandbox must be stopped by the container's limit, not by the node's, and the sandbox's /tmp counts against it".into());
        }
        // Stricter than the coder chart on one point: a size of zero is refused. A tmpfs given `size=0` has no limit at all.
        if !is_plain_quantity(&sandbox.tmp_size)
            || quantity_nanos(&sandbox.tmp_size).is_none_or(|q| q == 0)
        {
            bad(format!(
                "gitSandbox.tmpSize: {:?} is not a quantity above zero such as 64Mi",
                sandbox.tmp_size
            ));
        }
        if !(1..=MAX_SANDBOX_COMMANDS).contains(&sandbox.max_commands) {
            bad(format!(
                "gitSandbox.maxCommands: {} is not a whole number from 1 to {MAX_SANDBOX_COMMANDS} (GIT_SANDBOX_MAX_COMMANDS)",
                sandbox.max_commands
            ));
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }

    /// The `nodeAffinity` of the template: what the deployment wrote, and, when it admits node types (ANR-660), the requirement
    /// `labelKey In admitted` added to **every** required term (the terms are alternatives, so one left without it would let a pod
    /// go anywhere), or made the one term when there is none. A `null` where a map or a list belongs is absent, as the API server reads
    /// it; a value of another type, which cannot be pinned, is an error naming the path, and `validate` asks the same function so the
    /// operator refuses to start rather than drop the pin.
    fn node_affinity_for_template(&self) -> Result<Option<Value>, String> {
        let written = self.node_affinity_value().cloned();
        if self.node_type.is_empty() {
            return Ok(written);
        }
        /// The member `key` of `parent` as a mutable value: a missing or `null` one becomes `fresh`.
        fn slot<'a>(
            parent: &'a mut Value,
            key: &str,
            path: &str,
            fresh: Value,
        ) -> Result<&'a mut Value, String> {
            let map = parent.as_object_mut().ok_or_else(|| {
                format!("{path} is not a map, so it cannot be pinned to the node types")
            })?;
            let v = map.entry(key.to_owned()).or_insert_with(|| fresh.clone());
            if v.is_null() {
                *v = fresh;
            }
            Ok(v)
        }
        let requirement = json!({
            "key": self.node_type.label_key,
            "operator": "In",
            "values": self.node_type.admitted,
        });
        let mut affinity = written.unwrap_or_else(|| json!({}));
        let at = "nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution";
        let required = slot(
            &mut affinity,
            "requiredDuringSchedulingIgnoredDuringExecution",
            at,
            json!({}),
        )?;
        let terms_at = format!("{at}.nodeSelectorTerms");
        let terms = slot(required, "nodeSelectorTerms", at, json!([]))?
            .as_array_mut()
            .ok_or_else(|| {
                format!("{terms_at} is not a list, so it cannot be pinned to the node types")
            })?;
        if terms.is_empty() {
            terms.push(json!({}));
        }
        for (i, term) in terms.iter_mut().enumerate() {
            if term.is_null() {
                *term = json!({});
            }
            let at = format!("{terms_at}[{i}]");
            let expressions = slot(term, "matchExpressions", &at, json!([]))?
                .as_array_mut()
                .ok_or_else(|| format!("{at}.matchExpressions is not a list, so it cannot be pinned to the node types"))?;
            expressions.push(requirement.clone());
        }
        Ok(Some(affinity))
    }

    /// Whether a required term of the deployment's own `nodeAffinity` already has a `matchExpressions` entry on `key`, with any operator
    /// and any values: the node type's requirement would then be ANDed with something the deployment wrote about the same label.
    fn node_affinity_names_key(&self, key: &str) -> bool {
        self.node_affinity_value()
            .and_then(|a| {
                a.pointer("/requiredDuringSchedulingIgnoredDuringExecution/nodeSelectorTerms")
            })
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|t| t.get("matchExpressions").and_then(Value::as_array))
            .flatten()
            .any(|e| e.get("key").and_then(Value::as_str) == Some(key))
    }

    /// `nodeAffinity` when it says something: absent, `null` and `{}` say nothing.
    fn node_affinity_value(&self) -> Option<&Value> {
        self.node_affinity
            .as_ref()
            .filter(|v| v.as_object().is_none_or(|o| !o.is_empty()) && !v.is_null())
    }
}

/// A quantity as the coder chart's `_validate.tpl` takes the size of the sandbox's `/tmp`: digits, an optional fraction and
/// an optional binary or decimal suffix (`64Mi`, `0.5Gi`); no exponent, no milli.
fn is_plain_quantity(s: &str) -> bool {
    let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (whole, rest) = s.split_at(digits_end);
    if whole.is_empty() {
        return false;
    }
    let rest = match rest.strip_prefix('.') {
        Some(fraction) => {
            let end = fraction
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(fraction.len());
            if end == 0 {
                return false;
            }
            &fraction[end..]
        }
        None => rest,
    };
    matches!(rest, "" | "Ki" | "Mi" | "Gi" | "Ti" | "k" | "M" | "G" | "T")
}

/// A quantity in nano-units like `quantity_nanos`, but **exact**: `None` when any non-zero digit would be dropped (or the unit has an
/// exponent, or it does not fit 128 bits). The number is taken as an integer over a power of ten, so nothing is truncated on the way.
fn exact_nanos(s: &str) -> Option<u128> {
    if !is_quantity(s) {
        return None;
    }
    let end = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (number, suffix) = s.split_at(end);
    let unit = quantity_nanos(&format!("1{suffix}"))?;
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let digits: u128 = format!("{whole}{fraction}").parse().ok()?;
    let divisor = 10u128.checked_pow(u32::try_from(fraction.len()).ok()?)?;
    let total = digits.checked_mul(unit)?;
    (total % divisor == 0).then(|| total / divisor)
}

/// A label value: at most 63 characters of letters, digits, `-`, `_` and `.`, starting and ending with a letter or a digit.
fn is_label_value(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(c))
        && b[0].is_ascii_alphanumeric()
        && b[b.len() - 1].is_ascii_alphanumeric()
}

/// A label key: an optional DNS-subdomain prefix and `/`, then a name that is a label value.
fn is_label_key(s: &str) -> bool {
    match s.split_once('/') {
        Some((prefix, name)) => {
            prefix.len() <= 253 && is_dns_subdomain(prefix) && is_label_value(name)
        }
        None => is_label_value(s),
    }
}

fn is_sha256(s: &str) -> bool {
    s.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// `a.b.c.d/n`, each number in range.
fn is_ipv4_cidr(s: &str) -> bool {
    let Some((addr, bits)) = s.split_once('/') else {
        return false;
    };
    let octets: Vec<&str> = addr.split('.').collect();
    octets.len() == 4
        && octets
            .iter()
            .all(|o| !o.is_empty() && o.len() <= 3 && o.parse::<u8>().is_ok())
        && bits.parse::<u8>().is_ok_and(|b| b <= 32)
}

/// The prefix length of a CIDR (`10.0.0.0/8` is 8, `::/0` is 0), when it has one this file can read. The address
/// itself is the API server's to judge; this is only what the run pods' policy cannot allow.
fn cidr_prefix(cidr: &str) -> Option<u8> {
    cidr.rsplit_once('/')?.1.parse().ok()
}

/// The fields a `NetworkPolicy` egress rule may have, at each level. `k8s-openapi`'s networking types are generated
/// **without** `deny_unknown_fields`, so a misspelled key is dropped and the widening it causes is invisible: `port:`
/// for `ports:` opens every port, `matchLabel:` for `matchLabels:` makes the selector `{}`, which is every namespace
/// (ANR-1356). Every level is therefore walked against its list before the rule is read.
const EGRESS_RULE_FIELDS: [&str; 2] = ["to", "ports"];
const PEER_FIELDS: [&str; 3] = ["ipBlock", "namespaceSelector", "podSelector"];
const IP_BLOCK_FIELDS: [&str; 2] = ["cidr", "except"];
const SELECTOR_FIELDS: [&str; 2] = ["matchLabels", "matchExpressions"];
const REQUIREMENT_FIELDS: [&str; 3] = ["key", "operator", "values"];
const PORT_FIELDS: [&str; 3] = ["protocol", "port", "endPort"];

/// Refuse a key that is not one of `allowed` at `path`. A value that is not an object is left to the deserialiser,
/// which names it already.
fn only_fields(value: &Value, allowed: &[&str], path: &str) -> Result<(), String> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    if let Some(key) = object.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!(
            "{path}.{key} is not a field of it: the fields are {}",
            allowed.join(", ")
        ));
    }
    Ok(())
}

/// The fields of a toleration, of a `nodeAffinity` and of every level below it. `k8s-openapi`'s types are generated
/// without `deny_unknown_fields`, so a misspelled key is dropped and the widening is invisible: a
/// `requiredDuringScheduling` for `requiredDuringSchedulingIgnoredDuringExecution` leaves no required affinity, and a term
/// whose `key` is misspelled is dropped — the run pod is then schedulable on any node (ANR-1357).
const TOLERATION_FIELDS: [&str; 5] = ["key", "operator", "value", "effect", "tolerationSeconds"];
const NODE_AFFINITY_FIELDS: [&str; 2] = [
    "requiredDuringSchedulingIgnoredDuringExecution",
    "preferredDuringSchedulingIgnoredDuringExecution",
];
const NODE_SELECTOR_FIELDS: [&str; 1] = ["nodeSelectorTerms"];
const NODE_TERM_FIELDS: [&str; 2] = ["matchExpressions", "matchFields"];
const NODE_REQUIREMENT_FIELDS: [&str; 3] = ["key", "operator", "values"];
const PREFERRED_TERM_FIELDS: [&str; 2] = ["weight", "preference"];

/// A toleration, against its fields.
fn only_known_toleration(value: &Value, path: &str) -> Result<(), String> {
    only_fields(value, &TOLERATION_FIELDS, path)
}

/// A `nodeAffinity` at every level: the two scheduling blocks, the terms, their expressions and fields, and each
/// preferred term's preference.
fn only_known_node_affinity(value: &Value, path: &str) -> Result<(), String> {
    only_fields(value, &NODE_AFFINITY_FIELDS, path)?;
    if let Some(required) = value.get("requiredDuringSchedulingIgnoredDuringExecution") {
        let at = format!("{path}.requiredDuringSchedulingIgnoredDuringExecution");
        only_fields(required, &NODE_SELECTOR_FIELDS, &at)?;
        for (i, term) in required
            .get("nodeSelectorTerms")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            only_known_node_term(term, &format!("{at}.nodeSelectorTerms[{i}]"))?;
        }
    }
    for (i, preferred) in value
        .get("preferredDuringSchedulingIgnoredDuringExecution")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let at = format!("{path}.preferredDuringSchedulingIgnoredDuringExecution[{i}]");
        only_fields(preferred, &PREFERRED_TERM_FIELDS, &at)?;
        if let Some(term) = preferred.get("preference") {
            only_known_node_term(term, &format!("{at}.preference"))?;
        }
    }
    Ok(())
}

/// One node selector term, and every requirement of its two lists.
fn only_known_node_term(term: &Value, path: &str) -> Result<(), String> {
    only_fields(term, &NODE_TERM_FIELDS, path)?;
    for key in ["matchExpressions", "matchFields"] {
        for (i, requirement) in term
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            only_fields(
                requirement,
                &NODE_REQUIREMENT_FIELDS,
                &format!("{path}.{key}[{i}]"),
            )?;
        }
    }
    Ok(())
}

/// Walk every level of a rule against its allow-list: the rule, each peer, its `ipBlock`, its two selectors and their
/// requirements, and each port.
fn only_known_fields(value: &Value) -> Result<(), String> {
    only_fields(value, &EGRESS_RULE_FIELDS, "the rule")?;
    if let Some(peers) = value.get("to").and_then(Value::as_array) {
        for (i, peer) in peers.iter().enumerate() {
            let path = format!("to[{i}]");
            only_fields(peer, &PEER_FIELDS, &path)?;
            if let Some(block) = peer.get("ipBlock") {
                only_fields(block, &IP_BLOCK_FIELDS, &format!("{path}.ipBlock"))?;
            }
            for key in ["namespaceSelector", "podSelector"] {
                let Some(selector) = peer.get(key) else {
                    continue;
                };
                let path = format!("{path}.{key}");
                only_fields(selector, &SELECTOR_FIELDS, &path)?;
                for (j, e) in selector
                    .get("matchExpressions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    only_fields(
                        e,
                        &REQUIREMENT_FIELDS,
                        &format!("{path}.matchExpressions[{j}]"),
                    )?;
                }
            }
        }
    }
    for (i, port) in value
        .get("ports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        only_fields(port, &PORT_FIELDS, &format!("ports[{i}]"))?;
    }
    Ok(())
}

/// A `NetworkPolicy` egress entry of the settings as a neutral rule. Named ports are refused: the neutral
/// port is a number.
fn egress_rule(value: &Value) -> Result<EgressRule, String> {
    only_known_fields(value)?;
    let rule: NetworkPolicyEgressRule = serde_json::from_value(value.clone())
        .map_err(|e| format!("not a NetworkPolicy egress rule ({e})"))?;
    // Kubernetes reads a rule with no `to` as **every** destination: that is the whole internet, which is what this
    // policy is deny-by-default about (ANR-1353). A rule must name at least one peer.
    if rule.to.as_ref().is_none_or(|to| to.is_empty()) {
        return Err("the rule needs a `to`: with none it allows every destination".to_owned());
    }
    let mut to = Vec::new();
    for p in rule.to.iter().flatten() {
        // A range whose prefix is zero covers every address of its family: `0.0.0.0/0` is the same widening as no
        // `to` at all. Refused here rather than in `convert::peers`, which also reads the agent's own
        // `access.allowFrom`, where a wide range is a deployment's deliberate choice (ANR-1353).
        if let Some(block) = &p.ip_block
            && cidr_prefix(&block.cidr) == Some(0)
        {
            return Err(format!(
                "the ipBlock {:?} covers every address of its family: name the range it may reach",
                block.cidr
            ));
        }
        let (mut peers, problems) = crate::convert::peers(std::slice::from_ref(p));
        if let Some((_, message)) = problems.into_iter().next() {
            return Err(message);
        }
        to.append(&mut peers);
    }
    let mut ports = Vec::new();
    for p in rule.ports.iter().flatten() {
        let protocol = match p.protocol.as_deref().unwrap_or("TCP") {
            "TCP" => Protocol::Tcp,
            "UDP" => Protocol::Udp,
            "SCTP" => Protocol::Sctp,
            other => return Err(format!("protocol {other:?} is not TCP, UDP or SCTP")),
        };
        let port = match &p.port {
            None => None,
            Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(n)) => {
                Some(u16::try_from(*n).map_err(|_| format!("port {n} is not a port number"))?)
            }
            Some(k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(name)) => {
                return Err(format!("the named port {name:?}: a port is a number here"));
            }
        };
        let end_port = p
            .end_port
            .map(|n| u16::try_from(n).map_err(|_| format!("endPort {n} is not a port number")))
            .transpose()?;
        ports.push(EgressPort {
            protocol,
            port,
            end_port,
        });
    }
    Ok(EgressRule { to, ports })
}

// ----------------------------------------------------------------------------- what a service makes

/// What `resolve` needs of one service to make its run pods.
pub(crate) struct ForService<'a> {
    /// The service's name: the account, `RUN_POD_INSTANCE`, the name label of the pods.
    pub service: &'a str,
    /// The service's namespace: where the egress proxy's Service is, as the run pods' variables name it.
    pub namespace: &'a str,
    /// The workload whose pods make run pods (the workers), and so whose pods a run pod is placed beside.
    pub workload: &'a str,
    pub settings: &'a RunPodSettings,
    /// The class chosen, and its name.
    pub class: &'a RunPodClass,
    /// The coder's own image: the init container's.
    pub tools_image: &'a str,
    pub security: Security,
    /// Where the work volume is mounted in the coder and in the run pod.
    pub mount_path: &'a str,
    /// The claim behind it.
    pub claim: String,
    /// The package cache's claim (K1): mounted at `/cache` by every run pod, and the second claim the guard admits.
    pub cache_claim: Option<&'a str>,
    /// The claim is `ReadWriteOnce` and the coder's: the pod must go where the coder is.
    pub beside_the_coder: bool,
}

impl ForService<'_> {
    /// The `RuntimeClass` of this service's run pods (ANR-402): the class's own when it names one, else the deployment's, else none.
    /// The pod template and the guard's parameters read this one function, so a pod and the rule that judges it cannot disagree.
    fn runtime_class_name(&self) -> Option<&str> {
        self.class
            .runtime_class_name()
            .or_else(|| self.settings.runtime_class_name())
    }

    /// The pod every run pod is made from, as `_runpods.tpl` makes it.
    pub(crate) fn template(&self) -> Value {
        let s = self.settings;
        let run_image = s.image.reference();
        let container_security = json!({
            "allowPrivilegeEscalation": false,
            "capabilities": { "drop": ["ALL"] },
        });
        let mut pod_security = serde_json::Map::new();
        pod_security.insert("runAsNonRoot".into(), json!(true));
        pod_security.insert("runAsUser".into(), json!(self.security.run_as_user));
        pod_security.insert("runAsGroup".into(), json!(self.security.run_as_group));
        pod_security.insert("fsGroup".into(), json!(self.security.fs_group));
        if let Some(p) = self.security.fs_group_change_policy {
            pod_security.insert(
                "fsGroupChangePolicy".into(),
                json!(match p {
                    aap_ports::FsGroupChangePolicy::OnRootMismatch => "OnRootMismatch",
                    aap_ports::FsGroupChangePolicy::Always => "Always",
                }),
            );
        }
        pod_security.insert("seccompProfile".into(), json!({ "type": "RuntimeDefault" }));

        let mut env = vec![json!({
            "name": "CARGO_BUILD_JOBS",
            "value": s.cargo_build_jobs.to_string(),
        })];
        // The only way out to the internet (adam-rs ADR 0042): the egress proxy. The pod's network policy allows nothing
        // else, so a tool that ignores these gets no connection.
        let proxy_url = rp::egress::url(self.service, self.namespace);
        for name in &rp::egress::PROXY_ENV[..4] {
            env.push(json!({ "name": name, "value": proxy_url }));
        }
        let no_proxy = s.egress_proxy.no_proxy();
        for name in &rp::egress::PROXY_ENV[4..] {
            env.push(json!({ "name": name, "value": no_proxy }));
        }
        for (name, value) in &s.extra_env {
            env.push(json!({ "name": name, "value": value }));
        }

        let mut affinity = serde_json::Map::new();
        if self.beside_the_coder {
            // The volume is a ReadWriteOnce claim of the coder pod, attached to one node: run where the coder is.
            affinity.insert(
                "podAffinity".into(),
                json!({
                    "requiredDuringSchedulingIgnoredDuringExecution": [{
                        "topologyKey": "kubernetes.io/hostname",
                        "labelSelector": { "matchLabels": {
                            rp::AFFINITY_NAME_LABEL: self.workload,
                            rp::AFFINITY_INSTANCE_LABEL: self.service,
                        } },
                    }],
                }),
            );
        }
        // `validate` has refused an affinity that cannot be pinned. An unvalidated settings object reaching here is a bug, and tests must see it
        // (the debug assertion); in a release build the pin is left out and the admission policy, which requires it, refuses the pod: fail closed.
        match s.node_affinity_for_template() {
            Ok(Some(a)) => {
                affinity.insert("nodeAffinity".into(), a);
            }
            Ok(None) => {}
            Err(why) => debug_assert!(
                false,
                "nodeAffinity cannot be pinned and was not validated: {why}"
            ),
        }

        let mut spec = serde_json::Map::new();
        spec.insert("priorityClassName".into(), json!(s.priority_class));
        // K14a (AD-052): the sandboxed runtime, only when the deployment names one: a run pod without it is what it was.
        if let Some(class) = self.runtime_class_name() {
            spec.insert("runtimeClassName".into(), json!(class));
        }
        // A run pod holds no credential for the cluster.
        spec.insert("automountServiceAccountToken".into(), json!(false));
        spec.insert("enableServiceLinks".into(), json!(false));
        spec.insert(
            "terminationGracePeriodSeconds".into(),
            json!(rp::TERMINATION_GRACE_SECS),
        );
        spec.insert("securityContext".into(), Value::Object(pod_security));
        if !affinity.is_empty() {
            spec.insert("affinity".into(), Value::Object(affinity));
        }
        if !s.node_selector.is_empty() {
            spec.insert("nodeSelector".into(), json!(s.node_selector));
        }
        if !s.tolerations.is_empty() {
            spec.insert("tolerations".into(), json!(s.tolerations));
        }
        spec.insert(
            "initContainers".into(),
            json!([{
                "name": rp::TOOLS_CONTAINER,
                "image": self.tools_image,
                "imagePullPolicy": "IfNotPresent",
                "command": rp::TOOLS_COMMAND,
                "securityContext": container_security,
                "resources": {
                    "requests": { "cpu": rp::TOOLS_REQUEST_CPU, "memory": rp::TOOLS_REQUEST_MEMORY, "ephemeral-storage": rp::TOOLS_REQUEST_EPHEMERAL_STORAGE },
                    "limits": { "cpu": rp::TOOLS_LIMIT_CPU, "memory": rp::TOOLS_LIMIT_MEMORY, "ephemeral-storage": rp::TOOLS_LIMIT_EPHEMERAL_STORAGE },
                },
                "volumeMounts": [{ "name": rp::TOOLS_VOLUME, "mountPath": rp::TOOLS_COPY_DIR }],
            }]),
        );
        // The one mount of the workspace volume, whole: the coder replaces it by the run's own mounts. Then the
        // package cache, shared with the coder pod and the other run pods (K1, adam-rs ADR 0040), and the tools.
        let mut mounts =
            vec![json!({ "name": contract::WORK_VOLUME, "mountPath": self.mount_path })];
        let mut volumes = vec![
            json!({ "name": contract::WORK_VOLUME, "persistentVolumeClaim": { "claimName": &self.claim } }),
        ];
        if let Some(cache) = self.cache_claim {
            mounts
                .push(json!({ "name": contract::CACHE_VOLUME, "mountPath": contract::CACHE_DIR }));
            volumes.push(
                json!({ "name": contract::CACHE_VOLUME, "persistentVolumeClaim": { "claimName": cache } }),
            );
        }
        mounts.push(
            json!({ "name": rp::TOOLS_VOLUME, "mountPath": rp::TOOLS_DIR, "readOnly": true }),
        );
        volumes.push(json!({ "name": rp::TOOLS_VOLUME, "emptyDir": {} }));
        spec.insert(
            "containers".into(),
            json!([{
                "name": s.container,
                "image": run_image,
                "imagePullPolicy": s.image.pull_policy,
                "workingDir": self.mount_path,
                "command": rp::RUN_COMMAND,
                "securityContext": container_security,
                "env": env,
                "resources": { "requests": self.class.requests, "limits": self.class.limits },
                "volumeMounts": mounts,
            }]),
        );
        spec.insert("volumes".into(), Value::Array(volumes));
        let mut metadata = json!({ "labels": {
            "app.kubernetes.io/name": rp::name_label(self.service),
            "app.kubernetes.io/component": "run",
        } });
        // ANR-660 (AD-066): the class's size weight, for whatever reads the pod. A class that is not weighable (the placeholders of
        // `offering`, which `validate` is never asked about) carries none.
        if let Ok(weight) = SizeWeight::of(self.class) {
            metadata["annotations"] = json!({ rp::SIZE_WEIGHT_ANNOTATION: weight.to_string() });
        }
        json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": metadata,
            "spec": Value::Object(spec),
        })
    }

    /// The volumes of the workers' pods that the git sandbox and the account's token are made of (the volume of the pod
    /// template is `resolve`'s): the account's credentials, which only the coder container mounts; the socket's directory,
    /// which the coder container and the sandbox share; and the sandbox's `/tmp`.
    pub(crate) fn volumes(&self) -> Vec<VolumeSpec> {
        vec![
            VolumeSpec {
                name: rp::ACCOUNT_VOLUME.to_owned(),
                source: VolumeSource::AccountToken,
            },
            VolumeSpec {
                name: rp::git_sandbox::SOCKET_VOLUME.to_owned(),
                source: VolumeSource::Scratch(ScratchVolume {
                    in_memory: false,
                    size_limit: None,
                }),
            },
            VolumeSpec {
                name: rp::git_sandbox::TMP_VOLUME.to_owned(),
                source: VolumeSource::Scratch(ScratchVolume {
                    in_memory: true,
                    size_limit: Some(self.settings.git_sandbox.tmp_size.clone()),
                }),
            },
        ]
    }

    /// What the coder container mounts of them: the account's credentials, and the sandbox's socket.
    pub(crate) fn agent_mounts(&self) -> Vec<Mount> {
        vec![
            Mount {
                volume: rp::ACCOUNT_VOLUME.to_owned(),
                path: rp::ACCOUNT_DIR.to_owned(),
                read_only: true,
            },
            Mount {
                volume: rp::git_sandbox::SOCKET_VOLUME.to_owned(),
                path: rp::git_sandbox::SOCKET_DIR.to_owned(),
                read_only: false,
            },
        ]
    }

    /// The variable that tells the coder where the sandbox listens.
    pub(crate) fn agent_env(&self) -> EnvVar {
        EnvVar::literal(rp::git_sandbox::SOCKET_ENV, rp::git_sandbox::SOCKET)
    }

    /// The git sandbox: a native sidecar from the coder's own image (adam-rs `statefulset.yaml`, `git-sandbox`). It mounts
    /// the work volume at the same path as the coder (the commands carry paths), its socket's directory and a `/tmp` of its
    /// own, and **nothing else**: no Secret, no variable from a Secret, no token. It runs as the pod's user, with a
    /// read-only root file system. Its startup and liveness probes run a real command through the relay, because the server
    /// has no health endpoint.
    pub(crate) fn git_sandbox(&self, image: &str) -> Container {
        use rp::git_sandbox as g;
        let s = &self.settings.git_sandbox;
        let probe = |period, timeout, failures| Probe {
            action: ProbeAction::Exec {
                command: g::probe_command(self.mount_path),
            },
            period_secs: period,
            timeout_secs: Some(timeout),
            failure_threshold: Some(failures),
        };
        Container {
            name: g::CONTAINER.to_owned(),
            image: image.to_owned(),
            command: g::COMMAND.map(str::to_owned).to_vec(),
            args: Vec::new(),
            env: vec![
                EnvVar::literal(g::LISTEN, g::SOCKET),
                EnvVar::literal(g::ROOT, self.mount_path),
                EnvVar::literal(g::MAX_COMMANDS, s.max_commands.to_string()),
            ],
            port: None,
            mounts: vec![
                Mount {
                    volume: contract::WORK_VOLUME.to_owned(),
                    path: self.mount_path.to_owned(),
                    read_only: false,
                },
                Mount {
                    volume: g::SOCKET_VOLUME.to_owned(),
                    path: g::SOCKET_DIR.to_owned(),
                    read_only: false,
                },
                Mount {
                    volume: g::TMP_VOLUME.to_owned(),
                    path: g::TMP_DIR.to_owned(),
                    read_only: false,
                },
            ],
            probes: Probes {
                startup: Some(probe(
                    g::STARTUP_PERIOD,
                    g::STARTUP_TIMEOUT,
                    g::STARTUP_FAILURES,
                )),
                liveness: Some(probe(
                    g::LIVENESS_PERIOD,
                    g::LIVENESS_TIMEOUT,
                    g::LIVENESS_FAILURES,
                )),
                readiness: None,
            },
            resources: Resources {
                requests: s.resources.requests.clone(),
                limits: s.resources.limits.clone(),
            },
            read_only_root_filesystem: true,
            activity_port: None,
        }
    }

    /// The images a run pod may use, in the order the chart lists them: the run image, the coder's, the extras,
    /// each once.
    fn allowed_images(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        let mut out = Vec::new();
        for image in [self.settings.image.reference(), self.tools_image.to_owned()]
            .into_iter()
            .chain(self.settings.extra_allowed_images.iter().cloned())
        {
            if seen.insert(image.clone()) {
                out.push(image);
            }
        }
        out
    }

    /// The egress proxy of this service: the agent's own image, `adam-egress` told its settings by variables, and the
    /// allow-list as a file (adam-rs `run-pods-egress-proxy.yaml`). `deny` is what it never connects to.
    fn egress_proxy(&self, deny: &[String]) -> EgressProxy {
        use rp::egress as e;
        let p = &self.settings.egress_proxy;
        let mut env = vec![
            EnvVar::literal(e::LISTEN, format!("0.0.0.0:{}", e::PORT)),
            EnvVar::literal(
                e::ALLOW_FILE,
                format!("{}/{}", e::ALLOW_DIR, e::ALLOW_FILE_NAME),
            ),
            EnvVar::literal(
                e::PORTS,
                p.ports
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ),
        ];
        if !deny.is_empty() {
            env.push(EnvVar::literal(e::DENY_CIDRS, deny.join(",")));
        }
        EgressProxy {
            image: self.tools_image.to_owned(),
            command: e::COMMAND.iter().map(|c| (*c).to_owned()).collect(),
            env,
            port: e::PORT,
            health_path: e::HEALTH_PATH.to_owned(),
            replicas: p.replicas,
            allow: FileSet {
                name: e::allow_name(self.service),
                files: BTreeMap::from([(e::ALLOW_FILE_NAME.to_owned(), p.allow_file())]),
                immutable: false,
            },
            allow_dir: e::ALLOW_DIR.to_owned(),
            egress_ports: p.ports.clone(),
            deny_cidrs: deny.to_vec(),
            run_as: e::USER,
            resources: Resources {
                requests: p.resources.requests.clone(),
                limits: p.resources.limits.clone(),
            },
        }
    }

    /// What the provider makes beside the workloads.
    pub(crate) fn spec(&self) -> RunPods {
        let s = self.settings;
        let net = &s.network_policy;
        let mut except = Vec::new();
        for cidr in net.blocked_cidrs.iter().chain(&net.cluster_cidrs) {
            if !except.contains(cidr) {
                except.push(cidr.clone());
            }
        }
        let network = RunPodNetwork {
            dns: Peer {
                namespaces: Some(Selector {
                    match_labels: BTreeMap::from([(
                        "kubernetes.io/metadata.name".to_owned(),
                        net.dns_namespace.clone(),
                    )]),
                    match_expressions: Vec::new(),
                }),
                pods: Some(Selector {
                    match_labels: net.dns_pod_labels.clone(),
                    match_expressions: Vec::new(),
                }),
                cidr: None,
            },
            proxy: self.egress_proxy(&except),
            // `validate` has refused a rule that is not one; there is nothing to skip.
            extra_egress: net
                .extra_egress
                .iter()
                .filter_map(|r| egress_rule(r).ok())
                .collect(),
        };
        RunPods {
            service_account: self.service.to_owned(),
            role: s.role.clone(),
            priority_class: s.priority_class.clone(),
            runtime_class_name: self.runtime_class_name().map(str::to_owned),
            node_type: (!s.node_type.is_empty()).then(|| RunPodNodeType {
                label_key: s.node_type.label_key.clone(),
                admitted: s.node_type.admitted.clone(),
            }),
            quota: RunPodQuota {
                pods: s.quota.pods,
                limits_memory: s.quota.limits_memory.clone(),
            },
            network,
            pods: RunPodLabels {
                managed_by: rp::MANAGED_BY.to_owned(),
                run_label: rp::RUN_LABEL.to_owned(),
            },
            guard: RunPodGuard {
                images: self.allowed_images(),
                claim: self.claim.clone(),
                cache_claim: self.cache_claim.map(str::to_owned),
            },
        }
    }
}
