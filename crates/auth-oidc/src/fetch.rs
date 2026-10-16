//! Asking the issuer for its keys: the discovery document, when no `jwksUrl` is configured, and the JWKS.
//! Both are plain HTTP GETs of JSON.
//!
//! The issuer and the JWKS URL are the operator's own configuration, not an address a caller supplies, and a
//! deployment's identity provider is as often a private address (a Keycloak in the cluster) as a public one,
//! so there is no filter on the address as there would be for a URL a person typed. What the requests do is
//! bounded: a timeout, no redirect (a redirected request would go where the configuration did not name), a
//! body of at most 1 MiB, and the discovery document must say it is the issuer's own.
//!
//! **No request carries anything of a token**: the URLs are the configured ones, so a failure here cannot
//! name a `kid` or a claim, and [`FetchFailure`] is a closed set of words and one status code besides.

use std::fmt;
use std::time::Duration;

use reqwest::header::{ACCEPT, HeaderValue};
use serde_json::Value;
use url::Url;

/// The largest body read from the issuer.
pub(crate) const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Why the issuer's keys could not be had. A closed set: the words of a log and of the source of
/// [`AuthError::Unavailable`](aap_ports::AuthError::Unavailable), never a text taken from the issuer's
/// answer, from a token or from an error of the transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum FetchFailure {
    /// The request took longer than the timeout.
    Timeout,
    /// The connection could not be made (the name, the address, the handshake).
    Connect,
    /// The request failed on the way, or the answer could not be read to its end.
    Transport,
    /// The answer's status was not a success. A `3xx` is a redirect, which is never followed.
    Status(u16),
    /// The answer is larger than 1 MiB.
    TooLarge,
    /// The answer is not JSON.
    NotJson,
    /// The answer is JSON and not a key set.
    NotAKeySet,
    /// The key set has no key this build can verify with.
    NoUsableKey,
    /// The discovery document names another issuer than the configured one.
    DiscoveryIssuerMismatch,
    /// The discovery document has no `jwks_uri`.
    NoJwksUri,
    /// The `jwks_uri` is not an `http(s)` URL without credentials, or downgrades an `https` issuer to `http`.
    BadJwksUri,
}

impl fmt::Display for FetchFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => f.write_str("the request to the issuer timed out"),
            Self::Connect => f.write_str("the issuer could not be reached"),
            Self::Transport => f.write_str("the request to the issuer failed"),
            Self::Status(code) if (300..400).contains(code) => write!(
                f,
                "the issuer answered {code}, a redirect, which is never followed (configure the final URL)"
            ),
            Self::Status(code) => write!(f, "the issuer answered {code}"),
            Self::TooLarge => f.write_str("the issuer's answer is larger than 1 MiB"),
            Self::NotJson => f.write_str("the issuer's answer is not JSON"),
            Self::NotAKeySet => f.write_str("the issuer's answer is not a key set"),
            Self::NoUsableKey => f.write_str("the key set has no key this build can verify with"),
            Self::DiscoveryIssuerMismatch => {
                f.write_str("the discovery document names another issuer")
            }
            Self::NoJwksUri => f.write_str("the discovery document has no jwks_uri"),
            Self::BadJwksUri => f.write_str(
                "the jwks_uri is not an http(s) URL without credentials, or downgrades an https issuer",
            ),
        }
    }
}

impl std::error::Error for FetchFailure {}

/// Where the JWKS is: configured, or found in the discovery document.
#[derive(Debug, Clone)]
pub(crate) enum Source {
    /// `jwksUrl`.
    Url(Url),
    /// `{issuer}/.well-known/openid-configuration`, which names the JWKS.
    Discover { issuer: String, document: Url },
}

pub(crate) struct Fetcher {
    http: reqwest::Client,
    source: Source,
}

impl Fetcher {
    pub(crate) fn new(http: reqwest::Client, source: Source) -> Self {
        Self { http, source }
    }

    /// The JWKS document.
    pub(crate) async fn jwks(&self) -> Result<Value, FetchFailure> {
        let url = match &self.source {
            Source::Url(url) => url.clone(),
            Source::Discover { issuer, document } => self.discover(issuer, document).await?,
        };
        self.get_json(&url).await
    }

    /// The JWKS URL the discovery document names. The document must carry the issuer it was asked for (OpenID
    /// Connect Discovery 1.0, 4.3: a document that says another issuer is a mix-up), and the JWKS must not be
    /// reached over plain HTTP from an HTTPS issuer.
    async fn discover(&self, issuer: &str, document: &Url) -> Result<Url, FetchFailure> {
        let doc = self.get_json(document).await?;
        if doc.get("issuer").and_then(Value::as_str) != Some(issuer) {
            return Err(FetchFailure::DiscoveryIssuerMismatch);
        }
        let uri = doc
            .get("jwks_uri")
            .and_then(Value::as_str)
            .ok_or(FetchFailure::NoJwksUri)?;
        jwks_uri(document, uri)
    }

    async fn get_json(&self, url: &Url) -> Result<Value, FetchFailure> {
        let mut response = self
            .http
            .get(url.clone())
            .header(ACCEPT, HeaderValue::from_static("application/json"))
            .send()
            .await
            .map_err(|e| failure(&e))?;
        let status = response.status();
        if !status.is_success() {
            return Err(FetchFailure::Status(status.as_u16()));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|e| failure(&e))? {
            if body.len() + chunk.len() > MAX_BODY_BYTES {
                return Err(FetchFailure::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| FetchFailure::NotJson)
    }
}

/// The closed word for a transport error. The error's own text (an address, a resolver's or a handshake's
/// message, the configured URL: nothing of a token, which no request carries) goes to the debug log and
/// nowhere else.
fn failure(error: &reqwest::Error) -> FetchFailure {
    tracing::debug!(%error, "the request to the issuer failed");
    if error.is_timeout() {
        FetchFailure::Timeout
    } else if error.is_connect() {
        FetchFailure::Connect
    } else {
        FetchFailure::Transport
    }
}

/// The JWKS URL a discovery document at `document` names: an `http(s)` URL without credentials, and not plain
/// HTTP when the issuer is served over HTTPS.
fn jwks_uri(document: &Url, uri: &str) -> Result<Url, FetchFailure> {
    let jwks = Url::parse(uri).map_err(|_| FetchFailure::BadJwksUri)?;
    if !matches!(jwks.scheme(), "http" | "https")
        || !jwks.has_host()
        || !jwks.username().is_empty()
        || jwks.password().is_some()
    {
        return Err(FetchFailure::BadJwksUri);
    }
    if document.scheme() == "https" && jwks.scheme() != "https" {
        return Err(FetchFailure::BadJwksUri);
    }
    Ok(jwks)
}

/// The client every fetch goes through: a timeout, and no redirect.
pub(crate) fn client(
    timeout: Duration,
    use_system_proxy: bool,
) -> Result<reqwest::Client, reqwest::Error> {
    let builder = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::none());
    let builder = if use_system_proxy {
        builder
    } else {
        builder.no_proxy()
    };
    builder.build()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn the_jwks_uri_is_an_http_url_that_does_not_downgrade_the_issuer() {
        let https = Url::parse("https://idp.example/.well-known/openid-configuration").unwrap();
        let http = Url::parse("http://idp.local/.well-known/openid-configuration").unwrap();
        // Another host is fine (some issuers keep their keys elsewhere); another scheme is not.
        assert!(jwks_uri(&https, "https://keys.example/jwks").is_ok());
        assert!(jwks_uri(&http, "http://idp.local/jwks").is_ok());
        assert!(jwks_uri(&http, "https://idp.local/jwks").is_ok());
        for bad in [
            "http://idp.example/jwks",
            "ftp://idp.example/jwks",
            "file:///etc/passwd",
            "https://user:pass@idp.example/jwks",
            "/jwks",
            "not a url",
            "",
        ] {
            assert_eq!(
                jwks_uri(&https, bad).unwrap_err(),
                FetchFailure::BadJwksUri,
                "{bad}"
            );
        }
        assert!(jwks_uri(&http, "https://user@idp.local/jwks").is_err());
    }

    #[test]
    fn a_failure_is_a_sentence_and_a_redirect_says_what_to_do() {
        assert_eq!(
            FetchFailure::Status(503).to_string(),
            "the issuer answered 503"
        );
        assert!(
            FetchFailure::Status(302)
                .to_string()
                .contains("never followed")
        );
    }
}
