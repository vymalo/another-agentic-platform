//! The composition root (AD-020): the one place that names the implementations. The router is generic over the two ports;
//! this builds it from the Kubernetes objects and the OIDC authenticator, and serves it.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aap_auth_oidc::OidcAuthenticator;
use aap_forms::Defaults;
use aap_objects_kubernetes::KubernetesObjects;
use aap_platform_api::{ApiState, CorsOrigins, router};
use aap_ports::{AgentObjects, AuthError, Authenticator, SharedKind};
use anyhow::{Context, Result};
use axum::Router;
use clap::Parser;
use tokio::sync::watch;

use crate::config::{self, Settings};
use crate::serve::{self, Check, Health};

/// The settings of `api`. Every one is also an environment variable, which is how the chart sets them.
#[derive(Debug, Parser)]
#[command(
    name = "api",
    version,
    about = "The Platform API of the admin dashboard"
)]
pub struct Args {
    /// The configuration file (`config.example.yaml` documents every key).
    #[arg(long, env = "API_CONFIG_FILE")]
    pub config: PathBuf,

    /// Where the API listens. Wins over `listen` in the file.
    #[arg(long, env = "API_LISTEN_ADDR")]
    pub listen: Option<SocketAddr>,

    /// Where `/healthz` and `/readyz` are served. Wins over `healthListen` in the file.
    #[arg(long, env = "HEALTH_ADDR")]
    pub health_addr: Option<SocketAddr>,

    /// Read the configuration file, say whether the API can work with it, and exit. Needs no cluster and no issuer.
    #[arg(long)]
    pub check: bool,
}

/// How long the requests in flight have to finish after a stop is asked for, kept under Kubernetes' 30 seconds.
const DRAIN: Duration = Duration::from_secs(25);

/// The router over any two ports: the configuration of the deployment, and nothing of how the ports are made.
///
/// # Errors
///
/// The defaults are not usable (they were checked at load; this is the same check where the state is made).
pub fn build_router<O, A>(
    objects: O,
    auth: A,
    defaults: Defaults,
    cors: CorsOrigins,
) -> Result<Router>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let state = ApiState::new(objects, auth, defaults)
        .context("the deployment's defaults")?
        .with_cors(cors)
        .with_version(env!("CARGO_PKG_VERSION"));
    Ok(router(state))
}

/// What `/readyz` asks: the issuer's keys can be had (at once, while a copy can still verify), and the cluster's API server
/// answers a call that needs the API's own rights in its namespace (a `Role` that lacks a verb is not ready).
pub fn readiness<O, K, F>(objects: O, keys: K) -> Check
where
    O: AgentObjects + Clone + 'static,
    K: Fn() -> F + Send + Sync + 'static,
    F: Future<Output = Result<(), AuthError>> + Send + 'static,
{
    Arc::new(move || {
        let objects = objects.clone();
        let keys = keys();
        Box::pin(async move {
            keys.await
                .map_err(|_| "the issuer's keys are not available\n")?;
            objects.list_shared(SharedKind::Model).await.map_err(
                |_| "the cluster's API server does not answer, or refuses the API's rights\n",
            )?;
            Ok(())
        })
    })
}

/// Resolves on SIGTERM or SIGINT.
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

async fn signalled(mut rx: watch::Receiver<bool>) {
    // A dropped sender is a stop as well.
    let _ = rx.wait_for(|stop| *stop).await;
}

/// Serve until told to stop, then drain. The API stops first and the health endpoints last, so that a pod that is going is
/// `not ready` for as long as it takes.
async fn serve_both(
    api: tokio::net::TcpListener,
    router: Router,
    health: tokio::net::TcpListener,
    check: Check,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let draining = Arc::new(AtomicBool::new(false));
    let (stop_api, api_stopped) = watch::channel(false);
    let (stop_health, health_stopped) = watch::channel(false);
    let mut api = tokio::spawn(serve::serve(api, router, signalled(api_stopped)));
    let mut health = tokio::spawn(serve::serve(
        health,
        serve::health(Health {
            draining: draining.clone(),
            check,
        }),
        signalled(health_stopped),
    ));

    // The servers fail fast (a port in use is a deployment mistake); a stop is asked for by the signal.
    let outcome = tokio::select! {
        () = stop => {
            tracing::info!("shutting down");
            Ok(())
        }
        r = &mut api => Err(ended("the API server", r)),
        r = &mut health => Err(ended("the health server", r)),
    };
    draining.store(true, Ordering::Release);
    let _ = stop_api.send(true);
    if !api.is_finished() && tokio::time::timeout(DRAIN, &mut api).await.is_err() {
        tracing::warn!("requests were still in flight after {DRAIN:?}; stopping anyway");
        api.abort();
    }
    let _ = stop_health.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(2), &mut health).await;
    outcome
}

fn ended(what: &str, result: Result<Result<()>, tokio::task::JoinError>) -> anyhow::Error {
    match result {
        Ok(Ok(())) => anyhow::anyhow!("{what} stopped without being asked to"),
        Ok(Err(e)) => e.context(format!("{what} failed")),
        Err(e) => anyhow::Error::new(e).context(format!("{what} panicked")),
    }
}

/// `api`: read the configuration, compose, serve.
pub async fn run(args: Args) -> Result<()> {
    let Settings {
        listen,
        health_listen,
        oidc,
        cors,
        defaults,
    } = config::load(&args.config)?;
    let listen = args.listen.unwrap_or(listen);
    let health_listen = args.health_addr.unwrap_or(health_listen);
    tracing::info!(
        namespace = %defaults.namespace,
        origins = cors.as_slice().len(),
        "starting the Platform API {}",
        env!("CARGO_PKG_VERSION")
    );

    let client = kube::Client::try_default()
        .await
        .context("connecting to the cluster (in-cluster configuration, or KUBECONFIG)")?;
    let objects =
        KubernetesObjects::new(client, &defaults.namespace).with_offer_label(&defaults.offer_label);
    let auth = OidcAuthenticator::new(oidc).context("the token authenticator")?;
    let check = {
        let auth = auth.clone();
        readiness(objects.clone(), move || {
            let auth = auth.clone();
            async move { auth.ready().await }
        })
    };
    let router = build_router(objects, auth, defaults, cors)?;

    // Bound before anything is served: a port in use is found at once.
    let api = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    let health = tokio::net::TcpListener::bind(health_listen)
        .await
        .with_context(|| format!("binding {health_listen}"))?;
    serve_both(api, router, health, check, termination()).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::atomic::AtomicUsize;

    use aap_auth_oidc::testkit::TestAuthenticator;
    use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::config::ApiConfig;

    fn settings() -> Settings {
        ApiConfig::parse(include_str!("../config.example.yaml"))
            .unwrap()
            .validate()
            .unwrap()
    }

    /// One HTTP/1.1 request over a socket, and the status line and body of the answer.
    async fn http(addr: SocketAddr, path: &str, token: Option<&str>) -> (u16, String) {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let auth = token.map_or_else(String::new, |t| format!("Authorization: Bearer {t}\r\n"));
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: api\r\nConnection: close\r\n{auth}\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).await.unwrap();
        let status = answer.split(' ').nth(1).unwrap().parse().unwrap();
        let body = answer
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_owned())
            .unwrap_or_default();
        (status, body)
    }

    async fn bound() -> (tokio::net::TcpListener, SocketAddr) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        (listener, addr)
    }

    #[tokio::test]
    async fn the_router_comes_up_on_a_socket_with_the_memory_ports_and_stops_when_told() {
        let s = settings();
        let objects = MemoryObjects::in_namespace(&s.defaults.namespace);
        let auth = MemoryAuthenticator::new().with_token("t", "alice", ["platform:agents.read"]);
        let router = build_router(objects.clone(), auth, s.defaults, s.cors).unwrap();
        let check = readiness(objects, || async { Ok(()) });

        let (api, api_addr) = bound().await;
        let (health, health_addr) = bound().await;
        let (stop, stopped) = watch::channel(false);
        let running = tokio::spawn(serve_both(api, router, health, check, signalled(stopped)));

        assert_eq!(
            http(api_addr, "/v1/me", Some("t")).await,
            (
                200,
                "{\"permissions\":[\"platform:agents.read\"]}".to_owned()
            )
        );
        assert_eq!(http(api_addr, "/v1/me", None).await.0, 401);
        assert_eq!(http(api_addr, "/v1/agents", Some("t")).await.0, 200);
        assert_eq!(http(api_addr, "/v1/secret-keys", Some("t")).await.0, 403);
        assert_eq!(http(health_addr, "/healthz", None).await.0, 200);
        assert_eq!(http(health_addr, "/readyz", None).await.0, 200);
        assert_eq!(
            http(api_addr, "/healthz", None).await.0,
            404,
            "health is not on the API's port"
        );

        stop.send(true).unwrap();
        running.await.unwrap().unwrap();
        assert!(
            tokio::net::TcpStream::connect(api_addr).await.is_err(),
            "the port is closed"
        );
    }

    #[tokio::test]
    async fn the_router_comes_up_over_a_real_issuer_too() {
        let s = settings();
        let issuer = TestAuthenticator::start().await;
        let token = issuer.token_for("alice", &["platform:agents.read", "user"]);
        let objects = MemoryObjects::in_namespace(&s.defaults.namespace);
        let router =
            build_router(objects, issuer.authenticator().clone(), s.defaults, s.cors).unwrap();
        let (api, api_addr) = bound().await;
        let (health, _) = bound().await;
        let (stop, stopped) = watch::channel(false);
        let check: Check = Arc::new(|| Box::pin(async { Ok(()) }));
        let running = tokio::spawn(serve_both(api, router, health, check, signalled(stopped)));
        assert_eq!(http(api_addr, "/v1/me", Some(&token)).await.0, 200);
        assert_eq!(http(api_addr, "/v1/me", Some("garbage")).await.0, 401);
        stop.send(true).unwrap();
        running.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_server_that_ends_by_itself_ends_the_api_with_the_reason() {
        let stopped = ended("the API server", Ok(Ok(())));
        assert!(
            format!("{stopped:#}").contains("without being asked"),
            "{stopped:#}"
        );
        let failed = ended("the health server", Ok(Err(anyhow::anyhow!("boom"))));
        assert!(
            format!("{failed:#}").contains("the health server failed: boom"),
            "{failed:#}"
        );
    }

    #[tokio::test]
    async fn readiness_needs_the_keys_and_the_cluster() {
        let objects = MemoryObjects::new();
        let keys_up = Arc::new(AtomicBool::new(true));
        let probes = Arc::new(AtomicUsize::new(0));
        let check = {
            let up = keys_up.clone();
            let probes = probes.clone();
            readiness(objects.clone(), move || {
                let up = up.load(Ordering::Acquire);
                probes.fetch_add(1, Ordering::Relaxed);
                async move {
                    if up {
                        Ok(())
                    } else {
                        Err(AuthError::Unavailable("the keys are down".into()))
                    }
                }
            })
        };
        assert_eq!(check().await, Ok(()));
        objects.set_unavailable(true);
        assert_eq!(
            check().await,
            Err("the cluster's API server does not answer, or refuses the API's rights\n")
        );
        objects.set_unavailable(false);
        keys_up.store(false, Ordering::Release);
        assert_eq!(check().await, Err("the issuer's keys are not available\n"));
        assert_eq!(probes.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn the_settings_of_the_command_line_win_over_the_file() {
        let args = Args::try_parse_from([
            "api",
            "--config",
            "/etc/api/config.yaml",
            "--listen",
            "127.0.0.1:9000",
            "--health-addr",
            "127.0.0.1:9001",
        ])
        .unwrap();
        assert_eq!(args.listen, Some("127.0.0.1:9000".parse().unwrap()));
        assert_eq!(args.health_addr, Some("127.0.0.1:9001".parse().unwrap()));
        assert!(!args.check);
        assert!(
            Args::try_parse_from(["api"]).is_err(),
            "the configuration file is required"
        );
    }
}
