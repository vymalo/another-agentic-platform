//! Run pods (slice D15a, §60a): the deployment's settings for them, the Pod adam reads as its template
//! (`RUN_POD_TEMPLATE_FILE`), and what an `AgentConfig` with `coder.runPods` makes of both.
//!
//! The settings are the operator chart's (`runPods`, `runPodClasses`), handed in a file the composition root
//! reads once: this crate is pure and reads no file. The pod template is adam's input like the extra MCP
//! file is, so it is made here, as the chart's `_runpods.tpl` makes it, and the parity goldens
//! (`tests/golden/coder-run-pods*.json`) hold the two equal.

use std::collections::{BTreeMap, BTreeSet};

use aap_api::SecretKeyRef;
use aap_ports::{
    EgressPort, EgressProxy, EgressRule, EnvVar, FileSet, Peer, Protocol, Resources, RunPodGuard,
    RunPodLabels, RunPodNetwork, RunPodQuota, RunPods, Security, Selector,
};
use k8s_openapi::api::core::v1::{NodeAffinity, Toleration};
use k8s_openapi::api::networking::v1::NetworkPolicyEgressRule;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::contract::{self, run_pods as rp};
use crate::syntax::{is_dns_label, is_dns_subdomain, is_env_name, is_quantity, quantity_nanos};

/// What the deployment says about run pods: the operator chart's `runPods` block, its `runPodClasses`, the
/// names of the cluster-scoped objects it made, in the file `RUN_PODS_FILE` (`bin/operator/README.md`). An
/// unknown key is refused. Everything the file does not default is required: the chart writes it all, so the
/// defaults are in one place.
///
/// A `RunPodSettings` is **checked** by [`validate`](Self::validate); the operator refuses to start with one
/// that fails, as it refuses a default image with white space in it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunPodSettings {
    /// The `PriorityClass` of run pods, which the quota is scoped to and the admission policy requires. The
    /// chart made it (or the deployment manages it); the operator makes no cluster-scoped object.
    pub priority_class: String,
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
}

/// One size class: the resources of the run container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunPodClass {
    /// What the container is guaranteed.
    #[serde(default)]
    pub requests: BTreeMap<String, String>,
    /// What it may not exceed. `memory` is required: the quota counts it.
    #[serde(default)]
    pub limits: BTreeMap<String, String>,
}

/// The run container's image.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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

/// The resources of the egress proxy's container.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressResources {
    /// What the container is guaranteed.
    #[serde(default)]
    pub requests: BTreeMap<String, String>,
    /// What it may not exceed.
    #[serde(default)]
    pub limits: BTreeMap<String, String>,
}

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
            role: "stand-in".to_owned(),
            classes: classes
                .into_iter()
                .map(|name| {
                    (
                        name.to_owned(),
                        RunPodClass {
                            requests: BTreeMap::new(),
                            limits: BTreeMap::from([("memory".to_owned(), "1Gi".to_owned())]),
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
            wait_secs: 600,
            node_selector: BTreeMap::new(),
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
                resources: EgressResources {
                    requests: BTreeMap::new(),
                    limits: BTreeMap::new(),
                },
            },
        }
    }

    /// The names of the size classes, in order: what `GET /v1/info` offers.
    #[must_use]
    pub fn class_names(&self) -> Vec<String> {
        self.classes.keys().cloned().collect()
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
            } else if name == contract::env::MODEL_API_KEY || name == "CARGO_BUILD_JOBS" {
                bad(format!(
                    "extraEnv.{name}: the operator sets this variable itself ({})",
                    if name == "CARGO_BUILD_JOBS" {
                        "cargoBuildJobs"
                    } else {
                        "a secret is never a setting: the model key comes from the agent's Secret"
                    }
                ));
            }
            let _ = value;
        }
        for (i, t) in self.tolerations.iter().enumerate() {
            if serde_json::from_value::<Toleration>(t.clone()).is_err() {
                bad(format!("tolerations[{i}]: not a Kubernetes toleration"));
            }
        }
        if let Some(a) = self.node_affinity_value()
            && serde_json::from_value::<NodeAffinity>(a.clone()).is_err()
        {
            bad("nodeAffinity: not a Kubernetes nodeAffinity".into());
        }
        for (i, image) in self.extra_allowed_images.iter().enumerate() {
            if image.trim().is_empty() || image.contains(char::is_whitespace) {
                bad(format!(
                    "extraAllowedImages[{i}]: an image reference with no space"
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
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems)
        }
    }

    /// `nodeAffinity` when it says something: absent, `null` and `{}` say nothing.
    fn node_affinity_value(&self) -> Option<&Value> {
        self.node_affinity
            .as_ref()
            .filter(|v| v.as_object().is_none_or(|o| !o.is_empty()) && !v.is_null())
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

/// A `NetworkPolicy` egress entry of the settings as a neutral rule. Named ports are refused: the neutral
/// port is a number.
fn egress_rule(value: &Value) -> Result<EgressRule, String> {
    let rule: NetworkPolicyEgressRule = serde_json::from_value(value.clone())
        .map_err(|e| format!("not a NetworkPolicy egress rule ({e})"))?;
    let mut to = Vec::new();
    for p in rule.to.iter().flatten() {
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
    /// The claim is `ReadWriteOnce` and the coder's: the pod must go where the coder is.
    pub beside_the_coder: bool,
    pub model_key: &'a SecretKeyRef,
}

impl ForService<'_> {
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
        env.push(json!({
            "name": rp::MODEL_API_KEY,
            "valueFrom": { "secretKeyRef": { "name": self.model_key.name, "key": self.model_key.key } },
        }));

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
        if let Some(a) = s.node_affinity_value() {
            affinity.insert("nodeAffinity".into(), a.clone());
        }

        let mut spec = serde_json::Map::new();
        spec.insert("priorityClassName".into(), json!(s.priority_class));
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
                    "requests": { "cpu": rp::TOOLS_REQUEST_CPU, "memory": rp::TOOLS_REQUEST_MEMORY },
                    "limits": { "memory": rp::TOOLS_LIMIT_MEMORY },
                },
                "volumeMounts": [{ "name": rp::TOOLS_VOLUME, "mountPath": rp::TOOLS_COPY_DIR }],
            }]),
        );
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
                "volumeMounts": [
                    // The one mount of the workspace volume, whole: the coder replaces it by the run's own mounts.
                    { "name": contract::WORK_VOLUME, "mountPath": self.mount_path },
                    { "name": rp::TOOLS_VOLUME, "mountPath": rp::TOOLS_DIR, "readOnly": true },
                ],
            }]),
        );
        spec.insert(
            "volumes".into(),
            json!([
                { "name": contract::WORK_VOLUME, "persistentVolumeClaim": { "claimName": &self.claim } },
                { "name": rp::TOOLS_VOLUME, "emptyDir": {} },
            ]),
        );
        json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": { "labels": {
                "app.kubernetes.io/name": rp::name_label(self.service),
                "app.kubernetes.io/component": "run",
            } },
            "spec": Value::Object(spec),
        })
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
                secret: aap_ports::SecretRef::new(&self.model_key.name, &self.model_key.key),
            },
        }
    }
}
