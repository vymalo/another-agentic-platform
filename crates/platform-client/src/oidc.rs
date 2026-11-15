//! The sign-in of `aapctl` (§60b, K9b-2): OAuth 2.0 authorization code with PKCE (RFC 7636) for a public client, the refresh grant
//! (RFC 6749 §6) and the revocation of a refresh token (RFC 7009), over OpenID Connect discovery.
//!
//! * **Concrete code, no OIDC crate** (AD-020 is about the seams behind the API, and the issuer is a protocol, not a swappable
//!   implementation): the requests are built here over the HTTP rules of [`crate::Client`]: no redirect is followed, every request
//!   has a timeout, every answer a cap, `https` unless the host is a loopback address, and **never a proxy for a plain-`http`
//!   loopback host**.
//! * **What is secret is a [`Secret`]**: the verifier, the code and the refresh token have no `Debug` of their text and no
//!   `Display`. No [`OidcError`] holds a URL, a header, a code, a token or the text of the transport's error, and the error code an
//!   issuer sends back is cut to the characters RFC 6749 allows and to 64 of them.
//! * **No ID token is read.** Only the access token is used: the Platform API is the one that decides whether it is good.
//!
//! The listener the browser is sent back to, and the browser, are the caller's: this module builds the URL and takes the code.

use std::fmt;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use url::Url;

use crate::{Token, classify, is_loopback, read_capped};

/// The most an answer of the issuer may hold: its documents are a few kilobytes.
const MAX_ANSWER_BYTES: usize = 1 << 20;

/// The longest lifetime an access token may claim: a year.
const MAX_EXPIRES_IN: u64 = 31_536_000;

/// The longest a refresh token or any other secret of the exchange may be.
const MAX_SECRET_BYTES: usize = 16 * 1024;

/// Why a step of the sign-in did not give an answer.
#[derive(Debug, thiserror::Error)]
pub enum OidcError {
    /// A value the caller gave cannot be used (an issuer that is not an `https` address, a client id): nothing was sent.
    #[error("{0}")]
    Config(String),
    /// The issuer could not be reached or did not answer in time. `host` is `host[:port]` only; `class` a fixed phrase.
    #[error("cannot reach {host}: {class}")]
    Unreachable {
        /// The issuer's host and port.
        host: String,
        /// What went wrong, in a fixed set of phrases.
        class: &'static str,
    },
    /// The issuer answered with a status that is neither success nor an OAuth error (`5xx`, `429`, a redirect, a `404`).
    #[error("{host} answered {status}")]
    Server {
        /// The issuer's host and port.
        host: String,
        /// The status.
        status: u16,
    },
    /// A success whose body is not what the specifications describe, or a document that names another issuer or an endpoint that
    /// must not be used. The message holds no part of the body.
    #[error("the issuer's answer is not usable: {0}")]
    BadAnswer(String),
    /// The token endpoint refused the request with an OAuth error. `code` is the error code only, cleaned.
    #[error("the token endpoint refused the request: {code}")]
    Refused {
        /// `invalid_grant`, `invalid_client`, `access_denied`…, or `unrecognised`.
        code: String,
    },
    /// The refresh token is no longer good (`invalid_grant`): the person signs in again.
    #[error("the refresh token is not good any more")]
    InvalidGrant,
}

/// A value that must not be printed: the PKCE verifier, an authorization code, a refresh token.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    /// A secret from text: visible ASCII with no white space, and not empty or huge.
    ///
    /// # Errors
    ///
    /// [`OidcError::BadAnswer`], whose message never holds the text.
    pub fn new(text: &str) -> Result<Self, OidcError> {
        let text = text.trim();
        if text.is_empty()
            || text.len() > MAX_SECRET_BYTES
            || !text.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(OidcError::BadAnswer(
                "a token or code is empty, too long, or has a character that is not visible ASCII in it".to_owned(),
            ));
        }
        Ok(Self(text.to_owned()))
    }

    /// The text. Only for the request that needs it and for the 0600 file.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(…)")
    }
}

/// How requests to the issuer are made.
#[derive(Clone, Copy, Debug)]
pub struct OidcOptions {
    /// Use the proxy the environment names. **Never for an `http://` loopback host, whatever this says.**
    pub system_proxy: bool,
    /// How long to wait for a connection.
    pub connect_timeout: Duration,
    /// How long a whole request may take.
    pub request_timeout: Duration,
}

impl Default for OidcOptions {
    fn default() -> Self {
        Self {
            system_proxy: true,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
        }
    }
}

/// What discovery found out about the issuer, checked.
#[derive(Clone, Debug)]
pub struct Discovery {
    /// The issuer, exactly as the document says it (and as the caller named it).
    pub issuer: String,
    /// Where the browser is sent.
    pub authorization_endpoint: Url,
    /// Where the code and the refresh token are exchanged.
    pub token_endpoint: Url,
    /// Where a refresh token is revoked, if the issuer says **and the endpoint can be used** (see [`revocation_unusable`]).
    ///
    /// [`revocation_unusable`]: Discovery::revocation_unusable
    pub revocation_endpoint: Option<Url>,
    /// The document names a `revocation_endpoint` that the rules for an endpoint refuse: it is not used, and the caller says so.
    pub revocation_unusable: bool,
    /// The document lists `code_challenge_methods_supported` (and `S256` is in it). `false`: the issuer did not say, and the sign-in
    /// goes on protected by PKCE only if the issuer honours it.
    pub pkce_advertised: bool,
    /// The document says `authorization_response_iss_parameter_supported: true` (RFC 9207): a callback without `iss` is refused.
    pub iss_parameter_supported: bool,
}

/// What [`revoke`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revocation {
    /// The issuer revoked the token.
    Revoked,
    /// The issuer names no revocation endpoint: nothing was asked.
    NoEndpoint,
    /// The issuer names one that cannot be used (see [`Discovery::revocation_unusable`]): nothing was sent to it.
    EndpointUnusable,
}

/// The tokens an exchange gave.
#[derive(Debug)]
pub struct Tokens {
    /// The access token: what the Platform API is called with.
    pub access_token: Token,
    /// Seconds it is good for, if the issuer says (a bounded integer).
    pub expires_in: Option<u64>,
    /// A refresh token, if the issuer gave one (a rotated one replaces the old).
    pub refresh_token: Option<Secret>,
}

/// An endpoint of an issuer that a secret may be sent to: it is [`usable_endpoint`], and **it follows the issuer's scheme**: the
/// document of an `https` issuer may not name an `http` endpoint (not even a loopback one: a document is not the machine's), and the
/// endpoints of a loopback `http` issuer must be on a loopback host, so a document cannot send a code or a token somewhere else.
fn endpoint_allowed(issuer: &Url, endpoint: &Url) -> bool {
    usable_endpoint(endpoint)
        && if issuer.scheme() == "https" {
            endpoint.scheme() == "https"
        } else {
            is_loopback(endpoint)
        }
}

/// An endpoint the caller may send a secret to: `https`, or `http` for a loopback host only (a test issuer, a port-forward); no user
/// name or password; no fragment.
fn usable_endpoint(url: &Url) -> bool {
    let scheme_ok = match url.scheme() {
        "https" => true,
        "http" => is_loopback(url),
        _ => false,
    };
    scheme_ok
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

fn host_of(url: &Url) -> String {
    match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_owned(),
        _ => "the issuer".to_owned(),
    }
}

fn http_for(url: &Url, options: &OidcOptions) -> Result<reqwest::Client, OidcError> {
    let mut builder = reqwest::Client::builder()
        // A redirect would carry the request, and the person's trust, somewhere the person did not name.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(options.connect_timeout)
        .timeout(options.request_timeout)
        .user_agent(concat!("aapctl/", env!("CARGO_PKG_VERSION")));
    // A proxy named by the environment is a third party: it would see the code and the verifier of a request that never leaves the
    // machine.
    if !options.system_proxy || (url.scheme() == "http" && is_loopback(url)) {
        builder = builder.no_proxy();
    }
    builder
        .build()
        .map_err(|_| OidcError::Config("the HTTP client cannot be built".to_owned()))
}

/// One request, its answer capped. `trace` gets two lines: the method and the host, and the status. Never a path, a query or a body.
async fn request(
    method: reqwest::Method,
    url: &Url,
    form: Option<&[(&str, &str)]>,
    options: &OidcOptions,
    trace: &mut Vec<String>,
) -> Result<(u16, Vec<u8>), OidcError> {
    let host = host_of(url);
    let http = http_for(url, options)?;
    let mut builder = http.request(method.clone(), url.clone());
    if let Some(form) = form {
        let mut body = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in form {
            body.append_pair(k, v);
        }
        builder = builder
            .header("content-type", "application/x-www-form-urlencoded")
            .header("accept", "application/json")
            .body(body.finish());
    } else {
        builder = builder.header("accept", "application/json");
    }
    trace.push(format!("> {method} {host}"));
    let unreachable = |e: &reqwest::Error| OidcError::Unreachable {
        host: host.clone(),
        class: classify(e),
    };
    let response = builder.send().await.map_err(|e| unreachable(&e))?;
    let status = response.status().as_u16();
    trace.push(format!("< {status}"));
    let body = read_capped(response, MAX_ANSWER_BYTES)
        .await
        .map_err(|e| unreachable(&e))?
        .ok_or_else(|| {
            OidcError::BadAnswer("the answer is far larger than any this protocol sends".to_owned())
        })?;
    Ok((status, body))
}

/// Read `GET {issuer}/.well-known/openid-configuration` and check it: its `issuer` is exactly `issuer` (OpenID Connect Discovery 1.0
/// §4.3), it names an authorization endpoint and a token endpoint that can be used, and if it lists `code_challenge_methods_supported`
/// then `S256` is among them.
///
/// # Errors
///
/// [`OidcError::Config`] for an issuer that is not an `https` address (or `http` on a loopback host);
/// [`OidcError::Unreachable`] and [`OidcError::Server`] when it cannot be read; [`OidcError::BadAnswer`] for a document that fails a
/// check.
pub async fn discover(
    issuer: &str,
    options: &OidcOptions,
    trace: &mut Vec<String>,
) -> Result<Discovery, OidcError> {
    let base = Url::parse(issuer)
        .map_err(|_| OidcError::Config("the issuer is not an address".to_owned()))?;
    if !usable_endpoint(&base) || base.query().is_some() {
        return Err(OidcError::Config(
            "the issuer must be an https address with no user name, query or fragment (http only for a loopback host)".to_owned(),
        ));
    }
    let well_known = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let url = Url::parse(&well_known)
        .map_err(|_| OidcError::Config("the issuer is not an address".to_owned()))?;
    let (status, body) = request(reqwest::Method::GET, &url, None, options, trace).await?;
    if status != 200 {
        return Err(OidcError::Server {
            host: host_of(&url),
            status,
        });
    }
    let doc: Value = serde_json::from_slice(&body)
        .map_err(|_| OidcError::BadAnswer("the discovery document is not JSON".to_owned()))?;
    let text = |key: &str| doc.get(key).and_then(Value::as_str);
    // Exactly: OpenID Connect Discovery 1.0 section 4.3 compares the strings, so a parent, a child and a trailing slash are all another
    // issuer.
    if text("issuer") != Some(issuer) {
        let slash_only = text("issuer")
            .is_some_and(|named| named.trim_end_matches('/') == issuer.trim_end_matches('/'));
        return Err(OidcError::BadAnswer(if slash_only {
            "the discovery document names the issuer with or without a trailing slash where the one asked for has the other: write --issuer exactly as the document does, which is how the issuer is compared".to_owned()
        } else {
            "the discovery document names another issuer than the one asked for, so it is not that issuer's".to_owned()
        }));
    }
    let endpoint = |key: &str| -> Result<Url, OidcError> {
        let value = text(key)
            .ok_or_else(|| OidcError::BadAnswer(format!("the discovery document has no {key}")))?;
        let url = Url::parse(value)
            .map_err(|_| OidcError::BadAnswer(format!("{key} is not an address")))?;
        if endpoint_allowed(&base, &url) {
            Ok(url)
        } else {
            Err(OidcError::BadAnswer(format!(
                "{key} is not an address on the issuer's own terms (https for an https issuer, a loopback address for a loopback http one, no user name or fragment): no code or token is sent to it"
            )))
        }
    };
    let authorization_endpoint = endpoint("authorization_endpoint")?;
    let token_endpoint = endpoint("token_endpoint")?;
    let mut pkce_advertised = false;
    if let Some(methods) = doc.get("code_challenge_methods_supported") {
        let has_s256 = methods
            .as_array()
            .is_some_and(|a| a.iter().any(|m| m.as_str() == Some("S256")));
        if !has_s256 {
            return Err(OidcError::BadAnswer(
                "the issuer lists the PKCE methods it supports and S256 is not among them"
                    .to_owned(),
            ));
        }
        pkce_advertised = true;
    }
    let (revocation_endpoint, revocation_unusable) = if text("revocation_endpoint").is_some() {
        match endpoint("revocation_endpoint") {
            Ok(url) => (Some(url), false),
            Err(_) => (None, true),
        }
    } else {
        (None, false)
    };
    Ok(Discovery {
        issuer: issuer.to_owned(),
        authorization_endpoint,
        token_endpoint,
        revocation_endpoint,
        revocation_unusable,
        pkce_advertised,
        iss_parameter_supported: doc
            .get("authorization_response_iss_parameter_supported")
            .and_then(Value::as_bool)
            == Some(true),
    })
}

/// The PKCE verifier and challenge (RFC 7636) and the `state` of one sign-in.
pub struct Pkce {
    verifier: Secret,
    challenge: String,
    state: String,
}

impl fmt::Debug for Pkce {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Pkce(…)")
    }
}

/// `base64url(SHA-256(verifier))` without padding: the S256 challenge of RFC 7636 §4.2.
pub fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// 32 bytes of the operating system's randomness as base64url without padding (43 characters): the verifier, the `state` and the
/// ticket of the link the browser is opened with.
///
/// # Errors
///
/// [`OidcError::Config`] when the system has no randomness.
pub fn random_token() -> Result<String, OidcError> {
    let mut bytes = [0_u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|_| {
        OidcError::Config("the system has no source of randomness to sign in with".to_owned())
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

impl Pkce {
    /// A fresh verifier (32 bytes of the operating system's randomness, base64url without padding: 43 characters), its S256
    /// challenge and a `state` of 32 random bytes.
    ///
    /// # Errors
    ///
    /// [`OidcError::Config`] when the system has no randomness.
    pub fn new() -> Result<Self, OidcError> {
        let verifier = random_token()?;
        let challenge = challenge_of(&verifier);
        Ok(Self {
            verifier: Secret(verifier),
            challenge,
            state: random_token()?,
        })
    }

    /// The verifier, for the token request and nothing else.
    pub fn verifier(&self) -> &Secret {
        &self.verifier
    }

    /// The challenge, which goes in the URL.
    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    /// The `state`, which goes in the URL.
    pub fn state(&self) -> &str {
        &self.state
    }

    /// Whether `got` is this sign-in's `state`, compared in constant time.
    pub fn state_matches(&self, got: &str) -> bool {
        state_matches(&self.state, got)
    }
}

/// Whether two `state` values are equal, compared in constant time (for the bytes that are there: only a difference in length is
/// seen).
pub fn state_matches(expected: &str, got: &str) -> bool {
    expected.as_bytes().ct_eq(got.as_bytes()).into()
}

/// The URL the browser is sent to: the authorization endpoint with `response_type=code`, the client, the redirect, the scopes, the
/// `state` and the challenge with method `S256`. **Never the verifier.**
pub fn authorization_url(
    discovery: &Discovery,
    client_id: &str,
    redirect_uri: &str,
    scopes: &[String],
    pkce: &Pkce,
) -> Url {
    let mut url = discovery.authorization_endpoint.clone();
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", &scopes.join(" "))
        .append_pair("state", pkce.state())
        .append_pair("code_challenge", pkce.challenge())
        .append_pair("code_challenge_method", "S256");
    url
}

/// The characters RFC 6749 §5.2 allows in an `error` code, and the first 64 of them: what is safe to print and to show on a page.
pub fn error_code(value: Option<&str>) -> String {
    let code: String = value
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(64)
        .collect();
    if code.is_empty() {
        "unrecognised".to_owned()
    } else {
        code
    }
}

/// What an answer of the token endpoint is: tokens, or the failure it stands for.
fn read_token_answer(host: &str, status: u16, body: &[u8]) -> Result<Tokens, OidcError> {
    if status == 400 || status == 401 {
        let parsed: Option<Value> = serde_json::from_slice(body).ok();
        if let Some(code) = parsed
            .as_ref()
            .and_then(|v| v.get("error"))
            .and_then(Value::as_str)
        {
            let code = error_code(Some(code));
            return Err(if code == "invalid_grant" {
                OidcError::InvalidGrant
            } else {
                OidcError::Refused { code }
            });
        }
    }
    if status != 200 {
        return Err(OidcError::Server {
            host: host.to_owned(),
            status,
        });
    }
    let doc: Value = serde_json::from_slice(body)
        .map_err(|_| OidcError::BadAnswer("the token endpoint's answer is not JSON".to_owned()))?;
    let access = doc
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| OidcError::BadAnswer("the answer has no access_token".to_owned()))?;
    let access_token = Token::new(access).map_err(|_| {
        OidcError::BadAnswer(
            "the access_token is not a token that can be sent in a header".to_owned(),
        )
    })?;
    let bearer = doc
        .get("token_type")
        .and_then(Value::as_str)
        .is_some_and(|t| t.eq_ignore_ascii_case("bearer"));
    if !bearer {
        return Err(OidcError::BadAnswer(
            "the token_type is not Bearer, and a token of another type is not sent as one"
                .to_owned(),
        ));
    }
    let expires_in = match doc.get("expires_in") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_u64() {
            Some(n) if (1..=MAX_EXPIRES_IN).contains(&n) => Some(n),
            _ => {
                return Err(OidcError::BadAnswer(
                    "expires_in is not a whole number of seconds between 1 and a year".to_owned(),
                ));
            }
        },
    };
    let refresh_token = match doc.get("refresh_token") {
        None | Some(Value::Null) => None,
        Some(v) => Some(Secret::new(v.as_str().unwrap_or_default())?),
    };
    Ok(Tokens {
        access_token,
        expires_in,
        refresh_token,
    })
}

async fn token_request(
    discovery: &Discovery,
    options: &OidcOptions,
    form: &[(&str, &str)],
    trace: &mut Vec<String>,
) -> Result<Tokens, OidcError> {
    let (status, body) = request(
        reqwest::Method::POST,
        &discovery.token_endpoint,
        Some(form),
        options,
        trace,
    )
    .await?;
    read_token_answer(&host_of(&discovery.token_endpoint), status, &body)
}

/// Exchange the authorization `code` for tokens: `grant_type=authorization_code` with the `redirect_uri` of the authorization request
/// and the `code_verifier`, from a public client (no secret).
///
/// # Errors
///
/// [`OidcError::Refused`] (or [`OidcError::InvalidGrant`], for a code that is used, expired or whose verifier does not match);
/// [`OidcError::Unreachable`], [`OidcError::Server`] and [`OidcError::BadAnswer`] as for every step.
pub async fn exchange_code(
    discovery: &Discovery,
    options: &OidcOptions,
    client_id: &str,
    redirect_uri: &str,
    code: &Secret,
    verifier: &Secret,
    trace: &mut Vec<String>,
) -> Result<Tokens, OidcError> {
    token_request(
        discovery,
        options,
        &[
            ("grant_type", "authorization_code"),
            ("code", code.expose()),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", verifier.expose()),
        ],
        trace,
    )
    .await
    // A code that does not verify is a refusal of this sign-in, not the end of a saved session.
    .map_err(|e| match e {
        OidcError::InvalidGrant => OidcError::Refused {
            code: "invalid_grant".to_owned(),
        },
        other => other,
    })
}

/// Use a refresh token: `grant_type=refresh_token`. **The token is sent to the token endpoint of `discovery` and nowhere else**: the
/// caller discovered it from the issuer the token was saved with.
///
/// # Errors
///
/// [`OidcError::InvalidGrant`] when the issuer no longer accepts the token; the others as [`exchange_code`].
pub async fn refresh(
    discovery: &Discovery,
    options: &OidcOptions,
    client_id: &str,
    refresh_token: &Secret,
    trace: &mut Vec<String>,
) -> Result<Tokens, OidcError> {
    token_request(
        discovery,
        options,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose()),
            ("client_id", client_id),
        ],
        trace,
    )
    .await
}

/// Revoke a refresh token (RFC 7009) at the issuer's `revocation_endpoint`, once. [`Revocation::NoEndpoint`] when the issuer names none
/// and [`Revocation::EndpointUnusable`] when it names one that the rules for an endpoint refuse (nothing is sent to it).
///
/// # Errors
///
/// [`OidcError::Unreachable`], [`OidcError::Server`], or [`OidcError::Refused`].
pub async fn revoke(
    discovery: &Discovery,
    options: &OidcOptions,
    client_id: &str,
    refresh_token: &Secret,
    trace: &mut Vec<String>,
) -> Result<Revocation, OidcError> {
    if discovery.revocation_unusable {
        return Ok(Revocation::EndpointUnusable);
    }
    let Some(endpoint) = &discovery.revocation_endpoint else {
        return Ok(Revocation::NoEndpoint);
    };
    let (status, body) = request(
        reqwest::Method::POST,
        endpoint,
        Some(&[
            ("token", refresh_token.expose()),
            ("token_type_hint", "refresh_token"),
            ("client_id", client_id),
        ]),
        options,
        trace,
    )
    .await?;
    match status {
        200 => Ok(Revocation::Revoked),
        400 | 401 => {
            let parsed: Option<Value> = serde_json::from_slice(&body).ok();
            Err(OidcError::Refused {
                code: error_code(
                    parsed
                        .as_ref()
                        .and_then(|v| v.get("error"))
                        .and_then(Value::as_str),
                ),
            })
        }
        _ => Err(OidcError::Server {
            host: host_of(endpoint),
            status,
        }),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use serde_json::json;

    use super::*;

    #[test]
    fn the_challenge_is_the_s256_of_the_verifier() {
        // RFC 7636 Appendix B.
        assert_eq!(
            challenge_of("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn a_verifier_is_43_characters_of_the_base64url_alphabet_and_nothing_repeats() {
        let a = Pkce::new().unwrap();
        let b = Pkce::new().unwrap();
        assert_eq!(a.verifier().expose().len(), 43);
        assert!(
            a.verifier()
                .expose()
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
        );
        assert_eq!(a.challenge().len(), 43);
        assert_ne!(a.verifier(), b.verifier());
        assert_ne!(a.state(), b.state());
        assert_eq!(a.state().len(), 43, "32 random bytes");
        assert!(a.state_matches(a.state()));
        assert!(!a.state_matches(b.state()));
        assert!(!a.state_matches(""));
    }

    #[test]
    fn nothing_prints_a_secret() {
        let p = Pkce::new().unwrap();
        assert!(!format!("{p:?}").contains(p.verifier().expose()));
        assert!(!format!("{:?}", p.verifier()).contains(p.verifier().expose()));
    }

    #[test]
    fn an_error_code_keeps_the_characters_the_rfc_allows_and_64_of_them() {
        assert_eq!(error_code(Some("access_denied")), "access_denied");
        assert_eq!(error_code(Some("a\u{1b}[31mb<script>")), "a31mbscript");
        assert_eq!(error_code(Some(&"x".repeat(200))).len(), 64);
        assert_eq!(error_code(Some("\u{1b}<>")), "unrecognised");
        assert_eq!(error_code(None), "unrecognised");
    }

    #[test]
    fn a_token_answer_is_read_strictly() {
        let ok = br#"{"access_token":"abc.def.ghi","token_type":"bearer","expires_in":300,"refresh_token":"r-1"}"#;
        let t = read_token_answer("h", 200, ok).unwrap();
        assert_eq!(t.access_token.expose(), "abc.def.ghi");
        assert_eq!(t.expires_in, Some(300));
        assert_eq!(t.refresh_token.unwrap().expose(), "r-1");
        for bad in [
            &br#"{"access_token":"a b","token_type":"Bearer"}"#[..],
            br#"{"access_token":"abc","token_type":"MAC"}"#,
            br#"{"access_token":"abc"}"#,
            br#"{"access_token":"abc","token_type":"Bearer","expires_in":0}"#,
            br#"{"access_token":"abc","token_type":"Bearer","expires_in":99999999999}"#,
            br#"{"access_token":"abc","token_type":"Bearer","expires_in":"5"}"#,
            br#"{"access_token":"abc","token_type":"Bearer","refresh_token":"has space"}"#,
            b"not json",
        ] {
            assert!(
                matches!(
                    read_token_answer("h", 200, bad),
                    Err(OidcError::BadAnswer(_))
                ),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        assert!(matches!(
            read_token_answer(
                "h",
                400,
                br#"{"error":"invalid_grant","error_description":"secret words"}"#
            ),
            Err(OidcError::InvalidGrant)
        ));
        match read_token_answer(
            "h",
            400,
            br#"{"error":"access_denied","error_description":"secret words"}"#,
        ) {
            Err(OidcError::Refused { code }) => assert_eq!(code, "access_denied"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            read_token_answer("h", 503, b""),
            Err(OidcError::Server { status: 503, .. })
        ));
        assert!(matches!(
            read_token_answer("h", 404, b"{}"),
            Err(OidcError::Server { status: 404, .. })
        ));
    }

    // ------------------------------------------------------------------------------------------------ the second review

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A server that answers every request with the discovery document `make(issuer_without_path)` returns, and the issuer the
    /// document should be asked for at `path`.
    async fn serving(path: &str, make: impl FnOnce(&str) -> Value) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let body = make(&origin).to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let mut buffer = [0_u8; 2048];
                let _ = stream.read(&mut buffer).await;
                let answer = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(answer.as_bytes()).await;
            }
        });
        format!("{origin}{path}")
    }

    fn document(issuer: &str) -> Value {
        json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "code_challenge_methods_supported": ["S256"],
        })
    }

    async fn discovered(issuer: &str) -> Result<Discovery, OidcError> {
        discover(issuer, &OidcOptions::default(), &mut Vec::new()).await
    }

    #[tokio::test]
    async fn the_issuer_is_compared_exactly_and_a_trailing_slash_is_told() {
        // The document names a parent of the issuer asked for, a child, and the same with a slash: none of them is that issuer.
        let issuer = serving("/realms/x", document).await;
        let error = discovered(&issuer).await.unwrap_err();
        assert!(
            matches!(&error, OidcError::BadAnswer(m) if m.contains("another issuer") && !m.contains("trailing slash")),
            "{error}"
        );
        let issuer = serving("", |origin| document(&format!("{origin}/child"))).await;
        assert!(matches!(
            discovered(&issuer).await,
            Err(OidcError::BadAnswer(m)) if m.contains("another issuer")
        ));
        let issuer = serving("/", document).await;
        let error = discovered(&issuer).await.unwrap_err();
        assert!(
            matches!(&error, OidcError::BadAnswer(m) if m.contains("trailing slash")),
            "{error}"
        );
        // And the exact one is taken.
        let issuer = serving("", document).await;
        assert!(discovered(&issuer).await.is_ok());
    }

    #[tokio::test]
    async fn an_issuer_with_a_query_or_a_fragment_is_not_asked() {
        for issuer in [
            "http://127.0.0.1:1/?a=b",
            "http://127.0.0.1:1#f",
            "http://127.0.0.1:1?x",
        ] {
            assert!(
                matches!(discovered(issuer).await, Err(OidcError::Config(_))),
                "{issuer}"
            );
        }
    }

    #[test]
    fn an_endpoint_follows_the_issuers_scheme() {
        let allowed = |issuer: &str, endpoint: &str| {
            endpoint_allowed(&Url::parse(issuer).unwrap(), &Url::parse(endpoint).unwrap())
        };
        // An https issuer: https endpoints only, loopback or not.
        assert!(allowed(
            "https://idp.example.com/r",
            "https://idp.example.com/token"
        ));
        assert!(allowed(
            "https://idp.example.com/r",
            "https://other.example/token"
        ));
        assert!(!allowed(
            "https://idp.example.com/r",
            "http://127.0.0.1:8080/token"
        ));
        assert!(!allowed(
            "https://idp.example.com/r",
            "http://idp.example.com/token"
        ));
        // A loopback http issuer: loopback endpoints only.
        assert!(allowed("http://127.0.0.1:9", "http://127.0.0.1:9/token"));
        assert!(allowed("http://127.0.0.1:9", "https://localhost/token"));
        assert!(!allowed(
            "http://127.0.0.1:9",
            "https://idp.example.com/token"
        ));
        assert!(!allowed(
            "http://127.0.0.1:9",
            "http://idp.example.com/token"
        ));
        // And whatever the issuer, no user name or fragment.
        assert!(!allowed("https://a.example", "https://u:p@a.example/token"));
        assert!(!allowed("https://a.example", "https://a.example/token#f"));
    }

    #[tokio::test]
    async fn discovery_reads_the_pkce_and_iss_and_revocation_members() {
        let issuer = serving("", document).await;
        let d = discovered(&issuer).await.unwrap();
        assert!(d.pkce_advertised && !d.iss_parameter_supported && !d.revocation_unusable);
        assert!(d.revocation_endpoint.is_none());

        let issuer = serving("", |origin| {
            let mut doc = document(origin);
            doc.as_object_mut()
                .unwrap()
                .remove("code_challenge_methods_supported");
            doc["authorization_response_iss_parameter_supported"] = json!(true);
            doc["revocation_endpoint"] = json!(format!("{origin}/revoke"));
            doc
        })
        .await;
        let d = discovered(&issuer).await.unwrap();
        assert!(!d.pkce_advertised, "no list is not a list with S256");
        assert!(d.iss_parameter_supported);
        assert!(d.revocation_endpoint.is_some() && !d.revocation_unusable);

        // `plain` alone is a refusal; a list that has S256 is taken.
        let issuer = serving("", |origin| {
            let mut doc = document(origin);
            doc["code_challenge_methods_supported"] = json!(["plain"]);
            doc
        })
        .await;
        assert!(matches!(
            discovered(&issuer).await,
            Err(OidcError::BadAnswer(_))
        ));
        let issuer = serving("", |origin| {
            let mut doc = document(origin);
            doc["code_challenge_methods_supported"] = json!(["plain", "S256"]);
            doc
        })
        .await;
        assert!(discovered(&issuer).await.unwrap().pkce_advertised);

        // A revocation endpoint that cannot be used is not used, and is remembered as such.
        let issuer = serving("", |origin| {
            let mut doc = document(origin);
            doc["revocation_endpoint"] = json!("https://idp.example.com/revoke");
            doc
        })
        .await;
        let d = discovered(&issuer).await.unwrap();
        assert!(d.revocation_endpoint.is_none() && d.revocation_unusable);
        let secret = Secret::new("r-1").unwrap();
        let outcome = revoke(
            &d,
            &OidcOptions::default(),
            "aapctl",
            &secret,
            &mut Vec::new(),
        )
        .await;
        assert!(
            matches!(outcome, Ok(Revocation::EndpointUnusable)),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_token_answer_is_bounded_and_never_quotes_what_it_was_sent() {
        // An access token and a refresh token over 16 KiB are not kept.
        let long = "a".repeat(MAX_SECRET_BYTES + 1);
        let access = format!(r#"{{"access_token":"{long}","token_type":"Bearer"}}"#);
        assert!(matches!(
            read_token_answer("h", 200, access.as_bytes()),
            Err(OidcError::BadAnswer(_))
        ));
        let refresh =
            format!(r#"{{"access_token":"abc","token_type":"Bearer","refresh_token":"{long}"}}"#);
        assert!(matches!(
            read_token_answer("h", 200, refresh.as_bytes()),
            Err(OidcError::BadAnswer(_))
        ));
        // What a person may read of a failure never holds the body, whatever it was.
        for body in [
            &br#"SECRETWORDS not json"#[..],
            br#"{"access_token":"SECRETWORDS x","token_type":"Bearer"}"#,
            br#"{"access_token":"abc","token_type":"SECRETWORDS"}"#,
            br#"{"access_token":"abc","token_type":"Bearer","refresh_token":"SECRETWORDS y"}"#,
        ] {
            let error = read_token_answer("h", 200, body).unwrap_err();
            assert!(!error.to_string().contains("SECRETWORDS"), "{error}");
        }
        // An OAuth error with status 401 (a client the issuer does not know) is a refusal, not a server error.
        assert!(matches!(
            read_token_answer("h", 401, br#"{"error":"invalid_client"}"#),
            Err(OidcError::Refused { code }) if code == "invalid_client"
        ));
        // Its code is cleaned of control characters and markup.
        match read_token_answer("h", 400, "{\"error\":\"a\\u001b[31m<b>x\"}".as_bytes()) {
            Err(OidcError::Refused { code }) => {
                assert_eq!(code, "a31mbx");
                assert!(!code.contains('\u{1b}') && !code.contains('<'));
            }
            other => panic!("{other:?}"),
        }
        // A 401 with no OAuth error in it is the server's.
        assert!(matches!(
            read_token_answer("h", 401, b"nope"),
            Err(OidcError::Server { status: 401, .. })
        ));
    }

    #[test]
    fn an_endpoint_a_secret_may_go_to_is_https_or_a_loopback_http() {
        for (url, ok) in [
            ("https://idp.example.com/token", true),
            ("http://127.0.0.1:8080/token", true),
            ("http://localhost/token", true),
            ("http://[::1]:1/token", true),
            ("http://idp.example.com/token", false),
            ("https://user:pw@idp.example.com/token", false),
            ("https://idp.example.com/token#frag", false),
            ("ftp://127.0.0.1/token", false),
        ] {
            assert_eq!(usable_endpoint(&Url::parse(url).unwrap()), ok, "{url}");
        }
    }
}
