//! `operator run`: the composition root. The controllers are generic over a runtime provider and a
//! store provisioner (AD-020); this is the one place that names the types.

use std::net::SocketAddr;

use anyhow::Result;
use clap::Args;

/// Settings of `run`. Every one is also an environment variable, which is how the chart sets them.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// Watch one namespace (the namespaced operator of §93). Empty or unset: every namespace.
    #[arg(long, env = "WATCH_NAMESPACE")]
    pub watch_namespace: Option<String>,

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

    /// Seconds between looks at a service that is Ready or Suspended (the timer that makes a lost
    /// signal harmless).
    #[arg(long, env = "AAP_RESYNC_SECS", default_value_t = 300)]
    pub resync_secs: u64,

    /// Seconds between looks at a service that is rolling out, unwell, or held by a foreign object.
    #[arg(long, env = "AAP_RESYNC_PENDING_SECS", default_value_t = 15)]
    pub resync_pending_secs: u64,
}

// What `run` composes needs a runtime provider; without one the settings are parsed and unused.
#[cfg(feature = "runtime-kubernetes")]
impl RunArgs {
    /// An empty `WATCH_NAMESPACE` (a chart value left blank) means no namespace.
    fn namespace(&self) -> Option<String> {
        self.watch_namespace
            .clone()
            .filter(|n| !n.trim().is_empty())
    }

    fn options(&self) -> aap_controller::Options {
        aap_controller::Options {
            watch_namespace: self.namespace(),
            // The registry is S7: until then `Listed` says `RegistryDisabled`.
            registry: aap_controller::RegistryMode::Disabled,
            resync: aap_controller::Resync {
                settled: std::time::Duration::from_secs(self.resync_secs),
                pending: std::time::Duration::from_secs(self.resync_pending_secs),
                ..aap_controller::Resync::default()
            },
            concurrency: self.concurrency.max(1),
            instance: self.instance.clone().filter(|i| !i.is_empty()),
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

#[cfg(feature = "runtime-kubernetes")]
pub async fn run(args: RunArgs) -> Result<()> {
    use std::sync::Arc;

    use aap_api::AgentService;
    use aap_controller::{Operator, OwnerOf};
    use aap_runtime_kubernetes::{KubernetesRuntime, owner_handle};
    use anyhow::Context;
    use kube::{Resource, ResourceExt};
    use tokio::sync::watch;

    let options = args.options();
    tracing::info!(
        namespace = options.watch_namespace.as_deref().unwrap_or("(all)"),
        "starting the operator {}",
        env!("CARGO_PKG_VERSION")
    );

    // Everything below is the composition: a Kubernetes runtime, the store, the controllers.
    let client = kube::Client::try_default()
        .await
        .context("connecting to the cluster (in-cluster configuration, or KUBECONFIG)")?;
    let mut runtime = KubernetesRuntime::new(client.clone());
    if let Some(ns) = &options.watch_namespace {
        runtime = runtime.watching_namespace(ns);
    }
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
    let store = store(&client);
    let operator = Operator::new(client, runtime, store, owner, options);

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
        crate::serve::metrics(operator.metrics()),
        signalled(stopped.clone()),
    ));

    let asked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let asked_by_signal = asked.clone();
    let controllers = operator.run(async move {
        termination().await;
        asked_by_signal.store(true, std::sync::atomic::Ordering::Release);
        tracing::info!("shutting down");
    });
    tokio::pin!(controllers);

    // The servers fail fast (a port in use is a deployment mistake); the controllers end when told to.
    let result = tokio::select! {
        () = &mut controllers => {
            if asked.load(std::sync::atomic::Ordering::Acquire) {
                Ok(())
            } else {
                Err(anyhow::anyhow!("the controllers stopped without being asked to"))
            }
        }
        r = health => r.context("the health server task").and_then(|r| r).and(Err(anyhow::anyhow!("the health server stopped"))),
        r = metrics => r.context("the metrics server task").and_then(|r| r).and(Err(anyhow::anyhow!("the metrics server stopped"))),
    };
    // Stop whatever still serves.
    let _ = stop.send(true);
    result
}

#[cfg(not(feature = "runtime-kubernetes"))]
pub async fn run(args: RunArgs) -> Result<()> {
    let _ = args;
    anyhow::bail!(
        "this build has no runtime provider: rebuild with `--features runtime-kubernetes` (the default)"
    )
}
