//! What a composition root sets.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

/// Whether this operator serves an agent registry (S7). It decides the `Listed` condition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegistryMode {
    /// No registry is served: `Listed` is `False`, reason `RegistryDisabled`.
    #[default]
    Disabled,
    /// A registry lists the services the directory says to list.
    Enabled,
}

/// How long to wait before looking at a service again, by what the pass found. A signal (a change
/// of an object, an id from `RuntimeProvider::watch`) makes the controller look sooner; these are the
/// timer that keeps the controller honest when a signal is lost ("a notification is never the truth").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resync {
    /// The service is `Ready` or `Suspended`.
    pub settled: Duration,
    /// A rollout is in progress, the runtime is not well, the name is held by an object that is
    /// not ours (that object is not watched, so the controller has to look), or an object the config
    /// references is not there (the person is probably applying it, and the watch can be lost).
    pub pending: Duration,
    /// The store's backend is not installed: someone has to install it, which is no event of ours.
    pub not_installed: Duration,
}

impl Default for Resync {
    fn default() -> Self {
        Self {
            settled: Duration::from_secs(300),
            pending: Duration::from_secs(15),
            not_installed: Duration::from_secs(60),
        }
    }
}

/// How long one pass over one object may take before it is cut short (`Options::pass_timeout`). A pass is
/// a handful of requests that an API server answers in milliseconds; a minute is long for any of them.
pub(crate) const PASS_TIMEOUT: Duration = Duration::from_secs(60);

/// Whether the operator probes a `ToolProvider`'s MCP server, and the deadline of one probe (K3b, AD-053).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Probe {
    /// Off (the default): no request is made and the status has no `Reachable` condition.
    pub enabled: bool,
    /// How long one probe may take before it is `Reachable: False`, reason `Timeout`.
    pub timeout: Duration,
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout: Duration::from_secs(5),
        }
    }
}

/// The settings of [`Operator`](crate::Operator).
#[derive(Clone, Debug)]
pub struct Options {
    /// The namespaces to watch (K6b): `Some(list)` is exactly those, each reconciled by controllers and caches of
    /// its own, and `None` is every namespace (one `Api::all`, for an administrator). **`Some(empty)` is no shard
    /// at all**: it reconciles nothing and `/readyz` never turns true — it is *not* `None` (the composition root
    /// refuses one, and a duplicate). Every name is a DNS label (the composition root checks).
    pub watch_namespaces: Option<Vec<String>>,
    /// Whether a registry is served.
    pub registry: RegistryMode,
    /// Set by the registry while it refuses to list because its document would pass a limit of the
    /// contract (500 items, 1 MiB): every service that would have been listed is then `Listed: False`,
    /// reason `RegistryFull`. The composition root shares this one flag with the registry (the controller
    /// depends on no registry crate); a pass reads it, so a change reaches a service at its next pass.
    /// Meaningless with [`RegistryMode::Disabled`].
    pub registry_full: Arc<AtomicBool>,
    /// The timers.
    pub resync: Resync,
    /// Services reconciled at the same time (never the same one twice).
    pub concurrency: u16,
    /// The `reportingInstance` of the Events: the pod's name, so two operators can be told apart.
    pub instance: Option<String>,
    /// The longest one pass over one object may take. A pass that is not done by then is dropped (what it had
    /// asked of the API server is cancelled), fails with [`Error::TimedOut`](crate::Error::TimedOut), a
    /// transient error, and is tried again after the error policy's wait. Without a limit a request that is
    /// never answered holds the object's slot, so that no signal and no timer reaches it, for as long as the
    /// request lives (*seen 2026-10-16*: up to the 290 s of a watch; see the README, "A pass that hangs").
    pub pass_timeout: Duration,
    /// The image of an agent whose `AgentConfig` names none (`--default-agent-image`; AD-029, §60a).
    /// `None`: the operator has no default, and such a config is `ConfigInvalid`. A change of it is a
    /// new digest, and so a rollout, for exactly the agents that name no image.
    pub default_agent_image: Option<String>,
    /// The `RuntimeClass` of every agent's own pod (`defaults.agentRuntimeClass`, K14a, AD-052): the deployment's, `None` for
    /// none. A change of it is a new digest, and so a rollout, for every agent.
    pub agent_runtime_class: Option<String>,
    /// What the deployment says about run pods (the file's `runPods`; slice D15a): the size classes, the image, the
    /// quota, the network policy. `None`: this operator has no run pods, and a config that asks for them is
    /// `ConfigInvalid`, saying so. Changing it is a new digest, and so a rollout, for exactly the agents that
    /// ask for run pods and read what changed.
    pub run_pods: Option<aap_domain::RunPodSettings>,
    /// The claims the deployment allows as a package cache (`CACHE_ALLOWED_CLAIMS`, K1). Empty: none.
    pub cache_claims: std::collections::BTreeSet<String>,
    /// The ceiling of the access tier (`AGENT_MAX_ACCESS_TIER`, K1).
    pub max_access_tier: aap_api::AccessTierCap,
    /// The MCP probe of a `ToolProvider` (K3b): off by default.
    pub probe: Probe,
    /// How long data a deleted agent left behind (`deletionPolicy: Retain`) is kept before the operator deletes it:
    /// 90 days by default (`RETENTION_AFTER_DELETE`).
    pub retention: crate::Retention,
    /// How often the idle loop asks agents what work they hold (AD-058). It runs only when the operator is given
    /// an activity source ([`Operator::with_activity`](crate::Operator::with_activity)).
    pub sleep_poll: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            watch_namespaces: None,
            registry: RegistryMode::Disabled,
            registry_full: Arc::new(AtomicBool::new(false)),
            resync: Resync::default(),
            concurrency: 4,
            pass_timeout: PASS_TIMEOUT,
            instance: None,
            default_agent_image: None,
            agent_runtime_class: None,
            run_pods: None,
            cache_claims: std::collections::BTreeSet::new(),
            max_access_tier: aap_api::AccessTierCap::Auto,
            probe: Probe::default(),
            retention: crate::Retention::default(),
            sleep_poll: crate::sleeper::DEFAULT_POLL,
        }
    }
}
