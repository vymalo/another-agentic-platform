//! The configuration file of `operator run` (ANR-962, AD-064): one YAML document, read at start, and refused whole when
//! anything in it is wrong.
//!
//! **There is no secret in it.** The registry's token, the tracing headers and everything else that is secret is a
//! reference ([`SecretRef`](aap_trace_otlp::SecretRef)): a file a Secret is mounted at, or a variable. An unknown key is
//! refused at every level (`deny_unknown_fields` throughout), so that a misspelt key stops the operator at start and does
//! not leave a setting on its default; `version` is required and must be 1, so that a file of another shape says so.
//!
//! `operator run --check` loads and validates the file, and resolves no reference it names (a Secret is mounted in the pod,
//! not on the machine that checks the chart): it exits 0, or [`EXIT_CONFIG`] when the file is refused.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use aap_api::AccessTierCap;
use aap_domain::RunPodSettings;
use aap_trace_otlp::{OtlpSection, SecretRef};
use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// The version of the file this binary reads. A file that says another one is refused.
pub const VERSION: u32 = 1;

/// `EX_CONFIG` of `sysexits.h`: `operator run --check` exits with it when the file is refused, and `operator run` too.
pub const EXIT_CONFIG: u8 = 78;

/// The path the chart mounts the file at, and the default of `--config`.
pub const DEFAULT_PATH: &str = "/etc/aap-operator/config.yaml";

/// The file, as it is read. [`validate`](Config::validate) says whether the operator can work with it.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    /// 1. Required: a file of another shape is not read as this one.
    pub version: u32,
    /// Where the three listeners are.
    #[serde(default)]
    pub listen: ListenSection,
    /// Which namespaces are watched.
    #[serde(default)]
    pub watch: WatchSection,
    /// The timers of a pass and the number of passes at once.
    #[serde(default)]
    pub reconcile: ReconcileSection,
    /// The agent registry.
    #[serde(default)]
    pub registry: RegistrySection,
    /// What an agent gets when its config names nothing.
    #[serde(default)]
    pub defaults: DefaultsSection,
    /// The deployment's settings for run pods (slice D15a). Absent: this operator has no run pods.
    #[serde(default)]
    pub run_pods: Option<RunPodSettings>,
    /// Probing the MCP servers of `ToolProvider`s (K3b).
    #[serde(default)]
    pub tool_probe: ToolProbeSection,
    /// What the operator keeps of a deleted agent's store (ANR-770).
    #[serde(default)]
    pub retention: RetentionSection,
    /// Asking each agent what work it holds (AD-058 P2c).
    #[serde(default)]
    pub activity: ActivitySection,
    /// The `Lease` of a leader election (K7a).
    #[serde(default)]
    pub leader_election: LeaderElectionSection,
    /// The pod-minute meter (ANR-1103, AD-069): off by default.
    #[serde(default)]
    pub metering: MeteringSection,
    /// The log.
    #[serde(default)]
    pub log: LogSection,
    /// Traces (K5): the `telemetry.otlp` section, as the `OTEL_*` variables would say it.
    #[serde(default)]
    pub telemetry: TelemetrySection,
}

/// `listen`: the three ports.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ListenSection {
    /// Where `/healthz` and `/readyz` are served.
    #[serde(default = "default_health")]
    pub health: SocketAddr,
    /// Where `/metrics` is served.
    #[serde(default = "default_metrics")]
    pub metrics: SocketAddr,
    /// Where the agent registry is served (only with a token).
    #[serde(default = "default_registry")]
    pub registry: SocketAddr,
}

impl Default for ListenSection {
    fn default() -> Self {
        Self {
            health: default_health(),
            metrics: default_metrics(),
            registry: default_registry(),
        }
    }
}

fn default_health() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 8081))
}

fn default_metrics() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 9090))
}

fn default_registry() -> SocketAddr {
    SocketAddr::from(([0, 0, 0, 0], 8080))
}

/// `watch`: the namespaces.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WatchSection {
    /// The namespaces to watch, in order: **the first is the primary**, which names the cluster-scoped objects of run
    /// pods and the API's `defaults.namespace`. Empty: every namespace, which needs a `ClusterRole` the chart does not make.
    #[serde(default)]
    pub namespaces: Vec<String>,
}

/// `reconcile`: the timers and the concurrency.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReconcileSection {
    /// Services reconciled at the same time, per controller and per watched namespace. At least 1.
    #[serde(default = "default_concurrency")]
    pub concurrency: u16,
    /// Seconds one pass over one object may take.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Seconds between looks at a service that is Ready or Suspended.
    #[serde(default = "default_resync")]
    pub resync_secs: u64,
    /// Seconds between looks at a service that is rolling out, unwell, or held by a foreign object.
    #[serde(default = "default_resync_pending")]
    pub resync_pending_secs: u64,
}

impl Default for ReconcileSection {
    fn default() -> Self {
        Self {
            concurrency: default_concurrency(),
            timeout_secs: default_timeout(),
            resync_secs: default_resync(),
            resync_pending_secs: default_resync_pending(),
        }
    }
}

fn default_concurrency() -> u16 {
    4
}

fn default_timeout() -> u64 {
    60
}

fn default_resync() -> u64 {
    300
}

fn default_resync_pending() -> u64 {
    15
}

/// `registry`: the agent registry.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegistrySection {
    /// The URL of the registry document itself, sent as its `anchor`. Absent or empty: no `anchor`.
    #[serde(default)]
    pub public_url: Option<String>,
    /// The file (or variable) that holds the bearer token. **No token, no registry**: nothing is served on its port and
    /// every service is `Listed: False`, reason `RegistryDisabled` (fail closed). Read once, at start.
    #[serde(default, deserialize_with = "token_ref")]
    pub token: Option<SecretRef>,
}

/// `registry.token`, with the key in the message when the value is not a reference: a string there may be the token, so
/// the message never quotes it.
fn token_ref<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SecretRef>, D::Error> {
    use serde::de::Error as _;
    Option::<SecretRef>::deserialize(deserializer)
        .map_err(|_| D::Error::custom("token: a reference is { file: /path } or { env: NAME }"))
}

/// `defaults`: what an agent that names nothing gets.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefaultsSection {
    /// The image of an agent whose `AgentConfig` names none (AD-029): `repository:tag@sha256:…`.
    #[serde(default)]
    pub agent_image: Option<String>,
    /// The `RuntimeClass` of every agent's own pod (K14a, AD-052): the name of an object the cluster administrator made.
    #[serde(default)]
    pub agent_runtime_class: Option<String>,
    /// The claims the deployment allows as an agent's package cache (K1). Empty: no agent may have one.
    #[serde(default)]
    pub cache_allowed_claims: Vec<String>,
    /// The highest access tier an `AgentConfig` may ask for (K1).
    #[serde(default)]
    pub max_access_tier: AccessTierCap,
}

impl Default for DefaultsSection {
    fn default() -> Self {
        Self {
            agent_image: None,
            agent_runtime_class: None,
            cache_allowed_claims: Vec::new(),
            max_access_tier: AccessTierCap::Auto,
        }
    }
}

/// `toolProbe`: the MCP probe of a `ToolProvider`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolProbeSection {
    /// Probe the servers (`initialize` and `tools/list`) and write the reachability and tool names into the provider's
    /// status. Off: no request is made and the status has no `Reachable` condition.
    #[serde(default)]
    pub enabled: bool,
    /// How long one probe may take, in seconds (1 to 60).
    #[serde(default = "default_probe_timeout")]
    pub timeout_secs: u64,
}

impl Default for ToolProbeSection {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout_secs: default_probe_timeout(),
        }
    }
}

fn default_probe_timeout() -> u64 {
    5
}

/// `activity`: the HTTP activity source (AD-058 P2c), which asks each agent `GET /internal/activity` for the idle controller.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivitySection {
    /// Build the source. **Off by default**: no request is made. On, `token` is required, because the agents refuse a request
    /// without their bearer.
    #[serde(default)]
    pub enabled: bool,
    /// The port agents serve `/internal/activity` on (their `LISTEN_ADDR`).
    #[serde(default = "default_activity_port")]
    pub port: u16,
    /// How long one question may take, in seconds (1 to 30). An agent that does not answer by then counts as busy.
    #[serde(default = "default_activity_timeout")]
    pub timeout_secs: u64,
    /// The bearer the agents accept, by reference (a mounted Secret key). Read once at start, never logged.
    #[serde(default, deserialize_with = "token_ref")]
    pub token: Option<SecretRef>,
}

impl Default for ActivitySection {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_activity_port(),
            timeout_secs: default_activity_timeout(),
            token: None,
        }
    }
}

fn default_activity_port() -> u16 {
    8080
}

fn default_activity_timeout() -> u64 {
    3
}

/// `metering`: the pod-minute meter (ANR-1103, AD-069). **Off by default**, and then nothing here is read or asked.
///
/// On, the operator sends one usage line per pod and whole minute to the OpenTelemetry Collector, for the account the
/// orchestrator's broker names for the run. The file holds no secret: the broker's bearer is a reference.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeteringSection {
    /// Run the meter. On, `broker` and `collector` are required and `runPods.nodeType` must admit exactly one value.
    #[serde(default)]
    pub enabled: bool,
    /// The `source` of every line: a constant of the deployment, the same on every replica (`[A-Za-z0-9._:/-]`, at most 128).
    #[serde(default = "default_metering_source")]
    pub source: String,
    /// How often the meter looks at the pods, in seconds (5 to 300).
    #[serde(default = "default_metering_period")]
    pub period_secs: u64,
    /// The orchestrator's broker, which says which account a run belongs to.
    #[serde(default)]
    pub broker: MeteringBrokerSection,
    /// The OpenTelemetry Collector the lines go to (OTLP/HTTP JSON logs).
    #[serde(default)]
    pub collector: MeteringCollectorSection,
}

impl Default for MeteringSection {
    fn default() -> Self {
        Self {
            enabled: false,
            source: default_metering_source(),
            period_secs: default_metering_period(),
            broker: MeteringBrokerSection::default(),
            collector: MeteringCollectorSection::default(),
        }
    }
}

fn default_metering_source() -> String {
    "aap-operator".to_owned()
}

fn default_metering_period() -> u64 {
    30
}

/// `metering.broker`: the route `GET <url>/internal/runs/<run>/account`.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeteringBrokerSection {
    /// The broker's base URL (`http://broker.ns.svc:8080`): `http` or `https`, no credentials, query or fragment.
    #[serde(default)]
    pub url: Option<String>,
    /// The meter's bearer, by reference (a mounted Secret key). Read once at start, never logged.
    #[serde(default, deserialize_with = "token_ref")]
    pub token: Option<SecretRef>,
    /// One question's time limit, in seconds (1 to 60).
    #[serde(default = "default_broker_timeout")]
    pub timeout_secs: u64,
}

impl Default for MeteringBrokerSection {
    fn default() -> Self {
        Self {
            url: None,
            token: None,
            timeout_secs: default_broker_timeout(),
        }
    }
}

fn default_broker_timeout() -> u64 {
    5
}

/// `metering.collector`: the OTLP/HTTP endpoint (`<endpoint>/v1/logs`) and the sink's bounds.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeteringCollectorSection {
    /// The collector's base URL (`http://collector.ns.svc:4318`). `http` is allowed: it is in the cluster behind a NetworkPolicy.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// A PEM bundle that is the only trust for an `https` endpoint. Absent: the platform roots.
    #[serde(default)]
    pub ca_file: Option<String>,
    /// Lines the queue holds before the meter is told to wait (1 to 1000000).
    #[serde(default = "default_collector_queue")]
    pub queue: usize,
    /// Lines in one POST (1 to 10000).
    #[serde(default = "default_collector_batch")]
    pub batch: usize,
    /// How long a part batch waits before it is sent, in seconds (1 to 3600).
    #[serde(default = "default_collector_interval")]
    pub interval_secs: u64,
    /// One POST's time limit, in seconds (1 to 300).
    #[serde(default = "default_collector_timeout")]
    pub timeout_secs: u64,
}

impl Default for MeteringCollectorSection {
    fn default() -> Self {
        Self {
            endpoint: None,
            ca_file: None,
            queue: default_collector_queue(),
            batch: default_collector_batch(),
            interval_secs: default_collector_interval(),
            timeout_secs: default_collector_timeout(),
        }
    }
}

fn default_collector_queue() -> usize {
    4096
}

fn default_collector_batch() -> usize {
    512
}

fn default_collector_interval() -> u64 {
    5
}

fn default_collector_timeout() -> u64 {
    10
}

/// `retention`: what a deleted agent leaves behind.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetentionSection {
    /// Days the database and volumes of a deleted agent (`deletionPolicy: Retain`) are kept before they are deleted.
    /// `0` keeps them for ever and turns the sweep off.
    #[serde(default = "default_retention")]
    pub after_delete_days: u64,
}

impl Default for RetentionSection {
    fn default() -> Self {
        Self {
            after_delete_days: default_retention(),
        }
    }
}

fn default_retention() -> u64 {
    90
}

/// `leaderElection`: the `Lease` every replica campaigns for (K7a).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeaderElectionSection {
    /// Elect one replica to write. Off: this replica is the only one, and every pass writes.
    #[serde(default)]
    pub enabled: bool,
    /// The name of the `Lease`.
    #[serde(default = "default_lease_name")]
    pub name: String,
    /// The namespace of the `Lease`. Absent: the primary namespace, the first of `watch.namespaces`.
    #[serde(default)]
    pub namespace: Option<String>,
    /// How long the lease lives, in seconds: a holder that has not renewed for this long may be replaced.
    #[serde(default = "default_lease_secs")]
    pub lease_secs: u64,
    /// How often the holder renews the lease, in seconds.
    #[serde(default = "default_renew_secs")]
    pub renew_secs: u64,
    /// How often a replica that is not the holder tries, in seconds.
    #[serde(default = "default_retry_secs")]
    pub retry_secs: u64,
}

impl Default for LeaderElectionSection {
    fn default() -> Self {
        Self {
            enabled: false,
            name: default_lease_name(),
            namespace: None,
            lease_secs: default_lease_secs(),
            renew_secs: default_renew_secs(),
            retry_secs: default_retry_secs(),
        }
    }
}

fn default_lease_name() -> String {
    "aap-operator".to_owned()
}

fn default_lease_secs() -> u64 {
    15
}

fn default_renew_secs() -> u64 {
    10
}

fn default_retry_secs() -> u64 {
    2
}

/// `log`: the filter of the log.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogSection {
    /// A `tracing` filter, such as `info,kube=warn,aap_controller=debug`. `info` when absent.
    #[serde(default = "default_filter")]
    pub filter: String,
}

impl Default for LogSection {
    fn default() -> Self {
        Self {
            filter: default_filter(),
        }
    }
}

fn default_filter() -> String {
    "info".to_owned()
}

/// `telemetry`: the traces.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelemetrySection {
    /// The OTLP/HTTP endpoint and what to send with it. Absent: no span is made.
    #[serde(default)]
    pub otlp: Option<OtlpSection>,
}

/// A configuration the operator can work with.
#[derive(Debug)]
pub struct Settings {
    /// Where the three listeners are.
    pub listen: Listen,
    /// The namespaces to watch, in order; `None` is every namespace.
    pub namespaces: Option<Vec<String>>,
    /// The timers and the concurrency.
    pub reconcile: Reconcile,
    /// The registry.
    pub registry: Registry,
    /// What an agent that names nothing gets.
    pub defaults: Defaults,
    /// The run pods' settings, when the deployment has them.
    pub run_pods: Option<RunPodSettings>,
    /// The MCP probe.
    pub tool_probe: ToolProbe,
    /// Days a deleted agent's store is kept (`0`: for ever).
    pub retention_after_delete_days: u64,
    /// The activity source.
    pub activity: Activity,
    /// The election.
    pub leader: Leader,
    /// The pod-minute meter, when `metering.enabled` is set.
    pub metering: Option<Metering>,
    /// The `tracing` filter.
    pub log_filter: String,
    /// The traces, as the section says them (no header read yet).
    pub telemetry: Option<OtlpSection>,
}

/// The three listeners.
#[derive(Clone, Copy, Debug)]
pub struct Listen {
    pub health: SocketAddr,
    pub metrics: SocketAddr,
    pub registry: SocketAddr,
}

/// The timers of a pass and the number of passes at once.
#[derive(Clone, Copy, Debug)]
pub struct Reconcile {
    pub concurrency: u16,
    pub timeout: Duration,
    pub resync: Duration,
    pub resync_pending: Duration,
}

/// The registry.
#[derive(Clone, Debug)]
pub struct Registry {
    pub public_url: Option<String>,
    pub token: Option<SecretRef>,
}

/// What an agent that names nothing gets.
#[derive(Clone, Debug)]
pub struct Defaults {
    pub agent_image: Option<String>,
    pub agent_runtime_class: Option<String>,
    pub cache_allowed_claims: Vec<String>,
    pub max_access_tier: AccessTierCap,
}

/// The MCP probe.
#[derive(Clone, Copy, Debug)]
pub struct ToolProbe {
    pub enabled: bool,
    pub timeout: Duration,
}

/// The activity source.
#[derive(Clone, Debug)]
pub struct Activity {
    pub enabled: bool,
    pub port: u16,
    pub timeout: Duration,
    pub token: Option<SecretRef>,
}

/// The meter's settings, checked: only built when `metering.enabled` is true.
#[derive(Clone, Debug)]
pub struct Metering {
    /// The `source` of every line.
    pub source: String,
    /// The node label whose value is the node type (`runPods.nodeType.labelKey`).
    pub node_type_label_key: String,
    /// How often the pods are looked at.
    pub period: Duration,
    /// The broker's base URL.
    pub broker_url: url::Url,
    /// The broker's bearer, by reference.
    pub broker_token: SecretRef,
    /// One question's time limit.
    pub broker_timeout: Duration,
    /// The collector, with the sink's bounds.
    pub collector: aap_usage_otlp::UsageOtlpSettings,
}

/// The election, with the timings still as the file gave them (the provider's `Timings` needs the runtime feature).
#[derive(Clone, Debug)]
pub struct Leader {
    pub enabled: bool,
    pub name: String,
    pub namespace: Option<String>,
    pub lease_secs: u64,
    pub renew_secs: u64,
    pub retry_secs: u64,
}

impl Config {
    /// Read a file's text.
    ///
    /// # Errors
    ///
    /// The text is not YAML, or is not this file: a key it does not have, a value of the wrong type, a missing block.
    pub fn parse(text: &str) -> Result<Self> {
        serde_yaml::from_str(text).context("the configuration is not valid")
    }

    /// Whether the operator can work with this file, and the settings if it can.
    ///
    /// # Errors
    ///
    /// The first setting that is wrong, named: a version that is not 1, a name that is not a DNS label, an image with
    /// white space, timings the election cannot honour, run pods whose settings fail their checks, a filter `tracing`
    /// does not parse, a trace section the exporter could not use.
    pub fn validate(self) -> Result<Settings> {
        if self.version != VERSION {
            bail!(
                "version: {} is not the version of this file ({VERSION})",
                self.version
            );
        }
        check_listeners(&self.listen, self.registry.token.is_some())?;
        let metering = self.metering_settings()?;

        let mut namespaces: Vec<String> = Vec::new();
        for name in &self.watch.namespaces {
            check_label("watch.namespaces", name)?;
            if namespaces.iter().any(|n| n == name) {
                bail!(
                    "watch.namespaces: {name:?} is given twice: a namespace is watched once, by one set of controllers"
                );
            }
            namespaces.push(name.clone());
        }
        let namespaces = (!namespaces.is_empty()).then_some(namespaces);

        let reconcile = self.reconcile;
        for (key, value) in [
            ("reconcile.concurrency", u64::from(reconcile.concurrency)),
            ("reconcile.timeoutSecs", reconcile.timeout_secs),
            ("reconcile.resyncSecs", reconcile.resync_secs),
            ("reconcile.resyncPendingSecs", reconcile.resync_pending_secs),
        ] {
            if value == 0 {
                bail!("{key}: at least 1");
            }
        }

        let defaults = self.defaults;
        let agent_image = match defaults.agent_image.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(image) => {
                if image.contains(char::is_whitespace) {
                    bail!(
                        "defaults.agentImage: {image:?} has white space in it: an image reference such as repository:tag@sha256:… is expected"
                    );
                }
                Some(image.to_owned())
            }
        };
        let agent_runtime_class = match defaults.agent_runtime_class.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(class) => {
                if !aap_domain::is_dns_subdomain(class) {
                    bail!(
                        "defaults.agentRuntimeClass: {class:?} is not the name of a RuntimeClass (a DNS subdomain): the RuntimeClass object is the administrator's"
                    );
                }
                Some(class.to_owned())
            }
        };
        let cache_allowed_claims: Vec<String> = defaults
            .cache_allowed_claims
            .iter()
            .map(|c| c.trim().to_owned())
            .filter(|c| !c.is_empty())
            .collect();

        if let Some(run_pods) = &self.run_pods
            && let Err(problems) = run_pods.validate()
        {
            bail!(
                "{}",
                problems
                    .iter()
                    .map(|p| format!("runPods.{p}"))
                    .collect::<Vec<_>>()
                    .join("\n  ")
            );
        }

        if !(1..=60).contains(&self.tool_probe.timeout_secs) {
            bail!(
                "toolProbe.timeoutSecs: {} is not a whole number of seconds from 1 to 60",
                self.tool_probe.timeout_secs
            );
        }

        if self.activity.port == 0 {
            bail!("activity.port: 0 is not a port");
        }
        if !(1..=30).contains(&self.activity.timeout_secs) {
            bail!(
                "activity.timeoutSecs: {} is not a whole number of seconds from 1 to 30",
                self.activity.timeout_secs
            );
        }
        if self.activity.enabled && self.activity.token.is_none() {
            bail!(
                "activity.token: required while activity.enabled is true: the agents refuse a request with no bearer"
            );
        }

        let leader = self.leader_election;
        let mut leader_namespace = leader
            .namespace
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned);
        if leader.enabled {
            let namespace = leader_namespace
                .clone()
                .or_else(|| namespaces.as_deref().and_then(|l| l.first().cloned()));
            let Some(namespace) = namespace else {
                bail!(
                    "leaderElection.enabled is set and leaderElection.namespace names no namespace, and watch.namespaces names none either: the Lease needs the namespace the operator's Role is in"
                );
            };
            check_label("leaderElection.namespace", &namespace)?;
            leader_namespace = Some(namespace);
            let name = leader.name.trim();
            if name.is_empty() {
                bail!("leaderElection.name is empty: a Lease has a name");
            }
            check_label("leaderElection.name", name)?;
            let (lease, renew, retry) = (leader.lease_secs, leader.renew_secs, leader.retry_secs);
            if lease == 0 || renew == 0 || retry == 0 {
                bail!(
                    "leaderElection: leaseSecs, renewSecs and retrySecs are each at least 1 (they are {lease}, {renew} and {retry})"
                );
            }
            if !(retry < renew && renew < lease) {
                bail!(
                    "leaderElection: retry < renew < lease is required (they are {retry}, {renew} and {lease}): a lease that expires before it is renewed is no election"
                );
            }
        }

        if self.log.filter.trim().is_empty() {
            bail!("log.filter is empty: a `tracing` filter such as info is expected");
        }
        tracing_subscriber::EnvFilter::try_new(&self.log.filter)
            .map_err(|why| anyhow::anyhow!("log.filter is not a `tracing` filter ({why})"))?;

        if let Some(otlp) = &self.telemetry.otlp
            && let Err(why) = otlp.validate()
        {
            bail!("telemetry.otlp: {why}");
        }

        Ok(Settings {
            listen: Listen {
                health: self.listen.health,
                metrics: self.listen.metrics,
                registry: self.listen.registry,
            },
            namespaces,
            reconcile: Reconcile {
                concurrency: reconcile.concurrency,
                timeout: Duration::from_secs(reconcile.timeout_secs),
                resync: Duration::from_secs(reconcile.resync_secs),
                resync_pending: Duration::from_secs(reconcile.resync_pending_secs),
            },
            registry: Registry {
                public_url: self
                    .registry
                    .public_url
                    .map(|u| u.trim().to_owned())
                    .filter(|u| !u.is_empty()),
                token: self.registry.token,
            },
            defaults: Defaults {
                agent_image,
                agent_runtime_class,
                cache_allowed_claims,
                max_access_tier: defaults.max_access_tier,
            },
            run_pods: self.run_pods,
            tool_probe: ToolProbe {
                enabled: self.tool_probe.enabled,
                timeout: Duration::from_secs(self.tool_probe.timeout_secs),
            },
            retention_after_delete_days: self.retention.after_delete_days,
            activity: Activity {
                enabled: self.activity.enabled,
                port: self.activity.port,
                timeout: Duration::from_secs(self.activity.timeout_secs),
                token: self.activity.token,
            },
            leader: Leader {
                enabled: leader.enabled,
                name: leader.name.trim().to_owned(),
                namespace: leader_namespace,
                lease_secs: leader.lease_secs,
                renew_secs: leader.renew_secs,
                retry_secs: leader.retry_secs,
            },
            metering,
            log_filter: self.log.filter,
            telemetry: self.telemetry.otlp,
        })
    }

    /// `metering`: nothing while it is off; on, every part of it, checked. **Incomplete is refused**, so that a meter that
    /// would run blind never starts: no broker, no token, no collector, a node type the meter cannot tell.
    pub(crate) fn metering_settings(&self) -> Result<Option<Metering>> {
        let m = &self.metering;
        if !m.enabled {
            return Ok(None);
        }
        if !aap_usage::is_line_text(&m.source) {
            bail!("metering.source: 1 to 128 of [A-Za-z0-9._:/-]");
        }
        if !(5..=300).contains(&m.period_secs) {
            bail!(
                "metering.periodSecs: {} is not a whole number of seconds from 5 to 300",
                m.period_secs
            );
        }
        // The node type of a run pod is read from the node affinity the operator wrote into it, which names the admitted
        // values: with one, that is the type; with several the pod's type is not known, and a line with no type is refused.
        let node_type = self.run_pods.as_ref().map(|r| &r.node_type);
        let Some(node_type) = node_type.filter(|n| n.admitted.len() == 1) else {
            bail!(
                "metering.enabled: needs runPods.nodeType with exactly one admitted value, because the node type of a run pod is read from the node affinity (the meter reads no Node): without it, or with several values, no run pod's node type is known"
            );
        };
        let Some(url) = m
            .broker
            .url
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        else {
            bail!("metering.broker.url: required while metering.enabled is true");
        };
        let broker_url =
            url::Url::parse(url).map_err(|_| anyhow::anyhow!("metering.broker.url: not a URL"))?;
        aap_usage_broker::check_base(&broker_url)
            .map_err(|why| anyhow::anyhow!("metering.broker.url: {why}"))?;
        let Some(token) = m.broker.token.clone() else {
            bail!(
                "metering.broker.token: required while metering.enabled is true: the broker refuses a request with no bearer"
            );
        };
        if !(1..=60).contains(&m.broker.timeout_secs) {
            bail!(
                "metering.broker.timeoutSecs: {} is not a whole number of seconds from 1 to 60",
                m.broker.timeout_secs
            );
        }
        let c = &m.collector;
        let Some(endpoint) = c
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        else {
            bail!("metering.collector.endpoint: required while metering.enabled is true");
        };
        let endpoint = url::Url::parse(endpoint)
            .map_err(|_| anyhow::anyhow!("metering.collector.endpoint: not a URL"))?;
        let mut collector = aap_usage_otlp::UsageOtlpSettings::new(endpoint);
        collector.ca = c
            .ca_file
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(std::path::PathBuf::from);
        collector.queue = c.queue;
        collector.batch = c.batch;
        collector.interval = Duration::from_secs(c.interval_secs);
        collector.timeout = Duration::from_secs(c.timeout_secs);
        collector
            .validate()
            .map_err(|why| anyhow::anyhow!("metering.collector: {why}"))?;
        Ok(Some(Metering {
            source: m.source.clone(),
            node_type_label_key: node_type.label_key.clone(),
            period: Duration::from_secs(m.period_secs),
            broker_url,
            broker_token: token,
            broker_timeout: Duration::from_secs(m.broker.timeout_secs),
            collector,
        }))
    }
}

/// No two listeners on one port: the second `bind` would fail at start ("Address already in use"), so the check says it
/// first. The registry's port only counts when a token turns it on.
fn check_listeners(listen: &ListenSection, registry: bool) -> Result<()> {
    let mut listeners = vec![
        ("listen.health", listen.health),
        ("listen.metrics", listen.metrics),
    ];
    if registry {
        listeners.push(("listen.registry", listen.registry));
    }
    for (i, (a, addr_a)) in listeners.iter().enumerate() {
        for (b, addr_b) in &listeners[i + 1..] {
            if addr_a.port() == addr_b.port() {
                bail!(
                    "{a} and {b} are both on port {}: the operator's listeners each need a port of its own",
                    addr_a.port()
                );
            }
        }
    }
    Ok(())
}

/// Whether `name` is a namespace name: a DNS label, `^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$`, which is the rule the chart's
/// `_validate.tpl` holds its values to.
#[must_use]
pub fn is_dns_label(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 63
        && !name.starts_with('-')
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Refuse a name that is no namespace, naming the setting that gave it.
fn check_label(setting: &str, name: &str) -> Result<()> {
    if is_dns_label(name) {
        return Ok(());
    }
    bail!(
        "{setting}: {name:?} is not a namespace: a namespace is a DNS label (lowercase letters, digits and '-', starting and ending with a letter or a digit, at most 63 characters)"
    );
}

/// The JSON Schema of the file, generated from the types (ANR-962): committed as `config.schema.json`, and a test fails
/// when the two differ. `deny_unknown_fields` is what makes `additionalProperties: false` at every level.
///
/// # Errors
///
/// The schema cannot be written as JSON (it always can: this is a `Result` so that the caller says so rather than panicking).
pub fn schema() -> Result<String> {
    let schema = schemars::schema_for!(Config);
    let mut text =
        serde_json::to_string_pretty(&schema).context("writing the configuration's schema")?;
    text.push('\n');
    Ok(text)
}

/// Why [`load`] gave no settings. The two are different exit codes (the card's): a file that is **refused** (it does not parse,
/// names a key the operator does not have, or fails a check) is `EX_CONFIG`, 78, and a file that **cannot be read** (missing, a
/// directory, no permission) is an ordinary failure, 1: the first is the author's mistake, the second the pod's (a mount).
#[derive(Debug)]
pub enum LoadError {
    /// The file could not be read.
    Unreadable(anyhow::Error),
    /// The file was read and refused.
    Refused(anyhow::Error),
}

impl LoadError {
    /// The exit code of the process: [`EXIT_CONFIG`] for a refused file, 1 for one that cannot be read.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Unreadable(_) => 1,
            Self::Refused(_) => EXIT_CONFIG,
        }
    }
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(e) | Self::Refused(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// A reference the file names that the start could not use: a token file that is missing or empty, a variable that is unset or
/// empty. It is a refusal of the file (exit 78), found when the reference is read at start and not by `--check`, which holds
/// only the reference's shape. The message names the setting and the reference, never a value.
#[derive(Debug)]
pub struct ReferenceRefused(pub String);

impl std::fmt::Display for ReferenceRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ReferenceRefused {}

/// Whether `error`, or something it wraps, is a [`ReferenceRefused`]: the process then exits [`EXIT_CONFIG`].
#[must_use]
pub fn is_reference_refused(error: &anyhow::Error) -> bool {
    error.chain().any(|e| e.is::<ReferenceRefused>())
}

/// Read, parse and validate the file at `path`.
///
/// # Errors
///
/// [`LoadError::Unreadable`] when the file cannot be read, [`LoadError::Refused`] when [`Config::parse`] or
/// [`Config::validate`] refuses it.
pub fn load(path: &Path) -> Result<Settings, LoadError> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        // `InvalidData` is a file that was read and is not UTF-8: its content is what is wrong, so it is refused (78), where a
        // missing file, a directory or a mode that forbids the read is the pod's mount (1).
        let invalid = e.kind() == std::io::ErrorKind::InvalidData;
        let error = anyhow::Error::new(e).context(if invalid {
            format!(
                "the configuration file {} is not valid UTF-8",
                path.display()
            )
        } else {
            format!("the configuration file {} cannot be read", path.display())
        });
        if invalid {
            LoadError::Refused(error)
        } else {
            LoadError::Unreadable(error)
        }
    })?;
    Config::parse(&text)
        .and_then(Config::validate)
        .with_context(|| format!("the configuration file {}", path.display()))
        .map_err(LoadError::Refused)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const EXAMPLE: &str = include_str!("../config.example.yaml");

    fn settings(text: &str) -> Settings {
        Config::parse(text)
            .and_then(Config::validate)
            .unwrap_or_else(|e| panic!("{e:#}"))
    }

    fn refused(text: &str) -> String {
        format!(
            "{:#}",
            Config::parse(text)
                .and_then(Config::validate)
                .expect_err("a configuration the operator cannot work with")
        )
    }

    /// The least a deployment must say: a version, and nothing else.
    fn minimal() -> String {
        "version: 1\n".to_owned()
    }

    /// The chart's `runPods` block, as the chart writes it into the file: `runPods` and `runPodClasses` merged with the
    /// names of the objects the chart made.
    fn chart_run_pods() -> serde_json::Value {
        let values: serde_json::Value = serde_yaml::from_str(
            &std::fs::read_to_string(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../deploy/operator/values.yaml"
            ))
            .expect("the chart's values"),
        )
        .expect("YAML");
        let mut run_pods = values["runPods"].clone();
        let object = run_pods.as_object_mut().expect("a map");
        object.remove("enabled");
        object.remove("priorityClassName");
        object.insert("classes".into(), values["runPodClasses"].clone());
        object.insert(
            "priorityClass".into(),
            serde_json::json!("aap-operator-ns-run"),
        );
        object.insert("role".into(), serde_json::json!("aap-operator-run-pods"));
        run_pods
    }

    fn with_run_pods(change: impl FnOnce(&mut serde_json::Value)) -> String {
        let mut run_pods = chart_run_pods();
        change(&mut run_pods);
        let mut document: serde_json::Value =
            serde_yaml::from_str(&minimal()).expect("the minimal file");
        document["runPods"] = run_pods;
        serde_yaml::to_string(&document).expect("YAML")
    }

    /// A deployment with the chart's run pods admitting `admitted` of one node label, and `metering` (the body of an enabled
    /// `metering:` block) after `enabled: true`.
    fn metering_file(admitted: &[&str], metering: &str) -> String {
        let text = with_run_pods(|run_pods| {
            run_pods["nodeType"] = serde_json::json!({
                "labelKey": "example.com/node-type",
                "admitted": admitted,
            });
        });
        format!("{text}metering:\n  enabled: true\n{metering}")
    }

    const COMPLETE_METERING: &str = "  broker:\n    url: http://broker.ns.svc:8080\n    token: { env: METER_TOKEN }\n  collector:\n    endpoint: http://collector.ns.svc:4318\n";

    #[test]
    fn the_meter_is_off_by_default_and_the_example_is_off() {
        assert!(settings(&minimal()).metering.is_none());
        assert!(settings(EXAMPLE).metering.is_none());
        // Off: a half-written section is not looked at.
        assert!(
            settings("version: 1\nmetering:\n  broker:\n    url: ftp://x\n")
                .metering
                .is_none()
        );
    }

    #[test]
    fn a_complete_meter_carries_the_settings() {
        let m = settings(&metering_file(&["standard"], COMPLETE_METERING))
            .metering
            .expect("on");
        assert_eq!(m.source, "aap-operator");
        assert_eq!(m.node_type_label_key, "example.com/node-type");
        assert_eq!(m.period, Duration::from_secs(30));
        assert_eq!(m.broker_url.as_str(), "http://broker.ns.svc:8080/");
        assert!(matches!(m.broker_token, SecretRef::Env(ref n) if n == "METER_TOKEN"));
        assert_eq!(m.collector.queue, 4096);
        assert_eq!(m.collector.batch, 512);
    }

    #[test]
    fn an_incomplete_meter_is_refused_naming_the_setting() {
        let on = |admitted: &[&str], rest: &str| refused(&metering_file(admitted, rest));
        let no_broker_url =
            "  broker:\n    token: { env: T }\n  collector:\n    endpoint: http://c:4318\n";
        assert!(on(&["standard"], no_broker_url).contains("metering.broker.url"));
        let no_token =
            "  broker:\n    url: http://b:8080\n  collector:\n    endpoint: http://c:4318\n";
        assert!(on(&["standard"], no_token).contains("metering.broker.token"));
        let no_collector = "  broker:\n    url: http://b:8080\n    token: { env: T }\n";
        assert!(on(&["standard"], no_collector).contains("metering.collector.endpoint"));
        assert!(on(&["standard"], "").contains("metering.broker.url"));
        // A URL with credentials, a query or another scheme is not a broker.
        for url in ["http://u:p@b:8080", "http://b:8080/?a=1", "ftp://b"] {
            let rest = format!(
                "  broker:\n    url: \"{url}\"\n    token: {{ env: T }}\n  collector:\n    endpoint: http://c:4318\n"
            );
            assert!(
                on(&["standard"], &rest).contains("metering.broker.url"),
                "{url}"
            );
        }
        // A collector the sink would refuse.
        for bound in ["queue: 0", "batch: 0", "intervalSecs: 0", "timeoutSecs: 0"] {
            let rest = format!("{COMPLETE_METERING}    {bound}\n");
            assert!(
                on(&["standard"], &rest).contains("metering.collector"),
                "{bound}"
            );
        }
        for (key, value) in [
            ("source", "has space"),
            ("periodSecs", "4"),
            ("periodSecs", "301"),
        ] {
            let rest = format!("  {key}: {value:?}\n{COMPLETE_METERING}");
            assert!(
                on(&["standard"], &rest).contains(&format!("metering.{key}")),
                "{key}"
            );
        }
    }

    #[test]
    fn the_node_type_must_be_one_the_meter_can_read() {
        let refuse = |text: String| refused(&text);
        // Several values: a run pod's type is not known from its affinity.
        assert!(
            refuse(metering_file(&["a", "b"], COMPLETE_METERING))
                .contains("exactly one admitted value")
        );
        assert!(
            refuse(metering_file(&[], COMPLETE_METERING)).contains("exactly one admitted value")
        );
        // No run pods at all.
        let none = format!("version: 1\nmetering:\n  enabled: true\n{COMPLETE_METERING}");
        assert!(refuse(none).contains("runPods.nodeType"));
    }

    #[test]
    fn the_metering_section_refuses_an_unknown_key_at_every_level() {
        for text in [
            "version: 1\nmetering:\n  nope: 1\n",
            "version: 1\nmetering:\n  broker:\n    nope: 1\n",
            "version: 1\nmetering:\n  collector:\n    nope: 1\n",
        ] {
            assert!(Config::parse(text).is_err(), "{text}");
        }
        // A string where a reference belongs may be the token itself: the message never quotes it.
        let text = "version: 1\nmetering:\n  broker:\n    token: s3cret-value\n";
        let message = format!("{:#}", Config::parse(text).expect_err("not a reference"));
        assert!(!message.contains("s3cret-value"), "{message}");
    }

    #[test]
    fn the_example_is_a_configuration_the_operator_can_work_with() {
        let s = settings(EXAMPLE);
        assert_eq!(s.listen.health.port(), 8081);
        assert_eq!(s.listen.metrics.port(), 9090);
        assert_eq!(s.listen.registry.port(), 8080);
        assert_eq!(
            s.namespaces.as_deref(),
            Some(&["another-agentic-system".to_owned()][..])
        );
        assert_eq!(s.reconcile.concurrency, 4);
        assert_eq!(s.reconcile.timeout, Duration::from_secs(60));
        assert!(s.registry.token.is_some());
        assert!(s.defaults.agent_image.is_some());
        assert_eq!(s.defaults.max_access_tier, AccessTierCap::Auto);
        assert_eq!(s.tool_probe.timeout, Duration::from_secs(5));
        assert_eq!(s.retention_after_delete_days, 90);
        assert!(!s.leader.enabled);
        assert_eq!(s.log_filter, "info");
        assert!(s.telemetry.is_some(), "the example documents the traces");
    }

    #[test]
    fn the_example_has_no_secret_in_it() {
        // The words a secret would be written under; a reference (`token: { file }`, a Secret's name and key) is not one.
        let text = EXAMPLE.to_ascii_lowercase();
        for word in [
            "password",
            "clientsecret",
            "client_secret",
            "apikey:",
            "privatekey",
            "bearer ey",
            "-----begin",
            "ghp_",
            "github_pat_",
        ] {
            assert!(!text.contains(word), "{word}");
        }
    }

    #[test]
    fn the_version_is_required_and_must_be_one() {
        let error = refused("listen: { health: 0.0.0.0:1 }\n");
        assert!(error.contains("version"), "{error}");
        for version in ["0", "2", "-1"] {
            let error = refused(&format!("version: {version}\n"));
            assert!(
                error.contains("version") && error.contains('1'),
                "{version}: {error}"
            );
        }
    }

    #[test]
    fn a_key_the_file_does_not_have_stops_the_operator_and_names_it() {
        // At the top, in a block, and in a block of the CRD types below `runPods`.
        let error = refused(&format!("{}typo: x\n", minimal()));
        assert!(error.contains("typo"), "{error}");
        let error = refused(&format!(
            "{}watch: {{ namespaces: [], typo: x }}\n",
            minimal()
        ));
        assert!(error.contains("typo"), "{error}");
        let error = refused(&format!(
            "{}listen: {{ health: 0.0.0.0:1, typo: x }}\n",
            minimal()
        ));
        assert!(error.contains("typo"), "{error}");
        let error = refused(&format!(
            "{}telemetry: {{ otlp: {{ typo: x }} }}\n",
            minimal()
        ));
        assert!(error.contains("typo"), "{error}");
        let error = refused(&with_run_pods(|run_pods| {
            run_pods["typo"] = serde_json::json!(1);
        }));
        assert!(error.contains("typo"), "{error}");
        let error = refused(&with_run_pods(|run_pods| {
            run_pods["quota"]["typo"] = serde_json::json!(1);
        }));
        assert!(error.contains("typo"), "{error}");
    }

    #[test]
    fn a_secret_is_a_reference_and_never_a_value() {
        // A string where the reference belongs: refused, and the value is not echoed (it may be the token).
        let error = refused(&format!(
            "{}registry: {{ token: s3cret-token }}\n",
            minimal()
        ));
        assert!(error.contains("token"), "{error}");
        assert!(!error.contains("s3cret-token"), "{error}");
        // A reference that names neither a file nor a variable, and one that names both.
        for bad in ["{ path: /x }", "{ file: /x, env: NAME }", "{}"] {
            let error = refused(&format!("{}registry: {{ token: {bad} }}\n", minimal()));
            assert!(
                error.contains("token") || error.contains("file") || error.contains("env"),
                "{bad}: {error}"
            );
        }
        // The two references.
        let s = settings(&format!(
            "{}registry: {{ token: {{ file: /var/run/secrets/aap/registry/token }} }}\n",
            minimal()
        ));
        assert!(matches!(s.registry.token, Some(SecretRef::File(_))));
        let s = settings(&format!(
            "{}registry: {{ token: {{ env: AAP_REGISTRY_TOKEN }} }}\n",
            minimal()
        ));
        assert!(matches!(s.registry.token, Some(SecretRef::Env(_))));
    }

    #[test]
    fn the_default_image_needs_no_white_space_and_the_runtime_class_is_a_name() {
        let error = refused(&format!(
            "{}defaults: {{ agentImage: 'ghcr.io/x/y:tag with space' }}\n",
            minimal()
        ));
        assert!(error.contains("defaults.agentImage"), "{error}");
        let error = refused(&format!(
            "{}defaults: {{ agentRuntimeClass: 'Not A Class' }}\n",
            minimal()
        ));
        assert!(error.contains("defaults.agentRuntimeClass"), "{error}");
        // Empty is none, which is what a chart value left blank says.
        let s = settings(&format!(
            "{}defaults: {{ agentImage: '', agentRuntimeClass: '  ' }}\n",
            minimal()
        ));
        assert!(s.defaults.agent_image.is_none() && s.defaults.agent_runtime_class.is_none());
    }

    #[test]
    fn the_namespaces_are_dns_labels_each_given_once() {
        for bad in ["Agents", "a.b", "a,b", ""] {
            let error = refused(&format!(
                "{}watch: {{ namespaces: ['{bad}'] }}\n",
                minimal()
            ));
            assert!(error.contains("watch.namespaces"), "{bad}: {error}");
        }
        let error = refused(&format!("{}watch: {{ namespaces: [a, a] }}\n", minimal()));
        assert!(error.contains("twice"), "{error}");
        // Empty is every namespace, which is not the same as a list of one empty name.
        let s = settings(&minimal());
        assert!(s.namespaces.is_none());
    }

    #[test]
    fn two_listeners_on_one_port_are_refused_naming_both_keys() {
        let error = refused(&format!(
            "{}listen: {{ health: 0.0.0.0:9090 }}\n",
            minimal()
        ));
        assert!(
            error.contains("listen.health") && error.contains("listen.metrics"),
            "{error}"
        );
        // The registry's port only counts when a token turns it on.
        settings(&format!(
            "{}listen: {{ registry: 0.0.0.0:8081 }}\n",
            minimal()
        ));
        let error = refused(&format!(
            "{}listen: {{ registry: 0.0.0.0:8081 }}\nregistry: {{ token: {{ file: /t }} }}\n",
            minimal()
        ));
        assert!(
            error.contains("listen.registry") && error.contains("listen.health"),
            "{error}"
        );
    }

    #[test]
    fn the_timers_and_the_concurrency_are_at_least_one() {
        for (key, value) in [
            ("concurrency", 0),
            ("timeoutSecs", 0),
            ("resyncSecs", 0),
            ("resyncPendingSecs", 0),
        ] {
            let error = refused(&format!("{}reconcile: {{ {key}: {value} }}\n", minimal()));
            assert!(error.contains(key), "{key}: {error}");
        }
    }

    #[test]
    fn the_leader_election_needs_a_namespace_and_timings_it_can_honour() {
        // On with no namespace anywhere: refused, naming the key.
        let error = refused(&format!(
            "{}leaderElection: {{ enabled: true }}\n",
            minimal()
        ));
        assert!(error.contains("leaderElection.namespace"), "{error}");
        // The primary is the namespace when none is named.
        let s = settings(&format!(
            "{}watch: {{ namespaces: [agents] }}\nleaderElection: {{ enabled: true }}\n",
            minimal()
        ));
        assert_eq!(s.leader.namespace.as_deref(), Some("agents"));
        // A name that is no name, and timings that are no election.
        let error = refused(&format!(
            "{}watch: {{ namespaces: [agents] }}\nleaderElection: {{ enabled: true, name: 'Not A Name' }}\n",
            minimal()
        ));
        assert!(error.contains("leaderElection.name"), "{error}");
        for (lease, renew, retry) in [(2, 2, 1), (15, 15, 2), (15, 10, 10), (0, 0, 0)] {
            let error = refused(&format!(
                "{}watch: {{ namespaces: [agents] }}\nleaderElection: {{ enabled: true, leaseSecs: {lease}, renewSecs: {renew}, retrySecs: {retry} }}\n",
                minimal()
            ));
            assert!(
                error.contains("retry < renew < lease") || error.contains("at least 1"),
                "{lease}/{renew}/{retry}: {error}"
            );
        }
        // Off: the timings are not read, as the chart leaves them alone with one replica.
        settings(&format!(
            "{}leaderElection: {{ leaseSecs: 0 }}\n",
            minimal()
        ));
    }

    #[test]
    fn the_probes_timeout_is_a_whole_number_of_seconds_from_one_to_sixty() {
        for bad in [0, 61] {
            let error = refused(&format!(
                "{}toolProbe: {{ timeoutSecs: {bad} }}\n",
                minimal()
            ));
            assert!(error.contains("toolProbe.timeoutSecs"), "{bad}: {error}");
        }
        settings(&format!("{}toolProbe: {{ timeoutSecs: 60 }}\n", minimal()));
    }

    #[test]
    fn the_log_filter_is_one_tracing_parses() {
        let error = refused(&format!("{}log: {{ filter: '=' }}\n", minimal()));
        assert!(error.contains("log.filter"), "{error}");
        settings(&format!(
            "{}log: {{ filter: 'info,kube=warn,aap_controller=debug' }}\n",
            minimal()
        ));
    }

    #[test]
    fn the_trace_section_is_one_the_exporter_can_use() {
        // No endpoint: off, which is what an absent section says.
        let s = settings(&format!("{}telemetry: {{ otlp: {{}} }}\n", minimal()));
        assert!(s.telemetry.is_some_and(|t| t.endpoint.is_none()));
        // A URL with a credential, another scheme, no host, a sampler it does not know, a ratio above 1, a resource
        // attribute outside the closed list, a timeout outside 1 to 60000, and a header over plain http.
        let cases: [(&str, &str); 9] = [
            ("endpoint: http://u:p@c:4318/v1/traces", "endpoint"),
            ("endpoint: ftp://c/v1/traces", "endpoint"),
            ("endpoint: c:4318", "endpoint"),
            (
                "endpoint: http://c:4318/v1/traces\n    sampler: jaeger_remote",
                "sampler",
            ),
            (
                "endpoint: http://c:4318/v1/traces\n    sampler: traceidratio\n    samplerRatio: 1.5",
                "samplerRatio",
            ),
            (
                "endpoint: http://c:4318/v1/traces\n    resourceAttributes: { user.email: a }",
                "resourceAttributes",
            ),
            (
                "endpoint: http://c:4318/v1/traces\n    timeout: 60001",
                "timeout",
            ),
            (
                "endpoint: http://c:4318/v1/traces\n    headers: { authorization: { file: /h } }",
                "headers",
            ),
            (
                "endpoint: http://c:4318/v1/traces\n    serviceName: 'Not A Name'",
                "serviceName",
            ),
        ];
        for (body, key) in cases {
            let error = refused(&format!("{}telemetry:\n  otlp:\n    {body}\n", minimal()));
            assert!(
                error.contains(&format!("telemetry.otlp.{key}")),
                "{body}: {error}"
            );
        }
        // The accepted edge: https with a header, a ratio of 1, the whole resource, the longest timeout.
        settings(&format!(
            "{}telemetry:\n  otlp:\n    endpoint: https://c:4318/v1/traces\n    headers: {{ authorization: {{ file: /h }} }}\n    sampler: traceidratio\n    samplerRatio: 1\n    timeout: 60000\n    resourceAttributes: {{ deployment.environment.name: prod, k8s.cluster.name: c1.eu, service.namespace: aap }}\n",
            minimal()
        ));
    }

    #[test]
    fn the_run_pods_settings_are_the_charts_and_their_checks_are_named() {
        // The chart's block, as the chart renders it, is a file the operator takes.
        let s = settings(&with_run_pods(|_| {}));
        let run_pods = s.run_pods.expect("run pods");
        assert_eq!(run_pods.class_names(), ["standard"]);
        assert_eq!(run_pods.priority_class, "aap-operator-ns-run");
        // A mistake in it names the key, as `RunPodSettings::validate` does.
        let error = refused(&with_run_pods(|run_pods| {
            run_pods["quota"]["pods"] = serde_json::json!(0);
        }));
        assert!(error.contains("runPods.quota.pods"), "{error}");
        // A file from before the egress proxy, or with a switch the policy lost, is not the settings.
        let error = refused(&with_run_pods(|run_pods| {
            run_pods
                .as_object_mut()
                .expect("a map")
                .remove("egressProxy");
        }));
        assert!(error.contains("egressProxy"), "{error}");
        let error = refused(&with_run_pods(|run_pods| {
            run_pods["networkPolicy"]["enabled"] = serde_json::json!(false);
        }));
        assert!(error.contains("enabled"), "{error}");
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("aap-load-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_that_cannot_be_read_is_exit_1_and_one_that_is_refused_is_78() {
        let dir = temp("codes");
        // Cannot be read: a missing path, and a directory.
        let missing = load(&dir.join("nope.yaml")).unwrap_err();
        assert!(matches!(missing, LoadError::Unreadable(_)), "{missing}");
        assert_eq!(missing.exit_code(), 1);
        let directory = load(&dir).unwrap_err();
        assert!(matches!(directory, LoadError::Unreadable(_)), "{directory}");
        assert_eq!(directory.exit_code(), 1);
        // Refused: invalid YAML, an unknown key, a wrong version.
        for (name, text, fragment) in [
            ("yaml.yaml", "version: [1\n", "not valid"),
            ("key.yaml", "version: 1\nregistry:\n  tokne: x\n", "tokne"),
            ("version.yaml", "version: 2\n", "version"),
        ] {
            let file = dir.join(name);
            std::fs::write(&file, text).unwrap();
            let error = load(&file).unwrap_err();
            assert!(matches!(error, LoadError::Refused(_)), "{name}: {error}");
            assert_eq!(error.exit_code(), 78, "{name}");
            assert!(error.to_string().contains(fragment), "{name}: {error}");
        }
        // Not UTF-8: read, and refused for its content.
        let binary = dir.join("binary.yaml");
        std::fs::write(&binary, b"version: 1\n# \xff\xfe\n").unwrap();
        let error = load(&binary).unwrap_err();
        assert!(matches!(error, LoadError::Refused(_)), "{error}");
        assert_eq!(error.exit_code(), 78);
        assert!(error.to_string().contains("UTF-8"), "{error}");
        // Unreadable by mode: skipped under root, which reads anything.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file = dir.join("mode000.yaml");
            std::fs::write(&file, "version: 1\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
            if std::fs::read_to_string(&file).is_err() {
                let error = load(&file).unwrap_err();
                assert!(matches!(error, LoadError::Unreadable(_)), "{error}");
                assert_eq!(error.exit_code(), 1);
            } else {
                eprintln!("skipped the mode 000 case: this process can read it (root)");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_is_not_there_says_which() {
        let error = format!(
            "{:#}",
            load(Path::new("/nonexistent/operator.yaml")).unwrap_err()
        );
        assert!(error.contains("/nonexistent/operator.yaml"), "{error}");
    }

    #[test]
    fn the_exit_code_of_a_refused_file_is_ex_config() {
        assert_eq!(EXIT_CONFIG, 78);
    }
}
