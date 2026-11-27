//! The workload listener's TLS (ANR-1011, ADR-0042): a TCP listener whose connections are TLS 1.2 or 1.3 with a **client
//! certificate**, handed to axum as a [`Listener`], and the certificate each request came with.
//!
//! * **One trust.** Client certificates are verified at the handshake against the CA bundle of `workloadListener.clientCaFile`,
//!   the same bundle (read the same way, `aap_auth_workload::parse_bundle`) the authenticator is built from, so the TLS layer
//!   and the API cannot trust different CAs. A chain that does not lead to it never gets a request.
//! * **Required or optional** (`clientAuth`): `required` ends the handshake of a peer with no certificate; `optional` lets the
//!   connection through and the API answers its first request `401`. A certificate that is presented is verified either way.
//! * **The API decides.** The TLS layer is *a* check, not *the* check: the extended key usage (required, not required-if-present),
//!   the window and the exact SPIFFE id are the authenticator's, whatever the handshake let through.
//! * **Handshakes never block the listener.** Each runs in its own task under a deadline and a cap, so a peer that opens a
//!   connection and says nothing costs one slot for ten seconds and nobody else anything.
//! * **No client sees another's certificate**, and nothing here prints one: a certificate is `aap_ports::Certificate`, whose
//!   `Debug` is a length.
//! * **The server certificate is re-read** when its file changes (checked at most every 30 s, at a handshake), so that a renewed
//!   certificate does not need a restart. A file that cannot be read keeps the one in use and says so once. Rotating the *CA
//!   bundle* needs a restart: it is the trust, and it is read at start.
//!
//! Not built: the CRL of ADR-0042 decision 2 (fetched at most 2 h ago, fail closed when it cannot be read). It needs a source to
//! fetch from, which the deployment has not named, so a revoked certificate is refused only when it expires (ANR-1011, AD-067).

use std::fmt;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant, SystemTime};

use aap_ports::Certificate;
use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::connect_info::{ConnectInfo, Connected};
use axum::middleware::{Next, from_fn};
use axum::serve::{IncomingStream, Listener};
use rustls::RootCertStore;
use rustls::server::danger::ClientCertVerifier;
use rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;
use rustls::{ServerConfig, crypto::CryptoProvider};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use crate::config::ClientAuth;

/// How long a peer has to finish its handshake.
const HANDSHAKE: Duration = Duration::from_secs(10);
/// How many handshakes may be in flight at once.
const IN_FLIGHT: usize = 256;
/// How long a server certificate is kept before its file is looked at again.
const RECHECK: Duration = Duration::from_secs(30);

/// The server certificate and its key, from two files, re-read when either changes.
struct Reloading {
    cert_file: PathBuf,
    key_file: PathBuf,
    provider: Arc<CryptoProvider>,
    /// How long a certificate is kept before its file is looked at again.
    recheck: Duration,
    current: RwLock<Loaded>,
}

struct Loaded {
    key: Arc<CertifiedKey>,
    stamp: Stamp,
    checked: Instant,
    complained: bool,
}

type Stamp = (Option<SystemTime>, Option<SystemTime>);

impl fmt::Debug for Reloading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never the key, and not the paths' contents.
        f.debug_struct("Reloading").finish_non_exhaustive()
    }
}

fn stamp(cert: &Path, key: &Path) -> Stamp {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    (modified(cert), modified(key))
}

fn load_certified(cert: &Path, key: &Path, provider: &CryptoProvider) -> Result<CertifiedKey> {
    let cert_pem = std::fs::read(cert)
        .with_context(|| format!("reading the server certificate {}", cert.display()))?;
    let key_pem =
        std::fs::read(key).with_context(|| format!("reading the server key {}", key.display()))?;
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow::anyhow!("the server certificate is not PEM: {e}"))?;
    if chain.is_empty() {
        bail!("{} holds no certificate", cert.display());
    }
    let key = PrivateKeyDer::from_pem_slice(&key_pem)
        .map_err(|e| anyhow::anyhow!("the server key is not PEM: {e}"))?;
    let signing = provider
        .key_provider
        .load_private_key(key)
        .map_err(|e| anyhow::anyhow!("the server key cannot be used: {e}"))?;
    let certified = CertifiedKey::new(chain, signing);
    certified.keys_match().map_err(|e| {
        anyhow::anyhow!("the server key is not the key of the server certificate: {e}")
    })?;
    Ok(certified)
}

impl Reloading {
    fn new(
        cert_file: &Path,
        key_file: &Path,
        provider: Arc<CryptoProvider>,
        recheck: Duration,
    ) -> Result<Self> {
        let key = Arc::new(load_certified(cert_file, key_file, &provider)?);
        Ok(Self {
            cert_file: cert_file.to_owned(),
            key_file: key_file.to_owned(),
            provider,
            recheck,
            current: RwLock::new(Loaded {
                key,
                stamp: stamp(cert_file, key_file),
                checked: Instant::now(),
                complained: false,
            }),
        })
    }
}

impl ResolvesServerCert for Reloading {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let now = Instant::now();
        {
            let cur = self.current.read().unwrap_or_else(PoisonError::into_inner);
            if now.duration_since(cur.checked) < self.recheck {
                return Some(cur.key.clone());
            }
        }
        let mut cur = self.current.write().unwrap_or_else(PoisonError::into_inner);
        if now.duration_since(cur.checked) < self.recheck {
            return Some(cur.key.clone());
        }
        cur.checked = now;
        let seen = stamp(&self.cert_file, &self.key_file);
        if seen != cur.stamp {
            match load_certified(&self.cert_file, &self.key_file, &self.provider) {
                Ok(fresh) => {
                    cur.key = Arc::new(fresh);
                    cur.stamp = seen;
                    cur.complained = false;
                    tracing::info!("the workload listener's server certificate was reloaded");
                }
                Err(error) => {
                    if !cur.complained {
                        cur.complained = true;
                        tracing::warn!(error = %error, "the workload listener's server certificate changed and cannot be read: the one in use stays");
                    }
                }
            }
        }
        Some(cur.key.clone())
    }
}

/// The verifier of client certificates: the one CA bundle, `required` or `optional`.
fn client_verifier(
    ca_pem: &str,
    mode: ClientAuth,
    provider: &Arc<CryptoProvider>,
) -> Result<Arc<dyn ClientCertVerifier>> {
    let mut roots = RootCertStore::empty();
    let der = aap_auth_workload::parse_bundle(ca_pem).context("the client CA bundle")?;
    if der.is_empty() {
        bail!("the client CA bundle holds no certificate");
    }
    for cert in der {
        roots
            .add(cert)
            .context("a certificate of the client CA bundle is not a usable trust anchor")?;
    }
    let builder = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone());
    let builder = match mode {
        ClientAuth::Required => builder,
        ClientAuth::Optional => builder.allow_unauthenticated(),
    };
    builder
        .build()
        .map_err(|e| anyhow::anyhow!("the client verifier: {e}"))
}

/// The TLS configuration of the listener: ring, the default protocol versions (1.3 and 1.2), HTTP/1.1, the client verifier and
/// the reloading server certificate.
///
/// # Errors
///
/// A file that cannot be read or parsed, a bundle with no usable CA, or a key that is not the certificate's.
pub fn server_config(
    cert_file: &Path,
    key_file: &Path,
    ca_pem: &str,
    mode: ClientAuth,
) -> Result<Arc<ServerConfig>> {
    server_config_rechecking(cert_file, key_file, ca_pem, mode, RECHECK)
}

/// [`server_config`], looking at the server certificate's files every `recheck` (a test says 0).
fn server_config_rechecking(
    cert_file: &Path,
    key_file: &Path,
    ca_pem: &str,
    mode: ClientAuth,
    recheck: Duration,
) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = client_verifier(ca_pem, mode, &provider)?;
    let resolver = Reloading::new(cert_file, key_file, provider.clone(), recheck)?;
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| anyhow::anyhow!("TLS protocol versions: {e}"))?
        .with_client_cert_verifier(verifier)
        .with_cert_resolver(Arc::new(resolver));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// A TCP listener that completes TLS handshakes in tasks of their own and hands axum the connections that finished.
pub struct MtlsListener {
    accepted: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MtlsListener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl MtlsListener {
    /// Take over `tcp` and accept on it with `config`.
    ///
    /// # Errors
    ///
    /// The listener has no local address.
    pub fn new(tcp: TcpListener, config: Arc<ServerConfig>) -> io::Result<Self> {
        let local = tcp.local_addr()?;
        let (tx, accepted) = mpsc::channel(64);
        let acceptor = TlsAcceptor::from(config);
        let slots = Arc::new(Semaphore::new(IN_FLIGHT));
        let task = tokio::spawn(async move {
            loop {
                let (stream, peer) = match tcp.accept().await {
                    Ok(pair) => pair,
                    Err(error) => {
                        // Out of descriptors and the like: back off, do not spin and do not stop.
                        tracing::warn!(%error, "the workload listener could not accept a connection");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    // More handshakes in flight than the cap: this one is dropped, not queued behind the rest.
                    continue;
                };
                let acceptor = acceptor.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let outcome = timeout(HANDSHAKE, acceptor.accept(stream)).await;
                    drop(permit);
                    match outcome {
                        Ok(Ok(tls)) => {
                            let _ = tx.send((tls, peer)).await;
                        }
                        // The reason is the TLS layer's, never a certificate: a debug line is enough, and a peer that fails
                        // its handshake is not an event of the API.
                        Ok(Err(error)) => tracing::debug!(%error, "a workload handshake failed"),
                        Err(_) => tracing::debug!("a workload handshake timed out"),
                    }
                });
            }
        });
        Ok(Self {
            accepted,
            local,
            task,
        })
    }
}

impl Listener for MtlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            if let Some(pair) = self.accepted.recv().await {
                return pair;
            }
            // The accept task only ends when the listener is dropped, so this is never reached while it is served.
            std::future::pending::<()>().await;
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// The certificate the peer of a connection presented, read once per connection by axum and put on each of its requests.
#[derive(Clone)]
struct Peer(Option<Certificate>);

impl Connected<IncomingStream<'_, MtlsListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, MtlsListener>) -> Self {
        let (_, connection) = stream.io().get_ref();
        Self(
            connection
                .peer_certificates()
                .and_then(|chain| chain.first())
                .map(|leaf| Certificate::new(leaf.as_ref())),
        )
    }
}

/// `router`, with the peer's certificate on every request as the [`PeerCertificate`](aap_platform_api::PeerCertificate) the
/// workload guard reads. A request cannot carry one any other way: it is not a header.
fn with_peer(router: Router) -> Router {
    router.layer(from_fn(
        |ConnectInfo(peer): ConnectInfo<Peer>, mut request: axum::extract::Request, next: Next| async move {
            request
                .extensions_mut()
                .insert(aap_platform_api::PeerCertificate(peer.0));
            next.run(request).await
        },
    ))
}

/// Serve `router` on `listener` until `shutdown` resolves, then let the requests in flight finish.
pub async fn serve(
    listener: MtlsListener,
    router: Router,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let addr = listener.local_addr().context("the listening address")?;
    tracing::info!(%addr, "listening (mutual TLS)");
    axum::serve(
        listener,
        with_peer(router).into_make_service_with_connect_info::<Peer>(),
    )
    .with_graceful_shutdown(shutdown)
    .await
    .with_context(|| format!("serving {addr}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::{AtomicU32, Ordering};

    use aap_api::permission;
    use aap_auth_workload::testkit::{Leaf, LeafSpec, TestPki, Usage};
    use aap_auth_workload::{WorkloadAuth, WorkloadClient, WorkloadClients};
    use aap_platform_api::{ApiMetrics, metric};
    use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
    use aap_trace::Tracer;
    use rustls::ClientConfig;
    use rustls::pki_types::ServerName;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::watch;
    use tokio_rustls::TlsConnector;

    use super::*;
    use crate::run::{build_router, build_workload_router};

    const ORCHESTRATOR: &str = "spiffe://another/ns/test/sa/orchestrator";
    const NOBODY: &str = "spiffe://another/ns/test/sa/holds-nothing";
    const STRANGER: &str = "spiffe://another/ns/test/sa/stranger";

    /// A directory of files that is removed when the test ends.
    struct Dir(PathBuf);

    impl Dir {
        fn new() -> Self {
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "aap-mtls-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap();
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn defaults() -> aap_forms::Defaults {
        crate::config::ApiConfig::parse(include_str!("../config.example.yaml"))
            .unwrap()
            .validate()
            .unwrap()
            .defaults
    }

    /// The listener under test, over real sockets and a real TLS stack, with its PKIs.
    struct Rig {
        addr: SocketAddr,
        server_pki: TestPki,
        workload_pki: TestPki,
        metrics: Arc<ApiMetrics>,
        stop: watch::Sender<bool>,
        _dir: Dir,
    }

    async fn rig(mode: ClientAuth) -> Rig {
        let dir = Dir::new();
        let server_pki = TestPki::new("server CA");
        let workload_pki = TestPki::new("workload CA");
        let server = server_pki.leaf(&LeafSpec::server(&["localhost", "127.0.0.1"]));
        let cert = dir.write("tls.crt", &server.cert_pem);
        let key = dir.write("tls.key", &server.key_pem);
        let ca_pem = workload_pki.ca_pem();
        let clients = WorkloadClients {
            clients: vec![
                WorkloadClient {
                    spiffe_id: ORCHESTRATOR.to_owned(),
                    permissions: vec![permission::AGENTS_OWN.to_owned()],
                },
                WorkloadClient {
                    spiffe_id: NOBODY.to_owned(),
                    permissions: Vec::new(),
                },
            ],
        };
        let auth = WorkloadAuth::new(&ca_pem, &clients).unwrap();
        let tls = server_config(&cert, &key, &ca_pem, mode).unwrap();
        let metrics = Arc::new(ApiMetrics::new());
        let objects = MemoryObjects::in_namespace(&defaults().namespace);
        let router =
            build_workload_router(objects, auth, defaults(), metrics.clone(), Tracer::off())
                .unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let listener = MtlsListener::new(tcp, tls).unwrap();
        let (stop, stopped) = watch::channel(false);
        tokio::spawn(serve(listener, router, async move {
            let mut rx = stopped;
            let _ = rx.wait_for(|s| *s).await;
        }));
        Rig {
            addr,
            server_pki,
            workload_pki,
            metrics,
            stop,
            _dir: dir,
        }
    }

    impl Rig {
        fn leaf(&self, spec: &LeafSpec) -> Leaf {
            self.workload_pki.leaf(spec)
        }

        /// One request over a fresh TLS connection. `Err` is a connection that was refused or cut off before any answer: the
        /// TLS layer's refusal. `Ok` is the HTTP answer's status and body.
        async fn call(
            &self,
            client: Option<&Leaf>,
            path: &str,
            extra_headers: &str,
        ) -> Result<(u16, String), String> {
            let mut roots = RootCertStore::empty();
            for der in aap_auth_workload::parse_bundle(&self.server_pki.ca_pem()).unwrap() {
                roots.add(der).unwrap();
            }
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            let builder = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots);
            let config = match client {
                Some(leaf) => builder
                    .with_client_auth_cert(
                        vec![CertificateDer::from(leaf.cert_der.clone())],
                        PrivateKeyDer::from_pem_slice(leaf.key_pem.as_bytes()).unwrap(),
                    )
                    .unwrap(),
                None => builder.with_no_client_auth(),
            };
            let tcp = TcpStream::connect(self.addr)
                .await
                .map_err(|e| e.to_string())?;
            let mut tls = TlsConnector::from(Arc::new(config))
                .connect(ServerName::try_from("localhost").unwrap(), tcp)
                .await
                .map_err(|e| format!("handshake: {e}"))?;
            let request = format!(
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n{extra_headers}\r\n"
            );
            // TLS 1.3 completes the client's handshake before the server has verified the client's certificate: the refusal
            // arrives as an alert on the first read, so a write that fails and a read that fails are both a refusal.
            tls.write_all(request.as_bytes())
                .await
                .map_err(|e| format!("write: {e}"))?;
            let mut answer = Vec::new();
            tls.read_to_end(&mut answer)
                .await
                .map_err(|e| format!("read: {e}"))?;
            let answer = String::from_utf8_lossy(&answer).into_owned();
            let status = answer
                .split(' ')
                .nth(1)
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| "no HTTP answer".to_owned())?;
            let body = answer
                .split_once("\r\n\r\n")
                .map(|(_, b)| b.to_owned())
                .unwrap_or_default();
            Ok((status, body))
        }
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = self.stop.send(true);
        }
    }

    const OWNER_LIST: &str = "/v1/owners/alice/agents";

    #[tokio::test]
    async fn the_right_spiffe_id_reaches_the_owner_routes_and_nothing_else() {
        let rig = rig(ClientAuth::Required).await;
        let orchestrator = rig.leaf(&LeafSpec::client(ORCHESTRATOR));

        let (status, body) = rig.call(Some(&orchestrator), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("\"agents\""), "{body}");

        // A route of the owner routes that finds nothing is a 404 *of the route*: it was reached and authorized.
        let (status, body) = rig
            .call(Some(&orchestrator), "/v1/owners/alice/agents/missing", "")
            .await
            .unwrap();
        assert_eq!(status, 404, "{body}");
        assert!(body.contains("not-found"), "{body}");

        // Every other route of the person's API is not served on this listener, whatever the certificate holds.
        for path in [
            "/v1/me",
            "/v1/info",
            "/v1/agents",
            "/v1/agents/mine",
            "/v1/models",
            "/v1/tool-servers",
            "/v1/secrets/keys",
            "/v1/openapi.json",
            "/healthz",
        ] {
            let (status, body) = rig.call(Some(&orchestrator), path, "").await.unwrap();
            assert_eq!(status, 404, "{path}: {body}");
        }
        assert_eq!(
            rig.metrics.get(
                metric::AUTH_FAILURES,
                &[("reason", "cert_unknown_identity")]
            ),
            0,
            "nothing was refused so far"
        );
    }

    #[tokio::test]
    async fn an_identity_the_configuration_does_not_name_is_refused_with_its_reason() {
        let rig = rig(ClientAuth::Required).await;
        let stranger = rig.leaf(&LeafSpec::client(STRANGER));
        let (status, body) = rig.call(Some(&stranger), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 401, "{body}");
        assert!(
            body.contains("invalid-certificate") && body.contains("unknown identity"),
            "{body}"
        );
        assert!(
            !body.contains("stranger"),
            "the identity is not echoed: {body}"
        );
        assert_eq!(
            rig.metrics.get(
                metric::AUTH_FAILURES,
                &[("reason", "cert_unknown_identity")]
            ),
            1
        );
    }

    #[tokio::test]
    async fn a_named_identity_with_no_permission_is_403_naming_the_permission() {
        let rig = rig(ClientAuth::Required).await;
        let nobody = rig.leaf(&LeafSpec::client(NOBODY));
        let (status, body) = rig.call(Some(&nobody), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 403, "{body}");
        assert!(body.contains(permission::AGENTS_OWN), "{body}");
    }

    #[tokio::test]
    async fn a_certificate_of_another_ca_does_not_get_a_connection() {
        let rig = rig(ClientAuth::Required).await;
        let elsewhere = TestPki::new("another CA").leaf(&LeafSpec::client(ORCHESTRATOR));
        let refused = rig.call(Some(&elsewhere), OWNER_LIST, "").await;
        assert!(
            refused.is_err(),
            "no HTTP answer to another CA's certificate: {refused:?}"
        );
    }

    #[tokio::test]
    async fn a_certificate_for_another_use_does_not_get_a_connection() {
        let rig = rig(ClientAuth::Required).await;
        // The same CA and the same URI SAN, for server authentication: the prompt-signing leaf of the same PKI must not call the API.
        let mut spec = LeafSpec::client(ORCHESTRATOR);
        spec.usages = vec![Usage::Server];
        let wrong = rig.leaf(&spec);
        let refused = rig.call(Some(&wrong), OWNER_LIST, "").await;
        assert!(refused.is_err(), "{refused:?}");
    }

    #[tokio::test]
    async fn a_certificate_with_no_extended_key_usage_gets_a_connection_and_a_401() {
        // `webpki`'s client verifier takes a leaf that has no EKU extension at all (the check is required-if-present), so the TLS
        // layer lets it through; the authenticator requires the extension, by hand (ANR-1010). This is that test over a socket.
        let rig = rig(ClientAuth::Required).await;
        let mut spec = LeafSpec::client(ORCHESTRATOR);
        spec.usages = Vec::new();
        let no_eku = rig.leaf(&spec);
        let (status, body) = rig.call(Some(&no_eku), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 401, "{body}");
        assert!(body.contains("wrong use"), "{body}");
        assert_eq!(
            rig.metrics
                .get(metric::AUTH_FAILURES, &[("reason", "cert_wrong_use")]),
            1
        );
    }

    #[tokio::test]
    async fn an_expired_certificate_does_not_get_a_connection() {
        let rig = rig(ClientAuth::Required).await;
        let mut spec = LeafSpec::client(ORCHESTRATOR);
        spec.not_after = (2020, 1, 2);
        let expired = rig.leaf(&spec);
        let refused = rig.call(Some(&expired), OWNER_LIST, "").await;
        assert!(refused.is_err(), "{refused:?}");
    }

    #[tokio::test]
    async fn a_leaf_with_no_uri_san_or_two_is_a_401() {
        let rig = rig(ClientAuth::Required).await;
        let mut none = LeafSpec::client(ORCHESTRATOR);
        none.uris = Vec::new();
        none.hosts = vec!["orchestrator.test".to_owned()];
        let (status, body) = rig
            .call(Some(&rig.leaf(&none)), OWNER_LIST, "")
            .await
            .unwrap();
        assert_eq!(
            (status, body.contains("no identity")),
            (401, true),
            "{body}"
        );
        let mut two = LeafSpec::client(ORCHESTRATOR);
        two.uris.push(STRANGER.to_owned());
        let (status, body) = rig
            .call(Some(&rig.leaf(&two)), OWNER_LIST, "")
            .await
            .unwrap();
        assert_eq!(
            (status, body.contains("unknown identity")),
            (401, true),
            "{body}"
        );
    }

    #[tokio::test]
    async fn no_certificate_is_no_connection_when_required_and_a_401_when_optional() {
        let required = rig(ClientAuth::Required).await;
        assert!(required.call(None, OWNER_LIST, "").await.is_err());

        let optional = rig(ClientAuth::Optional).await;
        let (status, body) = optional.call(None, OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 401, "{body}");
        assert!(body.contains("missing-certificate"), "{body}");
    }

    #[tokio::test]
    async fn a_bearer_token_is_not_read_here_and_a_certificate_is_not_read_on_the_persons_listener()
    {
        // The person's token, a good one, beside no certificate: refused (neither listener falls back to the other) ...
        let optional = rig(ClientAuth::Optional).await;
        let (status, _) = optional
            .call(None, OWNER_LIST, "Authorization: Bearer person-token\r\n")
            .await
            .unwrap();
        assert_eq!(status, 401);
        // ... and a bearer beside a good certificate changes nothing: the certificate decides.
        let leaf = optional.leaf(&LeafSpec::client(ORCHESTRATOR));
        let (status, _) = optional
            .call(
                Some(&leaf),
                OWNER_LIST,
                "Authorization: Bearer nonsense\r\n",
            )
            .await
            .unwrap();
        assert_eq!(status, 200);

        // The person's router, built beside it from the same ports, holds no workload route for a token that holds
        // `platform:agents.own` only through the person's seam, and answers the owner routes as before: its authenticator is
        // the bearer one, and a connection's certificate is nobody's business there.
        let objects = MemoryObjects::in_namespace(&defaults().namespace);
        let auth = MemoryAuthenticator::new().with_token("t", "person", [permission::AGENTS_OWN]);
        let router = build_router(
            objects,
            auth,
            defaults(),
            aap_platform_api::CorsOrigins::none(),
            Arc::new(ApiMetrics::new()),
            Tracer::off(),
        )
        .unwrap();
        let res = tower::ServiceExt::oneshot(
            router,
            axum::http::Request::get(OWNER_LIST)
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            res.status(),
            401,
            "no token on the person's listener is a 401, certificate or not"
        );
    }

    /// The server certificate a client sees in a handshake with the listener at `addr`.
    async fn presented(addr: SocketAddr, server_ca: &TestPki, client: &Leaf) -> Vec<u8> {
        let mut roots = RootCertStore::empty();
        for der in aap_auth_workload::parse_bundle(&server_ca.ca_pem()).unwrap() {
            roots.add(der).unwrap();
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_client_auth_cert(
                vec![CertificateDer::from(client.cert_der.clone())],
                PrivateKeyDer::from_pem_slice(client.key_pem.as_bytes()).unwrap(),
            )
            .unwrap();
        let tcp = TcpStream::connect(addr).await.unwrap();
        let tls = TlsConnector::from(Arc::new(config))
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .unwrap();
        let (_, connection) = tls.get_ref();
        connection.peer_certificates().unwrap()[0].as_ref().to_vec()
    }

    #[tokio::test]
    async fn a_renewed_server_certificate_is_served_without_a_restart_and_a_broken_file_keeps_the_one_in_use()
     {
        let dir = Dir::new();
        let server_pki = TestPki::new("server CA");
        let workload_pki = TestPki::new("workload CA");
        let first = server_pki.leaf(&LeafSpec::server(&["localhost", "127.0.0.1"]));
        let cert = dir.write("tls.crt", &first.cert_pem);
        let key = dir.write("tls.key", &first.key_pem);
        let tls = server_config_rechecking(
            &cert,
            &key,
            &workload_pki.ca_pem(),
            ClientAuth::Required,
            Duration::ZERO,
        )
        .unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let (stop, stopped) = watch::channel(false);
        tokio::spawn(serve(
            MtlsListener::new(tcp, tls).unwrap(),
            Router::new(),
            async move {
                let mut rx = stopped;
                let _ = rx.wait_for(|s| *s).await;
            },
        ));
        let client = workload_pki.leaf(&LeafSpec::client(ORCHESTRATOR));

        assert_eq!(presented(addr, &server_pki, &client).await, first.cert_der);
        // Renewed: new files (a later modification time is the signal, so wait past the clock's granularity).
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let second = server_pki.leaf(&LeafSpec::server(&["localhost", "127.0.0.1"]));
        dir.write("tls.crt", &second.cert_pem);
        dir.write("tls.key", &second.key_pem);
        assert_eq!(presented(addr, &server_pki, &client).await, second.cert_der);
        // A file that cannot be read keeps the certificate in use: the listener does not go down with a bad renewal.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        dir.write("tls.crt", "not a certificate");
        assert_eq!(presented(addr, &server_pki, &client).await, second.cert_der);
        let _ = stop.send(true);
    }

    #[test]
    fn a_key_that_is_not_the_certificates_stops_the_listener_at_start() {
        let dir = Dir::new();
        let pki = TestPki::new("server CA");
        let a = pki.leaf(&LeafSpec::server(&["localhost"]));
        let b = pki.leaf(&LeafSpec::server(&["localhost"]));
        let cert = dir.write("tls.crt", &a.cert_pem);
        let key = dir.write("tls.key", &b.key_pem);
        let ca = dir.write("ca.pem", &pki.ca_pem());
        let error = server_config(
            &cert,
            &key,
            &std::fs::read_to_string(ca).unwrap(),
            ClientAuth::Required,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("not the key"), "{error:#}");
        // And a bundle with no CA in it.
        let key = dir.write("tls.key", &a.key_pem);
        let error = server_config(&cert, &key, "", ClientAuth::Required).unwrap_err();
        assert!(format!("{error:#}").contains("no certificate"), "{error:#}");
    }
}
