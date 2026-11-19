//! `operator run`: the composition root. The controllers are generic over a runtime provider and a
//! store provisioner (AD-020); this is the one place that names the types.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

/// Settings of `run`. Every one is also an environment variable, which is how the chart sets them.
#[derive(Clone, Debug, Args)]
pub struct RunArgs {
    /// Watch one namespace (the namespaced operator of §93), and the primary one when `WATCH_NAMESPACES` lists
    /// several. Empty or unset: every namespace, unless `WATCH_NAMESPACES` lists some.
    #[arg(long, env = "WATCH_NAMESPACE")]
    pub watch_namespace: Option<String>,

    /// Watch this list of namespaces, comma separated (K6b): one set of controllers and caches each, and one
    /// namespaced Role in each. **The primary** (`WATCH_NAMESPACE`) must be one of them; `WATCH_NAMESPACE` alone is
    /// the same as a list of one. Unset: `WATCH_NAMESPACE`, else every namespace.
    #[arg(long, env = "WATCH_NAMESPACES")]
    pub watch_namespaces: Option<String>,

    /// Where `/healthz` and `/readyz` are served.
    #[arg(long, env = "HEALTH_ADDR", default_value = "0.0.0.0:8081")]
    pub health_addr: SocketAddr,

    /// Where `/metrics` is served.
    #[arg(long, env = "METRICS_ADDR", default_value = "0.0.0.0:9090")]
    pub metrics_addr: SocketAddr,

    /// The name of this pod, for the `reportingInstance` of the Events.
    #[arg(long, env = "POD_NAME")]
    pub instance: Option<String>,

    /// Services reconciled at the same time.
    #[arg(long, env = "AAP_CONCURRENCY", default_value_t = 4)]
    pub concurrency: u16,

    /// Seconds one pass over one object may take. A pass that is not done by then is dropped and tried
    /// again after the error policy's wait: a request that is never answered would otherwise keep the
    /// object out of the controller for as long as it lives.
    #[arg(long, env = "AAP_RECONCILE_TIMEOUT_SECS", default_value_t = 60)]
    pub reconcile_timeout_secs: u64,

    /// Where the agent registry (`GET /registry/v1/agents`) is served. Only with the `registry` feature
    /// and a token.
    #[arg(long, env = "REGISTRY_ADDR", default_value = "0.0.0.0:8080")]
    pub registry_addr: SocketAddr,

    /// The file that holds the registry's bearer token (a mounted Secret). **No token, no registry**: with
    /// this unset, or the file missing or empty, nothing is served on the registry's port and every service
    /// is `Listed: False`, reason `RegistryDisabled` (fail closed). Read once at start.
    #[arg(long, env = "REGISTRY_TOKEN_FILE")]
    pub registry_token_file: Option<PathBuf>,

    /// The URL of the registry document itself, sent as its `anchor` (the contract asks a server to; a
    /// client ignores it). Unset: no `anchor`.
    #[arg(long, env = "REGISTRY_PUBLIC_URL")]
    pub registry_public_url: Option<String>,

    /// Seconds between looks at a service that is Ready or Suspended (the timer that makes a lost
    /// signal harmless).
    #[arg(long, env = "AAP_RESYNC_SECS", default_value_t = 300)]
    pub resync_secs: u64,

    /// Seconds between looks at a service that is rolling out, unwell, or held by a foreign object.
    #[arg(long, env = "AAP_RESYNC_PENDING_SECS", default_value_t = 15)]
    pub resync_pending_secs: u64,

    /// The image of an agent whose AgentConfig names none (AD-029, §60a): the coder image, a reference
    /// such as `repository:tag@sha256:…`. Unset or empty: the operator has no default, and an
    /// AgentConfig that names no image is `ConfigInvalid`. Changing it rolls every agent that names none
    /// (and no other), through the config digest.
    #[arg(long, env = "DEFAULT_AGENT_IMAGE")]
    pub default_agent_image: Option<String>,

    /// The `RuntimeClass` of every agent's own pod (K14a, AD-052): the name of an object the cluster
    /// ADMINISTRATOR made (the chart makes none). Unset or empty: no class, and every workload is what it
    /// was. Named, it goes into the pod template of every harness, and into the config digest, so naming
    /// it rolls the agents once. A name that is not a DNS subdomain stops the operator at start, not one
    /// agent at a time.
    #[arg(long, env = "AGENT_RUNTIME_CLASS")]
    pub agent_runtime_class: Option<String>,

    /// The file that holds the deployment's settings for run pods (slice D15a): the size classes, the run image,
    /// the quota, the network policy and the names of the cluster-scoped objects the chart made (a YAML file, a
    /// mounted ConfigMap; `aap_domain::RunPodSettings`, an unknown key refused). **Unset or empty: this operator has
    /// no run pods**, and an `AgentConfig` that asks for them is `ConfigInvalid`, saying so. Set and not a file
    /// that parses and passes the checks: the operator does not start. Read once, at start: a change is a new pod.
    #[arg(long, env = "RUN_PODS_FILE")]
    pub run_pods_file: Option<String>,

    /// The claims the deployment allows as an agent's package cache (`environment.cache.existingClaim`, K1), comma
    /// separated. **Empty (the default): no agent may have a cache**: the claim is any claim of the namespace to the
    /// pod that mounts it, so a deployment lists the ones made for the purpose.
    #[arg(long, env = "CACHE_ALLOWED_CLAIMS", value_delimiter = ',')]
    pub cache_allowed_claims: Vec<String>,

    /// The highest access tier an `AgentConfig` may ask for (`coder.maxAccessTier`, K1): `plan`, `edits`, `auto` or
    /// `bypass`. `auto`, the coder's own default, when unset. A config above it is `ConfigInvalid`; one that names none
    /// gets the ceiling when it is lower than `auto`.
    #[arg(long, env = "AGENT_MAX_ACCESS_TIER", default_value = "auto", value_parser = parse_tier)]
    pub max_access_tier: aap_api::AccessTierCap,

    /// Probe the MCP servers of `ToolProvider`s (K3b, AD-053): `initialize` and `tools/list`, and the reachability and
    /// tool names in the provider's status. **Off by default**: no request is made and the status has no `Reachable`
    /// condition. The chart sets it when `toolProbe.to` lists the peers the operator may reach.
    #[arg(long, env = "TOOL_PROBE_ENABLED")]
    pub tool_probe_enabled: bool,

    /// How long one probe may take, in seconds (1 to 60). A server that does not answer by then is `Reachable: False`,
    /// reason `Timeout`.
    #[arg(
        long,
        env = "TOOL_PROBE_TIMEOUT_SECS",
        default_value_t = 5,
        value_parser = clap::value_parser!(u64).range(1..=60)
    )]
    pub tool_probe_timeout_secs: u64,

    /// Elect one replica to write (K7a), with a `coordination.k8s.io/v1` Lease: every replica keeps its caches,
    /// serves `/healthz`, `/readyz`, `/metrics` and the registry, and **only the lease holder's passes write**.
    /// Off (the default): this replica is the only one, and every pass writes.
    #[arg(long, env = "LEADER_ELECTION_ENABLED")]
    pub leader_election_enabled: bool,

    /// The name of the Lease: the election is `<namespace>/<name>`.
    #[arg(long, env = "LEADER_ELECTION_NAME", default_value = "aap-operator")]
    pub leader_election_name: String,

    /// The namespace of the Lease. Unset: the primary namespace, the first of `WATCH_NAMESPACES` (or
    /// `WATCH_NAMESPACE`), which is where the operator's `Role` is. **With the election on and no namespace at
    /// all, the operator refuses to start**: a Lease lives in one.
    #[arg(long, env = "LEADER_ELECTION_NAMESPACE")]
    pub leader_election_namespace: Option<String>,

    /// How long the lease lives, in seconds: a holder that has not renewed for this long may be replaced.
    #[arg(long, env = "LEADER_ELECTION_LEASE_SECS", default_value_t = 15)]
    pub leader_election_lease_secs: u64,

    /// How often the holder renews the lease, in seconds.
    #[arg(long, env = "LEADER_ELECTION_RENEW_SECS", default_value_t = 10)]
    pub leader_election_renew_secs: u64,

    /// How often a replica that is not the holder tries, in seconds.
    #[arg(long, env = "LEADER_ELECTION_RETRY_SECS", default_value_t = 2)]
    pub leader_election_retry_secs: u64,
}

fn parse_tier(s: &str) -> Result<aap_api::AccessTierCap, String> {
    match s.trim() {
        "plan" => Ok(aap_api::AccessTierCap::Plan),
        "edits" => Ok(aap_api::AccessTierCap::Edits),
        "" | "auto" => Ok(aap_api::AccessTierCap::Auto),
        "bypass" => Ok(aap_api::AccessTierCap::Bypass),
        other => Err(format!(
            "{other:?} is not an access tier: plan, edits, auto or bypass"
        )),
    }
}

/// Whether `name` is a namespace name: a DNS label, `^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$`, which is the rule the
/// chart's `_validate.tpl` holds its values to.
#[cfg(feature = "runtime-kubernetes")]
fn is_dns_label(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Refuse a name that is no namespace, naming the variable that gave it.
#[cfg(feature = "runtime-kubernetes")]
fn check_label(variable: &str, name: &str) -> Result<()> {
    if is_dns_label(name) {
        return Ok(());
    }
    anyhow::bail!(
        "{variable} {name:?} is not a namespace: a namespace is a DNS label (lowercase letters, digits and '-', starting and ending with a letter or a digit, at most 63 characters)"
    );
}

/// The election this replica runs in (K7a), as `run` composed it.
#[cfg(feature = "runtime-kubernetes")]
#[derive(Clone, Debug)]
pub struct LeaderSettings {
    /// The namespace of the Lease.
    pub namespace: String,
    /// The name of the Lease.
    pub name: String,
    /// What this replica writes as the lease's `holderIdentity`.
    pub candidate: aap_ports::CandidateId,
    /// The lease, the renewal and the retry.
    pub timings: aap_leader_kubernetes::Timings,
}

/// The identity this replica writes as the lease's holder (K7a): `POD_NAME`, else `HOSTNAME`, else a refusal. Two
/// replicas with the same identity would renew each other's lease, which is no election at all.
#[cfg(feature = "runtime-kubernetes")]
fn candidate_id(instance: Option<&str>, hostname: Option<&str>) -> Result<aap_ports::CandidateId> {
    let name = instance
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| hostname.map(str::trim).filter(|name| !name.is_empty()));
    match name {
        Some(name) => Ok(aap_ports::CandidateId::new(name)),
        None => anyhow::bail!(
            "LEADER_ELECTION_ENABLED is set, and neither POD_NAME nor HOSTNAME names this replica: the Lease's holderIdentity would be empty, and two replicas would look like one"
        ),
    }
}

/// `HOSTNAME`, if the environment has one.
#[cfg(feature = "runtime-kubernetes")]
fn hostname() -> Option<String> {
    std::env::var("HOSTNAME").ok()
}

// What `run` composes needs a runtime provider; without one the settings are parsed and unused.
#[cfg(feature = "runtime-kubernetes")]
impl RunArgs {
    /// The default image, if there is one: an empty value (a chart value left blank) is none, and a
    /// reference with white space in it is a deployment mistake that stops the operator at start, not a
    /// `ConfigInvalid` on every agent that would have used it.
    ///
    /// # Errors
    ///
    /// The value has white space in it.
    pub fn default_image(&self) -> Result<Option<String>> {
        let Some(image) = self
            .default_agent_image
            .as_deref()
            .map(str::trim)
            .filter(|i| !i.is_empty())
        else {
            return Ok(None);
        };
        if image.contains(char::is_whitespace) {
            anyhow::bail!(
                "DEFAULT_AGENT_IMAGE (--default-agent-image) {image:?} has white space in it: an image reference such as repository:tag@sha256:… is expected"
            );
        }
        Ok(Some(image.to_owned()))
    }

    /// The `RuntimeClass` of the agents' pods, if there is one: an unset or empty value is none (a chart value left
    /// blank), and a name that is not a DNS subdomain is a deployment mistake that stops the operator at start, not
    /// one agent at a time.
    ///
    /// # Errors
    ///
    /// The name is not a DNS subdomain.
    pub fn agent_runtime_class(&self) -> Result<Option<String>> {
        let Some(class) = self
            .agent_runtime_class
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
        else {
            return Ok(None);
        };
        if !aap_domain::is_dns_subdomain(class) {
            anyhow::bail!(
                "AGENT_RUNTIME_CLASS (--agent-runtime-class) {class:?} is not the name of a RuntimeClass (a DNS subdomain): the RuntimeClass object is the administrator's"
            );
        }
        Ok(Some(class.to_owned()))
    }

    /// The settings for run pods, if there are any: an unset or empty `RUN_PODS_FILE` is none, and a file that cannot be
    /// read, is not YAML of the settings or fails their checks stops the operator at start, naming every mistake: a
    /// pod that ran with half the settings would make run pods the guard does not match.
    ///
    /// # Errors
    ///
    /// The file cannot be read or parsed, or a setting is refused.
    pub fn run_pod_settings(&self) -> Result<Option<aap_domain::RunPodSettings>> {
        use anyhow::Context;
        let Some(path) = self
            .run_pods_file
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
        else {
            return Ok(None);
        };
        let path = path.as_path();
        let text = std::fs::read_to_string(path).with_context(|| {
            format!(
                "RUN_PODS_FILE (--run-pods-file) {}: cannot be read",
                path.display()
            )
        })?;
        let settings: aap_domain::RunPodSettings =
            serde_yaml::from_str(&text).with_context(|| {
                format!(
                    "RUN_PODS_FILE (--run-pods-file) {}: not the settings of run pods",
                    path.display()
                )
            })?;
        if let Err(problems) = settings.validate() {
            anyhow::bail!(
                "RUN_PODS_FILE (--run-pods-file) {}:\n  {}",
                path.display(),
                problems.join("\n  ")
            );
        }
        Ok(Some(settings))
    }

    /// The namespaces to watch (K6b), from `WATCH_NAMESPACES` (a comma-separated list) and `WATCH_NAMESPACE` (the
    /// one-namespace shorthand and the primary):
    ///
    /// * neither, or both empty: every namespace (`None`);
    /// * `WATCH_NAMESPACE` alone: that one;
    /// * `WATCH_NAMESPACES` alone: exactly those, in the order given;
    /// * both: the list, and `WATCH_NAMESPACE` must be one of it — it is the primary, which names the cluster-scoped
    ///   objects of run pods and the API's `defaults.namespace`, so a primary nobody watches is a deployment mistake.
    ///
    /// A name is a DNS label (the same rule as the chart's `_validate.tpl`), and a list holds each name once: an
    /// operator that could not watch one of them would be as good as not running, so this refuses rather than
    /// starting with part of the fleet.
    ///
    /// # Errors
    ///
    /// A name that is not a DNS label, a name given twice, a list that is set and holds no name, or a primary that
    /// the list does not hold.
    pub fn namespaces(&self) -> Result<Option<Vec<String>>> {
        let primary = self
            .watch_namespace
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty());
        let Some(text) = self.watch_namespaces.as_deref() else {
            return match primary {
                None => Ok(None),
                Some(name) => {
                    check_label("WATCH_NAMESPACE (--watch-namespace)", name)?;
                    Ok(Some(vec![name.to_owned()]))
                }
            };
        };
        let mut names: Vec<String> = Vec::new();
        for name in text.split(',').map(str::trim) {
            check_label("WATCH_NAMESPACES (--watch-namespaces)", name)?;
            if names.iter().any(|n| n == name) {
                anyhow::bail!(
                    "WATCH_NAMESPACES (--watch-namespaces) {text:?} gives {name:?} twice: a namespace is watched once, by one set of controllers"
                );
            }
            names.push(name.to_owned());
        }
        if let Some(name) = primary
            && !names.iter().any(|n| n == name)
        {
            anyhow::bail!(
                "WATCH_NAMESPACE (--watch-namespace) {name:?} is not one of WATCH_NAMESPACES (--watch-namespaces) {text:?}: it is the primary, so it must be watched too"
            );
        }
        Ok(Some(names))
    }

    /// The election this replica is a candidate of (K7a), from the six `LEADER_ELECTION_*` settings:
    ///
    /// * off (the default): `None`, and the operator is an [`AlwaysLeader`](aap_ports::AlwaysLeader);
    /// * on: the namespace of the Lease is `LEADER_ELECTION_NAMESPACE`, else the **primary** namespace (the first
    ///   of `WATCH_NAMESPACES`, or `WATCH_NAMESPACE`) — the one the operator's `Role` is in — and a deployment
    ///   that names neither is refused, because a Lease lives in one;
    /// * the holder identity is `POD_NAME`, else `HOSTNAME` (see [`candidate_id`]);
    /// * the timings are the three `_SECS` values, refused when they do not satisfy `retry < renew < lease`, each
    ///   at least a second.
    ///
    /// # Errors
    ///
    /// One of those settings is missing or refused, naming the variable that gave it.
    pub fn leader_election(&self, namespaces: Option<&[String]>) -> Result<Option<LeaderSettings>> {
        if !self.leader_election_enabled {
            return Ok(None);
        }
        let namespace = self
            .leader_election_namespace
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .or_else(|| namespaces.and_then(|list| list.first().cloned()))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "LEADER_ELECTION_ENABLED is set and LEADER_ELECTION_NAMESPACE (--leader-election-namespace) names no namespace, and WATCH_NAMESPACE/WATCH_NAMESPACES names none either: the Lease needs the namespace the operator's Role is in"
                )
            })?;
        check_label(
            "LEADER_ELECTION_NAMESPACE (--leader-election-namespace)",
            &namespace,
        )?;
        let name = self.leader_election_name.trim();
        if name.is_empty() {
            anyhow::bail!(
                "LEADER_ELECTION_NAME (--leader-election-name) is empty: a Lease has a name"
            );
        }
        check_label("LEADER_ELECTION_NAME (--leader-election-name)", name)?;
        let timings = aap_leader_kubernetes::Timings {
            lease: std::time::Duration::from_secs(self.leader_election_lease_secs),
            renew: std::time::Duration::from_secs(self.leader_election_renew_secs),
            retry: std::time::Duration::from_secs(self.leader_election_retry_secs),
        };
        timings.check().map_err(|why| {
            anyhow::anyhow!(
                "LEADER_ELECTION_LEASE_SECS, LEADER_ELECTION_RENEW_SECS and LEADER_ELECTION_RETRY_SECS (--leader-election-lease-secs, --leader-election-renew-secs, --leader-election-retry-secs): {why}"
            )
        })?;
        let candidate = candidate_id(self.instance.as_deref(), hostname().as_deref())?;
        Ok(Some(LeaderSettings {
            namespace,
            name: name.to_owned(),
            candidate,
            timings,
        }))
    }

    fn options(
        &self,
        watch_namespaces: Option<Vec<String>>,
        registry: bool,
        registry_full: std::sync::Arc<std::sync::atomic::AtomicBool>,
        default_agent_image: Option<String>,
        agent_runtime_class: Option<String>,
        run_pods: Option<aap_domain::RunPodSettings>,
    ) -> aap_controller::Options {
        aap_controller::Options {
            watch_namespaces,
            registry: if registry {
                aap_controller::RegistryMode::Enabled
            } else {
                aap_controller::RegistryMode::Disabled
            },
            registry_full,
            resync: aap_controller::Resync {
                settled: std::time::Duration::from_secs(self.resync_secs),
                pending: std::time::Duration::from_secs(self.resync_pending_secs),
                ..aap_controller::Resync::default()
            },
            concurrency: self.concurrency.max(1),
            pass_timeout: std::time::Duration::from_secs(self.reconcile_timeout_secs.max(1)),
            instance: self.instance.clone().filter(|i| !i.is_empty()),
            default_agent_image,
            agent_runtime_class,
            run_pods,
            cache_claims: self
                .cache_allowed_claims
                .iter()
                .map(|c| c.trim().to_owned())
                .filter(|c| !c.is_empty())
                .collect(),
            max_access_tier: self.max_access_tier,
            probe: aap_controller::Probe {
                enabled: self.tool_probe_enabled,
                timeout: std::time::Duration::from_secs(self.tool_probe_timeout_secs),
            },
        }
    }
}

/// The store provisioner of this build: with `store-cnpg`, the one that makes a CloudNativePG cluster and
/// also serves a referenced Secret (it passes the whole suite of `aap-ports`, so the controller sees one
/// type); without it, referenced Secrets alone, and a cluster is refused as not installed.
#[cfg(all(feature = "runtime-kubernetes", feature = "store-cnpg"))]
fn store(client: &kube::Client) -> aap_store_cnpg::CnpgStore {
    aap_store_cnpg::CnpgStore::new(client.clone())
}

#[cfg(all(feature = "runtime-kubernetes", not(feature = "store-cnpg")))]
fn store(_client: &kube::Client) -> aap_store_secret::SecretStore {
    aap_store_secret::SecretStore::new()
}

/// The registry's token, or why there is none. **No token, no registry** (fail closed): the operator
/// keeps reconciling, says so loudly, and every service is `Listed: False`, reason `RegistryDisabled`.
#[cfg(all(feature = "runtime-kubernetes", feature = "registry"))]
fn registry_token(args: &RunArgs) -> Option<aap_registry::Token> {
    let Some(path) = &args.registry_token_file else {
        tracing::warn!(
            "no REGISTRY_TOKEN_FILE: the agent registry is not served (it has no token to demand)"
        );
        return None;
    };
    match aap_registry::Token::from_file(path) {
        Ok(token) => Some(token),
        Err(e) => {
            tracing::error!(
                "{}: {e}: the agent registry is not served (it has no token to demand)",
                path.display()
            );
            None
        }
    }
}

#[cfg(all(feature = "runtime-kubernetes", not(feature = "registry")))]
fn registry_token(args: &RunArgs) -> Option<()> {
    if args.registry_token_file.is_some() {
        tracing::warn!(
            "REGISTRY_TOKEN_FILE is set, but this build has no `registry` feature: nothing is served on the registry's port"
        );
    }
    None
}

/// How often the registry's document is built when nobody asks, so that `RegistryFull` is true to the
/// directory and not to the last request.
#[cfg(all(feature = "runtime-kubernetes", feature = "registry"))]
const REGISTRY_REFRESH: std::time::Duration = std::time::Duration::from_secs(15);

/// Resolves on SIGTERM or SIGINT.
#[cfg(feature = "runtime-kubernetes")]
async fn termination() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = term.recv() => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
                return;
            }
            Err(e) => tracing::warn!("no SIGTERM handler: {e}"),
        }
    }
    let _ = tokio::signal::ctrl_c().await;
}

/// The Kubernetes provider the controllers reconcile through. It looks for run pod objects to clear **only with settings
/// for run pods**: that is when the chart gives the operator the rights to read them, and looking without them would fail
/// every reconcile and every delete with `Forbidden` (slice D15a).
#[cfg(feature = "runtime-kubernetes")]
fn kubernetes_runtime(
    client: kube::Client,
    options: &aap_controller::Options,
) -> aap_runtime_kubernetes::KubernetesRuntime {
    let runtime = aap_runtime_kubernetes::KubernetesRuntime::new(client)
        .clearing_run_pods(options.run_pods.is_some());
    match &options.watch_namespaces {
        Some(namespaces) if !namespaces.is_empty() => {
            runtime.watching_namespaces(namespaces.clone())
        }
        // An empty list is no namespace at all: the operator asks the provider for nothing (see `Operator::run`), so
        // the provider is left as it is and its watch is never called.
        _ => runtime,
    }
}

/// How long the spans still queued have to reach the collector when the operator stops (K5), inside the 30 s of
/// `terminationGracePeriodSeconds`.
#[cfg(feature = "runtime-kubernetes")]
const FLUSH: std::time::Duration = std::time::Duration::from_secs(3);

/// `operator run`. The tracing variables are read first, before the cluster is asked anything. A setting that is refused never
/// stops the operator (tracing is not a security control, and platforms inject OTEL_* variables of their own): one warning, by
/// variable, and tracing is off. The spans still queued are flushed on every way out that returns, an error included (not when
/// the main future panics).
#[cfg(feature = "runtime-kubernetes")]
pub async fn run(args: RunArgs) -> Result<()> {
    let started = aap_trace_otlp::start("aap-operator", env!("CARGO_PKG_VERSION"));
    started.warn();
    let tracer = started.tracer;
    flushed(&tracer, run_with(args, tracer.clone())).await
}

/// Run `body`, then flush the spans still queued: on `Ok` and on `Err` alike.
#[cfg(feature = "runtime-kubernetes")]
async fn flushed<T>(
    tracer: &aap_trace::Tracer,
    body: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    let result = body.await;
    tracer.shutdown(FLUSH).await;
    result
}

/// The controllers, composed: the one place that hands the tracer and the election to the operator, so a test can
/// ask that it did.
#[cfg(feature = "runtime-kubernetes")]
#[allow(clippy::too_many_arguments)]
fn compose<R, S>(
    client: kube::Client,
    runtime: R,
    store: S,
    owner: aap_controller::OwnerOf,
    options: aap_controller::Options,
    tracer: &aap_trace::Tracer,
    leader: std::sync::Arc<dyn aap_ports::LeaderElection>,
) -> aap_controller::Operator<R, S>
where
    R: aap_ports::RuntimeProvider + Clone + 'static,
    S: aap_ports::StoreProvisioner + Clone + 'static,
{
    aap_controller::Operator::new(client, runtime, store, owner, options)
        .with_tracer(tracer.clone())
        .with_leader(leader)
}

#[cfg(feature = "runtime-kubernetes")]
async fn run_with(args: RunArgs, tracer: aap_trace::Tracer) -> Result<()> {
    use std::sync::Arc;

    use aap_api::AgentService;
    use aap_controller::OwnerOf;
    use aap_runtime_kubernetes::owner_handle;
    use anyhow::Context;
    use kube::{Resource, ResourceExt};
    use tokio::sync::watch;

    let default_image = args.default_image()?;
    let agent_runtime_class = args.agent_runtime_class()?;
    let run_pods = args.run_pod_settings()?;
    let watch_namespaces = args.namespaces()?;
    let leader = args.leader_election(watch_namespaces.as_deref())?;
    let registry_full = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let token = registry_token(&args);
    let options = args.options(
        watch_namespaces,
        token.is_some(),
        registry_full.clone(),
        default_image,
        agent_runtime_class,
        run_pods,
    );
    if options.watch_namespaces.is_none() {
        tracing::warn!(
            "WATCH_NAMESPACE and WATCH_NAMESPACES name no namespace: the operator watches every namespace, which needs a ClusterRole this chart does not make"
        );
    }
    tracing::info!(
        namespaces = options
            .watch_namespaces
            .as_ref()
            .map_or_else(|| "(all)".to_owned(), |n| n.join(",")),
        default_agent_image = options.default_agent_image.as_deref().unwrap_or("(none)"),
        agent_runtime_class = options.agent_runtime_class.as_deref().unwrap_or("(none)"),
        run_pod_size_classes = options
            .run_pods
            .as_ref()
            .map_or_else(|| "(no run pods)".to_owned(), |s| s.class_names().join(",")),
        tool_probe = options.probe.enabled,
        "starting the operator {}",
        env!("CARGO_PKG_VERSION")
    );

    // Everything below is the composition: a Kubernetes runtime, the store, the controllers.
    let client = kube::Client::try_default()
        .await
        .context("connecting to the cluster (in-cluster configuration, or KUBECONFIG)")?;
    let runtime = kubernetes_runtime(client.clone(), &options);
    // The encoding of an owner is the runtime provider's, so this is where it is made (the
    // controller never looks inside a handle).
    let owner: OwnerOf = Arc::new(|svc: &AgentService| {
        owner_handle(
            AgentService::api_version(&()),
            AgentService::kind(&()),
            svc.name_any(),
            svc.metadata.uid.clone().unwrap_or_default(),
        )
    });
    let store = Arc::new(store(&client));
    // The election (K7a): the one replica that holds the lease is the one whose passes write. The candidate id is
    // POD_NAME, else HOSTNAME; without either, the election refuses to start (a holderIdentity of "" would make two
    // replicas look like one), while a lone operator with no election needs no name at all.
    let election: std::sync::Arc<dyn aap_ports::LeaderElection> = match &leader {
        Some(settings) => {
            let lease = aap_leader_kubernetes::LeaseElection::new(
                client.clone(),
                &settings.namespace,
                &settings.name,
                settings.candidate.clone(),
            )
            .with_timings(settings.timings)
            .map_err(|why| anyhow::anyhow!("the leader election's timings: {why}"))?;
            tracing::info!(
                lease = %format!("{}/{}", settings.namespace, settings.name),
                candidate = settings.candidate.as_str(),
                lease_secs = settings.timings.lease.as_secs(),
                renew_secs = settings.timings.renew.as_secs(),
                retry_secs = settings.timings.retry.as_secs(),
                "leader election on: every replica serves, and only the lease holder's passes write"
            );
            Arc::new(lease)
        }
        None => {
            tracing::info!(
                "no leader election: this replica is the only one, and every pass writes"
            );
            let candidate = candidate_id(args.instance.as_deref(), hostname().as_deref())
                .unwrap_or_else(|_| aap_ports::CandidateId::new("operator"));
            Arc::new(aap_ports::AlwaysLeader::new(candidate))
        }
    };
    let operator = compose(
        client,
        runtime,
        store,
        owner,
        options,
        &tracer,
        election.clone(),
    );

    #[cfg(feature = "registry")]
    let registry_server = token.map(|token| {
        let mut registry = aap_registry::Registry::new(operator.directory(), token)
            .with_full_flag(registry_full.clone());
        if let Some(anchor) = args.registry_public_url.clone().filter(|u| !u.is_empty()) {
            registry = registry.with_anchor(anchor);
        }
        Arc::new(registry)
    });

    let (stop, stopped) = watch::channel(false);
    let signalled = |mut rx: watch::Receiver<bool>| async move {
        // A dropped sender is a stop as well.
        let _ = rx.wait_for(|stop| *stop).await;
    };
    let health = tokio::spawn(crate::serve::serve(
        args.health_addr,
        crate::serve::health(operator.readiness()),
        signalled(stopped.clone()),
    ));
    let metrics = tokio::spawn(crate::serve::serve(
        args.metrics_addr,
        crate::serve::metrics_for(operator.metrics(), &tracer),
        signalled(stopped.clone()),
    ));

    // The registry: served on its port, and its document built on a timer besides, so that the flag the
    // controller reads (`RegistryFull`) follows the directory when nobody asks.
    #[cfg(feature = "registry")]
    let registry = tokio::spawn({
        let registry = registry_server.clone();
        let addr = args.registry_addr;
        let stopped = stopped.clone();
        async move {
            let Some(registry) = registry else {
                // Not served: the select below must not end on this arm.
                return std::future::pending::<Result<()>>().await;
            };
            let refresh = {
                let registry = registry.clone();
                tokio::spawn(async move {
                    loop {
                        // The result is the flag's and the log's: nobody is waiting for it.
                        let _ = registry.document().await;
                        tokio::time::sleep(REGISTRY_REFRESH).await;
                    }
                })
            };
            let served = crate::serve::serve(addr, registry.router(), signalled(stopped)).await;
            refresh.abort();
            served
        }
    });
    #[cfg(not(feature = "registry"))]
    let registry = tokio::spawn(std::future::pending::<Result<()>>());

    // The signal, once: the log, the flag, and the stop that every task listens to. It is a task of its own so
    // that the campaign below can share the same signal.
    let asked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _signal = tokio::spawn({
        let asked = asked.clone();
        let stop = stop.clone();
        async move {
            termination().await;
            asked.store(true, std::sync::atomic::Ordering::Release);
            tracing::info!("shutting down");
            let _ = stop.send(true);
        }
    });

    let controllers = operator.run(signalled(stopped.clone()));
    tokio::pin!(controllers);
    // The campaign (K7a) is a task of the same shutdown: on SIGTERM the holder releases the lease, so the next
    // replica does not wait for it to expire.
    let campaign = election.run(Box::pin(signalled(stopped.clone())));
    tokio::pin!(campaign);
    let mut campaign_done = false;
    let mut controllers_done = false;

    // The servers fail fast (a port in use is a deployment mistake); the controllers and the campaign end when
    // told to.
    let result = tokio::select! {
        () = &mut controllers => {
            controllers_done = true;
            if asked.load(std::sync::atomic::Ordering::Acquire) {
                Ok(())
            } else {
                Err(anyhow::anyhow!("the controllers stopped without being asked to"))
            }
        }
        () = &mut campaign => {
            campaign_done = true;
            if asked.load(std::sync::atomic::Ordering::Acquire) {
                Ok(())
            } else {
                Err(anyhow::anyhow!("the leader election stopped without being asked to"))
            }
        }
        r = health => r.context("the health server task").and_then(|r| r).and(Err(anyhow::anyhow!("the health server stopped"))),
        r = metrics => r.context("the metrics server task").and_then(|r| r).and(Err(anyhow::anyhow!("the metrics server stopped"))),
        r = registry => r.context("the registry task").and_then(|r| r).and(Err(anyhow::anyhow!("the registry stopped"))),
    };
    // Stop whatever still serves, and give the campaign its bounded moment to release the lease, then the controllers
    // their grace to let the passes in flight finish. The campaign arm wins on a shutdown (the election's run returns
    // with the stop), so without this drain every SIGTERM would drop the controllers mid-pass.
    let _ = stop.send(true);
    if asked.load(std::sync::atomic::Ordering::Acquire) {
        drain(
            campaign_done,
            &mut campaign,
            controllers_done,
            &mut controllers,
        )
        .await;
    }
    result
}

/// Give whatever the winning arm of the shutdown left running its bounded moment: the campaign releases the lease
/// ([`LEASE_RELEASE_WAIT`]; the release itself is bounded to two seconds), then the controllers finish their passes
/// ([`CONTROLLER_DRAIN_WAIT`]).
///
/// The campaign arm wins a shutdown — [`AlwaysLeader`](aap_ports::AlwaysLeader)'s `run` **is** the shutdown — so
/// without this drain every SIGTERM would drop the controllers mid-pass. A future that does not end within its
/// bound is dropped, which is what the pod's own grace would do anyway.
#[cfg(feature = "runtime-kubernetes")]
async fn drain(
    campaign_done: bool,
    campaign: &mut (impl std::future::Future<Output = ()> + Unpin),
    controllers_done: bool,
    controllers: &mut (impl std::future::Future<Output = ()> + Unpin),
) {
    if !campaign_done {
        let _ = tokio::time::timeout(LEASE_RELEASE_WAIT, &mut *campaign).await;
    }
    if !controllers_done {
        let _ = tokio::time::timeout(CONTROLLER_DRAIN_WAIT, &mut *controllers).await;
    }
}

/// How long the campaign may take to release the lease once the operator has stopped, inside the 30 s of
/// `terminationGracePeriodSeconds`: the release itself is bounded to two seconds.
#[cfg(feature = "runtime-kubernetes")]
const LEASE_RELEASE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// How long the controllers get to finish their passes once the stop is sent: the shards' own grace is five seconds.
#[cfg(feature = "runtime-kubernetes")]
const CONTROLLER_DRAIN_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(not(feature = "runtime-kubernetes"))]
pub async fn run(args: RunArgs) -> Result<()> {
    let _ = args;
    anyhow::bail!(
        "this build has no runtime provider: rebuild with `--features runtime-kubernetes` (the default)"
    )
}

#[cfg(all(test, feature = "runtime-kubernetes"))]
#[allow(clippy::expect_used)] // a test may
mod tests {
    use super::*;

    /// A client that is never used: building one makes no request.
    fn client() -> kube::Client {
        let config = kube::Config::new("http://127.0.0.1:9".parse().expect("a URL"));
        kube::Client::try_from(config).expect("a client")
    }

    /// What a pass needs of the operator is the tracer it was composed with: an operator composed with none makes no span.
    #[tokio::test]
    async fn the_operator_is_composed_with_the_tracer() {
        let compose_with = |tracer: &aap_trace::Tracer| {
            compose(
                client(),
                aap_ports::memory::MemoryRuntime::new(),
                aap_ports::memory::MemoryStore::new(),
                std::sync::Arc::new(|_| aap_ports::OwnerHandle::none()),
                aap_controller::Options::default(),
                tracer,
                std::sync::Arc::new(aap_ports::AlwaysLeader::default()),
            )
        };
        let (on, _) = aap_trace::Tracer::in_memory(aap_trace::Sampler::AlwaysOn);
        assert!(compose_with(&on).tracer().enabled());
        assert!(!compose_with(&aap_trace::Tracer::off()).tracer().enabled());
    }

    /// K7a: the election the composition root builds reaches the operator, and an operator with none is a leader.
    #[tokio::test]
    async fn the_operator_is_composed_with_the_election_it_was_given() {
        let compose_with = |leader: std::sync::Arc<dyn aap_ports::LeaderElection>| {
            compose(
                client(),
                aap_ports::memory::MemoryRuntime::new(),
                aap_ports::memory::MemoryStore::new(),
                std::sync::Arc::new(|_| aap_ports::OwnerHandle::none()),
                aap_controller::Options::default(),
                &aap_trace::Tracer::off(),
                leader,
            )
        };
        let always = compose_with(std::sync::Arc::new(aap_ports::AlwaysLeader::new(
            aap_ports::CandidateId::new("pod-1"),
        )));
        assert!(always.leader().is_leader(), "AlwaysLeader holds it");
        assert_eq!(always.leader().candidate().as_str(), "pod-1");

        let follower = aap_ports::memory::MemoryElection::new("election", "b");
        let operator = compose_with(std::sync::Arc::new(follower));
        assert!(
            !operator.leader().is_leader(),
            "a candidate whose campaign has not run holds nothing"
        );
    }

    /// K7a: the campaign arm wins a shutdown (`AlwaysLeader`'s `run` **is** the shutdown), and the composition root
    /// still drains what it left running: without it every SIGTERM would drop the controllers mid-pass, and a graceful
    /// stop would leave the lease to expire instead of releasing it.
    #[tokio::test]
    async fn the_shutdown_drains_what_the_winning_arm_left_running() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        // The campaign ended (it won the select); the controllers are still running, and are awaited.
        let finished = Arc::new(AtomicBool::new(false));
        let controllers_finished = finished.clone();
        let mut campaign = std::pin::pin!(std::future::ready(()));
        let mut controllers = std::pin::pin!(async move {
            controllers_finished.store(true, Ordering::Release);
        });
        drain(true, &mut campaign, false, &mut controllers).await;
        assert!(
            finished.load(Ordering::Acquire),
            "the controllers were not drained: the passes in flight would be dropped on every SIGTERM"
        );

        // And the other way round: the controllers ended, and the campaign releases the lease.
        let released = Arc::new(AtomicBool::new(false));
        let campaign_released = released.clone();
        let mut campaign = std::pin::pin!(async move {
            campaign_released.store(true, Ordering::Release);
        });
        let mut controllers = std::pin::pin!(std::future::ready(()));
        drain(false, &mut campaign, true, &mut controllers).await;
        assert!(
            released.load(Ordering::Acquire),
            "the campaign was not drained: the lease would expire instead of being released"
        );
    }

    /// K7a: the six election settings as the binary parses them with nothing set: **off** (a chart that renders no
    /// `LEADER_ELECTION_ENABLED`, which is every replica count of one, is a lone operator that writes), and the
    /// client-go timers.
    #[test]
    fn the_run_settings_default_to_no_election_and_the_client_go_timers() {
        use clap::Parser as _;

        let cli = crate::Cli::try_parse_from(["operator", "run"]).expect("the defaults parse");
        let crate::Command::Run(args) = cli.command else {
            panic!("`run` parses to the controllers");
        };
        assert!(
            !args.leader_election_enabled,
            "the election must be off unless the chart sets LEADER_ELECTION_ENABLED: a lone replica would campaign for a \
             Lease its Role does not grant, never hold it, and write nothing"
        );
        assert_eq!(args.leader_election_name, "aap-operator");
        assert_eq!(args.leader_election_namespace, None);
        assert_eq!(
            (
                args.leader_election_lease_secs,
                args.leader_election_renew_secs,
                args.leader_election_retry_secs
            ),
            (15, 10, 2),
            "the client-go defaults, and the ones the chart's values.yaml holds"
        );
    }

    /// The operator flushes the spans still queued when it ends, whether it ended well or not.
    #[tokio::test]
    async fn the_operator_flushes_its_spans_on_the_way_out() {
        for outcome in [Ok(()), Err(anyhow::anyhow!("the operator failed"))] {
            let exporter = aap_trace::testkit::MemoryExporter::default();
            let tracer = aap_trace::Pipeline::start(
                exporter.clone(),
                aap_trace::Limits::default(),
                aap_trace::Resource::new("test", "0"),
                aap_trace::Sampler::AlwaysOn,
            );
            let failed = outcome.is_err();
            let result = flushed(&tracer, async {
                tracer
                    .root(
                        aap_trace::SpanName::fixed("t").expect("a name"),
                        aap_trace::SpanKind::Internal,
                    )
                    .end();
                outcome
            })
            .await;
            assert_eq!(result.is_err(), failed);
            assert_eq!(exporter.spans().len(), 1, "the span reached the exporter");
        }
    }

    #[tokio::test]
    async fn the_provider_looks_for_run_pods_to_clear_only_with_settings_for_them() {
        let mut options = aap_controller::Options::default();
        assert!(
            !kubernetes_runtime(client(), &options).is_clearing_run_pods(),
            "without settings the operator has no right to read an account, a quota or a binding: it must not look"
        );
        options.run_pods = Some(aap_domain::RunPodSettings::offering(["standard"]));
        assert!(kubernetes_runtime(client(), &options).is_clearing_run_pods());
        options.watch_namespaces = Some(vec!["agents".to_owned()]);
        assert!(
            kubernetes_runtime(client(), &options).is_clearing_run_pods(),
            "and watching one namespace changes nothing of it"
        );
    }

    /// The namespaces the runtime provider watches are the ones the operator was given (K6b), and an empty list
    /// (no `WATCH_NAMESPACES` and no `WATCH_NAMESPACE`) is every namespace.
    #[tokio::test]
    async fn the_runtime_provider_watches_the_namespaces_the_operator_was_given() {
        let mut options = aap_controller::Options::default();
        assert!(
            kubernetes_runtime(client(), &options)
                .watched_namespaces()
                .is_empty()
        );
        options.watch_namespaces = Some(vec!["agents".to_owned(), "other".to_owned()]);
        assert_eq!(
            kubernetes_runtime(client(), &options).watched_namespaces(),
            ["agents", "other"]
        );
    }

    /// The settings of `run` as the environment would give them: the two namespace variables, and nothing else set.
    fn args(one: Option<&str>, list: Option<&str>) -> RunArgs {
        RunArgs {
            watch_namespace: one.map(str::to_owned),
            watch_namespaces: list.map(str::to_owned),
            health_addr: "127.0.0.1:0".parse().expect("an address"),
            metrics_addr: "127.0.0.1:0".parse().expect("an address"),
            instance: None,
            concurrency: 4,
            reconcile_timeout_secs: 60,
            registry_addr: "127.0.0.1:0".parse().expect("an address"),
            registry_token_file: None,
            registry_public_url: None,
            resync_secs: 300,
            resync_pending_secs: 15,
            default_agent_image: None,
            agent_runtime_class: None,
            run_pods_file: None,
            cache_allowed_claims: Vec::new(),
            max_access_tier: aap_api::AccessTierCap::Auto,
            tool_probe_enabled: false,
            tool_probe_timeout_secs: 5,
            leader_election_enabled: false,
            leader_election_name: "aap-operator".to_owned(),
            leader_election_namespace: None,
            leader_election_lease_secs: 15,
            leader_election_renew_secs: 10,
            leader_election_retry_secs: 2,
        }
    }

    /// `WATCH_NAMESPACES` and `WATCH_NAMESPACE`, as the chart sets them: the list, the one-namespace shorthand, and the
    /// primary that must be one of the list.
    #[test]
    fn the_namespaces_are_the_list_and_the_alias_is_the_primary() {
        // Neither, or a blank one (a chart value left empty): every namespace.
        assert_eq!(args(None, None).namespaces().expect("none"), None);
        assert_eq!(args(Some(""), None).namespaces().expect("blank"), None);
        assert_eq!(args(Some("  "), None).namespaces().expect("blank"), None);
        // The shorthand alone: that one, and it is checked like a name of the list.
        assert_eq!(
            args(Some("agents"), None).namespaces().expect("one"),
            Some(vec!["agents".to_owned()])
        );
        // The list alone: exactly those, in the order given (which is the order of the shards).
        assert_eq!(
            args(None, Some("b,a")).namespaces().expect("a list"),
            Some(vec!["b".to_owned(), "a".to_owned()])
        );
        // Both: the list, and the alias must be one of it.
        assert_eq!(
            args(Some("a"), Some("a, b")).namespaces().expect("both"),
            Some(vec!["a".to_owned(), "b".to_owned()])
        );
        let err = args(Some("c"), Some("a,b"))
            .namespaces()
            .expect_err("a primary nobody watches");
        assert!(
            format!("{err:#}").contains("\"c\"") && format!("{err:#}").contains("primary"),
            "{err:#}"
        );
    }

    /// A list the operator cannot watch is refused at start, naming what is wrong: an operator that started with part
    /// of the fleet would leave the rest of its namespaces unwatched without saying so.
    #[test]
    fn a_namespace_that_is_not_a_dns_label_a_duplicate_or_an_empty_list_is_refused() {
        // A list that is set and holds no name: a blank one, and a comma or a trailing one.
        for list in ["", " ", ","] {
            let err = args(None, Some(list)).namespaces().expect_err(list);
            assert!(
                format!("{err:#}").contains("not a namespace"),
                "{list}: {err:#}"
            );
        }
        // A name that is not a DNS label, in the list and as the shorthand.
        for name in [
            "Agents",
            "agents_1",
            "-agents",
            "agents-",
            "a.b",
            &"a".repeat(64),
        ] {
            let err = args(None, Some(name)).namespaces().expect_err(name);
            assert!(format!("{err:#}").contains("DNS label"), "{name}: {err:#}");
            assert!(args(Some(name), None).namespaces().is_err(), "{name}");
        }
        // A name given twice.
        let err = args(None, Some("a,b,a"))
            .namespaces()
            .expect_err("a duplicate");
        assert!(format!("{err:#}").contains("twice"), "{err:#}");
        // And the rule itself, at its edges.
        assert!(is_dns_label("a"));
        assert!(is_dns_label("a-b-c"));
        assert!(is_dns_label("a1"));
        assert!(is_dns_label(&"a".repeat(63)));
        assert!(!is_dns_label(""));
        assert!(!is_dns_label(&"a".repeat(64)));
        assert!(!is_dns_label("-a") && !is_dns_label("a-"));
    }

    /// K7a: the election is off by default; on, it takes the primary namespace when none is named, refuses
    /// timings the loop cannot honour, and refuses a replica that cannot name itself.
    #[test]
    fn the_leader_election_is_off_by_default_and_takes_the_primary_namespace() {
        // Off: nothing is asked of the cluster, and the operator is the only replica.
        let off = args(Some("agents"), None);
        assert!(
            off.leader_election(Some(&["agents".to_owned()]))
                .expect("off is not an error")
                .is_none()
        );

        // On, with a pod name: the primary namespace, the default lease name and the default timings.
        let mut on = args(Some("agents"), None);
        on.leader_election_enabled = true;
        on.instance = Some("pod-7".to_owned());
        let settings = on
            .leader_election(Some(&["agents".to_owned()]))
            .expect("on")
            .expect("an election");
        assert_eq!(settings.namespace, "agents", "the primary namespace");
        assert_eq!(settings.name, "aap-operator");
        assert_eq!(settings.candidate.as_str(), "pod-7");
        assert_eq!(settings.timings, aap_leader_kubernetes::Timings::default());

        // An explicit namespace wins over the primary.
        let mut named = on.clone();
        named.leader_election_namespace = Some("ops".to_owned());
        assert_eq!(
            named
                .leader_election(Some(&["agents".to_owned()]))
                .expect("on")
                .expect("an election")
                .namespace,
            "ops"
        );

        // A list of several namespaces: the first is the primary.
        assert_eq!(
            on.leader_election(Some(&["one".to_owned(), "two".to_owned()]))
                .expect("on")
                .expect("an election")
                .namespace,
            "one"
        );

        // On, and no namespace anywhere: refused, naming the variable.
        let mut nameless = args(None, None);
        nameless.leader_election_enabled = true;
        nameless.instance = Some("pod-7".to_owned());
        let error = nameless
            .leader_election(None)
            .expect_err("a lease needs a namespace");
        assert!(
            format!("{error:#}").contains("LEADER_ELECTION_NAMESPACE"),
            "{error:#}"
        );
        assert!(
            nameless.leader_election(Some(&[])).is_err(),
            "an empty list has no primary"
        );

        // A namespace that is not one, and a name that is not one.
        let mut wrong = on.clone();
        wrong.leader_election_namespace = Some("Agents".to_owned());
        assert!(wrong.leader_election(Some(&["agents".to_owned()])).is_err());
        let mut unnamed = on.clone();
        unnamed.leader_election_name = "  ".to_owned();
        assert!(
            unnamed
                .leader_election(Some(&["agents".to_owned()]))
                .is_err()
        );
    }

    /// K7a: timings the loop cannot honour are refused, naming the variables that gave them.
    #[test]
    fn inconsistent_leader_election_timings_are_refused() {
        let mut on = args(Some("agents"), None);
        on.leader_election_enabled = true;
        on.instance = Some("pod-7".to_owned());
        for (lease, renew, retry) in [(2, 2, 1), (15, 15, 2), (15, 10, 10), (0, 0, 0)] {
            let mut bad = on.clone();
            bad.leader_election_lease_secs = lease;
            bad.leader_election_renew_secs = renew;
            bad.leader_election_retry_secs = retry;
            let error = bad
                .leader_election(Some(&["agents".to_owned()]))
                .expect_err("inconsistent timings");
            let text = format!("{error:#}");
            assert!(text.contains("LEADER_ELECTION_LEASE_SECS"), "{text}");
            assert!(
                text.contains("retry < renew < lease") || text.contains("at least 1 s"),
                "{text}"
            );
        }
    }

    /// K7a: the candidate's identity is `POD_NAME`, else `HOSTNAME`, else a refusal.
    #[test]
    fn the_candidate_is_the_pod_name_then_the_hostname() {
        assert_eq!(
            candidate_id(Some("pod-7"), Some("host"))
                .expect("the pod name wins")
                .as_str(),
            "pod-7"
        );
        assert_eq!(
            candidate_id(None, Some("host"))
                .expect("the hostname")
                .as_str(),
            "host"
        );
        assert_eq!(
            candidate_id(Some("  "), Some("host"))
                .expect("a blank pod name is none")
                .as_str(),
            "host"
        );
        let error = candidate_id(None, None).expect_err("no identity");
        let text = format!("{error:#}");
        assert!(text.contains("POD_NAME"), "{text}");
        assert!(text.contains("HOSTNAME"), "{text}");
        assert!(
            candidate_id(Some(" "), Some("")).is_err(),
            "two blank names are none"
        );
    }
}
