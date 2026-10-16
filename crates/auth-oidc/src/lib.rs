//! [`Authenticator`] for the Platform API (§60a, slice D6): a bearer JWT signed by an OIDC issuer, checked here
//! against the issuer's keys, so that the API is an OAuth 2 resource server and trusts no header a client can
//! send. The values of the configured roles claim are the person's permissions.
//!
//! This is the checker of another-agentic-system's orchestrator (its `orch-auth-jwt`, ADR 0033 there), without
//! DPoP and without a user or an e-mail claim; the [README](https://github.com/vymalo/another-agentic-platform/blob/main/crates/auth-oidc/README.md)
//! has the two diagrams (a request, and the life of the key cache) and the list of what is left out.
//!
//! # What is checked, in this order
//!
//! - **Size**: a token of more than 16 KiB ([`MAX_TOKEN_BYTES`]) is refused unread.
//! - **Shape**: three base64url parts whose first is a JSON header, and an `alg` among RS256, RS384, ES256 and
//!   EdDSA. `none` and the HMAC family are refused whatever key they name (the algorithm confusion attack).
//!   Nothing has been fetched yet: a token that is not one never makes a request to the issuer.
//! - **Key**: the keys of the issuer's JWKS that the header's `kid` names and whose kind is the algorithm's,
//!   every one of them (two keys may share a `kid`; the token is good if any of them signed it). A token with
//!   no `kid` takes the one key of its kind, if there is exactly one. A JWKS key that cannot verify (symmetric,
//!   too small, for encryption, another curve) is skipped.
//! - **Signature**, then **claims**, in this order: `exp` required and not past, `nbf` not in the future, both
//!   with 60 seconds of leeway ([`LEEWAY_SECS`]); `iss` a text equal to the configured issuer; one of the
//!   configured audiences in `aud`; `sub` required, not blank and without a control character. `iat` is **not**
//!   required. With several claims wrong, the word is the first in this order.
//! - **Roles**: the values at the configured claim. A claim that is absent or of another shape is no
//!   permission, and does not refuse the token.
//!
//! Every refusal is one word of [`InvalidToken`](aap_ports::InvalidToken); none holds a part of the token.
//!
//! # The key cache
//!
//! The keys are a copy this process may lose at any time. They are fetched on first use (and by
//! [`OidcAuthenticator::ready`]), refreshed once they are 10 minutes old ([`REFRESH_INTERVAL`]), fetched
//! again when a token names a `kid` the copy lacks **at most once in 30 seconds** ([`KID_REFETCH_INTERVAL`]:
//! a flood of invented ids is not a flood of requests to the issuer), and not again for 5 seconds after a
//! failure ([`RETRY_INTERVAL`]).
//!
//! **A fetch runs on a task of its own, one at a time, and no lock is held while it runs.** A request waits for
//! it only when it must: with no copy of the keys (or one past its grace) it waits for the fetch and fails
//! closed ([`AuthError::Unavailable`]; a caller may retry) if there is still nothing to verify with; when a
//! token names a `kid` the copy lacks, that request waits for the fetch, and no other does. A copy older than
//! the refresh interval and within the grace ([`STALE_GRACE`], an hour) **is used at once** and is refreshed in
//! the background, so a slow or hanging issuer holds up nobody who has a copy; the keys an issuer withdrew
//! therefore stop verifying when that refresh is over, and a request that comes after a long quiet is verified
//! against the copy it finds. A request that is dropped while it waits leaves the fetch running, and the keys
//! it brings are recorded all the same.
//!
//! The keys are read from `jwksUrl`, or from the `jwks_uri` of `{issuer}/.well-known/openid-configuration`,
//! whose `issuer` must be the configured one. No redirect is followed, a body over 1 MiB is refused, a request
//! times out (each request, 5 seconds), and `HTTP(S)_PROXY` is honoured.

#![warn(missing_docs)]

mod claims;
mod config;
mod fetch;
mod keys;
#[cfg(feature = "testkit")]
pub mod testkit;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use aap_ports::{AuthError, Authenticator, InvalidToken, Principal};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, TokenData, Validation, decode, decode_header};
use serde_json::{Map, Value};
use tokio::sync::watch;
use url::Url;

pub use crate::config::{
    ConfigError, FETCH_TIMEOUT, KID_REFETCH_INTERVAL, LEEWAY_SECS, MAX_TOKEN_BYTES, OidcConfig,
    REFRESH_INTERVAL, RETRY_INTERVAL, STALE_GRACE, Tuning,
};
pub use crate::fetch::FetchFailure;
use crate::fetch::{Fetcher, Source};
use crate::keys::{ALLOWED, KeySet, Parsed};

/// Why a token could not be checked: the issuer's keys are not to be had. It is the source of
/// [`AuthError::Unavailable`], which the API answers `503`, and it holds **nothing of the token**: the
/// requests for the keys carry none, and the cause is a [`FetchFailure`], a closed set of words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum KeysUnavailable {
    /// No copy of the keys is held, and the last attempt to fetch them failed.
    #[error("no copy of the issuer's keys is held")]
    NoCopy(#[source] Option<FetchFailure>),
    /// The copy is older than the refresh interval plus the grace, and the last attempt to fetch the keys
    /// failed.
    #[error("the copy of the issuer's keys is too old to verify with")]
    TooOld(#[source] Option<FetchFailure>),
    /// The copy may still verify, but it lacks the key the token names and the last attempt to fetch the
    /// keys failed: the issuer may have rotated, and the token is neither known to be good nor to be bad.
    #[error("the copy of the issuer's keys lacks the key of the token, and could not be refreshed")]
    Unrefreshed(#[source] FetchFailure),
}

impl KeysUnavailable {
    /// Why the last fetch failed, when it is known.
    pub fn cause(&self) -> Option<FetchFailure> {
        match self {
            Self::NoCopy(cause) | Self::TooOld(cause) => *cause,
            Self::Unrefreshed(cause) => Some(*cause),
        }
    }
}

/// The copy of the keys, and what is known about fetching them. Behind a lock that is held to read or to
/// change this and for nothing else: never across a request to the issuer.
#[derive(Default)]
struct State {
    /// The keys of the last successful fetch, and when it started.
    keys: Option<(Arc<KeySet>, Instant)>,
    /// When the last fetch, successful or not, started.
    last_attempt: Option<Instant>,
    /// When the last fetch failed and why, until one succeeds.
    last_failure: Option<(Instant, FetchFailure)>,
    /// The fetch that is running, if one is: what a request waits on. It ends when its sender is dropped.
    in_flight: Option<watch::Receiver<()>>,
}

impl State {
    /// The fetch that is running, if there is one.
    fn running(&mut self) -> Option<watch::Receiver<()>> {
        match &self.in_flight {
            // A fetch whose sender is gone is over; a task that was dropped before it ran leaves such a mark.
            Some(wait) if wait.has_changed().is_err() => {
                self.in_flight = None;
                None
            }
            Some(wait) => Some(wait.clone()),
            None => None,
        }
    }

    /// Whether the last fetch failed less than `retry` ago.
    fn backing_off(&self, retry: Duration) -> bool {
        self.last_failure
            .is_some_and(|(failed, _)| failed.elapsed() < retry)
    }
}

struct Inner {
    issuer: String,
    audiences: Vec<String>,
    roles_claim: String,
    tuning: Tuning,
    fetcher: Fetcher,
    state: Mutex<State>,
}

/// A running fetch, from inside the task that does it. However the task ends (it finished, it panicked, the
/// runtime stopped), the outcome is recorded and the fetch is over: the waiters wake after, and find it.
struct Flight {
    inner: Arc<Inner>,
    started: Instant,
    outcome: Option<Result<Parsed, FetchFailure>>,
    /// Dropped after the outcome is recorded: the waiters wake when it is gone.
    _done: watch::Sender<()>,
}

impl Drop for Flight {
    fn drop(&mut self) {
        let outcome = self.outcome.take().unwrap_or(Err(FetchFailure::Transport));
        self.inner.record(self.started, outcome);
    }
}

/// The authenticator of the Platform API. Cheap to clone: clones share the keys.
#[derive(Clone)]
pub struct OidcAuthenticator {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for OidcAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcAuthenticator")
            .field("issuer", &self.inner.issuer)
            .field("audiences", &self.inner.audiences)
            .field("roles_claim", &self.inner.roles_claim)
            .finish_non_exhaustive()
    }
}

impl OidcAuthenticator {
    /// Builds the authenticator with the intervals of this crate. It reads nothing: the keys are fetched on
    /// first use.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] for a configuration that cannot work ([`OidcConfig::validate`]) or an HTTP client that
    /// cannot be built.
    pub fn new(config: OidcConfig) -> Result<Self, ConfigError> {
        Self::with_tuning(config, Tuning::default())
    }

    /// As [`new`](Self::new), with the intervals and limits of `tuning` (a test shortens them).
    ///
    /// # Errors
    ///
    /// As [`new`](Self::new).
    pub fn with_tuning(config: OidcConfig, tuning: Tuning) -> Result<Self, ConfigError> {
        install_crypto_provider();
        let (issuer_url, jwks_url) = config.parts()?;
        // The keys every token is checked against travel in the clear: anyone on the path can swap them. Said once,
        // here, and not as a refusal: a Keycloak in the cluster is often reached over plain http, on purpose. The
        // URL is not repeated (it is the operator's own, and the log has no need of it).
        if issuer_url.scheme() == "http" || jwks_url.as_ref().is_some_and(|u| u.scheme() == "http")
        {
            tracing::warn!(
                "plain http is configured for the token issuer or its keys; anyone on the network path could swap the keys every token is checked against (use https unless the network is trusted)"
            );
        }
        let source = match jwks_url {
            Some(url) => Source::Url(url),
            None => {
                // OpenID Connect Discovery 1.0, 4: a terminating `/` of the issuer is removed (one, never all
                // of them), and the path is appended. `Url` writes a bare host as `http://host/`.
                let base = issuer_url.as_str();
                let base = base.strip_suffix('/').unwrap_or(base);
                let document = Url::parse(&format!("{base}/.well-known/openid-configuration"))
                    .map_err(|_| ConfigError::Issuer)?;
                Source::Discover {
                    issuer: config.issuer.clone(),
                    document,
                }
            }
        };
        let http = fetch::client(tuning.timeout, tuning.use_system_proxy)
            .map_err(|e| ConfigError::HttpClient(e.without_url().into()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                issuer: config.issuer,
                audiences: config.audiences,
                roles_claim: config.roles_claim,
                tuning,
                fetcher: Fetcher::new(http, source),
                state: Mutex::new(State::default()),
            }),
        })
    }

    /// Whether the keys can be had: they are fetched now if there is no fresh copy. For a readiness probe of
    /// the API, which should not take requests it can only answer `503`.
    ///
    /// # Errors
    ///
    /// [`AuthError::Unavailable`] when there is no copy that may still verify.
    pub async fn ready(&self) -> Result<(), AuthError> {
        self.inner
            .current()
            .await
            .map(|_| ())
            .map_err(|e| AuthError::Unavailable(e.into()))
    }
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        // Nothing here panics with the lock held; a poisoned lock still holds a state that was valid.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Asks the issuer for its keys. Holds no lock: the state is touched after, by [`record`](Self::record).
    async fn fetch_keys(&self) -> Result<Parsed, FetchFailure> {
        let document = self.fetcher.jwks().await?;
        let parsed = KeySet::parse(&document).ok_or(FetchFailure::NotAKeySet)?;
        if parsed.skipped > 0 {
            tracing::debug!(
                skipped = parsed.skipped,
                usable = parsed.set.len(),
                "keys of the issuer this build cannot use"
            );
        }
        if parsed.set.is_empty() {
            return Err(FetchFailure::NoUsableKey);
        }
        Ok(parsed)
    }

    /// Records what came of the fetch that started at `started`, and ends it.
    fn record(&self, started: Instant, outcome: Result<Parsed, FetchFailure>) {
        let (recovered, failed) = {
            let mut state = self.state();
            state.in_flight = None;
            match outcome {
                Ok(parsed) => {
                    let recovered = state.last_failure.take().is_some();
                    state.keys = Some((Arc::new(parsed.set), started));
                    (recovered, None)
                }
                Err(cause) => {
                    let first = state.last_failure.is_none();
                    state.last_failure = Some((Instant::now(), cause));
                    (false, Some((cause, first)))
                }
            }
        };
        if recovered {
            tracing::info!("the token issuer's keys can be fetched again");
        }
        match failed {
            Some((cause, true)) => {
                tracing::warn!(%cause, "the token issuer's keys cannot be fetched; tokens are refused until they can");
            }
            Some((cause, false)) => {
                tracing::debug!(%cause, "the token issuer's keys still cannot be fetched");
            }
            None => {}
        }
    }

    /// Starts a fetch of the keys on a task of its own, and gives what to wait on for it to be over. **A fetch
    /// belongs to nobody who waits for it**: a request that is dropped while it waits leaves it running, and the
    /// keys it brings are recorded all the same. Called with the lock held, and not to be called while a fetch is
    /// running (`state.running()`).
    fn start_fetch(self: &Arc<Self>, state: &mut State) -> watch::Receiver<()> {
        let started = Instant::now();
        let (done, wait) = watch::channel(());
        let inner = Arc::clone(self);
        // The task cannot record before this returns: `record` takes the lock the caller holds.
        tokio::spawn(async move {
            let mut flight = Flight {
                inner,
                started,
                outcome: None,
                _done: done,
            };
            flight.outcome = Some(flight.inner.fetch_keys().await);
        });
        state.last_attempt = Some(started);
        state.in_flight = Some(wait.clone());
        wait
    }

    /// The copy, if it may still verify: no older than the refresh interval and the grace.
    fn usable(&self, state: &State) -> Option<Arc<KeySet>> {
        let (keys, at) = state.keys.as_ref()?;
        (at.elapsed() <= self.tuning.refresh_interval + self.tuning.stale_grace)
            .then(|| Arc::clone(keys))
    }

    /// Why there is no key to verify with.
    fn unavailable(state: &State) -> KeysUnavailable {
        let cause = state.last_failure.map(|(_, cause)| cause);
        if state.keys.is_some() {
            KeysUnavailable::TooOld(cause)
        } else {
            KeysUnavailable::NoCopy(cause)
        }
    }

    /// The keys to verify with.
    ///
    /// - A fresh copy is used at once.
    /// - A copy older than the refresh interval and within its grace is used at once too, and a refresh is
    ///   started in the background if none is running and the last one did not fail just now: **a request does
    ///   not wait for a refresh while a copy can verify**.
    /// - With no copy, or one past its grace, the request waits for the fetch that is running (it starts one
    ///   unless the last failed just now) and fails closed if there is still nothing to verify with.
    async fn current(self: &Arc<Self>) -> Result<Arc<KeySet>, KeysUnavailable> {
        let wait = {
            let mut state = self.state();
            if let Some((keys, at)) = &state.keys {
                let age = at.elapsed();
                if age < self.tuning.refresh_interval {
                    return Ok(Arc::clone(keys));
                }
                if age <= self.tuning.refresh_interval + self.tuning.stale_grace {
                    let keys = Arc::clone(keys);
                    if state.running().is_none() && !state.backing_off(self.tuning.retry_interval) {
                        self.start_fetch(&mut state);
                    }
                    return Ok(keys);
                }
            }
            match state.running() {
                Some(wait) => wait,
                None if state.backing_off(self.tuning.retry_interval) => {
                    return Err(Self::unavailable(&state));
                }
                None => self.start_fetch(&mut state),
            }
        };
        finished(wait).await;
        let state = self.state();
        self.usable(&state).ok_or_else(|| Self::unavailable(&state))
    }

    /// The keys after a token named a `kid` that `seen` lacks: the fetch that is running, or one of its own
    /// unless a fetch was tried within the interval (any fetch counts) or the last one failed just now. Only the
    /// request that names the unknown key waits.
    ///
    /// # Errors
    ///
    /// [`KeysUnavailable::Unrefreshed`] when the keys cannot be fetched: the token might be good, signed by a
    /// key the issuer has published since, and it is not told that it is bad. When the last fetch succeeded
    /// the key is not the issuer's, and the caller says so.
    async fn after_unknown_kid(
        self: &Arc<Self>,
        seen: &Arc<KeySet>,
    ) -> Result<Arc<KeySet>, KeysUnavailable> {
        let wait = {
            let mut state = self.state();
            if let Some((keys, _)) = &state.keys
                && !Arc::ptr_eq(keys, seen)
            {
                return Ok(Arc::clone(keys));
            }
            match state.running() {
                Some(wait) => Some(wait),
                None => {
                    let recent = state
                        .last_attempt
                        .is_some_and(|at| at.elapsed() < self.tuning.kid_refetch_interval);
                    if recent || state.backing_off(self.tuning.retry_interval) {
                        None
                    } else {
                        Some(self.start_fetch(&mut state))
                    }
                }
            }
        };
        if let Some(wait) = wait {
            finished(wait).await;
        }
        let state = self.state();
        if let Some((_, cause)) = state.last_failure {
            return Err(KeysUnavailable::Unrefreshed(cause));
        }
        Ok(self.usable(&state).unwrap_or_else(|| Arc::clone(seen)))
    }

    /// What the library checks: the signature, then `exp` (required, and not past) and `nbf`, both with the
    /// leeway. `iss`, `aud` and `sub` are checked after, by [`check_claims`](Self::check_claims) and
    /// [`claims::principal`], in that order: the library's own checks of them read a list where a text is meant,
    /// let a claim of the wrong type through, and report the first of several missing claims in the order of a
    /// hash set.
    fn validation(&self, alg: Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.leeway = LEEWAY_SECS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.validate_aud = false;
        // `iat` is not required: §60a names `exp` only.
        validation.set_required_spec_claims(&["exp"]);
        validation
    }

    /// `iss` and `aud` of a token whose signature and times have passed. `iss` is one text, equal to the issuer:
    /// the library would accept a list that holds it, and so a token that names another issuer beside ours.
    /// `aud` is a text, or a list of texts, and one of the configured audiences is among them.
    fn check_claims(&self, claims: &Map<String, Value>) -> Result<(), InvalidToken> {
        if claims.get("iss").and_then(Value::as_str) != Some(self.issuer.as_str()) {
            return Err(InvalidToken::WrongIssuer);
        }
        let ours = |aud: &str| self.audiences.iter().any(|ours| ours == aud);
        let named = match claims.get("aud") {
            Some(Value::String(aud)) => ours(aud),
            Some(Value::Array(auds)) => {
                auds.iter().all(Value::is_string) && auds.iter().filter_map(Value::as_str).any(ours)
            }
            _ => false,
        };
        if !named {
            return Err(InvalidToken::WrongAudience);
        }
        Ok(())
    }

    /// The token decoded with the first of `keys` that verifies its signature. Only a bad signature moves on to
    /// the next key; any other word (the times, a claim) is of a token that a key did verify, and ends it.
    fn decode_with_any(
        &self,
        token: &str,
        alg: Algorithm,
        keys: &[DecodingKey],
    ) -> Result<TokenData<Map<String, Value>>, AuthError> {
        let validation = self.validation(alg);
        for key in keys {
            match decode::<Map<String, Value>>(token, key, &validation) {
                Ok(data) => return Ok(data),
                Err(e) if matches!(e.kind(), ErrorKind::InvalidSignature) => {}
                Err(e) => return Err(AuthError::Invalid(reason(e.kind()))),
            }
        }
        Err(AuthError::Invalid(InvalidToken::BadSignature))
    }

    /// The claims of `token`, once its size, header, key, signature and claims have passed.
    async fn verify(self: &Arc<Self>, token: &str) -> Result<Map<String, Value>, AuthError> {
        let invalid = |reason: InvalidToken| AuthError::Invalid(reason);
        if token.len() > MAX_TOKEN_BYTES {
            return Err(invalid(InvalidToken::Malformed));
        }
        // The header says which key to use, and is read before anything is trusted. A token that is not three
        // base64url parts of JSON, and `alg: none` (which is no algorithm of ours), end here: no fetch, no key.
        let header = decode_header(token).map_err(|_| invalid(InvalidToken::Malformed))?;
        if !ALLOWED.contains(&header.alg) {
            return Err(invalid(InvalidToken::Malformed));
        }
        let unavailable = |e: KeysUnavailable| AuthError::Unavailable(e.into());
        let candidates = |keys: &KeySet| -> Vec<DecodingKey> {
            keys.select(header.alg, header.kid.as_deref())
                .into_iter()
                .map(|key| key.decoding.clone())
                .collect()
        };
        let mut keys = self.current().await.map_err(unavailable)?;
        let mut decoding = candidates(&keys);
        if decoding.is_empty() {
            keys = self.after_unknown_kid(&keys).await.map_err(unavailable)?;
            decoding = candidates(&keys);
            if decoding.is_empty() {
                return Err(invalid(InvalidToken::UnknownKey));
            }
        }
        let data = self.decode_with_any(token, header.alg, &decoding)?;
        self.check_claims(&data.claims).map_err(invalid)?;
        Ok(data.claims)
    }
}

/// Makes the process-level crypto provider of `jsonwebtoken` this crate's, aws-lc-rs. The library chooses it from
/// its features when exactly one backend is enabled, and **panics on the first verification when two are**, as
/// they are in a binary that links another crate with the `rust_crypto` backend. Idempotent: the provider can be
/// installed once per process, and one that is already there (ours, or one the binary chose) is left alone.
fn install_crypto_provider() {
    let _ = jsonwebtoken::crypto::aws_lc::DEFAULT_PROVIDER.install_default();
}

/// Waits for a fetch to be over: its sender is dropped, and nothing is ever sent.
async fn finished(mut wait: watch::Receiver<()>) {
    let _ = wait.changed().await;
}

/// The word for what the library refused. The closed set has no word for a token that is not valid *yet*
/// (`nbf`), and `Expired` is the nearest: the token is outside its time. The only claim the library requires
/// is `exp`; `iss`, `aud` and `sub` are checked by this crate, with their own words.
fn reason(kind: &ErrorKind) -> InvalidToken {
    match kind {
        ErrorKind::InvalidSignature => InvalidToken::BadSignature,
        ErrorKind::ExpiredSignature | ErrorKind::ImmatureSignature => InvalidToken::Expired,
        ErrorKind::MissingRequiredClaim(_) => InvalidToken::MissingClaim,
        _ => InvalidToken::Malformed,
    }
}

impl Authenticator for OidcAuthenticator {
    async fn authenticate(&self, token: Option<&str>) -> Result<Principal, AuthError> {
        let Some(token) = token.filter(|token| !token.is_empty()) else {
            return Err(AuthError::Missing);
        };
        let inner = &self.inner;
        let claims = inner.verify(token).await?;
        claims::principal(&claims, &inner.roles_claim).map_err(AuthError::Invalid)
    }
}
