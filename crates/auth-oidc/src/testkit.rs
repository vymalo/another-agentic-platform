//! A token issuer on a local port, for tests (feature `testkit`): its discovery document and its JWKS over real
//! HTTP, keys of every kind the authenticator reads, and tokens signed as a case says, the ones that must be
//! refused included. [`TestAuthenticator`] pairs one with an [`OidcAuthenticator`] and is what
//! `aap_ports::authenticator_conformance!` is run against.
//!
//! The keys (RSA 2048, P-256, Ed25519, twice: one set the issuer publishes, one it does not until
//! [`TestIdp::publish_unpublished_keys`]) are made once per process. Each [`TestIdp`] has its own port, its own
//! publication state and its own counters, so tests run side by side. **The keys are throwaway test keys, made
//! at run time: no private key is in the repository.**

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

pub mod issuer_cli;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aap_ports::testkit::AuthenticatorUnderTest;
use aap_ports::{AuthError, Authenticator, Principal};
use aws_lc_rs::rand::SystemRandom;
use aws_lc_rs::rsa::{KeyPair as RsaKeyPair, KeySize, PublicKeyComponents};
use aws_lc_rs::signature::{
    ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, Ed25519KeyPair, KeyPair as _, RSA_PKCS1_SHA256,
    RSA_PKCS1_SHA384,
};
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value, json};
use tokio::task::JoinHandle;

use crate::{OidcAuthenticator, OidcConfig, Tuning};

/// The audience of [`TestAuthenticator`].
pub const TEST_AUDIENCE: &str = "aap-test-api";

/// The roles claim of [`TestAuthenticator`]: a flat claim, as on netcup.
pub const TEST_ROLES_CLAIM: &str = "agentic_roles";

/// The interval of [`TestAuthenticator`]'s cache, short so that no case waits on the real ones.
pub const TEST_INTERVAL: Duration = Duration::from_millis(300);

/// A signature algorithm the authenticator must accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alg {
    /// RSASSA-PKCS1-v1_5 with SHA-256.
    Rs256,
    /// RSASSA-PKCS1-v1_5 with SHA-384.
    Rs384,
    /// ECDSA over P-256 with SHA-256.
    Es256,
    /// EdDSA over Ed25519.
    EdDsa,
}

/// Every algorithm the authenticator must accept.
pub const ALGORITHMS: [Alg; 4] = [Alg::Rs256, Alg::Rs384, Alg::Es256, Alg::EdDsa];

/// How the issuer signs a token it mints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signing {
    /// With a key the issuer publishes, of this algorithm. The header names its `kid`.
    Published(Alg),
    /// With a key the issuer has **not** published (yet): the header names a `kid` of its own.
    Unpublished(Alg),
    /// With a key the issuer has not published, and a header that names the `kid` of the key it does publish
    /// (an attacker's token that claims the issuer's key): the signature cannot verify.
    UnpublishedUnderPublishedKid(Alg),
    /// With a key the issuer publishes, and no `kid` in the header.
    PublishedWithoutKid(Alg),
    /// With `alg: none` and an empty signature.
    NoneAlg,
    /// With `alg: HS256`, the bytes of a published public key being the secret (the algorithm confusion attack).
    Hs256WithPublicKey,
}

fn b64(bytes: impl AsRef<[u8]>) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// One generation of keys: an RSA key (RS256 and RS384), a P-256 key and an Ed25519 key.
struct Generation {
    name: &'static str,
    rsa: RsaKeyPair,
    ec: EcdsaKeyPair,
    ed: Ed25519KeyPair,
}

impl Generation {
    fn make(name: &'static str) -> Self {
        Generation {
            name,
            rsa: RsaKeyPair::generate(KeySize::Rsa2048).unwrap(),
            ec: EcdsaKeyPair::generate(&ECDSA_P256_SHA256_FIXED_SIGNING).unwrap(),
            ed: Ed25519KeyPair::generate().unwrap(),
        }
    }

    fn kid(&self, alg: Alg) -> String {
        let kind = match alg {
            Alg::Rs256 | Alg::Rs384 => "rsa",
            Alg::Es256 => "ec",
            Alg::EdDsa => "ed",
        };
        format!("{}-{kind}", self.name)
    }

    fn jwks(&self) -> Vec<Value> {
        let rsa = PublicKeyComponents::<Vec<u8>>::from(self.rsa.public_key());
        let ec = self.ec.public_key().as_ref().to_vec();
        assert_eq!(ec.len(), 65, "an uncompressed P-256 point");
        vec![
            json!({"kty": "RSA", "use": "sig", "kid": self.kid(Alg::Rs256), "n": b64(&rsa.n), "e": b64(&rsa.e)}),
            json!({"kty": "EC", "use": "sig", "crv": "P-256", "kid": self.kid(Alg::Es256),
                   "x": b64(&ec[1..33]), "y": b64(&ec[33..65])}),
            json!({"kty": "OKP", "use": "sig", "crv": "Ed25519", "kid": self.kid(Alg::EdDsa),
                   "x": b64(self.ed.public_key().as_ref())}),
        ]
    }

    fn sign(&self, alg: Alg, message: &[u8]) -> Vec<u8> {
        match alg {
            Alg::Rs256 | Alg::Rs384 => {
                let padding: &'static dyn aws_lc_rs::signature::RsaEncoding = if alg == Alg::Rs256 {
                    &RSA_PKCS1_SHA256
                } else {
                    &RSA_PKCS1_SHA384
                };
                let mut signature = vec![0; self.rsa.public_modulus_len()];
                self.rsa
                    .sign(padding, &SystemRandom::new(), message, &mut signature)
                    .unwrap();
                signature
            }
            Alg::Es256 => self
                .ec
                .sign(&SystemRandom::new(), message)
                .unwrap()
                .as_ref()
                .to_vec(),
            Alg::EdDsa => self.ed.sign(message).as_ref().to_vec(),
        }
    }
}

fn generations() -> &'static (Generation, Generation) {
    static KEYS: OnceLock<(Generation, Generation)> = OnceLock::new();
    KEYS.get_or_init(|| (Generation::make("pub"), Generation::make("unpub")))
}

/// What signs a token's signing input.
type Signer<'a> = Box<dyn Fn(&[u8]) -> Vec<u8> + 'a>;

fn alg_name(alg: Alg) -> &'static str {
    match alg {
        Alg::Rs256 => "RS256",
        Alg::Rs384 => "RS384",
        Alg::Es256 => "ES256",
        Alg::EdDsa => "EdDSA",
    }
}

/// Seconds since the epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Sets `value` at the dotted `path` of `claims`, making the objects on the way (`realm_access.roles`); a path
/// without a dot is a flat claim.
pub fn set_path(claims: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            claims.insert(path.to_owned(), value);
        }
        Some((head, rest)) => {
            let entry = claims
                .entry(head.to_owned())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(inner) = entry {
                set_path(inner, rest, value);
            }
        }
    }
}

#[derive(Default)]
struct Shared {
    issuer: String,
    unpublished_published: AtomicBool,
    jwks_down: AtomicBool,
    discovery_down: AtomicBool,
    jwks_fetches: AtomicUsize,
    discovery_fetches: AtomicUsize,
}

async fn discovery(State(shared): State<Arc<Shared>>) -> Response {
    shared.discovery_fetches.fetch_add(1, Ordering::SeqCst);
    if shared.discovery_down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    axum::Json(json!({"issuer": shared.issuer, "jwks_uri": format!("{}/jwks", shared.issuer)}))
        .into_response()
}

async fn jwks(State(shared): State<Arc<Shared>>) -> Response {
    shared.jwks_fetches.fetch_add(1, Ordering::SeqCst);
    if shared.jwks_down.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let (published, unpublished) = generations();
    let mut keys = published.jwks();
    if shared.unpublished_published.load(Ordering::SeqCst) {
        keys.extend(unpublished.jwks());
    }
    axum::Json(json!({ "keys": keys })).into_response()
}

/// A token issuer on `127.0.0.1`, stopped when dropped.
pub struct TestIdp {
    shared: Arc<Shared>,
    server: JoinHandle<()>,
}

impl Drop for TestIdp {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl TestIdp {
    /// Starts the issuer on a free port of `127.0.0.1`.
    pub async fn start() -> Self {
        Self::start_at("127.0.0.1:0", "127.0.0.1").await
    }

    /// Starts the issuer on `bind` (`0.0.0.0:0` for a free port on every interface) and names itself
    /// `http://<public_host>:<the port it got>`: that is the `iss` of its tokens and the `issuer` of its
    /// discovery document, so a client that reaches it at that address (a pod in a cluster, at the host's address on the
    /// cluster's network) sees the same name it was configured with. An IPv6 literal is written with its brackets.
    pub async fn start_at(bind: &str, public_host: &str) -> Self {
        Self::try_start_at(bind, public_host).await.unwrap()
    }

    /// [`TestIdp::start_at`] that reports an address it cannot listen on (in use, not this machine's, not allowed) instead of panicking.
    pub async fn try_start_at(bind: &str, public_host: &str) -> std::io::Result<Self> {
        // The keys are made before the first request needs them.
        let _ = generations();
        let listener = tokio::net::TcpListener::bind(bind).await?;
        let issuer = format!("http://{public_host}:{}", listener.local_addr()?.port());
        let shared = Arc::new(Shared {
            issuer,
            ..Shared::default()
        });
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .with_state(Arc::clone(&shared));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Ok(TestIdp { shared, server })
    }

    /// The issuer, as `iss` must say it: `http://127.0.0.1:<port>`.
    pub fn issuer(&self) -> String {
        self.shared.issuer.clone()
    }

    /// The JWKS URL, for a configuration that names it.
    pub fn jwks_url(&self) -> String {
        format!("{}/jwks", self.shared.issuer)
    }

    /// The `kid` of the published key of `alg`, as a token's header names it.
    pub fn published_kid(&self, alg: Alg) -> String {
        generations().0.kid(alg)
    }

    /// The `kid` of the key that is not published (yet) for `alg`.
    pub fn unpublished_kid(&self, alg: Alg) -> String {
        generations().1.kid(alg)
    }

    /// Makes the JWKS answer 503 (`true`) or answer again.
    pub fn set_jwks_down(&self, down: bool) {
        self.shared.jwks_down.store(down, Ordering::SeqCst);
    }

    /// Makes the discovery document answer 503 (`true`) or answer again.
    pub fn set_discovery_down(&self, down: bool) {
        self.shared.discovery_down.store(down, Ordering::SeqCst);
    }

    /// Requests of the JWKS so far.
    pub fn jwks_fetches(&self) -> usize {
        self.shared.jwks_fetches.load(Ordering::SeqCst)
    }

    /// Requests of the discovery document so far.
    pub fn discovery_fetches(&self) -> usize {
        self.shared.discovery_fetches.load(Ordering::SeqCst)
    }

    /// Publishes the second set of keys: the issuer rotates.
    pub fn publish_unpublished_keys(&self) {
        self.shared
            .unpublished_published
            .store(true, Ordering::SeqCst);
    }

    /// The configuration of an authenticator for this issuer: the audience [`TEST_AUDIENCE`] and the roles
    /// claim [`TEST_ROLES_CLAIM`].
    pub fn config(&self) -> OidcConfig {
        OidcConfig::new(self.issuer(), [TEST_AUDIENCE], TEST_ROLES_CLAIM)
    }

    /// Withdraws the second set of keys again: the issuer stops publishing keys it had published.
    pub fn withdraw_unpublished_keys(&self) {
        self.shared
            .unpublished_published
            .store(false, Ordering::SeqCst);
    }

    /// The claims of a good token for `audience` and `subject`: this issuer, valid now for five minutes, with
    /// no roles.
    pub fn claims(&self, audience: &str, subject: &str) -> Map<String, Value> {
        let now = now();
        let claims = json!({
            "iss": self.issuer(), "aud": audience, "sub": subject,
            "iat": now, "exp": now + 300,
        });
        claims.as_object().unwrap().clone()
    }

    /// A good token (RS256) for `audience` and `subject`, with no roles.
    pub fn token(&self, audience: &str, subject: &str) -> String {
        self.mint(
            &self.claims(audience, subject),
            Signing::Published(Alg::Rs256),
        )
    }

    /// A good token (RS256) for `audience` and `subject` that carries `roles` at `roles_claim` (a dotted path is
    /// nested: `resource_access.client.roles`), and a `jti` no other token of this process has, so that two tokens
    /// made in the same second differ.
    pub fn token_with_roles(
        &self,
        audience: &str,
        subject: &str,
        roles_claim: &str,
        roles: &[impl AsRef<str>],
    ) -> String {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let mut claims = self.claims(audience, subject);
        claims.insert(
            "jti".to_owned(),
            json!(NEXT.fetch_add(1, Ordering::Relaxed).to_string()),
        );
        let roles: Vec<&str> = roles.iter().map(AsRef::as_ref).collect();
        set_path(&mut claims, roles_claim, json!(roles));
        self.mint(&claims, Signing::Published(Alg::Rs256))
    }

    /// A token with exactly `claims`, signed as `signing` says.
    pub fn mint(&self, claims: &Map<String, Value>, signing: Signing) -> String {
        let (published, unpublished) = generations();
        let (header, signer): (Value, Signer<'_>) = match signing {
            Signing::Published(alg) => (
                json!({"alg": alg_name(alg), "typ": "JWT", "kid": published.kid(alg)}),
                Box::new(move |m| published.sign(alg, m)),
            ),
            Signing::Unpublished(alg) => (
                json!({"alg": alg_name(alg), "typ": "JWT", "kid": unpublished.kid(alg)}),
                Box::new(move |m| unpublished.sign(alg, m)),
            ),
            Signing::UnpublishedUnderPublishedKid(alg) => (
                json!({"alg": alg_name(alg), "typ": "JWT", "kid": published.kid(alg)}),
                Box::new(move |m| unpublished.sign(alg, m)),
            ),
            Signing::PublishedWithoutKid(alg) => (
                json!({"alg": alg_name(alg), "typ": "JWT"}),
                Box::new(move |m| published.sign(alg, m)),
            ),
            Signing::NoneAlg => (
                json!({"alg": "none", "typ": "JWT"}),
                Box::new(|_| Vec::new()),
            ),
            Signing::Hs256WithPublicKey => {
                let secret = published.rsa.public_key().as_ref().to_vec();
                (
                    json!({"alg": "HS256", "typ": "JWT", "kid": published.kid(Alg::Rs256)}),
                    Box::new(move |m| {
                        let key = aws_lc_rs::hmac::Key::new(aws_lc_rs::hmac::HMAC_SHA256, &secret);
                        aws_lc_rs::hmac::sign(&key, m).as_ref().to_vec()
                    }),
                )
            }
        };
        let signing_input = format!(
            "{}.{}",
            b64(serde_json::to_vec(&header).unwrap()),
            b64(serde_json::to_vec(claims).unwrap())
        );
        let signature = signer(signing_input.as_bytes());
        format!("{signing_input}.{}", b64(signature))
    }

    /// A token with exactly this `header` and these `claims` and this `signature` (raw bytes), signed by
    /// nothing: for the headers that must be refused before any key is looked at (`alg` RS512, PS256, ES384,
    /// HS384, an unknown name, a header that is not an object).
    pub fn forge(&self, header: &Value, claims: &Map<String, Value>, signature: &[u8]) -> String {
        format!(
            "{}.{}.{}",
            b64(serde_json::to_vec(header).unwrap()),
            b64(serde_json::to_vec(claims).unwrap()),
            b64(signature)
        )
    }
}

/// A [`TestIdp`] and an [`OidcAuthenticator`] configured for it ([`TEST_AUDIENCE`], [`TEST_ROLES_CLAIM`],
/// intervals of [`TEST_INTERVAL`]). It is an [`Authenticator`], and an [`AuthenticatorUnderTest`]: the type
/// `aap_ports::authenticator_conformance!` is run with, and what the Platform API's router tests (D7) use to
/// get tokens for people who hold the permissions a case needs.
pub struct TestAuthenticator {
    idp: TestIdp,
    auth: OidcAuthenticator,
}

impl TestAuthenticator {
    /// An issuer, and an authenticator for it.
    pub async fn start() -> Self {
        Self::start_with(|tuning| tuning).await
    }

    /// As [`start`](Self::start), with the tuning that `adjust` makes of the test one.
    pub async fn start_with(adjust: impl FnOnce(Tuning) -> Tuning) -> Self {
        let idp = TestIdp::start().await;
        let auth = Self::build(&idp, adjust);
        Self { idp, auth }
    }

    fn build(idp: &TestIdp, adjust: impl FnOnce(Tuning) -> Tuning) -> OidcAuthenticator {
        let tuning = adjust(Tuning {
            kid_refetch_interval: TEST_INTERVAL,
            retry_interval: TEST_INTERVAL,
            use_system_proxy: false,
            ..Tuning::default()
        });
        OidcAuthenticator::with_tuning(idp.config(), tuning).unwrap()
    }

    /// The issuer.
    pub fn idp(&self) -> &TestIdp {
        &self.idp
    }

    /// The authenticator.
    pub fn authenticator(&self) -> &OidcAuthenticator {
        &self.auth
    }

    /// A token for `subject` that carries `permissions` in the roles claim.
    pub fn token_for(&self, subject: &str, permissions: &[&str]) -> String {
        self.idp
            .token_with_roles(TEST_AUDIENCE, subject, TEST_ROLES_CLAIM, permissions)
    }
}

impl Authenticator for TestAuthenticator {
    async fn authenticate(&self, token: Option<&str>) -> Result<Principal, AuthError> {
        self.auth.authenticate(token).await
    }
}

impl AuthenticatorUnderTest for TestAuthenticator {
    async fn issue(&self, subject: &str, permissions: &[&str]) -> String {
        self.token_for(subject, permissions)
    }

    async fn without_keys(&self) -> Option<(Self, String)> {
        let idp = TestIdp::start().await;
        idp.set_jwks_down(true);
        let auth = Self::build(&idp, |tuning| tuning);
        let off = Self { idp, auth };
        let token = off.token_for("anybody", &["platform:agents.read"]);
        Some((off, token))
    }
}
