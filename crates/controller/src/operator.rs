//! The two controllers, wired to their triggers (§59a, "Reconciliation"), one set per namespace watched (K6b).
//!
//! The `AgentService` controller is triggered by
//!
//! * its own object (kube-rs's `Controller`, which also owns the reflector the directory reads);
//! * a change of an `AgentConfig`, mapped to every service in that namespace that names it;
//! * a change or a deletion of a `ModelEndpoint` or a `ToolProvider` (AD-029), mapped through the configs
//!   that reference it to the services that run them. These two watches are **isolated** ([`isolated`]):
//!   kube's `Controller` puts one backoff around all its triggers, so a watch that cannot list its kind
//!   (the CRD is not installed, or the `Role` has no `list` on it) would throttle every trigger, the
//!   services' own included, and no agent would be reconciled while it lasts;
//! * [`RuntimeProvider::watch`]: the ids of runtimes that changed, mapped to the service of the same
//!   scope and name, so the controller never watches a workload itself;
//! * a timer per pass ([`Resync`](crate::Resync)), because a signal is never the truth.
//!
//! Two more controllers write the status of the shared kinds (K2, [`crate::shared`]): one for `ModelEndpoint`, one for
//! `ToolProvider`. They share **one** reflector of `AgentConfig` and one of `AgentService` ([`reflected`], [`tee`]): the
//! cache is written before a change is passed on, so the cache a pass reads is never behind the trigger that woke it. What
//! the status costs beyond the services' own watches is those two watches and their caches, and the two of the shared
//! kinds themselves. They gate nothing: `/readyz` does not wait for them and no service waits for a status. The services'
//! watches of the shared kinds ignore a change that is only the status ([`generation_changes`]): the operator's own write
//! must not reconcile every service that uses the object.
//!
//! Not a trigger: a change of a referenced Secret. The operator has no right on Secrets (AD-024), so
//! it cannot watch them; the kubelet's word about a pod is how a missing one is learned, and that
//! arrives through the runtime's watch.
//!
//! # One shard per namespace (K6b)
//!
//! An operator that watches a list of namespaces ([`Options::watch_namespaces`]) holds **one [`Shard`] per namespace**:
//! its own controllers, its own caches, and its own `Api::namespaced`. Never `Api::all` and never a `ClusterRole`, so a
//! namespaced `Role` in each namespace is all the rights it needs (AD-055).
//!
//! The shards must not share a cache: kube 4.0.0's `reflektor` **swaps its store** (*verified 2026-11-18*, `kube-runtime-4.0.0/src/reflector/store.rs`) when the listing that precedes a watch
//! is done (`InitDone`), so two watches writing into one store would have the second listing replace the first's objects.
//! The directory is the union of the shards' caches ([`ReflectorDirectory::from_stores`]), the counters are one set, and
//! `/readyz` turns true only when **every** shard's caches have listed what they watch: an operator whose second
//! namespace is not readable is not ready, and the namespace that is still reconciles.
//!
//! [`RuntimeProvider::watch`] is called **once** and its ids are fanned out ([`fan_out`]): each call is a set of watches
//! of the API server, so one per shard would be one set too many. Each shard keeps the ids of the namespaces it watches.
//!
//! # Leader election (K7a)
//!
//! [`Operator::with_leader`] gives the operator the election this replica is a candidate of. Every pass of the four
//! controllers asks it first: a replica that is not the holder returns `Ok(Action::requeue(`[`FOLLOWER_RECHECK`]`))`
//! and writes nothing, while the holder writes. The caches, the watches, `/readyz` and the registry are the same on
//! both, which is what makes a second replica worth running. The campaign itself is the composition root's
//! ([`LeaderElection::run`](aap_ports::LeaderElection::run)): this module only asks.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aap_api::{AgentConfig, AgentService, ModelEndpoint, ToolProvider};
use aap_ports::{AlwaysLeader, LeaderElection, RuntimeId, RuntimeProvider, StoreProvisioner};
use futures::stream::BoxStream;
use futures::{FutureExt, Stream, StreamExt};
use kube::runtime::controller::{Action, Config as ControllerConfig};
use kube::runtime::reflector::{self, ObjectRef, Store};
use kube::runtime::{Controller, WatchStreamExt, watcher};
use kube::{Api, Client, Resource, ResourceExt};
use tokio::sync::{mpsc, watch};

use crate::config::{config_error_policy, reconcile_config};
use crate::context::{ConfigContext, Context, OwnerOf};
use crate::directory::ReflectorDirectory;
use crate::metrics::{Metrics, name};
use crate::options::Options;
use crate::service::{reconcile_service, service_error_policy};
use crate::shared::{
    SharedContext, endpoint_error_policy, provider_error_policy, reconcile_endpoint,
    reconcile_provider, touched_by_config, touched_by_service,
};
use crate::watches::{
    endpoint_of_config, providers_of_config, services_of_endpoint, services_of_provider,
};

/// A handle that says whether the controllers have listed the cluster. `/readyz` reads it.
#[derive(Clone, Debug, Default)]
pub struct Readiness(Arc<AtomicBool>);

impl Readiness {
    /// Every cache of every namespace has synced.
    pub fn is_ready(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Say it is. [`Operator::run`] does when every cache has synced; a test of something that
    /// reads the handle can do it by hand.
    pub fn mark_ready(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// The controllers of one namespace the operator watches (K6b): its own cache of the services, and the
/// namespaced `Api`s every kind of that shard is read and watched through.
struct Shard {
    /// `None`: every namespace (one `Api::all`, for an administrator).
    namespace: Option<String>,
    services: Controller<AgentService>,
}

impl Shard {
    /// The namespace, for a log line: the name, or `(all)`.
    fn name(&self) -> &str {
        self.namespace.as_deref().unwrap_or("(all)")
    }
}

/// The controllers of the v0 operator, over a runtime provider and a store provisioner.
pub struct Operator<R, S> {
    client: Client,
    runtime: R,
    store: S,
    owner: OwnerOf,
    options: Options,
    metrics: Arc<Metrics>,
    readiness: Readiness,
    tracer: aap_trace::Tracer,
    /// Who holds the election this replica is a candidate of (K7a): only the leader's passes write.
    leader: Arc<dyn LeaderElection>,
    /// One per namespace watched, in the order of [`Options::watch_namespaces`]; empty once [`run`](Self::run)
    /// has taken them.
    shards: Vec<Shard>,
    /// Where the idle loop asks what work an agent holds (AD-058); none: no agent sleeps.
    activity: Option<crate::ActivityFn>,
    sleepers: Arc<crate::Sleepers>,
    /// The time of a pass, when a test fixes it.
    clock: Option<crate::Clock>,
    /// The pod-minute meter's sink and account source (ANR-673); none: no meter.
    meter: Option<(crate::SinkFn, crate::OutcomesFn, crate::AccountFn)>,
    meter_config: crate::MeterConfig,
    /// Where the meter keeps the facts of vanished pods (ANR-1198); none: in memory only.
    meter_tombstones: Option<crate::TombstonesFn>,
}

impl<R, S> Operator<R, S>
where
    R: RuntimeProvider + Clone + 'static,
    S: StoreProvisioner + Clone + 'static,
{
    /// Compose the controllers. Nothing runs, and nothing is asked of the cluster, until
    /// [`run`](Self::run).
    pub fn new(client: Client, runtime: R, store: S, owner: OwnerOf, options: Options) -> Self {
        let shards = watched_namespaces(&options)
            .into_iter()
            .map(|namespace| Shard {
                services: Controller::new(
                    api_in::<AgentService>(&client, namespace.as_deref()),
                    watcher::Config::default(),
                ),
                namespace,
            })
            .collect();
        Self {
            client,
            runtime,
            store,
            owner,
            options,
            metrics: Arc::new(Metrics::new()),
            readiness: Readiness::default(),
            tracer: aap_trace::Tracer::off(),
            leader: Arc::new(AlwaysLeader::default()),
            shards,
            activity: None,
            sleepers: Arc::new(crate::Sleepers::default()),
            clock: None,
            meter: None,
            meter_config: crate::MeterConfig::default(),
            meter_tombstones: None,
        }
    }

    /// Turn the pod-minute meter on (ANR-673, AD-069): while this replica leads, it sends one event per pod and
    /// whole minute to `sink`, for the account `accounts` names. **Off without it**, and `operator run` does not
    /// call it yet (a later slice, AD-069).
    #[must_use]
    pub fn with_meter<K, A>(mut self, sink: K, accounts: A) -> Self
    where
        K: aap_usage::UsageSink + 'static,
        A: aap_usage::AccountSource + 'static,
    {
        let (send, outcomes) = crate::erase_sink_with_outcomes(sink);
        self.meter = Some((send, outcomes, crate::erase_accounts(accounts)));
        self
    }

    /// Keep the meter's facts of vanished pods in `store` (ANR-1198), so a crash or a failover loses no minute of a
    /// pod that is gone. [`KubeTombstones`](crate::KubeTombstones) is the one for a cluster. Without it the facts
    /// are in memory only. Has no effect unless [`with_meter`](Self::with_meter) is called.
    #[must_use]
    pub fn with_meter_tombstones<T: aap_usage::TombstoneStore + 'static>(
        mut self,
        store: T,
    ) -> Self {
        self.meter_tombstones = Some(crate::erase_tombstones(store));
        self
    }

    /// The meter's source name, node label key and period (see [`MeterConfig`](crate::MeterConfig)).
    #[must_use]
    pub fn with_meter_config(mut self, config: crate::MeterConfig) -> Self {
        self.meter_config = config;
        self
    }

    /// Let agents that set `scaling.idleTimeout` sleep (AD-058, P2b): the idle loop asks `source` what each holds
    /// and scales to zero the ones that report no work for that long. **Off without it**, whatever the objects say.
    #[must_use]
    pub fn with_activity<A: aap_ports::ActivitySource + 'static>(mut self, source: A) -> Self {
        self.activity = Some(crate::erase_activity(source));
        self
    }

    /// Whether the idle loop has an activity source ([`with_activity`](Self::with_activity)): what a composition root's
    /// test asks, so that a source built and never handed over is caught.
    #[must_use]
    pub fn has_activity(&self) -> bool {
        self.activity.is_some()
    }

    /// Elect `leader`'s holder as the one whose passes write (K7a). The default is [`AlwaysLeader`]: one
    /// replica, and every pass writes.
    ///
    /// A replica that is not the leader still runs every controller and keeps every cache, and still serves
    /// `/readyz` and the registry; a pass it is woken for returns
    /// `Ok(Action::requeue(`[`FOLLOWER_RECHECK`]`))` and writes nothing, so it looks again within the
    /// election's own patience rather than waiting for a change of an object that may not come.
    #[must_use]
    pub fn with_leader(mut self, leader: Arc<dyn LeaderElection>) -> Self {
        self.leader = leader;
        self
    }

    /// The election this replica is a candidate of.
    pub fn leader(&self) -> &Arc<dyn LeaderElection> {
        &self.leader
    }

    /// Make a span of every pass with `tracer` (K5, §48a). Off by default.
    #[must_use]
    pub fn with_tracer(mut self, tracer: aap_trace::Tracer) -> Self {
        self.tracer = tracer;
        self
    }

    /// The tracer the controllers will make their spans with.
    #[must_use]
    pub fn tracer(&self) -> &aap_trace::Tracer {
        &self.tracer
    }

    /// Take the time of every `AgentService` pass from `clock` (a scheduled wake is judged by it). The real clock by
    /// default; this is for a test that travels in time.
    #[must_use]
    pub fn with_clock(mut self, clock: crate::Clock) -> Self {
        self.clock = Some(clock);
        self
    }

    /// The counters, for `/metrics`.
    pub fn metrics(&self) -> Arc<Metrics> {
        self.metrics.clone()
    }

    /// Whether the controllers have synced, for `/readyz`.
    pub fn readiness(&self) -> Readiness {
        self.readiness.clone()
    }

    /// The agents of the cluster as the services controllers' caches have them: the
    /// [`AgentDirectory`](aap_ports::AgentDirectory) of the registry (S7). One cache per namespace watched, and
    /// the directory is their union.
    pub fn directory(&self) -> ReflectorDirectory {
        ReflectorDirectory::from_stores(self.shards.iter().map(|s| s.services.store()).collect())
    }

    /// Run every shard's controllers until `shutdown` resolves, then let the passes in flight finish.
    pub async fn run(mut self, shutdown: impl Future<Output = ()> + Send + 'static) {
        let (stop, stopped) = watch::channel(false);
        tokio::spawn(async move {
            shutdown.await;
            // Every receiver may be gone already: nothing is left to stop.
            let _ = stop.send(true);
        });
        let signal = |mut rx: watch::Receiver<bool>| async move {
            // A closed channel is a stopped one.
            let _ = rx.wait_for(|stop| *stop).await;
        };

        let metrics = self.metrics.clone();
        let cfg = ControllerConfig::default().concurrency(self.options.concurrency);
        let shards = std::mem::take(&mut self.shards);

        // One watch of the provider for every shard: a call is a set of watches of the API server, and one
        // per shard would be one set too many. Every signal is counted once, whatever its namespace.
        let (feed_signals, signals): (Pin<Box<dyn Future<Output = ()> + Send>>, Vec<_>) =
            if shards.is_empty() {
                // No shard: ask the provider for nothing at all (its empty list means every namespace), and wait for
                // the stop. The readiness below stays false.
                (Box::pin(futures::future::pending()), Vec::new())
            } else {
                let counted = self.runtime.watch().map({
                    let metrics = metrics.clone();
                    move |id| {
                        metrics.inc(name::RUNTIME_SIGNALS, &[]);
                        id
                    }
                });
                // The idle loop's proposals to sleep ask for a pass of the reconcile, which writes the record.
                let counted = futures::stream::select(counted, received(self.sleepers.nudges()));
                let (feed, signals) = fan_out(counted, shards.len());
                (Box::pin(feed), signals)
            };

        let mut running: Vec<Pin<Box<dyn Future<Output = ()> + Send>>> = Vec::new();
        let mut service_caches: Vec<Store<AgentService>> = Vec::new();
        let mut config_caches: Vec<Store<AgentConfig>> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for (shard, signals) in shards.into_iter().zip(signals) {
            names.push(shard.name().to_owned());
            // The configs come first: the services' watches of the shared objects map through their cache.
            let configs_controller = Controller::new(
                api_in::<AgentConfig>(&self.client, shard.namespace.as_deref()),
                watcher::Config::default(),
            )
            .with_config(cfg.clone())
            .graceful_shutdown_on(signal(stopped.clone()));
            service_caches.push(shard.services.store());
            config_caches.push(configs_controller.store());
            let mut context = Context::new(
                self.client.clone(),
                self.runtime.clone(),
                self.store.clone(),
                self.owner.clone(),
                self.options.clone(),
            )
            .with_metrics(metrics.clone())
            .with_tracer(self.tracer.clone())
            .with_sleepers(self.sleepers.clone());
            if let Some(clock) = &self.clock {
                context = context.with_clock(clock.clone());
            }
            let ctx = Arc::new(context);
            let config_ctx = Arc::new(
                ConfigContext::new(self.client.clone(), metrics.clone())
                    .with_pass_timeout(self.options.pass_timeout)
                    .with_tracer(self.tracer.clone()),
            );
            running.push(Box::pin(run_shard(
                shard,
                configs_controller,
                ctx,
                config_ctx,
                signals,
                metrics.clone(),
                self.tracer.clone(),
                self.leader.clone(),
                stopped.clone(),
                cfg.clone(),
            )));
        }
        // The provider's watch has no end of its own: it ends when the operator is told to stop, and its readers with
        // it (a controller's `reconcile_on` stream ending stops nothing by itself: the graceful shutdown does).
        let feed = {
            let stopped = stopped.clone();
            async move {
                let stop = signal(stopped);
                let _ = futures::future::select(std::pin::pin!(feed_signals), std::pin::pin!(stop))
                    .await;
            }
        };
        running.push(Box::pin(feed));
        // The retention sweep: one task for every namespace watched (an empty scope is every namespace).
        let scopes = self
            .options
            .watch_namespaces
            .clone()
            .unwrap_or_else(|| vec![String::new()]);
        let scopes_for_sleep = scopes.clone();
        if !scopes.is_empty() {
            running.push(Box::pin(crate::retention::run(
                self.runtime.clone(),
                self.store.clone(),
                scopes,
                self.options.retention,
                self.leader.clone(),
                metrics.clone(),
                stopped.clone(),
            )));
        }

        if let (Some((sink, outcomes, accounts)), false) =
            (self.meter.clone(), scopes_for_sleep.is_empty())
        {
            let period = self.meter_config.period;
            running.push(Box::pin(crate::meter::run(
                crate::meter::MeterLoop {
                    client: self.client.clone(),
                    meter: Arc::new({
                        let leader = self.leader.clone();
                        let meter = crate::Meter::new(
                            sink,
                            accounts,
                            self.meter_config.clone(),
                            metrics.clone(),
                        )
                        .with_outcomes(outcomes)
                        .with_continue(move || leader.is_leader());
                        match self.meter_tombstones.clone() {
                            Some(store) => meter.with_tombstones(store),
                            None => meter,
                        }
                    }),
                    scopes: scopes_for_sleep.clone(),
                    leader: self.leader.clone(),
                },
                period,
                stopped.clone(),
            )));
        }

        if let (Some(activity), false) = (self.activity.clone(), scopes_for_sleep.is_empty()) {
            running.push(Box::pin(crate::sleeper::run(
                crate::sleeper::IdleLoop {
                    client: self.client.clone(),
                    activity,
                    sleepers: self.sleepers.clone(),
                    scopes: scopes_for_sleep,
                    period: self.options.sleep_poll,
                    mark_max_age: crate::wake::mark_max_age(
                        self.options.sleep_poll,
                        self.options.pass_timeout,
                    ),
                    clock: self.clock.clone().unwrap_or_else(crate::system_clock),
                    leader: self.leader.clone(),
                },
                self.runtime.clone(),
                stopped.clone(),
            )));
        }

        // Ready once **every** shard's caches have listed what they watch: an operator whose second namespace
        // cannot be read is not ready, and the namespace that can still reconciles. A cache that never lists (a
        // namespace the `Role` does not allow, a namespace that does not exist) leaves this waiting: the shutdown
        // is what ends it, not a cache. Every 30 s the namespaces still waiting are named, because otherwise the
        // only symptom is a failing readiness probe.
        let ready = self.readiness.clone();
        let synced = {
            let stopped = stopped.clone();
            async move {
                let all = async {
                    if service_caches.is_empty() || config_caches.is_empty() {
                        tracing::warn!(
                            "no namespace to watch: the operator reconciles nothing and is never ready"
                        );
                        return;
                    }
                    for reader in &service_caches {
                        if reader.wait_until_ready().await.is_err() {
                            return;
                        }
                    }
                    for reader in &config_caches {
                        if reader.wait_until_ready().await.is_err() {
                            return;
                        }
                    }
                    ready.mark_ready();
                    tracing::info!(namespaces = names.join(","), "the caches are synced");
                };
                tokio::pin!(all);
                let mut tick = tokio::time::interval(Duration::from_secs(30));
                tick.tick().await;
                loop {
                    tokio::select! {
                        () = &mut all => break,
                        () = signal(stopped.clone()) => break,
                        _ = tick.tick() => {
                            let mut missing: Vec<&str> = Vec::new();
                            for (i, store) in service_caches.iter().enumerate() {
                                if store.wait_until_ready().now_or_never().is_none() {
                                    missing.push(names[i].as_str());
                                }
                            }
                            for (i, store) in config_caches.iter().enumerate() {
                                if store.wait_until_ready().now_or_never().is_none() {
                                    missing.push(names[i].as_str());
                                }
                            }
                            missing.sort_unstable();
                            missing.dedup();
                            if !missing.is_empty() {
                                tracing::warn!(
                                    namespaces = missing.join(","),
                                    "these namespaces' caches have not listed: /readyz stays false, and the registry is unready for every namespace"
                                );
                            }
                        }
                    }
                }
            }
        };
        running.push(Box::pin(synced));
        futures::future::join_all(running).await;
    }
}

/// Whether the shard watching `namespace` (or every namespace, `None`) takes the runtime signal `id` (K6b): a shard
/// takes the ids of the namespaces it watches and no others, so one change of the provider's watch is one pass in
/// the shard that owns it, not one pass in every shard.
fn signal_in(namespace: Option<&str>, id: &RuntimeId) -> bool {
    namespace.is_none_or(|ns| ns == id.scope())
}

/// The namespaces to watch: one per name of [`Options::watch_namespaces`], or one for every namespace.
fn watched_namespaces(options: &Options) -> Vec<Option<String>> {
    match &options.watch_namespaces {
        // An empty list is no shard at all: it reconciles nothing and `/readyz` never turns true. It must not fall
        // back to `None` (every namespace, which is `Api::all` and a `ClusterRole` the chart does not make): that
        // would turn a caller's mistake into a cluster-wide operator. The composition root refuses one.
        Some(list) => {
            // A duplicate is one namespace, not two shards (which would make two entries of one id, and the registry
            // lists neither). The composition root refuses one; a library caller is deduped here.
            let mut seen = std::collections::BTreeSet::new();
            list.iter()
                .filter(|name| seen.insert(name.as_str()))
                .cloned()
                .map(Some)
                .collect()
        }
        None => vec![None],
    }
}

/// Everything one namespace runs: the services, the configs, the two status controllers of the shared kinds,
/// and the feeds their caches are read from.
#[allow(clippy::too_many_arguments)]
async fn run_shard<R, S>(
    shard: Shard,
    configs_controller: Controller<AgentConfig>,
    ctx: Arc<Context<R, S>>,
    config_ctx: Arc<ConfigContext>,
    signals: BoxStream<'static, RuntimeId>,
    metrics: Arc<Metrics>,
    tracer: aap_trace::Tracer,
    leader: Arc<dyn LeaderElection>,
    stopped: watch::Receiver<bool>,
    cfg: ControllerConfig,
) where
    R: RuntimeProvider + 'static,
    S: StoreProvisioner + 'static,
{
    let signal = |mut rx: watch::Receiver<bool>| async move {
        // A closed channel is a stopped one.
        let _ = rx.wait_for(|stop| *stop).await;
    };
    let scope = shard.name().to_owned();
    let namespace = shard.namespace.clone();
    let client = ctx.client.clone();
    let config_reader = configs_controller.store();
    // What the status controllers of the shared kinds need of the settings, taken before the services' controller
    // takes the context.
    let (pass_timeout, probe) = (ctx.options.pass_timeout, ctx.options.probe);

    // The services.
    let services = shard.services;
    let reader = services.store();
    let signals = signals
        .filter({
            let namespace = namespace.clone();
            move |id| futures::future::ready(signal_in(namespace.as_deref(), id))
        })
        .map(|id| ObjectRef::<AgentService>::new(id.name()).within(id.scope()));
    let mapper_reader = reader.clone();
    let (endpoint_services, endpoint_configs) = (reader.clone(), config_reader.clone());
    let (provider_services, provider_configs) = (reader.clone(), config_reader.clone());
    let service_leader = leader.clone();
    let services = services
        .with_config(cfg.clone())
        .graceful_shutdown_on(signal(stopped.clone()))
        .watches(
            api_in::<AgentConfig>(&client, namespace.as_deref()),
            watcher::Config::default(),
            move |config| services_of(&mapper_reader, &config),
        )
        .watches_stream(
            isolated(api_in::<ModelEndpoint>(&client, namespace.as_deref())),
            move |endpoint| services_of_endpoint(&endpoint_services, &endpoint_configs, &endpoint),
        )
        .watches_stream(
            isolated(api_in::<ToolProvider>(&client, namespace.as_deref())),
            move |provider| services_of_provider(&provider_services, &provider_configs, &provider),
        )
        .reconcile_on(signals)
        .run(
            move |svc, ctx| {
                let leader = service_leader.clone();
                async move {
                    if !leader.is_leader() {
                        return Ok(Action::requeue(FOLLOWER_RECHECK));
                    }
                    reconcile_service(svc, ctx).await
                }
            },
            service_error_policy,
            ctx,
        )
        .for_each(|result| async move { log_plumbing("agentservice", result) });
    // A controller whose cache never lists — a namespace the `Role` does not allow — may not reach its shutdown trigger;
    // the grace bounds it either way, with a warning that names it and the namespace.
    let services = until_stopped(
        "agentservice",
        &scope,
        GRACE,
        services,
        signal(stopped.clone()),
    );

    // The configs.
    let config_leader = leader.clone();
    let configs = configs_controller
        .run(
            move |config, ctx| {
                let leader = config_leader.clone();
                async move {
                    if !leader.is_leader() {
                        return Ok(Action::requeue(FOLLOWER_RECHECK));
                    }
                    reconcile_config(config, ctx).await
                }
            },
            config_error_policy,
            config_ctx,
        )
        .for_each(|result| async move { log_plumbing("agentconfig", result) });
    let configs = until_stopped(
        "agentconfig",
        &scope,
        GRACE,
        configs,
        signal(stopped.clone()),
    );

    // The status of the shared kinds (K2). One cache of the configs and one of the services for both controllers, each
    // fed by one watch whose changes go to the two controllers after the cache has them.
    let (shared_configs, config_events) =
        reflected(api_in::<AgentConfig>(&client, namespace.as_deref()));
    let (shared_services, service_events) =
        reflected(api_in::<AgentService>(&client, namespace.as_deref()));
    let (feed_configs, [endpoint_config_events, provider_config_events]) = tee(config_events);
    let (feed_services, [endpoint_service_events, provider_service_events]) = tee(service_events);
    let feeds = until_stopped(
        "the shared kinds' watches",
        &scope,
        GRACE,
        async {
            futures::join!(feed_configs, feed_services);
        },
        signal(stopped.clone()),
    );

    let endpoints = {
        // The object's own watch is isolated too: a kind whose CRD is absent must not keep the controller from
        // stopping (its own failing watch would hold the shutdown until the process is killed).
        let (own, own_events) = reflected(api_in::<ModelEndpoint>(&client, namespace.as_deref()));
        let controller = Controller::for_stream(own_events, own.clone())
            .with_config(cfg.clone())
            .graceful_shutdown_on(signal(stopped.clone()));
        let ctx = Arc::new(
            SharedContext::new(
                client.clone(),
                metrics.clone(),
                shared_configs.clone(),
                shared_services.clone(),
            )
            .with_pass_timeout(pass_timeout)
            .with_tracer(tracer.clone())
            .with_probe(probe),
        );
        let (own_c, services_c, configs_s) =
            (own.clone(), shared_services.clone(), shared_configs.clone());
        let endpoint_leader = leader.clone();
        let run = controller
            .watches_stream(endpoint_config_events, move |config| {
                touched_by_config(&config, &services_c, &own_c, endpoint_of_config)
            })
            .watches_stream(endpoint_service_events, move |service| {
                touched_by_service(&service, &configs_s, &own, endpoint_of_config)
            })
            .run(
                move |endpoint, ctx| {
                    let leader = endpoint_leader.clone();
                    async move {
                        if !leader.is_leader() {
                            return Ok(Action::requeue(FOLLOWER_RECHECK));
                        }
                        reconcile_endpoint(endpoint, ctx).await
                    }
                },
                endpoint_error_policy,
                ctx,
            )
            .for_each(|result| async move { log_plumbing("modelendpoint", result) });
        until_stopped("modelendpoint", &scope, GRACE, run, signal(stopped.clone()))
    };
    let providers = {
        let (own, own_events) = reflected(api_in::<ToolProvider>(&client, namespace.as_deref()));
        let controller = Controller::for_stream(own_events, own.clone())
            .with_config(cfg.clone())
            .graceful_shutdown_on(signal(stopped.clone()));
        let ctx = Arc::new(
            SharedContext::new(
                client.clone(),
                metrics.clone(),
                shared_configs.clone(),
                shared_services.clone(),
            )
            .with_pass_timeout(pass_timeout)
            .with_tracer(tracer.clone())
            .with_probe(probe),
        );
        let (own_c, services_c, configs_s) =
            (own.clone(), shared_services.clone(), shared_configs.clone());
        let provider_leader = leader.clone();
        let run = controller
            .watches_stream(provider_config_events, move |config| {
                touched_by_config(&config, &services_c, &own_c, providers_of_config)
            })
            .watches_stream(provider_service_events, move |service| {
                touched_by_service(&service, &configs_s, &own, providers_of_config)
            })
            .run(
                move |provider, ctx| {
                    let leader = provider_leader.clone();
                    async move {
                        if !leader.is_leader() {
                            return Ok(Action::requeue(FOLLOWER_RECHECK));
                        }
                        reconcile_provider(provider, ctx).await
                    }
                },
                provider_error_policy,
                ctx,
            )
            .for_each(|result| async move { log_plumbing("toolprovider", result) });
        until_stopped("toolprovider", &scope, GRACE, run, signal(stopped.clone()))
    };

    futures::join!(services, configs, endpoints, providers, feeds);
}

/// How long a shared controller may take to stop once it has been told to.
const GRACE: Duration = Duration::from_secs(5);

/// How soon a pass that found this replica is not the leader looks again (K7a). The election's own changes are
/// a stream ([`LeaderElection::following`]), but a controller's passes are woken by the cluster's objects: a
/// replica that becomes the leader must start writing without waiting for one to change, and one that stops
/// being it must stop, so both look again within this.
pub const FOLLOWER_RECHECK: Duration = Duration::from_secs(30);

/// `run` until it ends, or until `stopped` has resolved and `grace` more has passed.
///
/// kube's `Controller::run` may not look at its shutdown trigger until the cache of its own kind has listed the
/// cluster, so a controller of a kind whose CRD is not installed would hold the shutdown for ever (*seen 2026-11-14
/// in `kinds_whose_crds_are_not_installed_stall_no_service`*). The status passes of the shared kinds (K2) are
/// idempotent and gate nothing, so after the grace one that is still running is dropped. **That is all it covers**: it
/// also hides any other reason why a controller did not stop (a pass that hangs, a stuck stream), which is why the
/// expiry of the grace is a warning that names the controller and the namespace, never a silent end.
async fn until_stopped(
    controller: &str,
    namespace: &str,
    grace: Duration,
    run: impl Future<Output = ()>,
    stopped: impl Future<Output = ()>,
) {
    let mut run = Box::pin(run);
    let stopped = Box::pin(stopped);
    if let futures::future::Either::Left(_) = futures::future::select(run.as_mut(), stopped).await {
        return;
    }
    if tokio::time::timeout(grace, run).await.is_err() {
        tracing::warn!(
            controller,
            namespace,
            grace_seconds = grace.as_secs(),
            "the controller did not stop within its grace after the shutdown and was dropped: either its kind's CRD is not installed (its cache never listed the cluster) or something in it hangs"
        );
    }
}

/// The two ends of one stream of changes, for two controllers: what `events` yields goes to both, in order. `events`
/// is a [`reflected`] stream, which writes its cache **before** it yields, so a pass that a copy wakes reads a cache that
/// has the change. The returned future drives the stream and must be polled; it ends when the stream does.
fn tee<K>(
    events: impl Stream<Item = Result<K, watcher::Error>> + Send + 'static,
) -> (impl Future<Output = ()>, [Trigger<K>; 2])
where
    K: Clone + Send + 'static,
{
    let ((first, a), (second, b)) = (mpsc::unbounded_channel(), mpsc::unbounded_channel());
    let feed = async move {
        let mut events = std::pin::pin!(events);
        while let Some(item) = events.next().await {
            if let Ok(object) = item {
                // A controller that has stopped has dropped its end: nothing is lost by that.
                let _ = first.send(object.clone());
                let _ = second.send(object);
            }
        }
    };
    (feed, [receiving(a), receiving(b)])
}

/// One stream's items, to every one of `count` readers, in order: the runtime provider's signals, once per
/// namespace (K6b). The returned future drives the stream and must be polled; it ends when the stream does.
fn fan_out<T>(
    items: impl Stream<Item = T> + Send + 'static,
    count: usize,
) -> (impl Future<Output = ()>, Vec<BoxStream<'static, T>>)
where
    T: Clone + Send + 'static,
{
    let (senders, streams): (Vec<_>, Vec<_>) = (0..count)
        .map(|_| {
            let (tx, rx) = mpsc::unbounded_channel();
            (tx, received(rx))
        })
        .unzip();
    let feed = async move {
        let mut items = std::pin::pin!(items);
        while let Some(item) = items.next().await {
            // A reader that has stopped has dropped its end: nothing is lost by that.
            for tx in &senders {
                let _ = tx.send(item.clone());
            }
        }
    };
    (feed, streams)
}

/// A stream of the changes a [`tee`] passes on.
type Trigger<K> = Pin<Box<dyn Stream<Item = Result<K, watcher::Error>> + Send>>;

fn receiving<K: Send + 'static>(rx: mpsc::UnboundedReceiver<K>) -> Trigger<K> {
    Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|object| (Ok(object), rx))
    }))
}

/// A stream of the items a [`fan_out`] passes on.
fn received<T: Send + 'static>(rx: mpsc::UnboundedReceiver<T>) -> BoxStream<'static, T> {
    futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    })
    .boxed()
}

/// A watch's events with the **changes that are only a status** left out: an object whose `generation` is the one this
/// stream last saw for its `uid` is not passed on. A deletion always is, and so is an object with no generation (an API
/// server that gives none), and the first sight of any. A CRD with a status subresource moves the generation with the spec
/// and not with the status, so this is what makes the operator's own status writes on the shared kinds reconcile nobody
/// (K2): one flip of `AgentsReady` under a fleet would otherwise be a pass for every service that names the object.
fn generation_changes<K>(
    events: impl Stream<Item = Result<watcher::Event<K>, watcher::Error>> + Send + 'static,
) -> impl Stream<Item = Result<watcher::Event<K>, watcher::Error>> + Send + 'static
where
    K: Resource<DynamicType = ()> + Send + 'static,
{
    use watcher::Event;
    let mut seen: HashMap<String, i64> = HashMap::new();
    events.filter(move |item| {
        let keep = match item {
            Ok(Event::Apply(o) | Event::InitApply(o)) => {
                match (o.meta().uid.clone(), o.meta().generation) {
                    (Some(uid), Some(generation)) => {
                        seen.insert(uid, generation) != Some(generation)
                    }
                    _ => true,
                }
            }
            Ok(Event::Delete(o)) => {
                if let Some(uid) = &o.meta().uid {
                    seen.remove(uid);
                }
                true
            }
            Ok(Event::Init | Event::InitDone) | Err(_) => true,
        };
        futures::future::ready(keep)
    })
}

/// What a controller's stream says besides a finished pass. A pass's own failure was logged by the error
/// policy; a request for an object that is gone is expected. A watch that breaks is not silent: with the CRDs
/// not installed, or no right to list them, that is all an operator would ever say.
fn log_plumbing<T>(
    controller: &str,
    result: Result<T, kube::runtime::controller::Error<crate::Error, watcher::Error>>,
) {
    use kube::runtime::controller::Error as Plumbing;
    match result {
        Ok(_) | Err(Plumbing::ReconcilerFailed(..)) => {}
        Err(Plumbing::ObjectNotFound(object)) => tracing::debug!(controller, "{object} is gone"),
        Err(e @ (Plumbing::QueueError(_) | Plumbing::RunnerError(_))) => {
            tracing::warn!(
                controller,
                "{e}: {}",
                std::error::Error::source(&e).map_or_else(String::new, ToString::to_string)
            );
        }
    }
}

/// The changes and deletions of a shared kind, as a trigger that **cannot fail**.
///
/// `Controller::run` puts a single `StreamBackoff` around the merge of all its triggers, so an error from
/// one of them delays them all. A watch of a kind whose CRD is not installed (the operator is rolled out
/// before `deploy/operator-crds`, or the CRDs chart is older) fails its list with a 404 for as long as
/// that lasts, and so does one the `Role` does not allow (403): left in the merge, it held every service
/// back, each for a backoff's length, thirty seconds and more once the backoff had grown (*measured
/// 2026-10-15 with kube-runtime 4.0.0 on the tests' fake API server, in virtual time: a service that
/// existed at start was Ready after 11 to 16 s, one created later after 27 to 42 s, and with this
/// function at once; the fake answers instantly, so a real API server is no faster*). Here the watcher takes its own backoff,
/// which paces both its retries and the warning, and an error is logged and dropped. The watch finds the
/// kind when it appears: its next list answers.
///
/// Deletions stay in (`touched_objects`), which is what makes a deleted referent block its services at
/// once. A change that is only the status is left out ([`generation_changes`]).
fn isolated<K>(api: Api<K>) -> impl Stream<Item = Result<K, watcher::Error>> + Send + 'static
where
    K: Resource<DynamicType = ()>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug
        + Send
        + 'static,
{
    quiet(generation_changes(watcher(api, watcher::Config::default())))
}

/// A cache of a kind, and the changes and deletions that feed it, as a trigger that cannot fail ([`isolated`]'s rules).
/// The cache is written **before** the change is yielded, which is why the shared controllers (K2) use it and not a second
/// watch beside the cache of the services controller: a pass woken by the change reads a cache that has it.
fn reflected<K>(
    api: Api<K>,
) -> (
    Store<K>,
    impl Stream<Item = Result<K, watcher::Error>> + Send + 'static,
)
where
    K: Resource<DynamicType = ()>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug
        + Send
        + Sync
        + 'static,
{
    let (store, writer) = reflector::store();
    (
        store,
        quiet(reflector::reflector(
            writer,
            watcher(api, watcher::Config::default()),
        )),
    )
}

/// The backoff, the touched objects, and the errors logged and dropped, of [`isolated`].
fn quiet<K, S>(events: S) -> impl Stream<Item = Result<K, watcher::Error>> + Send + 'static
where
    K: Resource<DynamicType = ()> + Clone + std::fmt::Debug + Send + 'static,
    S: Stream<Item = Result<watcher::Event<K>, watcher::Error>> + Send + 'static,
{
    events
        .default_backoff()
        .touched_objects()
        .filter_map(|item| async move {
            match item {
                Ok(object) => Some(Ok(object)),
                Err(error) => {
                    tracing::warn!(
                        kind = %K::kind(&()),
                        "cannot watch the kind, so a change of its objects will not reach the agents that use them until the next timer \
                         (is the CRD of deploy/operator-crds installed, and may the operator's Role list and watch it?): {error}"
                    );
                    None
                }
            }
        })
}

fn api_in<K>(client: &Client, namespace: Option<&str>) -> Api<K>
where
    K: kube::Resource<Scope = kube::core::NamespaceResourceScope, DynamicType = ()>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    }
}

/// The services of a namespace that run `config`.
fn services_of(
    services: &Store<AgentService>,
    config: &AgentConfig,
) -> Vec<ObjectRef<AgentService>> {
    let (Some(ns), name) = (config.namespace(), config.name_any()) else {
        return Vec::new();
    };
    services
        .state()
        .iter()
        .filter(|s| s.namespace().as_deref() == Some(ns.as_str()) && s.spec.config_ref.name == name)
        .map(|s| ObjectRef::from_obj(&**s))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fmt::Debug;
    use std::sync::Mutex;

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    use super::*;

    /// Every warning the code under test logs, as `field=value` text.
    #[derive(Clone, Default)]
    struct Warnings(Arc<Mutex<Vec<String>>>);

    struct Fields(String);
    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
            self.0.push_str(&format!("{}={value:?} ", field.name()));
        }
    }

    impl Subscriber for Warnings {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            if *event.metadata().level() == tracing::Level::WARN {
                let mut fields = Fields(String::new());
                event.record(&mut fields);
                self.0.lock().unwrap().push(fields.0);
            }
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    #[tokio::test(start_paused = true)]
    async fn a_controller_that_does_not_stop_within_its_grace_is_dropped_with_a_warning_that_names_it()
     {
        let warnings = Warnings::default();
        let _guard = tracing::subscriber::set_default(warnings.clone());
        until_stopped(
            "modelendpoint",
            "team",
            Duration::from_secs(5),
            futures::future::pending::<()>(),
            futures::future::ready(()),
        )
        .await;
        let seen = warnings.0.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].contains("controller=\"modelendpoint\""), "{seen:?}");
        assert!(seen[0].contains("namespace=\"team\""), "{seen:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_controller_that_stops_within_its_grace_or_before_the_shutdown_says_nothing() {
        let warnings = Warnings::default();
        let _guard = tracing::subscriber::set_default(warnings.clone());
        // It ends one second after the shutdown.
        until_stopped(
            "toolprovider",
            "(all)",
            Duration::from_secs(5),
            async { tokio::time::sleep(Duration::from_secs(1)).await },
            futures::future::ready(()),
        )
        .await;
        // It ends before the shutdown is asked for.
        until_stopped(
            "toolprovider",
            "(all)",
            Duration::from_secs(5),
            futures::future::ready(()),
            futures::future::pending::<()>(),
        )
        .await;
        assert!(warnings.0.lock().unwrap().is_empty());
    }

    /// K6b: a shard takes the runtime signals of the namespaces it watches, and one that watches every namespace
    /// takes them all: without it every signal would be a pass in every shard.
    #[test]
    fn a_shard_takes_the_runtime_signals_of_the_namespaces_it_watches_only() {
        assert!(signal_in(Some("team"), &RuntimeId::new("team", "chat")));
        assert!(
            !signal_in(Some("team"), &RuntimeId::new("other", "chat")),
            "another namespace's change is not this shard's"
        );
        assert!(signal_in(None, &RuntimeId::new("other", "chat")));
    }

    /// K6b: the namespaces of the settings, as the shards are built from them.
    #[test]
    fn one_shard_per_namespace_and_one_for_every_namespace_without_a_list() {
        assert_eq!(watched_namespaces(&Options::default()), [None]);
        let empty = Options {
            watch_namespaces: Some(Vec::new()),
            ..Options::default()
        };
        assert!(
            watched_namespaces(&empty).is_empty(),
            "an empty list is no shard, never every namespace"
        );
        let twice = Options {
            watch_namespaces: Some(vec!["a".to_owned(), "a".to_owned()]),
            ..Options::default()
        };
        assert_eq!(
            watched_namespaces(&twice),
            [Some("a".to_owned())],
            "a duplicate is one namespace, not two shards"
        );
        let options = Options {
            watch_namespaces: Some(vec!["a".to_owned(), "b".to_owned()]),
            ..Options::default()
        };
        assert_eq!(
            watched_namespaces(&options),
            [Some("a".to_owned()), Some("b".to_owned())]
        );
    }
}
