//! The composition root (AD-020): the one place that names the implementations. The router is generic over the two ports;
//! this builds it from the Kubernetes objects and the OIDC authenticator, and serves it.

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use aap_api::ModelEndpoint;
use aap_auth_oidc::OidcAuthenticator;
use aap_forms::Defaults;
use aap_objects_kubernetes::KubernetesObjects;
use aap_platform_api::{ApiMetrics, ApiState, CorsOrigins, router};
use aap_ports::{AgentObjects, AuthError, Authenticator};
use aap_trace::Tracer;
use anyhow::{Context, Result};
use axum::Router;
use clap::Parser;
use futures::future::BoxFuture;
use kube::api::ListParams;
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

    /// Where `/metrics` is served. Wins over `metricsListen` in the file.
    #[arg(long, env = "METRICS_ADDR")]
    pub metrics_addr: Option<SocketAddr>,

    /// Read the configuration file, say whether the API can work with it, and exit. Needs no cluster and no issuer.
    #[arg(long)]
    pub check: bool,
}

/// How long the requests in flight have to finish after a stop is asked for, kept under Kubernetes' 30 seconds.
const DRAIN: Duration = Duration::from_secs(25);

/// How long the spans still queued have to reach the collector once the API has drained (K5). It runs at the same time as the
/// 2 s stop of the health and metrics servers, so the whole stop is at most `DRAIN` plus this: the chart's floor for the grace
/// period (the preStop sleep plus 32 s) is not moved.
const FLUSH: Duration = Duration::from_secs(3);

/// The router over any two ports, counting into `metrics`: the configuration of the deployment, and nothing of how the ports
/// are made.
///
/// # Errors
///
/// The defaults are not usable (they were checked at load; this is the same check where the state is made).
pub fn build_router<O, A>(
    objects: O,
    auth: A,
    defaults: Defaults,
    cors: CorsOrigins,
    metrics: Arc<ApiMetrics>,
    tracer: Tracer,
) -> Result<Router>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let state = ApiState::new(objects, auth, defaults)
        .context("the deployment's defaults")?
        .with_cors(cors)
        .with_metrics(metrics)
        .with_tracer(tracer)
        .with_version(env!("CARGO_PKG_VERSION"));
    Ok(router(state))
}

/// What `/readyz` asks: the issuer's keys can be had (at once, while a copy can still verify), and the cluster's API server
/// answers a call that needs the API's own rights in its namespace (a `Role` that lacks a verb is not ready). Both are asked
/// at most once in a few seconds, however many probes come (`serve.rs`).
pub fn readiness<C, CF, CE, K, KF>(cluster: C, keys: K) -> Check
where
    C: Fn() -> CF + Send + Sync + 'static,
    CF: Future<Output = Result<(), CE>> + Send + 'static,
    CE: std::error::Error + 'static,
    K: Fn() -> KF + Send + Sync + 'static,
    KF: Future<Output = Result<(), AuthError>> + Send + 'static,
{
    Arc::new(move || {
        let cluster = cluster();
        let keys = keys();
        Box::pin(async move {
            keys.await
                .map_err(|_| "the issuer's keys are not available\n")?;
            cluster.await.map_err(|error| {
                tracing::debug!(%error, "the cluster check failed");
                "the cluster's API server does not answer, or refuses the API's rights\n"
            })?;
            Ok(())
        })
    })
}

/// The cluster call of the readiness check: a `list` of the `ModelEndpoint`s of the namespace, which needs the `list` verb
/// the API's `Role` grants, **of one object**: the check asks the right and moves nothing else.
pub fn cluster_probe(
    client: kube::Client,
    namespace: &str,
) -> impl Fn() -> BoxFuture<'static, Result<(), kube::Error>> + Send + Sync + 'static {
    let models: kube::Api<ModelEndpoint> = kube::Api::namespaced(client, namespace);
    move || {
        let models = models.clone();
        Box::pin(async move {
            models
                .list(&ListParams::default().limit(1))
                .await
                .map(|_| ())
        })
    }
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
    metrics: (tokio::net::TcpListener, Arc<ApiMetrics>),
    tracer: Tracer,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    let draining = Arc::new(AtomicBool::new(false));
    let (stop_api, api_stopped) = watch::channel(false);
    let (stop_health, health_stopped) = watch::channel(false);
    let (stop_metrics, metrics_stopped) = watch::channel(false);
    let api = tokio::spawn(serve::serve(api, router, signalled(api_stopped)));
    let health = tokio::spawn(serve::serve(
        health,
        serve::health(Health {
            draining: draining.clone(),
            check,
        }),
        signalled(health_stopped),
    ));

    let (metrics_listener, counters) = metrics;
    let metrics_server = tokio::spawn(serve::serve(
        metrics_listener,
        serve::metrics_with_traces(counters, tracer.stats()),
        signalled(metrics_stopped),
    ));

    supervise(
        Running {
            draining,
            api,
            health,
            metrics: metrics_server,
            stop_api,
            stop_health,
            stop_metrics,
            tracer,
        },
        stop,
    )
    .await
}

type Server = tokio::task::JoinHandle<Result<()>>;

/// The three servers as they run, and what stops each.
struct Running {
    draining: Arc<AtomicBool>,
    api: Server,
    health: Server,
    metrics: Server,
    stop_api: watch::Sender<bool>,
    stop_health: watch::Sender<bool>,
    stop_metrics: watch::Sender<bool>,
    tracer: Tracer,
}

/// Wait for the stop or for a server to end, then drain. **A handle that has completed is never polled again** (a second poll
/// of a `JoinHandle` panics): the one that ended is reported, the others are awaited only if they still run.
async fn supervise(mut s: Running, stop: impl Future<Output = ()>) -> Result<()> {
    // The servers fail fast (a port in use is a deployment mistake); a stop is asked for by the signal.
    let outcome = tokio::select! {
        () = stop => {
            tracing::info!("shutting down");
            Ok(())
        }
        r = &mut s.api => Err(ended("the API server", r)),
        r = &mut s.health => Err(ended("the health server", r)),
        r = &mut s.metrics => Err(ended("the metrics server", r)),
    };
    s.draining.store(true, Ordering::Release);
    let _ = s.stop_api.send(true);
    if !s.api.is_finished() && tokio::time::timeout(DRAIN, &mut s.api).await.is_err() {
        tracing::warn!("requests were still in flight after {DRAIN:?}; stopping anyway");
        s.api.abort();
    }
    let _ = s.stop_health.send(true);
    let _ = s.stop_metrics.send(true);
    // After the drain, so the spans of the last requests are in the queue; at the same time as the stop of the two small
    // servers. A flush that does not finish is given up on at its deadline, never waited for.
    let stopping = async {
        for server in [&mut s.health, &mut s.metrics] {
            if !server.is_finished() {
                let _ = tokio::time::timeout(Duration::from_secs(2), server).await;
            }
        }
    };
    let (_, spans) = tokio::join!(stopping, s.tracer.shutdown(FLUSH));
    if s.tracer.enabled() {
        tracing::info!(
            exported = spans.exported,
            dropped =
                spans.dropped_queue_full + spans.dropped_export_failed + spans.dropped_rejected,
            "trace spans flushed"
        );
    }
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
        metrics_listen,
        oidc,
        cors,
        defaults,
    } = config::load(&args.config)?;
    // Before the cluster and the issuer: a bad tracing variable is found at once, and says which.
    let tracer = aap_trace_otlp::tracer("aap-api", env!("CARGO_PKG_VERSION"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let listen = args.listen.unwrap_or(listen);
    let health_listen = args.health_addr.unwrap_or(health_listen);
    let metrics_listen = args.metrics_addr.unwrap_or(metrics_listen);
    tracing::info!(
        namespace = %defaults.namespace,
        origins = cors.as_slice().len(),
        "starting the Platform API {}",
        env!("CARGO_PKG_VERSION")
    );

    let client = kube::Client::try_default()
        .await
        .context("connecting to the cluster (in-cluster configuration, or KUBECONFIG)")?;
    let probe = cluster_probe(client.clone(), &defaults.namespace);
    let objects =
        KubernetesObjects::new(client, &defaults.namespace).with_offer_label(&defaults.offer_label);
    let auth = OidcAuthenticator::new(oidc).context("the token authenticator")?;
    let check = {
        let auth = auth.clone();
        readiness(probe, move || {
            let auth = auth.clone();
            async move { auth.ready().await }
        })
    };
    let counters = Arc::new(ApiMetrics::new());
    let router = build_router(
        objects,
        auth,
        defaults,
        cors,
        counters.clone(),
        tracer.clone(),
    )?;

    // Bound before anything is served: a port in use is found at once.
    let api = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    let health = tokio::net::TcpListener::bind(health_listen)
        .await
        .with_context(|| format!("binding {health_listen}"))?;
    let metrics = tokio::net::TcpListener::bind(metrics_listen)
        .await
        .with_context(|| format!("binding {metrics_listen}"))?;
    serve_both(
        api,
        router,
        health,
        check,
        (metrics, counters),
        tracer,
        termination(),
    )
    .await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use std::sync::Mutex;
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
        let counters = Arc::new(ApiMetrics::new());
        let router = build_router(
            objects.clone(),
            auth,
            s.defaults,
            s.cors,
            counters.clone(),
            Tracer::off(),
        )
        .unwrap();
        let check = readiness(
            || async { Ok::<(), std::convert::Infallible>(()) },
            || async { Ok(()) },
        );

        let (api, api_addr) = bound().await;
        let (health, health_addr) = bound().await;
        let (metrics, metrics_addr) = bound().await;
        let (stop, stopped) = watch::channel(false);
        let running = tokio::spawn(serve_both(
            api,
            router,
            health,
            check,
            (metrics, counters),
            Tracer::off(),
            signalled(stopped),
        ));

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
        // The requests above were counted, and the text is on the metrics port and nowhere else.
        let (status, text) = http(metrics_addr, "/metrics", None).await;
        assert_eq!(status, 200);
        assert!(
            text.contains(
                "aap_api_requests_total{route=\"/v1/me\",method=\"GET\",status=\"200\"} 1"
            ),
            "{text}"
        );
        assert!(
            text.contains("aap_api_auth_failures_total{reason=\"missing\"} 1"),
            "{text}"
        );
        assert!(!text.contains("\"t\""), "no token: {text}");
        assert!(
            !text.contains("aap_trace_"),
            "tracing is off: its series are not there: {text}"
        );
        assert_eq!(http(api_addr, "/metrics", None).await.0, 404);
        assert_eq!(http(health_addr, "/metrics", None).await.0, 404);
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
        let counters = Arc::new(ApiMetrics::new());
        let router = build_router(
            objects,
            issuer.authenticator().clone(),
            s.defaults,
            s.cors,
            counters.clone(),
            Tracer::off(),
        )
        .unwrap();
        let (api, api_addr) = bound().await;
        let (health, _) = bound().await;
        let (metrics, _) = bound().await;
        let (stop, stopped) = watch::channel(false);
        let check: Check = Arc::new(|| Box::pin(async { Ok(()) }));
        let running = tokio::spawn(serve_both(
            api,
            router,
            health,
            check,
            (metrics, counters),
            Tracer::off(),
            signalled(stopped),
        ));
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

    fn running(api: Server, health: Server, metrics: Server) -> Running {
        Running {
            draining: Arc::new(AtomicBool::new(false)),
            api,
            health,
            metrics,
            stop_api: watch::channel(false).0,
            stop_health: watch::channel(false).0,
            stop_metrics: watch::channel(false).0,
            tracer: Tracer::off(),
        }
    }

    /// A server that has already ended when the others are drained is not polled again: the error is the clean one, never a
    /// panic of a `JoinHandle` polled after completion. Each of the three, first.
    #[tokio::test(start_paused = true)]
    async fn the_server_that_ended_first_is_reported_and_never_polled_twice() {
        let pending = || tokio::spawn(std::future::pending::<Result<()>>());
        let gone = || tokio::spawn(async { Ok(()) });
        for (what, s) in [
            ("the metrics server", running(pending(), pending(), gone())),
            ("the health server", running(pending(), gone(), pending())),
            ("the API server", running(gone(), pending(), pending())),
        ] {
            let error = tokio::time::timeout(
                Duration::from_secs(30),
                supervise(s, std::future::pending()),
            )
            .await
            .expect("it returned")
            .unwrap_err();
            let text = format!("{error:#}");
            assert!(
                text.contains(what) && text.contains("without being asked"),
                "{text}"
            );
        }
    }

    /// A tracer that keeps its spans in a `MemoryExporter`, and the exporter.
    fn memory_tracer(
        mode: aap_trace::testkit::Mode,
    ) -> (Tracer, aap_trace::testkit::MemoryExporter) {
        let exporter = aap_trace::testkit::MemoryExporter::new(mode);
        let tracer = aap_trace::Pipeline::start(
            exporter.clone(),
            aap_trace::Limits::default(),
            aap_trace::Resource::new("aap-api", "0"),
            aap_trace::Sampler::AlwaysOn,
        );
        (tracer, exporter)
    }

    fn root_span(tracer: &Tracer, name: &'static str) -> aap_trace::Span {
        tracer.root(
            aap_trace::SpanName::fixed(name),
            aap_trace::SpanKind::Server,
        )
    }

    /// The spans of the last requests are flushed **after** the API has drained, so a span made while it drains is exported.
    #[tokio::test(start_paused = true)]
    async fn the_flush_comes_after_the_drain() {
        let (tracer, exporter) = memory_tracer(aap_trace::testkit::Mode::Keep);
        root_span(&tracer, "before the stop").end();
        let (stop_api, mut api_stopped) = watch::channel(false);
        let in_flight = tracer.clone();
        let api = tokio::spawn(async move {
            let _ = api_stopped.wait_for(|stop| *stop).await;
            // The request that was in flight when the stop came ends during the drain.
            tokio::time::sleep(Duration::from_secs(10)).await;
            root_span(&in_flight, "during the drain").end();
            Ok(())
        });
        let pending = || tokio::spawn(std::future::pending::<Result<()>>());
        let mut s = running(api, pending(), pending());
        s.stop_api = stop_api;
        s.tracer = tracer;
        supervise(s, async {}).await.unwrap();
        let names: Vec<String> = exporter
            .spans()
            .iter()
            .map(|s| s.name.as_str().to_owned())
            .collect();
        assert_eq!(names, ["before the stop", "during the drain"]);
    }

    /// A collector that never answers does not hold the stop for more than the flush's deadline.
    #[tokio::test(start_paused = true)]
    async fn a_flush_that_hangs_is_given_up_on_at_its_deadline() {
        let (tracer, _exporter) = memory_tracer(aap_trace::testkit::Mode::Hang);
        root_span(&tracer, "never delivered").end();
        // Servers that stop when they are told to, as the real ones do: only the flush is slow.
        let stoppable = || {
            let (tx, mut rx) = watch::channel(false);
            let server = tokio::spawn(async move {
                let _ = rx.wait_for(|stop| *stop).await;
                Ok(())
            });
            (tx, server)
        };
        let ((stop_api, api), (stop_health, health), (stop_metrics, metrics)) =
            (stoppable(), stoppable(), stoppable());
        let mut s = running(api, health, metrics);
        s.stop_api = stop_api;
        s.stop_health = stop_health;
        s.stop_metrics = stop_metrics;
        s.tracer = tracer;
        let started = tokio::time::Instant::now();
        supervise(s, async {}).await.unwrap();
        let took = started.elapsed();
        assert!(
            took >= FLUSH && took < FLUSH + Duration::from_secs(1),
            "{took:?}"
        );
    }

    /// The API on a socket with tracing on: a request is a span at the collector with the API's service name, and the two
    /// series of the pipeline are on `/metrics`.
    #[tokio::test]
    async fn the_api_on_a_socket_sends_its_spans_to_a_collector() {
        let collector = aap_trace_otlp::testkit::TestCollector::start().await;
        let endpoint = collector.endpoint();
        let tracer = aap_trace_otlp::tracer_with("aap-api", "0.1.0", move |k| {
            (k == "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").then(|| endpoint.clone())
        })
        .unwrap();
        let s = settings();
        let objects = MemoryObjects::in_namespace(&s.defaults.namespace);
        let auth = MemoryAuthenticator::new().with_token("t", "alice", ["platform:agents.read"]);
        let counters = Arc::new(ApiMetrics::new());
        let router = build_router(
            objects,
            auth,
            s.defaults,
            s.cors,
            counters.clone(),
            tracer.clone(),
        )
        .unwrap();
        let (api, api_addr) = bound().await;
        let (health, _) = bound().await;
        let (metrics, metrics_addr) = bound().await;
        let (stop, stopped) = watch::channel(false);
        let check: Check = Arc::new(|| Box::pin(async { Ok(()) }));
        let running = tokio::spawn(serve_both(
            api,
            router,
            health,
            check,
            (metrics, counters),
            tracer,
            signalled(stopped),
        ));
        assert_eq!(http(api_addr, "/v1/me", Some("t")).await.0, 200);
        let (_, text) = http(metrics_addr, "/metrics", None).await;
        assert!(
            text.contains("aap_trace_spans_exported_total"),
            "the pipeline's series are there when tracing is on: {text}"
        );
        stop.send(true).unwrap();
        running.await.unwrap().unwrap();
        assert!(collector.service_names().iter().all(|n| n == "aap-api"));
        assert!(
            collector.spans().iter().any(|s| s["name"] == "GET /v1/me"),
            "the span of the request reached the collector at the flush"
        );
    }

    #[derive(Debug)]
    struct Down;

    impl std::fmt::Display for Down {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("down")
        }
    }

    impl std::error::Error for Down {}

    #[tokio::test(start_paused = true)]
    async fn readiness_needs_the_keys_and_the_cluster() {
        let keys_up = Arc::new(AtomicBool::new(true));
        let cluster_up = Arc::new(AtomicBool::new(true));
        let probes = Arc::new(AtomicUsize::new(0));
        let check = {
            let up = keys_up.clone();
            let cluster = cluster_up.clone();
            let probes = probes.clone();
            readiness(
                move || {
                    let up = cluster.load(Ordering::Acquire);
                    async move { if up { Ok(()) } else { Err(Down) } }
                },
                move || {
                    let up = up.load(Ordering::Acquire);
                    probes.fetch_add(1, Ordering::Relaxed);
                    async move {
                        if up {
                            Ok(())
                        } else {
                            Err(AuthError::Unavailable("the keys are down".into()))
                        }
                    }
                },
            )
        };
        assert_eq!(check().await, Ok(()));
        cluster_up.store(false, Ordering::Release);
        assert_eq!(
            check().await,
            Err("the cluster's API server does not answer, or refuses the API's rights\n")
        );
        cluster_up.store(true, Ordering::Release);
        keys_up.store(false, Ordering::Release);
        assert_eq!(check().await, Err("the issuer's keys are not available\n"));
        assert_eq!(probes.load(Ordering::Relaxed), 3);
    }

    /// A client whose API server records what it is asked and answers `status` with `body`.
    fn recording_client(
        status: u16,
        body: serde_json::Value,
    ) -> (kube::Client, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let service = tower::service_fn(move |request: axum::http::Request<kube::client::Body>| {
            record
                .lock()
                .unwrap()
                .push(format!("{} {}", request.method(), request.uri()));
            let body = body.clone();
            async move {
                Ok::<_, std::convert::Infallible>(
                    axum::http::Response::builder()
                        .status(status)
                        .header("content-type", "application/json")
                        .body(kube::client::Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        });
        (kube::Client::new(service, "default"), seen)
    }

    #[tokio::test]
    async fn the_cluster_probe_lists_one_model_endpoint_of_the_namespace_and_no_more() {
        let (client, seen) = recording_client(
            200,
            serde_json::json!({
                "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "ModelEndpointList",
                "metadata": { "resourceVersion": "1" }, "items": []
            }),
        );
        let probe = cluster_probe(client, "agents-ns");
        probe().await.unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            seen,
            ["GET /apis/agents.vymalo.com/v1alpha1/namespaces/agents-ns/modelendpoints?&limit=1"],
            "one call, of one object, in the namespace"
        );
    }

    #[tokio::test]
    async fn the_cluster_probe_fails_when_the_server_refuses_the_right() {
        let (client, _) = recording_client(
            403,
            serde_json::json!({
                "apiVersion": "v1", "kind": "Status", "status": "Failure", "reason": "Forbidden",
                "message": "modelendpoints is forbidden", "code": 403
            }),
        );
        assert!(cluster_probe(client, "agents-ns")().await.is_err());
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
            "--metrics-addr",
            "127.0.0.1:9002",
        ])
        .unwrap();
        assert_eq!(args.listen, Some("127.0.0.1:9000".parse().unwrap()));
        assert_eq!(args.health_addr, Some("127.0.0.1:9001".parse().unwrap()));
        assert_eq!(args.metrics_addr, Some("127.0.0.1:9002".parse().unwrap()));
        assert!(!args.check);
        assert!(
            Args::try_parse_from(["api"]).is_err(),
            "the configuration file is required"
        );
    }
}
