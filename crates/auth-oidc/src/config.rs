//! The configuration of the authenticator, and what is refused at construction.
//!
//! [`OidcConfig`] is what a configuration file says (`bin/api` reads one YAML file, D7); [`Tuning`] is what
//! a test shortens and a file cannot: the intervals of the key cache are security settings (how long a
//! withdrawn key may still verify), not deployment settings.

use std::time::Duration;

use serde::Deserialize;
use url::Url;

/// How old the keys may be before they are fetched again (10 minutes).
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(600);

/// The least time between two fetches that an unknown `kid` causes (30 seconds).
pub const KID_REFETCH_INTERVAL: Duration = Duration::from_secs(30);

/// The least time between a failed fetch and the next one (5 seconds).
pub const RETRY_INTERVAL: Duration = Duration::from_secs(5);

/// How long keys older than the refresh interval go on verifying while the issuer cannot be reached (1 hour).
pub const STALE_GRACE: Duration = Duration::from_secs(3600);

/// The time one request to the issuer may take (5 seconds).
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// The leeway of `exp` and `nbf`, in seconds (§60a: 60).
pub const LEEWAY_SECS: u64 = 60;

/// The longest token read, in bytes. A longer one is refused unread.
pub const MAX_TOKEN_BYTES: usize = 16 * 1024;

/// The configuration of the authenticator (the `auth` block of the Platform API's file).
///
/// ```yaml
/// issuer: https://keycloak.example/realms/another-agentic
/// audiences: [another-agentic]
/// rolesClaim: agentic_roles
/// # jwksUrl: https://keycloak.example/realms/another-agentic/protocol/openid-connect/certs
/// ```
///
/// Reading it checks nothing: [`OidcAuthenticator::new`](crate::OidcAuthenticator::new) does
/// ([`validate`](Self::validate)). An unknown key is an error, so that a misspelt `audiences` does not leave
/// the API on its default.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OidcConfig {
    /// The issuer: `iss` must equal it exactly, and its discovery document is
    /// `{issuer}/.well-known/openid-configuration`.
    pub issuer: String,
    /// The audiences this API accepts: one of them must be among the token's `aud`. At least one.
    pub audiences: Vec<String>,
    /// The claim whose values are the person's permissions, as a dotted path into the objects of the token
    /// (`agentic_roles` on netcup; `realm_access.roles` is Keycloak's own). A claim whose own name has dots is
    /// found by that name first. There is no default: the API has no permission to give without it.
    pub roles_claim: String,
    /// Where the JWKS is, when not in the discovery document.
    #[serde(default)]
    pub jwks_url: Option<String>,
}

impl OidcConfig {
    /// A configuration for `issuer`, `audiences` and the claim that holds the permissions.
    pub fn new(
        issuer: impl Into<String>,
        audiences: impl IntoIterator<Item = impl Into<String>>,
        roles_claim: impl Into<String>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            audiences: audiences.into_iter().map(Into::into).collect(),
            roles_claim: roles_claim.into(),
            jwks_url: None,
        }
    }

    /// Reads the JWKS from `url` instead of the discovery document.
    #[must_use]
    pub fn with_jwks_url(mut self, url: impl Into<String>) -> Self {
        self.jwks_url = Some(url.into());
        self
    }

    /// Whether this configuration can work.
    ///
    /// # Errors
    ///
    /// [`ConfigError`] for an issuer or a JWKS URL that is not an absolute `http(s)` URL with a host and no
    /// credentials (and, for the issuer, no query or fragment), no audience or a blank one, and a blank
    /// roles claim.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.parts().map(|_| ())
    }

    /// The issuer and the JWKS URL, parsed.
    pub(crate) fn parts(&self) -> Result<(Url, Option<Url>), ConfigError> {
        let issuer = http_url(&self.issuer)
            .filter(|u| u.query().is_none() && u.fragment().is_none())
            .ok_or(ConfigError::Issuer)?;
        if self.audiences.is_empty() || self.audiences.iter().any(|a| a.trim().is_empty()) {
            return Err(ConfigError::Audiences);
        }
        if self.roles_claim.trim().is_empty() {
            return Err(ConfigError::RolesClaim);
        }
        let jwks = self
            .jwks_url
            .as_deref()
            .map(|url| http_url(url).ok_or(ConfigError::JwksUrl))
            .transpose()?;
        Ok((issuer, jwks))
    }
}

/// An absolute `http(s)` URL with a host and without credentials, written without surrounding white space
/// (the text is compared to `iss`, so a space that the URL parser would drop would never match).
pub(crate) fn http_url(text: &str) -> Option<Url> {
    if text != text.trim() {
        return None;
    }
    let url = Url::parse(text).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.has_host()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

/// The intervals of the key cache and the limits of a fetch. The defaults are the constants of this crate;
/// a test shortens them. Not readable from a file.
#[derive(Clone, Debug)]
pub struct Tuning {
    /// How old the keys may be before they are fetched again.
    pub refresh_interval: Duration,
    /// The least time between two fetches that an unknown `kid` causes.
    pub kid_refetch_interval: Duration,
    /// The least time between a failed fetch and the next one.
    pub retry_interval: Duration,
    /// How long keys older than the refresh interval go on verifying while the issuer cannot be reached.
    pub stale_grace: Duration,
    /// The time one request to the issuer may take.
    pub timeout: Duration,
    /// Whether to honour `HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY` from the environment.
    pub use_system_proxy: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            refresh_interval: REFRESH_INTERVAL,
            kid_refetch_interval: KID_REFETCH_INTERVAL,
            retry_interval: RETRY_INTERVAL,
            stale_grace: STALE_GRACE,
            timeout: FETCH_TIMEOUT,
            use_system_proxy: true,
        }
    }
}

/// Why an [`OidcAuthenticator`](crate::OidcAuthenticator) could not be built. None of the variants holds a
/// value of the configuration: a log of it shows which setting is wrong, not what it said.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The issuer is not an absolute `http(s)` URL with a host, without credentials, query, fragment or
    /// white space around it.
    #[error("the issuer is not an http(s) URL with a host, without credentials, query or fragment")]
    Issuer,
    /// The JWKS URL is not an absolute `http(s)` URL with a host and no credentials.
    #[error("the JWKS URL is not an http(s) URL with a host and no credentials")]
    JwksUrl,
    /// No audience, or a blank one: every token would be refused, or every one accepted.
    #[error("at least one audience is needed, and none may be blank")]
    Audiences,
    /// The roles claim is blank: nobody would hold a permission.
    #[error("the roles claim is blank")]
    RolesClaim,
    /// The HTTP client could not be built (the TLS roots, the proxy settings).
    #[error("cannot build the HTTP client")]
    HttpClient(#[source] Box<dyn std::error::Error + Send + Sync>),
}
