//! The two controllers, wired to their triggers (§59a, "Reconciliation").
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

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aap_api::{AgentConfig, AgentService, ModelEndpoint, ToolProvider};
use aap_ports::{RuntimeProvider, StoreProvisioner};
use futures::{Stream, StreamExt};
use kube::runtime::controller::Config as ControllerConfig;
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
    /// Both reflectors have synced.
    pub fn is_ready(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Say it is. [`Operator::run`] does when both caches have synced; a test of something that
    /// reads the handle can do it by hand.
    pub fn mark_ready(&self) {
        self.0.store(true, Ordering::Release);
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
    services: Option<Controller<AgentService>>,
}

impl<R, S> Operator<R, S>
where
    R: RuntimeProvider + 'static,
    S: StoreProvisioner + 'static,
{
    /// Compose the controllers. Nothing runs, and nothing is asked of the cluster, until
    /// [`run`](Self::run).
    pub fn new(client: Client, runtime: R, store: S, owner: OwnerOf, options: Options) -> Self {
        let services = Controller::new(
            api::<AgentService>(&client, &options),
            watcher::Config::default(),
        );
        Self {
            client,
            runtime,
            store,
            owner,
            options,
            metrics: Arc::new(Metrics::new()),
            readiness: Readiness::default(),
            tracer: aap_trace::Tracer::off(),
            services: Some(services),
        }
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

    /// The counters, for `/metrics`.
    pub fn metrics(&self) -> Arc<Metrics> {
        self.metrics.clone()
    }

    /// Whether the controllers have synced, for `/readyz`.
    pub fn readiness(&self) -> Readiness {
        self.readiness.clone()
    }

    /// The agents of the cluster as the services controller's cache has them: the
    /// [`AgentDirectory`](aap_ports::AgentDirectory) of the registry (S7).
    pub fn directory(&self) -> ReflectorDirectory {
        ReflectorDirectory::new(self.reader())
    }

    fn reader(&self) -> Store<AgentService> {
        // `services` is only taken by `run`, which consumes `self`.
        self.services
            .as_ref()
            .map(Controller::store)
            .unwrap_or_else(|| kube::runtime::reflector::store().0)
    }

    /// Run both controllers until `shutdown` resolves, then let the passes in flight finish.
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
        let namespace = self.options.watch_namespace.clone();
        let cfg = ControllerConfig::default().concurrency(self.options.concurrency);

        // The configs come first: the services' watches of the shared objects map through their cache.
        let configs_controller = Controller::new(
            api::<AgentConfig>(&self.client, &self.options),
            watcher::Config::default(),
        )
        .with_config(cfg.clone())
        .graceful_shutdown_on(signal(stopped.clone()));
        let config_reader = configs_controller.store();

        // The services.
        let Some(services) = self.services.take() else {
            return;
        };
        let reader = services.store();
        let ctx = Arc::new(
            Context::new(
                self.client.clone(),
                self.runtime,
                self.store,
                self.owner,
                self.options.clone(),
            )
            .with_metrics(metrics.clone())
            .with_tracer(self.tracer.clone()),
        );
        let signals = {
            let metrics = metrics.clone();
            let namespace = namespace.clone();
            ctx.runtime
                .watch()
                .filter(move |id| {
                    metrics.inc(name::RUNTIME_SIGNALS, &[]);
                    futures::future::ready(namespace.as_deref().is_none_or(|ns| ns == id.scope()))
                })
                .map(|id| ObjectRef::<AgentService>::new(id.name()).within(id.scope()))
        };
        let mapper_reader = reader.clone();
        let (endpoint_services, endpoint_configs) = (reader.clone(), config_reader.clone());
        let (provider_services, provider_configs) = (reader.clone(), config_reader.clone());
        let services = services
            .with_config(cfg.clone())
            .graceful_shutdown_on(signal(stopped.clone()))
            .watches(
                api::<AgentConfig>(&self.client, &self.options),
                watcher::Config::default(),
                move |config| services_of(&mapper_reader, &config),
            )
            .watches_stream(
                isolated(api::<ModelEndpoint>(&self.client, &self.options)),
                move |endpoint| {
                    services_of_endpoint(&endpoint_services, &endpoint_configs, &endpoint)
                },
            )
            .watches_stream(
                isolated(api::<ToolProvider>(&self.client, &self.options)),
                move |provider| {
                    services_of_provider(&provider_services, &provider_configs, &provider)
                },
            )
            .reconcile_on(signals)
            .run(reconcile_service, service_error_policy, ctx)
            .for_each(|result| async move { log_plumbing("agentservice", result) });

        // The configs.
        let config_ctx = Arc::new(
            ConfigContext::new(self.client.clone(), metrics.clone())
                .with_pass_timeout(self.options.pass_timeout)
                .with_tracer(self.tracer.clone()),
        );
        let configs = configs_controller
            .run(reconcile_config, config_error_policy, config_ctx)
            .for_each(|result| async move { log_plumbing("agentconfig", result) });

        // The status of the shared kinds (K2). One cache of the configs and one of the services for both controllers, each
        // fed by one watch whose changes go to the two controllers after the cache has them.
        let (shared_configs, config_events) =
            reflected(api::<AgentConfig>(&self.client, &self.options));
        let (shared_services, service_events) =
            reflected(api::<AgentService>(&self.client, &self.options));
        let (feed_configs, [endpoint_config_events, provider_config_events]) = tee(config_events);
        let (feed_services, [endpoint_service_events, provider_service_events]) =
            tee(service_events);
        let feeds = until_stopped(
            "the shared kinds' watches",
            GRACE,
            async {
                futures::join!(feed_configs, feed_services);
            },
            signal(stopped.clone()),
        );

        let endpoints = {
            // The object's own watch is isolated too: a kind whose CRD is absent must not keep the controller from
            // stopping (its own failing watch would hold the shutdown until the process is killed).
            let (own, own_events) = reflected(api::<ModelEndpoint>(&self.client, &self.options));
            let controller = Controller::for_stream(own_events, own.clone())
                .with_config(cfg.clone())
                .graceful_shutdown_on(signal(stopped.clone()));
            let ctx = Arc::new(
                SharedContext::new(
                    self.client.clone(),
                    metrics.clone(),
                    shared_configs.clone(),
                    shared_services.clone(),
                )
                .with_pass_timeout(self.options.pass_timeout)
                .with_tracer(self.tracer.clone())
                .with_probe(self.options.probe),
            );
            let (own_c, services_c, configs_s) =
                (own.clone(), shared_services.clone(), shared_configs.clone());
            let run = controller
                .watches_stream(endpoint_config_events, move |config| {
                    touched_by_config(&config, &services_c, &own_c, endpoint_of_config)
                })
                .watches_stream(endpoint_service_events, move |service| {
                    touched_by_service(&service, &configs_s, &own, endpoint_of_config)
                })
                .run(reconcile_endpoint, endpoint_error_policy, ctx)
                .for_each(|result| async move { log_plumbing("modelendpoint", result) });
            until_stopped("modelendpoint", GRACE, run, signal(stopped.clone()))
        };
        let providers = {
            let (own, own_events) = reflected(api::<ToolProvider>(&self.client, &self.options));
            let controller = Controller::for_stream(own_events, own.clone())
                .with_config(cfg.clone())
                .graceful_shutdown_on(signal(stopped.clone()));
            let ctx = Arc::new(
                SharedContext::new(
                    self.client.clone(),
                    metrics.clone(),
                    shared_configs.clone(),
                    shared_services.clone(),
                )
                .with_pass_timeout(self.options.pass_timeout)
                .with_tracer(self.tracer.clone())
                .with_probe(self.options.probe),
            );
            let (own_c, services_c, configs_s) =
                (own.clone(), shared_services.clone(), shared_configs.clone());
            let run = controller
                .watches_stream(provider_config_events, move |config| {
                    touched_by_config(&config, &services_c, &own_c, providers_of_config)
                })
                .watches_stream(provider_service_events, move |service| {
                    touched_by_service(&service, &configs_s, &own, providers_of_config)
                })
                .run(reconcile_provider, provider_error_policy, ctx)
                .for_each(|result| async move { log_plumbing("toolprovider", result) });
            until_stopped("toolprovider", GRACE, run, signal(stopped.clone()))
        };

        // Ready once both caches have listed the cluster.
        let ready = self.readiness.clone();
        let synced = async move {
            if reader.wait_until_ready().await.is_ok()
                && config_reader.wait_until_ready().await.is_ok()
            {
                ready.mark_ready();
                tracing::info!("the caches are synced");
            }
        };
        futures::join!(services, configs, endpoints, providers, feeds, synced);
    }
}

/// How long a shared controller may take to stop once it has been told to.
const GRACE: Duration = Duration::from_secs(5);

/// `run` until it ends, or until `stopped` has resolved and `grace` more has passed.
///
/// kube's `Controller::run` does not look at its shutdown trigger until the cache of its own kind has listed the
/// cluster, so a controller of a kind whose CRD is not installed would hold the shutdown for ever (*seen 2026-11-14
/// in `kinds_whose_crds_are_not_installed_stall_no_service`*). The status passes of the shared kinds (K2) are
/// idempotent and gate nothing, so after the grace one that is still running is dropped. **That is all it covers**: it
/// also hides any other reason why a controller did not stop (a pass that hangs, a stuck stream), which is why the
/// expiry of the grace is a warning that names the controller, never a silent end.
async fn until_stopped(
    controller: &str,
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

/// A stream of the changes a [`tee`] passes on.
type Trigger<K> = Pin<Box<dyn Stream<Item = Result<K, watcher::Error>> + Send>>;

fn receiving<K: Send + 'static>(rx: mpsc::UnboundedReceiver<K>) -> Trigger<K> {
    Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|object| (Ok(object), rx))
    }))
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

fn api<K>(client: &Client, options: &Options) -> Api<K>
where
    K: kube::Resource<Scope = kube::core::NamespaceResourceScope, DynamicType = ()>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
{
    match &options.watch_namespace {
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
            Duration::from_secs(5),
            futures::future::pending::<()>(),
            futures::future::ready(()),
        )
        .await;
        let seen = warnings.0.lock().unwrap().clone();
        assert_eq!(seen.len(), 1, "{seen:?}");
        assert!(seen[0].contains("controller=\"modelendpoint\""), "{seen:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_controller_that_stops_within_its_grace_or_before_the_shutdown_says_nothing() {
        let warnings = Warnings::default();
        let _guard = tracing::subscriber::set_default(warnings.clone());
        // It ends one second after the shutdown.
        until_stopped(
            "toolprovider",
            Duration::from_secs(5),
            async { tokio::time::sleep(Duration::from_secs(1)).await },
            futures::future::ready(()),
        )
        .await;
        // It ends before the shutdown is asked for.
        until_stopped(
            "toolprovider",
            Duration::from_secs(5),
            futures::future::ready(()),
            futures::future::pending::<()>(),
        )
        .await;
        assert!(warnings.0.lock().unwrap().is_empty());
    }
}
