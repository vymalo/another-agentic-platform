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
//! - **Key**: the key of the issuer's JWKS that the header's `kid` names and whose kind is the algorithm's. A
//!   JWKS key that cannot verify (symmetric, too small, for encryption, another curve) is skipped.
//! - **Signature**, then **claims**: `iss` equal to the configured issuer; one of the configured audiences in
//!   `aud`; `exp` required and not past, `nbf` not in the future, both with 60 seconds of leeway
//!   ([`LEEWAY_SECS`]); `sub` required. `iat` is **not** required.
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
//! failure ([`RETRY_INTERVAL`]). One request at a time. While there is no copy, or it is older than the
//! refresh interval plus an hour ([`STALE_GRACE`]), every token is [`AuthError::Unavailable`] (fail closed,
//! and not a refusal: a caller may retry); in between, the copy goes on verifying while the issuer is down.
//!
//! The keys are read from `jwksUrl`, or from the `jwks_uri` of `{issuer}/.well-known/openid-configuration`,
//! whose `issuer` must be the configured one. No redirect is followed, a body over 1 MiB is refused, a request
//! times out, and `HTTP(S)_PROXY` is honoured.

#![warn(missing_docs)]

mod claims;
mod config;
mod fetch;
mod keys;
#[cfg(feature = "testkit")]
pub mod testkit;

use std::sync::Arc;
use std::time::Instant;

use aap_ports::{AuthError, Authenticator, InvalidToken, Principal};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, Validation, decode, decode_header};
use serde_json::{Map, Value};
use tokio::sync::Mutex;
use url::Url;

pub use crate::config::{
    ConfigError, FETCH_TIMEOUT, KID_REFETCH_INTERVAL, LEEWAY_SECS, MAX_TOKEN_BYTES, OidcConfig,
    REFRESH_INTERVAL, RETRY_INTERVAL, STALE_GRACE, Tuning,
};
pub use crate::fetch::FetchFailure;
use crate::fetch::{Fetcher, Source};
use crate::keys::{ALLOWED, KeySet};

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

/// The copy of the keys, and what is known about fetching them.
#[derive(Default)]
struct State {
    /// The keys of the last successful fetch, and when it started.
    keys: Option<(Arc<KeySet>, Instant)>,
    /// When the last fetch, successful or not, started.
    last_attempt: Option<Instant>,
    /// When the last fetch failed and why, until one succeeds.
    last_failure: Option<(Instant, FetchFailure)>,
}

struct Inner {
    issuer: String,
    audiences: Vec<String>,
    roles_claim: String,
    tuning: Tuning,
    fetcher: Fetcher,
    state: Mutex<State>,
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
        let (issuer_url, jwks_url) = config.parts()?;
        let source = match jwks_url {
            Some(url) => Source::Url(url),
            None => {
                let base = issuer_url.as_str().trim_end_matches('/');
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
    /// Fetches the keys, under the lock, and records what came of it.
    async fn fetch(&self, state: &mut State) -> Result<(), FetchFailure> {
        let started = Instant::now();
        state.last_attempt = Some(started);
        let fetched = match self.fetcher.jwks().await {
            Ok(document) => match KeySet::parse(&document) {
                Some(parsed) if !parsed.set.is_empty() => Ok(parsed),
                Some(_) => Err(FetchFailure::NoUsableKey),
                None => Err(FetchFailure::NotAKeySet),
            },
            Err(cause) => Err(cause),
        };
        match fetched {
            Ok(parsed) => {
                if state.last_failure.take().is_some() {
                    tracing::info!("the token issuer's keys can be fetched again");
                }
                if parsed.skipped > 0 {
                    tracing::debug!(
                        skipped = parsed.skipped,
                        "keys of the issuer this build cannot use"
                    );
                }
                state.keys = Some((Arc::new(parsed.set), started));
                Ok(())
            }
            Err(cause) => {
                if state.last_failure.is_none() {
                    tracing::warn!(%cause, "the token issuer's keys cannot be fetched; tokens are refused until they can");
                } else {
                    tracing::debug!(%cause, "the token issuer's keys still cannot be fetched");
                }
                state.last_failure = Some((Instant::now(), cause));
                Err(cause)
            }
        }
    }

    /// The copy, if it may still verify: no older than the refresh interval and the grace.
    fn usable(&self, state: &State) -> Option<Arc<KeySet>> {
        let (keys, at) = state.keys.as_ref()?;
        (at.elapsed() <= self.tuning.refresh_interval + self.tuning.stale_grace)
            .then(|| Arc::clone(keys))
    }

    /// The keys to verify with: the copy while it is fresh, else a fetch (not sooner than the retry interval
    /// after a failure), else the stale copy within its grace.
    async fn current(&self) -> Result<Arc<KeySet>, KeysUnavailable> {
        let mut state = self.state.lock().await;
        if let Some((keys, at)) = &state.keys
            && at.elapsed() < self.tuning.refresh_interval
        {
            return Ok(Arc::clone(keys));
        }
        let waiting = state
            .last_failure
            .is_some_and(|(failed, _)| failed.elapsed() < self.tuning.retry_interval);
        if !waiting {
            // The outcome is in the state: a failure is in `last_failure`, a success in `keys`.
            let _ = self.fetch(&mut state).await;
        }
        self.usable(&state).ok_or_else(|| {
            let cause = state.last_failure.map(|(_, cause)| cause);
            if state.keys.is_some() {
                KeysUnavailable::TooOld(cause)
            } else {
                KeysUnavailable::NoCopy(cause)
            }
        })
    }

    /// The keys after a token named a `kid` that `seen` lacks: fetched again, unless a fetch was tried within
    /// the interval, or someone else already did.
    ///
    /// # Errors
    ///
    /// [`KeysUnavailable::Unrefreshed`] when the keys cannot be fetched: the token might be good, signed by a
    /// key the issuer has published since, and it is not told that it is bad. When the last fetch succeeded
    /// the key is not the issuer's, and the caller says so.
    async fn after_unknown_kid(&self, seen: &Arc<KeySet>) -> Result<Arc<KeySet>, KeysUnavailable> {
        let mut state = self.state.lock().await;
        if let Some((keys, _)) = &state.keys
            && !Arc::ptr_eq(keys, seen)
        {
            return Ok(Arc::clone(keys));
        }
        let recent = state
            .last_attempt
            .is_some_and(|at| at.elapsed() < self.tuning.kid_refetch_interval);
        if !recent {
            let _ = self.fetch(&mut state).await;
        }
        if let Some((_, cause)) = state.last_failure {
            return Err(KeysUnavailable::Unrefreshed(cause));
        }
        Ok(self.usable(&state).unwrap_or_else(|| Arc::clone(seen)))
    }

    fn validation(&self, alg: Algorithm) -> Validation {
        let mut validation = Validation::new(alg);
        validation.leeway = LEEWAY_SECS;
        validation.validate_exp = true;
        validation.validate_nbf = true;
        validation.validate_aud = true;
        validation.set_issuer(&[&self.issuer]);
        validation.set_audience(&self.audiences);
        // `iat` is not required: §60a names `exp` only.
        validation.set_required_spec_claims(&["exp", "iss", "aud"]);
        validation
    }

    /// The claims of `token`, once its size, header, key, signature and claims have passed.
    async fn verify(&self, token: &str) -> Result<Map<String, Value>, AuthError> {
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
        let mut keys = self.current().await.map_err(unavailable)?;
        let key = match keys.select(header.alg, header.kid.as_deref()) {
            Some(key) => key.decoding.clone(),
            None => {
                keys = self.after_unknown_kid(&keys).await.map_err(unavailable)?;
                match keys.select(header.alg, header.kid.as_deref()) {
                    Some(key) => key.decoding.clone(),
                    None => return Err(invalid(InvalidToken::UnknownKey)),
                }
            }
        };
        let data = decode::<Map<String, Value>>(token, &key, &self.validation(header.alg))
            .map_err(|e| invalid(reason(e.kind())))?;
        Ok(data.claims)
    }
}

/// The word for what the library refused. The closed set has no word for a token that is not valid *yet*
/// (`nbf`), and `Expired` is the nearest: the token is outside its time. A missing `iss` or `aud` is the
/// wrong issuer or audience: the token does not name this API; `MissingClaim` is for `sub` and `exp`.
fn reason(kind: &ErrorKind) -> InvalidToken {
    match kind {
        ErrorKind::InvalidSignature => InvalidToken::BadSignature,
        ErrorKind::ExpiredSignature | ErrorKind::ImmatureSignature => InvalidToken::Expired,
        ErrorKind::InvalidIssuer => InvalidToken::WrongIssuer,
        ErrorKind::InvalidAudience => InvalidToken::WrongAudience,
        ErrorKind::MissingRequiredClaim(claim) => match claim.as_str() {
            "iss" => InvalidToken::WrongIssuer,
            "aud" => InvalidToken::WrongAudience,
            _ => InvalidToken::MissingClaim,
        },
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
