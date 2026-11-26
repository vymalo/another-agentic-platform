//! `operator run`: the composition root. The controllers are generic over a runtime provider and a
//! store provisioner (AD-020); this is the one place that names the types.
//!
//! Every setting comes from one configuration file (ANR-962, AD-064), read and validated by [`crate::config`] before this
//! module runs. `POD_NAME`, `POD_NAMESPACE` and `HOSTNAME` are still the environment's: they are the downward API's
//! identity of this replica, not settings of a deployment.

use anyhow::Result;

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

/// The name of this pod, for the `reportingInstance` of the Events and the lease's `holderIdentity`: `POD_NAME`, which the
/// chart sets from the downward API. Not a setting of the configuration file: two replicas with the same identity would
/// renew each other's lease, which is no election at all.
#[cfg(feature = "runtime-kubernetes")]
fn instance() -> Option<String> {
    std::env::var("POD_NAME")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
}

/// The identity this replica writes as the lease's holder (K7a): `POD_NAME`, else `HOSTNAME`, else a refusal.
#[cfg(feature = "runtime-kubernetes")]
fn candidate_id(instance: Option<&str>, hostname: Option<&str>) -> Result<aap_ports::CandidateId> {
    let name = instance
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .or_else(|| hostname.map(str::trim).filter(|name| !name.is_empty()));
    match name {
        Some(name) => Ok(aap_ports::CandidateId::new(name)),
        None => anyhow::bail!(
            "leaderElection.enabled is set, and neither POD_NAME nor HOSTNAME names this replica: the Lease's holderIdentity would be empty, and two replicas would look like one"
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
impl crate::config::Settings {
    /// The election this replica is a candidate of (K7a), from the file's `leaderElection` block: off is `None` and the
    /// operator is an [`AlwaysLeader`](aap_ports::AlwaysLeader); on, the namespace is the block's, else the **primary**
    /// (the first of `watch.namespaces`, where the operator's `Role` is), and the holder identity is `POD_NAME`, else
    /// `HOSTNAME`.
    ///
    /// # Errors
    ///
    /// The file was validated already; this refuses only a replica that cannot name itself.
    fn leader_election(&self) -> Result<Option<LeaderSettings>> {
        if !self.leader.enabled {
            return Ok(None);
        }
        let namespace = self
            .leader
            .namespace
            .clone()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "leaderElection.enabled is set and leaderElection.namespace names no namespace, and watch.namespaces names none either: the Lease needs the namespace the operator's Role is in"
                )
            })?;
        let timings = aap_leader_kubernetes::Timings {
            lease: std::time::Duration::from_secs(self.leader.lease_secs),
            renew: std::time::Duration::from_secs(self.leader.renew_secs),
            retry: std::time::Duration::from_secs(self.leader.retry_secs),
        };
        timings
            .check()
            .map_err(|why| anyhow::anyhow!("leaderElection: {why}"))?;
        let candidate = candidate_id(instance().as_deref(), hostname().as_deref())?;
        Ok(Some(LeaderSettings {
            namespace,
            name: self.leader.name.clone(),
            candidate,
            timings,
        }))
    }

    /// What the controllers are told, from the file.
    fn options(
        &self,
        registry: bool,
        registry_full: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> aap_controller::Options {
        aap_controller::Options {
            watch_namespaces: self.namespaces.clone(),
            registry: if registry {
                aap_controller::RegistryMode::Enabled
            } else {
                aap_controller::RegistryMode::Disabled
            },
            registry_full,
            resync: aap_controller::Resync {
                settled: self.reconcile.resync,
                pending: self.reconcile.resync_pending,
                ..aap_controller::Resync::default()
            },
            concurrency: self.reconcile.concurrency,
            pass_timeout: self.reconcile.timeout,
            instance: instance(),
            default_agent_image: self.defaults.agent_image.clone(),
            agent_runtime_class: self.defaults.agent_runtime_class.clone(),
            run_pods: self.run_pods.clone(),
            cache_claims: self.defaults.cache_allowed_claims.iter().cloned().collect(),
            max_access_tier: self.defaults.max_access_tier,
            retention: aap_controller::Retention {
                after_delete: (self.retention_after_delete_days > 0).then(|| {
                    std::time::Duration::from_secs(
                        self.retention_after_delete_days
                            .saturating_mul(24 * 60 * 60),
                    )
                }),
                ..aap_controller::Retention::default()
            },
            probe: aap_controller::Probe {
                enabled: self.tool_probe.enabled,
                timeout: self.tool_probe.timeout,
            },
            // The binary builds the activity source (P2c) but does not hand it to the operator yet, so the idle loop does not run.
            sleep_poll: aap_controller::DEFAULT_POLL,
        }
    }
}

/// The HTTP activity source (AD-058 P2c), or `None` while `activity.enabled` is off. **Fail closed**: enabled, the bearer is
/// read now and a reference that gives nothing (a missing or unreadable file, an empty or unset variable, a blank text) is an
/// error at start, never a source that asks with no bearer. `--check` does not come here: it holds only the reference's shape.
///
/// # Errors
///
/// The bearer cannot be read or is blank, or the HTTP client cannot be built.
pub fn activity_source(
    activity: &crate::config::Activity,
) -> anyhow::Result<Option<aap_activity_http::HttpActivity>> {
    use aap_trace_otlp::SecretRef;
    use anyhow::Context;
    if !activity.enabled {
        if activity.token.is_some() {
            tracing::warn!(
                "activity.token is set but activity.enabled is not: no agent is asked about its work"
            );
        }
        return Ok(None);
    }
    let reference = activity
        .token
        .as_ref()
        .context("activity.token: required while activity.enabled is true: the agents refuse a request with no bearer")?;
    let refused = |why: String| {
        anyhow::Error::new(crate::config::ReferenceRefused(format!(
            "activity.token {reference:?}: {why}"
        )))
    };
    let text = match reference {
        SecretRef::File(path) => std::fs::read_to_string(path)
            .map_err(|e| refused(format!("the file cannot be read ({})", e.kind())))?,
        SecretRef::Env(name) => {
            std::env::var(name).map_err(|_| refused("the variable is not set".to_owned()))?
        }
    };
    let token = aap_activity_http::Token::new(&text)
        .map_err(|_| refused("the bearer is empty".to_owned()))?;
    let source =
        aap_activity_http::HttpActivity::in_cluster(activity.port, activity.timeout, Some(token))?;
    Ok(Some(source))
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

/// The registry's token. **No token configured, no registry** (fail closed): the operator keeps reconciling, says so loudly,
/// and every service is `Listed: False`, reason `RegistryDisabled`. **A token configured and unusable is a refusal of the
/// file** (exit 78, as `activity.token`'s is): a missing or empty file, an unset or empty variable. It used to be the same as
/// none, and a deployment whose Secret was misnamed then ran for hours with no registry and a single log line (AD-064).
///
/// # Errors
///
/// [`ReferenceRefused`](crate::config::ReferenceRefused): the reference gives no token. The message names the reference, never
/// a value.
#[cfg(all(feature = "runtime-kubernetes", feature = "registry"))]
fn registry_token(registry: &crate::config::Registry) -> Result<Option<aap_registry::Token>> {
    use aap_trace_otlp::SecretRef;
    let Some(reference) = &registry.token else {
        tracing::warn!(
            "no registry.token: the agent registry is not served (it has no token to demand)"
        );
        return Ok(None);
    };
    let token = match reference {
        SecretRef::File(path) => aap_registry::Token::from_file(path).map_err(|e| e.to_string()),
        SecretRef::Env(name) => std::env::var(name)
            .ok()
            .and_then(|text| aap_registry::Token::new(&text))
            .ok_or_else(|| "the variable is not set or is empty".to_owned()),
    };
    token.map(Some).map_err(|why| {
        // The reference, never the value: a file's path and a variable's name are not the token.
        anyhow::Error::new(crate::config::ReferenceRefused(format!(
            "registry.token {reference:?}: {why}"
        )))
    })
}

#[cfg(all(feature = "runtime-kubernetes", not(feature = "registry")))]
fn registry_token(registry: &crate::config::Registry) -> Result<Option<()>> {
    if registry.token.is_some() {
        tracing::warn!(
            "registry.token is set, but this build has no `registry` feature: nothing is served on the registry's port"
        );
    }
    Ok(None)
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

/// `operator run`. The traces are started first, from the file's `telemetry.otlp` section, before the cluster is asked
/// anything. A reference that cannot be read (a headers file, a certificate) never stops the operator: one warning, by
/// setting, and tracing is off. The spans still queued are flushed on every way out that returns, an error included (not
/// when the main future panics).
#[cfg(feature = "runtime-kubernetes")]
pub async fn run(settings: crate::config::Settings) -> Result<()> {
    let started = aap_trace_otlp::start_section(
        "aap-operator",
        env!("CARGO_PKG_VERSION"),
        settings.telemetry.as_ref(),
    );
    started.warn();
    let tracer = started.tracer;
    flushed(&tracer, run_with(settings, tracer.clone())).await
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
async fn run_with(settings: crate::config::Settings, tracer: aap_trace::Tracer) -> Result<()> {
    use std::sync::Arc;

    use aap_api::AgentService;
    use aap_controller::OwnerOf;
    use aap_runtime_kubernetes::owner_handle;
    use anyhow::Context;
    use kube::{Resource, ResourceExt};
    use tokio::sync::watch;

    let leader = settings.leader_election()?;
    let registry_full = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Built and checked now so a bad setting stops the start; the idle controller (P2b) is what polls it.
    let activity = activity_source(&settings.activity)?;
    let token = registry_token(&settings.registry)?;
    let options = settings.options(token.is_some(), registry_full.clone());
    if options.watch_namespaces.is_none() {
        tracing::warn!(
            "watch.namespaces names no namespace: the operator watches every namespace, which needs a ClusterRole this chart does not make"
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
        activity = activity.is_some(),
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
        Some(leader) => {
            let lease = aap_leader_kubernetes::LeaseElection::new(
                client.clone(),
                &leader.namespace,
                &leader.name,
                leader.candidate.clone(),
            )
            .with_timings(leader.timings)
            .map_err(|why| anyhow::anyhow!("the leader election's timings: {why}"))?;
            tracing::info!(
                lease = %format!("{}/{}", leader.namespace, leader.name),
                candidate = leader.candidate.as_str(),
                lease_secs = leader.timings.lease.as_secs(),
                renew_secs = leader.timings.renew.as_secs(),
                retry_secs = leader.timings.retry.as_secs(),
                "leader election on: every replica serves, and only the lease holder's passes write"
            );
            Arc::new(lease)
        }
        None => {
            tracing::info!(
                "no leader election: this replica is the only one, and every pass writes"
            );
            let candidate = candidate_id(instance().as_deref(), hostname().as_deref())
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
        if let Some(anchor) = settings.registry.public_url.clone() {
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
        settings.listen.health,
        crate::serve::health(operator.readiness()),
        signalled(stopped.clone()),
    ));
    let metrics = tokio::spawn(crate::serve::serve(
        settings.listen.metrics,
        crate::serve::metrics_for(operator.metrics(), &tracer),
        signalled(stopped.clone()),
    ));

    // The registry: served on its port, and its document built on a timer besides, so that the flag the
    // controller reads (`RegistryFull`) follows the directory when nobody asks.
    #[cfg(feature = "registry")]
    let registry = tokio::spawn({
        let registry = registry_server.clone();
        let addr = settings.listen.registry;
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
pub async fn run(settings: crate::config::Settings) -> Result<()> {
    let _ = settings;
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

    /// The settings a file gives, as `operator run` reads them.
    fn settings(yaml: &str) -> crate::config::Settings {
        crate::config::Config::parse(yaml)
            .expect("a configuration")
            .validate()
            .expect("a configuration the operator can work with")
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

    /// The namespaces the runtime provider watches are the ones the file gives (K6b), and none is every namespace.
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

    /// K7a: the election is off by default; on, its namespace is the primary when the file names none, and its timings are
    /// the file's. (The candidate is this pod's, which the test of [`candidate_id`] holds; a process has one `POD_NAME`.)
    #[test]
    fn the_leader_election_is_off_by_default_and_takes_the_primary_namespace() {
        let off = settings("version: 1\nwatch: { namespaces: [agents] }\n");
        assert!(!off.leader.enabled, "off: the operator is the only replica");
        assert!(
            off.leader_election()
                .expect("off is not an error")
                .is_none()
        );

        let on = settings(
            "version: 1\nwatch: { namespaces: [agents] }\nleaderElection: { enabled: true }\n",
        );
        assert_eq!(on.leader.name, "aap-operator");
        assert_eq!(
            on.leader.namespace.as_deref(),
            Some("agents"),
            "the primary"
        );
        assert_eq!(
            (
                on.leader.lease_secs,
                on.leader.renew_secs,
                on.leader.retry_secs
            ),
            (15, 10, 2),
            "the client-go defaults, and the ones the chart's values.yaml holds"
        );

        let named = settings(
            "version: 1\nwatch: { namespaces: [agents] }\nleaderElection: { enabled: true, namespace: ops, leaseSecs: 30, renewSecs: 20, retrySecs: 3 }\n",
        );
        assert_eq!(named.leader.namespace.as_deref(), Some("ops"));
        assert_eq!(
            (
                named.leader.lease_secs,
                named.leader.renew_secs,
                named.leader.retry_secs
            ),
            (30, 20, 3)
        );
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

    /// Ruling 5 of ANR-962, and AD-064 (changed on upgrade): `--check` holds a reference's shape, and the start reads it and
    /// fails closed. No `registry.token` is no registry; a token that is configured and cannot be used (a missing or empty file,
    /// an unset or empty variable) refuses the file, which is exit 78, never a registry that demands nothing.
    #[cfg(all(feature = "runtime-kubernetes", feature = "registry"))]
    #[test]
    fn a_registry_token_that_is_configured_and_unusable_refuses_the_file() {
        use aap_trace_otlp::SecretRef;
        let registry = |token: Option<SecretRef>| crate::config::Registry {
            public_url: None,
            token,
        };
        assert!(
            registry_token(&registry(None))
                .expect("none is no registry")
                .is_none(),
            "no reference is no registry, and not a refusal"
        );
        let refused = |token: SecretRef| {
            let error = registry_token(&registry(Some(token))).expect_err("a refusal");
            assert!(crate::config::is_reference_refused(&error), "{error:#}");
            assert!(format!("{error:#}").contains("registry.token"), "{error:#}");
        };
        let dir = std::env::temp_dir().join(format!("aap-registry-token-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let file = dir.join("token");
        refused(SecretRef::File(file.clone())); // missing
        std::fs::write(&file, " \n").expect("write");
        refused(SecretRef::File(file.clone())); // empty
        refused(SecretRef::Env("AAP_TEST_NO_SUCH_REGISTRY_TOKEN".to_owned())); // unset
        std::fs::write(&file, "registry-bearer\n").expect("write");
        assert!(
            registry_token(&registry(Some(SecretRef::File(file))))
                .expect("a readable file")
                .is_some()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_activity_source_is_off_by_default_and_fails_closed_without_a_token() {
        let off = settings("version: 1\n");
        assert!(
            activity_source(&off.activity)
                .expect("off is fine")
                .is_none()
        );

        let dir = std::env::temp_dir().join(format!("aap-activity-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory");
        let file = dir.join("token");
        let on = |token: &str| {
            settings(&format!(
                "version: 1\nactivity:\n  enabled: true\n  token: {token}\n"
            ))
        };

        let text = format!(
            "{:#}",
            activity_source(&on(&format!("{{ file: {} }}", file.display())).activity)
                .expect_err("missing file")
        );
        assert!(text.contains("activity.token"), "{text}");
        assert!(text.contains("cannot be read"), "{text}");

        std::fs::write(&file, " \n").expect("write");
        assert!(
            activity_source(&on(&format!("{{ file: {} }}", file.display())).activity).is_err(),
            "a blank token is refused"
        );

        std::fs::write(&file, "s3cret-bearer\n").expect("write");
        let source = activity_source(&on(&format!("{{ file: {} }}", file.display())).activity)
            .expect("a token")
            .expect("a source");
        assert!(
            !format!("{source:?}").contains("s3cret"),
            "the token never prints"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
