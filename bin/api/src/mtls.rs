//! The workload listener's TLS (ANR-1011, AD-068): a TCP listener whose connections are TLS 1.2 or 1.3 with a **client
//! certificate**, handed to axum as a [`Listener`], and the certificate each request came with.
//!
//! * **One trust.** Client certificates are verified at the handshake against the CA bundle of `workloadListener.clientCaFile`,
//!   the same bundle (read the same way, `aap_auth_workload::parse_bundle`) the authenticator is built from, so the TLS layer
//!   and the API cannot trust different CAs. A chain that does not lead to it never gets a request.
//! * **A client certificate is always required** at the handshake: a peer with none does not complete it. (There was a
//!   `clientAuth: optional`; the owner removed it on 2026-11-27, and a file that still says `clientAuth` is refused as an unknown key.)
//! * **The API decides.** The TLS layer is *a* check, not *the* check: the extended key usage (required, not required-if-present),
//!   the window and the exact SPIFFE id are the authenticator's, whatever the handshake let through.
//! * **Handshakes never block the listener.** Each runs in its own task under a deadline and a cap, so a peer that opens a
//!   connection and says nothing costs one slot for ten seconds and nobody else anything.
//! * **No client sees another's certificate**, and nothing here prints one: a certificate is `aap_ports::Certificate`, whose
//!   `Debug` is a length.
//! * **The server certificate and the client CA bundle are re-read** when their files change (told by inode, length and
//!   modification time; checked at most every 30 s, at a handshake and before a request is authenticated), so that a renewal or a
//!   CA rotation needs no restart. The next handshake's verifier and the authenticator take the new bundle **together**; a
//!   connection already established is not cut, and its next request is judged by the new bundle. A file that cannot be read, or a
//!   bundle that holds no usable CA, keeps the one in use and warns once.
//!
//! Not built: the CRL the workload-certificate decision asks for (fetched at most 2 h ago, fail closed when it cannot be read). It needs a source to
//! fetch from, which the deployment has not named, so a revoked certificate is refused only when it expires (ANR-1011, AD-068).

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};

use aap_auth_workload::{WorkloadAuth, WorkloadClients};
use aap_platform_api::PeerCertificate;
use aap_ports::{Certificate, ClientAuthError, ClientAuthenticator, Principal};
use anyhow::{Context, Result, bail};
use axum::{Extension, Router};
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use rustls::RootCertStore;
use rustls::client::danger::HandshakeSignatureValid;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{ClientHello, ResolvesServerCert, WebPkiClientVerifier};
use rustls::sign::CertifiedKey;
use rustls::{DigitallySignedStruct, DistinguishedName, SignatureScheme};
use rustls::{ServerConfig, crypto::CryptoProvider};
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateDer, PrivateKeyDer, UnixTime};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc, watch};
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

/// How long a server certificate is kept before its file is looked at again.
pub const RECHECK: Duration = Duration::from_secs(30);

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

/// What identifies the content of a file without reading it: its inode, its length and its modification time. **Not the time alone**: a
/// renewal that keeps the old time (a copy that preserves it, a clock that went back) still replaces the file, and a replaced file has
/// another inode (a mounted Secret is swapped by a symlink, so the target is another file).
type Stamp = (Option<FileId>, Option<FileId>);
type FileId = (u64, u64, Option<std::time::SystemTime>);

impl fmt::Debug for Reloading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never the key, and not the paths' contents.
        f.debug_struct("Reloading").finish_non_exhaustive()
    }
}

fn file_id(p: &Path) -> Option<FileId> {
    let m = std::fs::metadata(p).ok()?;
    #[cfg(unix)]
    let inode = std::os::unix::fs::MetadataExt::ino(&m);
    #[cfg(not(unix))]
    let inode = 0;
    Some((inode, m.len(), m.modified().ok()))
}

fn stamp(cert: &Path, key: &Path) -> Stamp {
    (file_id(cert), file_id(key))
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

/// The verifier of client certificates for one CA bundle: a client certificate is **required**.
fn client_verifier(
    ca_pem: &str,
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
    WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider.clone())
        .build()
        .map_err(|e| anyhow::anyhow!("the client verifier: {e}"))
}

/// The one trust of the workload listener: the client CA bundle, **read again when its file changes**, and what is built from it, the
/// TLS layer's verifier and the API's authenticator, which are swapped **together** under one lock so that a handshake and a request
/// never see two bundles. A bundle that cannot be read, or holds no usable CA, or whose table is refused, keeps the one in use and warns
/// once. A connection already established is not cut: its next request is judged by the bundle that is current then.
pub struct Trust {
    ca_file: PathBuf,
    clients: WorkloadClients,
    provider: Arc<CryptoProvider>,
    recheck: Duration,
    current: RwLock<Current>,
}

struct Current {
    auth: Arc<WorkloadAuth>,
    verifier: Arc<dyn ClientCertVerifier>,
    stamp: Option<FileId>,
    checked: Instant,
    complained: bool,
}

impl Trust {
    /// Read `ca_file` and build the trust, looking at the file again every `recheck`.
    ///
    /// # Errors
    ///
    /// The file cannot be read, holds no usable CA, or the table of `clients` is refused.
    pub fn load(ca_file: &Path, clients: &WorkloadClients, recheck: Duration) -> Result<Arc<Self>> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let stamp = file_id(ca_file);
        let (auth, verifier) = Self::build(ca_file, clients, &provider)?;
        Ok(Arc::new(Self {
            ca_file: ca_file.to_owned(),
            clients: clients.clone(),
            provider,
            recheck,
            current: RwLock::new(Current {
                auth,
                verifier,
                stamp,
                checked: Instant::now(),
                complained: false,
            }),
        }))
    }

    fn build(
        ca_file: &Path,
        clients: &WorkloadClients,
        provider: &Arc<CryptoProvider>,
    ) -> Result<(Arc<WorkloadAuth>, Arc<dyn ClientCertVerifier>)> {
        let pem = std::fs::read_to_string(ca_file)
            .with_context(|| format!("reading the client CA bundle {}", ca_file.display()))?;
        let auth = WorkloadAuth::new(&pem, clients).context("the workload authenticator")?;
        let verifier = client_verifier(&pem, provider)?;
        Ok((Arc::new(auth), verifier))
    }

    /// Look at the file if it has not been looked at for `recheck`, and swap in the new bundle if it changed and is usable.
    fn refresh(&self) {
        let now = Instant::now();
        {
            let cur = self.current.read().unwrap_or_else(PoisonError::into_inner);
            if now.duration_since(cur.checked) < self.recheck {
                return;
            }
        }
        let mut cur = self.current.write().unwrap_or_else(PoisonError::into_inner);
        if now.duration_since(cur.checked) < self.recheck {
            return;
        }
        cur.checked = now;
        let seen = file_id(&self.ca_file);
        if seen == cur.stamp {
            return;
        }
        match Self::build(&self.ca_file, &self.clients, &self.provider) {
            Ok((auth, verifier)) => {
                cur.auth = auth;
                cur.verifier = verifier;
                cur.stamp = seen;
                cur.complained = false;
                tracing::info!("the workload listener's client CA bundle was reloaded");
            }
            Err(error) => {
                // Not remembered as seen: it is tried again when the file changes again, and warned about once until then.
                cur.stamp = seen;
                if !cur.complained {
                    cur.complained = true;
                    tracing::warn!(error = %format!("{error:#}"), "the workload listener's client CA bundle changed and cannot be used: the one in use stays");
                }
            }
        }
    }

    fn snapshot(&self) -> (Arc<WorkloadAuth>, Arc<dyn ClientCertVerifier>) {
        self.refresh();
        let cur = self.current.read().unwrap_or_else(PoisonError::into_inner);
        (cur.auth.clone(), cur.verifier.clone())
    }
}

/// The API's authenticator over the live trust.
pub struct TrustAuth(pub Arc<Trust>);

impl ClientAuthenticator for TrustAuth {
    async fn authenticate(
        &self,
        certificate: Option<&Certificate>,
    ) -> Result<Principal, ClientAuthError> {
        let (auth, _) = self.0.snapshot();
        auth.authenticate(certificate).await
    }
}

/// The TLS layer's verifier over the live trust.
struct TrustVerifier(Arc<Trust>);

impl fmt::Debug for TrustVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrustVerifier").finish_non_exhaustive()
    }
}

impl ClientCertVerifier for TrustVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        // The names of the CAs a client may choose its certificate by: not offered, since the bundle can change under a handshake. A
        // client holds the certificate it was given.
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        self.0
            .snapshot()
            .1
            .verify_client_cert(end_entity, intermediates, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0
            .snapshot()
            .1
            .verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.0
            .snapshot()
            .1
            .verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.snapshot().1.supported_verify_schemes()
    }
}

/// The TLS configuration of the listener: ring, the default protocol versions (1.3 and 1.2), HTTP/1.1, the live client verifier (a
/// client certificate is required) and the reloading server certificate.
///
/// # Errors
///
/// A file that cannot be read or parsed, or a key that is not the certificate's.
pub fn server_config(
    cert_file: &Path,
    key_file: &Path,
    trust: &Arc<Trust>,
) -> Result<Arc<ServerConfig>> {
    server_config_rechecking(cert_file, key_file, trust, RECHECK)
}

/// [`server_config`], looking at the server certificate's files every `recheck` (a test says 0).
fn server_config_rechecking(
    cert_file: &Path,
    key_file: &Path,
    trust: &Arc<Trust>,
    recheck: Duration,
) -> Result<Arc<ServerConfig>> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let resolver = Reloading::new(cert_file, key_file, provider.clone(), recheck)?;
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| anyhow::anyhow!("TLS protocol versions: {e}"))?
        .with_client_cert_verifier(Arc::new(TrustVerifier(trust.clone())))
        .with_cert_resolver(Arc::new(resolver));
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// What the listener bounds. **Every number is a bound on what one peer, or all of them, can hold**: a handshake that does not finish, a
/// connection that says nothing, and a flood of either.
#[derive(Clone, Debug)]
pub struct Limits {
    /// How long a peer has to finish its TLS handshake.
    pub handshake_timeout: Duration,
    /// How many handshakes may be in flight at once, all sources together.
    pub max_handshakes: usize,
    /// How many handshakes one source address may have in flight (or waiting for a slot): one peer cannot take every slot.
    pub max_handshakes_per_ip: usize,
    /// How long a connection that found every slot taken waits for one before it is dropped.
    pub slot_wait: Duration,
    /// How many connections may be open after their handshake, all sources together.
    pub max_connections: usize,
    /// How long a request's head may take to arrive (hyper's `header_read_timeout`, which also bounds the wait for the next request
    /// of a kept-alive connection).
    pub header_read_timeout: Duration,
    /// How long a connection may go without a byte moving either way (a body that never arrives, a peer that stops reading).
    pub idle_timeout: Duration,
}

impl Limits {
    /// The defaults, with the three settings a deployment may change.
    pub fn new(
        handshake_timeout: Duration,
        max_handshakes_per_ip: usize,
        max_connections: usize,
    ) -> Self {
        Self {
            handshake_timeout,
            max_handshakes: 256,
            max_handshakes_per_ip,
            slot_wait: Duration::from_millis(500),
            max_connections,
            header_read_timeout: Duration::from_secs(15),
            idle_timeout: Duration::from_secs(60),
        }
    }
}

/// The handshakes each source address has in flight. Released when the guard is dropped.
#[derive(Clone, Default)]
struct PerIp(Arc<Mutex<HashMap<IpAddr, usize>>>);

struct IpSlot {
    map: PerIp,
    ip: IpAddr,
}

impl PerIp {
    /// A slot for `ip`, or `None` when it already has `max`.
    fn take(&self, ip: IpAddr, max: usize) -> Option<IpSlot> {
        let mut map = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let held = map.entry(ip).or_insert(0);
        if *held >= max {
            return None;
        }
        *held += 1;
        Some(IpSlot {
            map: self.clone(),
            ip,
        })
    }
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut map = self.map.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = map.get_mut(&self.ip) {
            *held -= 1;
            if *held == 0 {
                map.remove(&self.ip);
            }
        }
    }
}

/// A TCP listener that completes TLS handshakes in tasks of their own and hands over the connections that finished.
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
    /// Take over `tcp` and accept on it with `config`, under `limits`.
    ///
    /// A source address that already has `max_handshakes_per_ip` handshakes in flight has the next connection closed **at once**, before
    /// any TLS work; one that is within its share but finds every global slot taken waits `slot_wait` for one and is then dropped.
    ///
    /// # Errors
    ///
    /// The listener has no local address.
    pub fn new(tcp: TcpListener, config: Arc<ServerConfig>, limits: &Limits) -> io::Result<Self> {
        let local = tcp.local_addr()?;
        let (tx, accepted) = mpsc::channel(64);
        let acceptor = TlsAcceptor::from(config);
        let slots = Arc::new(Semaphore::new(limits.max_handshakes));
        let per_ip = PerIp::default();
        let limits = limits.clone();
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
                let Some(ip_slot) = per_ip.take(peer.ip(), limits.max_handshakes_per_ip) else {
                    tracing::debug!(
                        "a workload source has its share of handshakes in flight: connection closed"
                    );
                    continue;
                };
                let (acceptor, tx, slots, limits) =
                    (acceptor.clone(), tx.clone(), slots.clone(), limits.clone());
                tokio::spawn(async move {
                    // The wait for a slot is bounded, and counted against the source's share while it lasts.
                    let permit = match timeout(limits.slot_wait, slots.acquire_owned()).await {
                        Ok(Ok(permit)) => permit,
                        _ => {
                            tracing::debug!("every handshake slot was taken: connection closed");
                            return;
                        }
                    };
                    let outcome = timeout(limits.handshake_timeout, acceptor.accept(stream)).await;
                    drop((permit, ip_slot));
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

    /// The next connection that finished its handshake.
    async fn accept(&mut self) -> Option<TlsStream<TcpStream>> {
        self.accepted.recv().await.map(|(tls, _)| tls)
    }

    /// Where it listens.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.local)
    }
}

/// An IO that fails with `TimedOut` when nothing has moved for `idle` while a read or a write is waiting: a peer that goes quiet
/// mid-request, or never reads its answer, does not keep a connection for ever.
struct Idle<T> {
    inner: T,
    sleep: Pin<Box<tokio::time::Sleep>>,
    idle: Duration,
}

impl<T> Idle<T> {
    fn new(inner: T, idle: Duration) -> Self {
        Self {
            inner,
            sleep: Box::pin(tokio::time::sleep(idle)),
            idle,
        }
    }

    fn moved(&mut self) {
        self.sleep
            .as_mut()
            .reset(tokio::time::Instant::now() + self.idle);
    }

    fn quiet(&mut self, cx: &mut TaskContext<'_>) -> bool {
        self.sleep.as_mut().poll(cx).is_ready()
    }
}

fn timed_out() -> io::Error {
    io::Error::new(io::ErrorKind::TimedOut, "the connection was idle")
}

impl<T: AsyncRead + Unpin> AsyncRead for Idle<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(result) => {
                self.moved();
                Poll::Ready(result)
            }
            Poll::Pending if self.quiet(cx) => Poll::Ready(Err(timed_out())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Idle<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match Pin::new(&mut self.inner).poll_write(cx, buf) {
            Poll::Ready(result) => {
                self.moved();
                Poll::Ready(result)
            }
            Poll::Pending if self.quiet(cx) => Poll::Ready(Err(timed_out())),
            Poll::Pending => Poll::Pending,
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// The certificates the peer of a connection presented: the leaf, and the intermediates it sent after it.
fn peer_certificate(tls: &TlsStream<TcpStream>) -> PeerCertificate {
    let (_, connection) = tls.get_ref();
    PeerCertificate(connection.peer_certificates().and_then(|chain| {
        let (leaf, rest) = chain.split_first()?;
        Some(
            Certificate::new(leaf.as_ref())
                .with_intermediates(rest.iter().map(|der| der.as_ref().to_vec())),
        )
    }))
}

/// Serve one connection: HTTP/1.1 only (the ALPN says so), with the head's timeout and the idle timeout, until it ends or `stop`.
async fn serve_connection(
    tls: TlsStream<TcpStream>,
    router: Router,
    limits: Limits,
    mut stop: watch::Receiver<bool>,
) {
    // The peer's certificates are a property of the connection, put on every request of it: not a header, so no request can say its own.
    let peer = peer_certificate(&tls);
    let service = TowerToHyperService::new(router.layer(Extension(peer)));
    let io = TokioIo::new(Idle::new(tls, limits.idle_timeout));
    let connection = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header_read_timeout)
        .serve_connection(io, service);
    let mut connection = std::pin::pin!(connection);
    tokio::select! {
        result = connection.as_mut() => {
            if let Err(error) = result {
                tracing::debug!(%error, "a workload connection ended");
            }
        }
        () = async { let _ = stop.wait_for(|stopped| *stopped).await; } => {
            // Finish the request in flight, answer no other.
            connection.as_mut().graceful_shutdown();
            let _ = connection.await;
        }
    }
}

/// Serve `router` on `listener` until `shutdown` resolves, then let the requests in flight finish.
///
/// The connection cap is `limits.max_connections`: a connection that finds none free is closed, and the others are not made to wait.
pub async fn serve(
    mut listener: MtlsListener,
    router: Router,
    limits: Limits,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let addr = listener.local_addr().context("the listening address")?;
    tracing::info!(%addr, "listening (mutual TLS)");
    let slots = Arc::new(Semaphore::new(limits.max_connections));
    let (stop, stopped) = watch::channel(false);
    let mut tasks = tokio::task::JoinSet::new();
    let mut shutdown = std::pin::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => {
                let Some(tls) = accepted else { break };
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    tracing::debug!("the workload listener has its connections: one closed");
                    continue;
                };
                let (router, limits, stopped) = (router.clone(), limits.clone(), stopped.clone());
                tasks.spawn(async move {
                    serve_connection(tls, router, limits, stopped).await;
                    drop(permit);
                });
            }
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    let _ = stop.send(true);
    while tasks.join_next().await.is_some() {}
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::{AtomicU32, Ordering};

    use aap_api::permission;
    use aap_auth_workload::testkit::{Leaf, LeafSpec, TestPki, Usage};
    use aap_auth_workload::{WorkloadClient, WorkloadClients};
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
        dir: Dir,
        ca_file: PathBuf,
    }

    fn limits() -> Limits {
        Limits::new(Duration::from_secs(5), 8, 256)
    }

    async fn rig() -> Rig {
        rig_with(limits()).await
    }

    async fn rig_with(limits: Limits) -> Rig {
        let dir = Dir::new();
        let server_pki = TestPki::new("server CA");
        let workload_pki = TestPki::new("workload CA");
        let server = server_pki.leaf(&LeafSpec::server(&["localhost", "127.0.0.1"]));
        let cert = dir.write("tls.crt", &server.cert_pem);
        let key = dir.write("tls.key", &server.key_pem);
        let ca_file = dir.write("ca.crt", &workload_pki.ca_pem());
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
        // Every file is looked at on each handshake and request (a recheck of zero): the reloads are tested, not waited for.
        let trust = Trust::load(&ca_file, &clients, Duration::ZERO).unwrap();
        let tls = server_config_rechecking(&cert, &key, &trust, Duration::ZERO).unwrap();
        let metrics = Arc::new(ApiMetrics::new());
        let objects = MemoryObjects::in_namespace(&defaults().namespace);
        let router = build_workload_router(
            objects,
            TrustAuth(trust),
            defaults(),
            metrics.clone(),
            Tracer::off(),
        )
        .unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let listener = MtlsListener::new(tcp, tls, &limits).unwrap();
        let (stop, stopped) = watch::channel(false);
        tokio::spawn(serve(listener, router, limits, async move {
            let mut rx = stopped;
            let _ = rx.wait_for(|s| *s).await;
        }));
        Rig {
            addr,
            server_pki,
            workload_pki,
            metrics,
            stop,
            dir,
            ca_file,
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
                        chain_of(leaf),
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

    /// What a client sends: its leaf, then the intermediates it was issued through.
    fn chain_of(leaf: &Leaf) -> Vec<CertificateDer<'static>> {
        std::iter::once(&leaf.cert_der)
            .chain(&leaf.intermediates)
            .map(|der| CertificateDer::from(der.clone()))
            .collect()
    }

    const OWNER_LIST: &str = "/v1/owners/alice/agents";

    #[tokio::test]
    async fn the_right_spiffe_id_reaches_the_owner_routes_and_nothing_else() {
        let rig = rig().await;
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
        let rig = rig().await;
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
        let rig = rig().await;
        let nobody = rig.leaf(&LeafSpec::client(NOBODY));
        let (status, body) = rig.call(Some(&nobody), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 403, "{body}");
        assert!(body.contains(permission::AGENTS_OWN), "{body}");
    }

    #[tokio::test]
    async fn a_certificate_of_another_ca_does_not_get_a_connection() {
        let rig = rig().await;
        let elsewhere = TestPki::new("another CA").leaf(&LeafSpec::client(ORCHESTRATOR));
        let refused = rig.call(Some(&elsewhere), OWNER_LIST, "").await;
        assert!(
            refused.is_err(),
            "no HTTP answer to another CA's certificate: {refused:?}"
        );
    }

    #[tokio::test]
    async fn a_certificate_for_another_use_does_not_get_a_connection() {
        let rig = rig().await;
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
        let rig = rig().await;
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
        let rig = rig().await;
        let mut spec = LeafSpec::client(ORCHESTRATOR);
        spec.not_after = (2020, 1, 2);
        let expired = rig.leaf(&spec);
        let refused = rig.call(Some(&expired), OWNER_LIST, "").await;
        assert!(refused.is_err(), "{refused:?}");
    }

    #[tokio::test]
    async fn a_leaf_with_no_uri_san_or_two_is_a_401() {
        let rig = rig().await;
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
    async fn a_client_certificate_is_always_required_at_the_handshake() {
        // There is no `clientAuth: optional` (removed by the owner's decision of 2026-11-27): no certificate, no connection, with or
        // without a bearer token beside it.
        let rig = rig().await;
        assert!(rig.call(None, OWNER_LIST, "").await.is_err());
        let with_token = rig
            .call(None, OWNER_LIST, "Authorization: Bearer person-token\r\n")
            .await;
        assert!(with_token.is_err(), "{with_token:?}");
    }

    #[tokio::test]
    async fn a_bearer_token_is_not_read_here_and_a_certificate_is_not_read_on_the_persons_listener()
    {
        // A bearer beside a good certificate changes nothing: the certificate decides (and a bearer with no certificate never gets
        // a connection: a_client_certificate_is_always_required_at_the_handshake).
        let optional = rig().await;
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
        // The request carries what the workload listener would have put on it: the peer's valid certificate, and the connection's
        // address. The person's router reads neither: it is judged by its token, and it has none.
        let mut request = axum::http::Request::get(OWNER_LIST)
            .body(axum::body::Body::empty())
            .unwrap();
        let valid = optional.leaf(&LeafSpec::client(ORCHESTRATOR));
        request
            .extensions_mut()
            .insert(PeerCertificate(Some(Certificate::new(valid.cert_der))));
        request
            .extensions_mut()
            .insert(axum::extract::ConnectInfo(SocketAddr::from((
                [127, 0, 0, 1],
                50000,
            ))));
        let res = tower::ServiceExt::oneshot(router, request).await.unwrap();
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
                chain_of(client),
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

    /// Replace `name` by a new file (another inode) that keeps the modification time `mtime`: a renewal that a time alone would not see.
    fn replace_keeping_time(dir: &Dir, name: &str, text: &str, mtime: std::time::SystemTime) {
        let tmp = dir.write(&format!("{name}.new"), text);
        std::fs::File::options()
            .write(true)
            .open(&tmp)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        std::fs::rename(&tmp, dir.0.join(name)).unwrap();
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
        let mtime = std::fs::metadata(&cert).unwrap().modified().unwrap();
        let ca = dir.write("ca.crt", &workload_pki.ca_pem());
        let trust = Trust::load(&ca, &WorkloadClients::default(), Duration::ZERO).unwrap();
        let tls = server_config_rechecking(&cert, &key, &trust, Duration::ZERO).unwrap();
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = tcp.local_addr().unwrap();
        let (stop, stopped) = watch::channel(false);
        let l = limits();
        tokio::spawn(serve(
            MtlsListener::new(tcp, tls, &l).unwrap(),
            Router::new(),
            l,
            async move {
                let mut rx = stopped;
                let _ = rx.wait_for(|s| *s).await;
            },
        ));
        let client = workload_pki.leaf(&LeafSpec::client(ORCHESTRATOR));

        assert_eq!(presented(addr, &server_pki, &client).await, first.cert_der);
        // Renewed, with **the same modification time** (a copy that preserves it): another inode, so it is seen.
        let second = server_pki.leaf(&LeafSpec::server(&["localhost", "127.0.0.1"]));
        replace_keeping_time(&dir, "tls.crt", &second.cert_pem, mtime);
        replace_keeping_time(&dir, "tls.key", &second.key_pem, mtime);
        assert_eq!(presented(addr, &server_pki, &client).await, second.cert_der);
        // A file that cannot be read keeps the certificate in use: the listener does not go down with a bad renewal.
        replace_keeping_time(&dir, "tls.crt", "not a certificate", mtime);
        assert_eq!(presented(addr, &server_pki, &client).await, second.cert_der);
        let _ = stop.send(true);
    }

    #[test]
    fn a_file_is_told_by_its_inode_its_length_and_its_time_and_not_by_its_time_alone() {
        let dir = Dir::new();
        let a = dir.write("a", "same length");
        let key = dir.write("k", "k");
        let mtime = std::fs::metadata(&a).unwrap().modified().unwrap();
        let before = stamp(&a, &key);
        assert_eq!(before, stamp(&a, &key), "the same file is the same stamp");
        // Same length, same time, another file.
        replace_keeping_time(&dir, "a", "other bytes", mtime);
        assert_ne!(before, stamp(&a, &key), "another inode is another stamp");
        // The same file, another length.
        let before = stamp(&a, &key);
        std::fs::write(&a, "a longer content than before").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&a)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert_ne!(before, stamp(&a, &key), "another length is another stamp");
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
        let trust = Trust::load(&ca, &WorkloadClients::default(), Duration::ZERO).unwrap();
        let error = server_config(&cert, &key, &trust).unwrap_err();
        assert!(format!("{error:#}").contains("not the key"), "{error:#}");
        // And a bundle with no CA in it stops the trust at start.
        let empty = dir.write("empty.pem", "");
        let error = Trust::load(&empty, &WorkloadClients::default(), Duration::ZERO)
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("no certificate"), "{error:#}");
    }

    // ---- A PKI that issues from an intermediate (the owner's decision of 2026-11-27: OpenBao issues the workload leaves) -----------

    #[tokio::test]
    async fn a_leaf_through_an_intermediate_reaches_the_owner_routes_with_only_the_root_in_the_bundle()
     {
        let rig = rig().await;
        // root -> intermediate -> intermediate -> leaf; the bundle holds the root alone, and the client sends the leaf and the two.
        let issuing = rig
            .workload_pki
            .intermediate("issuing")
            .intermediate("deeper");
        let leaf = issuing.leaf(&LeafSpec::client(ORCHESTRATOR));
        assert_eq!(leaf.intermediates.len(), 2);
        let (status, body) = rig.call(Some(&leaf), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 200, "{body}");
        // The same chain with a leaf of an identity the file does not name is the authenticator's refusal, not a chain error.
        let stranger = issuing.leaf(&LeafSpec::client(STRANGER));
        let (status, body) = rig.call(Some(&stranger), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 401, "{body}");
        assert!(body.contains("unknown identity"), "{body}");
    }

    #[tokio::test]
    async fn a_leaf_that_does_not_bring_its_intermediate_or_brings_another_cas_gets_no_connection()
    {
        let rig = rig().await;
        let issuing = rig.workload_pki.intermediate("issuing");
        let mut alone = issuing.leaf(&LeafSpec::client(ORCHESTRATOR));
        alone.intermediates.clear();
        let refused = rig.call(Some(&alone), OWNER_LIST, "").await;
        assert!(refused.is_err(), "no intermediate, no chain: {refused:?}");

        // An intermediate that chains to another root, however well formed.
        let foreign = TestPki::new("foreign root").intermediate("issuing");
        let leaf = foreign.leaf(&LeafSpec::client(ORCHESTRATOR));
        let refused = rig.call(Some(&leaf), OWNER_LIST, "").await;
        assert!(refused.is_err(), "{refused:?}");

        // The right leaf with the wrong intermediate sent beside it.
        let mut mixed = issuing.leaf(&LeafSpec::client(ORCHESTRATOR));
        mixed.intermediates = foreign.leaf(&LeafSpec::client(ORCHESTRATOR)).intermediates;
        let refused = rig.call(Some(&mixed), OWNER_LIST, "").await;
        assert!(refused.is_err(), "{refused:?}");
    }

    // ---- Handshakes and connections are bounded ----------------------------------------------------------------------------------

    /// A TCP connection that never starts a handshake.
    async fn stalled(addr: SocketAddr) -> TcpStream {
        TcpStream::connect(addr).await.unwrap()
    }

    /// Whether the server closes `stream` within `within` (a read that ends, or fails).
    async fn closed_within(stream: &mut TcpStream, within: Duration) -> bool {
        let mut buf = [0u8; 16];
        matches!(
            timeout(within, stream.read(&mut buf)).await,
            Ok(Ok(0) | Err(_))
        )
    }

    #[tokio::test]
    async fn one_source_cannot_take_more_than_its_share_of_handshakes() {
        let mut l = limits();
        l.max_handshakes_per_ip = 2;
        l.handshake_timeout = Duration::from_secs(2);
        let rig = rig_with(l).await;
        let (mut a, mut b) = (stalled(rig.addr).await, stalled(rig.addr).await);
        // The third connection of that address is closed at once, before any TLS work.
        let mut third = stalled(rig.addr).await;
        assert!(
            closed_within(&mut third, Duration::from_millis(800)).await,
            "the source's third handshake was not refused"
        );
        // And so a good client of the same address finds no room while the two stall ...
        let leaf = rig.leaf(&LeafSpec::client(ORCHESTRATOR));
        assert!(rig.call(Some(&leaf), OWNER_LIST, "").await.is_err());
        // ... and finds it again once they time out.
        assert!(closed_within(&mut a, Duration::from_secs(4)).await);
        assert!(closed_within(&mut b, Duration::from_secs(4)).await);
        let (status, body) = rig.call(Some(&leaf), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 200, "{body}");
    }

    #[tokio::test]
    async fn a_handshake_that_says_nothing_is_dropped_at_its_deadline() {
        let mut l = limits();
        l.handshake_timeout = Duration::from_millis(400);
        let rig = rig_with(l).await;
        let mut quiet = stalled(rig.addr).await;
        let started = Instant::now();
        assert!(closed_within(&mut quiet, Duration::from_secs(3)).await);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "closed after {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_connection_that_finds_every_slot_taken_waits_briefly_for_one_before_it_is_dropped() {
        let leaf_for = |rig: &Rig| rig.leaf(&LeafSpec::client(ORCHESTRATOR));
        // One slot for everybody, held for 300 ms by a stalled peer; a good client waits up to 2 s for it, and is served.
        let mut l = limits();
        l.max_handshakes = 1;
        l.handshake_timeout = Duration::from_millis(300);
        l.slot_wait = Duration::from_secs(2);
        let rig = rig_with(l).await;
        let _held = stalled(rig.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (status, body) = rig
            .call(Some(&leaf_for(&rig)), OWNER_LIST, "")
            .await
            .unwrap();
        assert_eq!(status, 200, "waited for the slot: {body}");

        // The same, with a wait shorter than the stall: dropped, not queued for ever.
        let mut l = limits();
        l.max_handshakes = 1;
        l.handshake_timeout = Duration::from_secs(3);
        l.slot_wait = Duration::from_millis(100);
        let rig = rig_with(l).await;
        let _held = stalled(rig.addr).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let started = Instant::now();
        assert!(
            rig.call(Some(&leaf_for(&rig)), OWNER_LIST, "")
                .await
                .is_err()
        );
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A TLS connection of `leaf`, after its handshake, with nothing sent.
    async fn connected(rig: &Rig, leaf: &Leaf) -> tokio_rustls::client::TlsStream<TcpStream> {
        let mut roots = RootCertStore::empty();
        for der in aap_auth_workload::parse_bundle(&rig.server_pki.ca_pem()).unwrap() {
            roots.add(der).unwrap();
        }
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(roots)
                .with_client_auth_cert(
                    chain_of(leaf),
                    PrivateKeyDer::from_pem_slice(leaf.key_pem.as_bytes()).unwrap(),
                )
                .unwrap();
        TlsConnector::from(Arc::new(config))
            .connect(
                ServerName::try_from("localhost").unwrap(),
                TcpStream::connect(rig.addr).await.unwrap(),
            )
            .await
            .unwrap()
    }

    /// Whether the server ends the connection (an answer and a close, or a close) within `within`.
    async fn ends_within(
        tls: &mut tokio_rustls::client::TlsStream<TcpStream>,
        within: Duration,
    ) -> bool {
        let mut sink = Vec::new();
        timeout(within, tls.read_to_end(&mut sink)).await.is_ok()
    }

    #[tokio::test]
    async fn a_peer_that_sends_no_request_head_is_cut_off_and_so_is_one_that_sends_half_of_it() {
        let mut l = limits();
        l.header_read_timeout = Duration::from_millis(500);
        let rig = rig_with(l).await;
        let leaf = rig.leaf(&LeafSpec::client(ORCHESTRATOR));
        // Nothing at all after the handshake.
        let mut silent = connected(&rig, &leaf).await;
        assert!(
            ends_within(&mut silent, Duration::from_secs(4)).await,
            "a silent connection was kept"
        );
        // Half a head, slowly.
        let mut slow = connected(&rig, &leaf).await;
        slow.write_all(b"GET /v1/owners/alice/agents HTTP/1.1\r\nHost: l")
            .await
            .unwrap();
        assert!(
            ends_within(&mut slow, Duration::from_secs(4)).await,
            "a half-sent head was kept"
        );
    }

    #[tokio::test]
    async fn a_connection_that_stops_in_the_middle_of_a_body_is_cut_off_by_the_idle_timeout() {
        let mut l = limits();
        l.idle_timeout = Duration::from_millis(500);
        let rig = rig_with(l).await;
        let leaf = rig.leaf(&LeafSpec::client(ORCHESTRATOR));
        let mut stuck = connected(&rig, &leaf).await;
        stuck
            .write_all(
                b"PUT /v1/owners/alice/agents/x HTTP/1.1\r\nHost: l\r\nContent-Type: application/json\r\nIf-None-Match: *\r\nContent-Length: 1000\r\n\r\n{",
            )
            .await
            .unwrap();
        assert!(
            ends_within(&mut stuck, Duration::from_secs(4)).await,
            "a stalled body kept its connection"
        );
    }

    #[tokio::test]
    async fn open_connections_are_capped_and_a_closed_one_makes_room() {
        let mut l = limits();
        l.max_connections = 1;
        let rig = rig_with(l).await;
        let leaf = rig.leaf(&LeafSpec::client(ORCHESTRATOR));
        // One connection open and idle; a second completes its handshake and is closed unserved.
        let first = connected(&rig, &leaf).await;
        let second = rig.call(Some(&leaf), OWNER_LIST, "").await;
        assert!(
            second.is_err(),
            "a second connection was served over the cap: {second:?}"
        );
        drop(first);
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (status, body) = rig.call(Some(&leaf), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 200, "{body}");
    }

    #[test]
    fn a_source_slot_is_given_back() {
        let per_ip = PerIp::default();
        let ip: IpAddr = [10, 0, 0, 1].into();
        let a = per_ip.take(ip, 1).unwrap();
        assert!(per_ip.take(ip, 1).is_none());
        assert!(
            per_ip.take([10, 0, 0, 2].into(), 1).is_some(),
            "another source is not affected"
        );
        drop(a);
        assert!(per_ip.take(ip, 1).is_some());
    }

    // ---- The client CA bundle is reloaded live ---------------------------------------------------------------------------------

    /// One HTTP/1.1 response from a kept-alive connection: the head and as many body bytes as its `Content-Length` says.
    async fn read_response(tls: &mut tokio_rustls::client::TlsStream<TcpStream>) -> String {
        let mut got = Vec::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = timeout(Duration::from_secs(5), tls.read(&mut buf))
                .await
                .expect("an answer within 5 s")
                .expect("a read");
            assert!(
                n > 0,
                "the connection was closed: {}",
                String::from_utf8_lossy(&got)
            );
            got.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&got).into_owned();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let length = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if body.len() >= length {
                    return text;
                }
            }
        }
    }

    #[tokio::test]
    async fn a_new_ca_bundle_is_honoured_without_a_restart_and_an_established_connection_is_not_cut()
     {
        let rig = rig().await;
        let old_leaf = rig.leaf(&LeafSpec::client(ORCHESTRATOR));
        let new_pki = TestPki::new("rotated workload CA");
        let new_leaf = new_pki.leaf(&LeafSpec::client(ORCHESTRATOR));
        let mtime = std::fs::metadata(&rig.ca_file).unwrap().modified().unwrap();

        // Before the swap: the old CA's leaf is served, the new CA's gets no connection.
        assert_eq!(
            rig.call(Some(&old_leaf), OWNER_LIST, "").await.unwrap().0,
            200
        );
        assert!(rig.call(Some(&new_leaf), OWNER_LIST, "").await.is_err());
        // A connection of the old CA's leaf, open across the swap, and shown to be served before it (a TLS 1.3 client is "connected"
        // before the server has checked its certificate).
        let mut open = connected(&rig, &old_leaf).await;
        let head = format!("GET {OWNER_LIST} HTTP/1.1\r\nHost: l\r\n\r\n");
        open.write_all(head.as_bytes()).await.unwrap();
        let first = read_response(&mut open).await;
        assert!(first.starts_with("HTTP/1.1 200"), "{first}");

        // The swap: the CA file is replaced, with the old modification time (told by its inode).
        replace_keeping_time(&rig.dir, "ca.crt", &new_pki.ca_pem(), mtime);

        // The connection that was open is not cut: its next request is answered, and it is judged by the new bundle (401, bad chain).
        // This is the first thing to look at the file after the swap, so it is the authenticator, not a handshake, that takes it up.
        open.write_all(head.as_bytes()).await.unwrap();
        let answer = read_response(&mut open).await;
        assert!(answer.starts_with("HTTP/1.1 401"), "{answer}");
        assert!(answer.contains("bad chain"), "{answer}");
        // After it: the new CA's leaf is served, the old one's gets no connection (the verifier and the authenticator agree).
        let (status, body) = rig.call(Some(&new_leaf), OWNER_LIST, "").await.unwrap();
        assert_eq!(status, 200, "{body}");
        assert!(rig.call(Some(&old_leaf), OWNER_LIST, "").await.is_err());
    }

    #[tokio::test]
    async fn a_broken_or_missing_ca_bundle_keeps_the_one_in_use() {
        let rig = rig().await;
        let leaf = rig.leaf(&LeafSpec::client(ORCHESTRATOR));
        let mtime = std::fs::metadata(&rig.ca_file).unwrap().modified().unwrap();
        for broken in [
            "not a certificate",
            "",
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        ] {
            replace_keeping_time(&rig.dir, "ca.crt", broken, mtime);
            let (status, body) = rig.call(Some(&leaf), OWNER_LIST, "").await.unwrap();
            assert_eq!(
                status, 200,
                "a bundle of {broken:?} replaced the trust: {body}"
            );
        }
        // A leaf (which is no CA) in place of the bundle is refused as a trust, too.
        let not_a_ca = rig.leaf(&LeafSpec::client(STRANGER)).cert_pem;
        replace_keeping_time(&rig.dir, "ca.crt", &not_a_ca, mtime);
        assert_eq!(rig.call(Some(&leaf), OWNER_LIST, "").await.unwrap().0, 200);
        // The file gone.
        std::fs::remove_file(&rig.ca_file).unwrap();
        assert_eq!(rig.call(Some(&leaf), OWNER_LIST, "").await.unwrap().0, 200);
        // And a good bundle written afterwards is taken up: the failures were not final.
        let new_pki = TestPki::new("rotated workload CA");
        std::fs::write(&rig.ca_file, new_pki.ca_pem()).unwrap();
        let new_leaf = new_pki.leaf(&LeafSpec::client(ORCHESTRATOR));
        assert_eq!(
            rig.call(Some(&new_leaf), OWNER_LIST, "").await.unwrap().0,
            200
        );
    }

    #[test]
    fn an_unchanged_file_is_not_rebuilt() {
        let dir = Dir::new();
        let ca = dir.write("ca.crt", &TestPki::new("CA").ca_pem());
        let trust = Trust::load(&ca, &WorkloadClients::default(), Duration::ZERO).unwrap();
        let (a, _) = trust.snapshot();
        let (b, _) = trust.snapshot();
        assert!(Arc::ptr_eq(&a, &b), "the same file was built twice");
    }

    #[test]
    fn the_trust_is_looked_at_again_only_after_its_interval() {
        let dir = Dir::new();
        let pki = TestPki::new("CA");
        let ca = dir.write("ca.crt", &pki.ca_pem());
        let mtime = std::fs::metadata(&ca).unwrap().modified().unwrap();
        let trust =
            Trust::load(&ca, &WorkloadClients::default(), Duration::from_secs(3600)).unwrap();
        let before = trust.snapshot().0;
        replace_keeping_time(&dir, "ca.crt", &TestPki::new("other").ca_pem(), mtime);
        assert!(
            Arc::ptr_eq(&before, &trust.snapshot().0),
            "the file was read again within its interval"
        );
    }
}
