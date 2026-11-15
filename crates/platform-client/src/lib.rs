//! A client of the Platform API (§60b, K9b): what `aapctl` calls the API with.
//!
//! * **A concrete client, no transport trait.** The Platform API is the platform's own contract and not swappable infrastructure
//!   (AD-020 is about the seams behind it); its conformance is the OpenAPI document, and [`Op`] is held to it by a test.
//! * **The token is in a [`Token`]** and nowhere else: `Debug` shows nothing of it, the `Authorization` header is marked
//!   sensitive, and no error or trace line holds a header, a URL's query or the text of the transport's error (a `reqwest` error
//!   can carry the URL it was sent to, so no message is made from one).
//! * **Every write is conditional** ([`Condition`]): `If-None-Match: *` creates, `If-Match: <tag>` replaces or deletes. There is no
//!   way to send one without.
//! * **Errors are classified**: an answer with a status of 400 or more is a [`Problem`] read from the body; a failure to talk to
//!   the server is [`ClientError::Unreachable`]. What a person is told, and the exit code, are the caller's.
//! * **reqwest 0.12 with rustls and ring**, as `aap-auth-oidc`: `crates/auth-oidc/README.md` says why never 0.13.
//!
//! The sign-in (OAuth 2.0 authorization code with PKCE, and the refresh grant) is [`oidc`]: concrete code over the same HTTP rules.
//! A token comes from the caller, from `AAPCTL_TOKEN`, from a pasted token or from that sign-in.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod oidc;
mod op;
mod problem;
mod token;

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, ETAG, HeaderValue};
use serde_json::Value;
use url::{Host, Url};

pub use op::{Collection, Op};
pub use problem::{
    FieldProblem, Problem, YamlOnlyReason, clean, clean_multiline, has_terminal_hazard,
    json_for_terminal, terminal_hazard_kind,
};
pub use token::{MAX_TOKEN_BYTES, Token};

/// The most an answer may hold: the API's own bodies are far smaller, so a larger one is not the API.
const MAX_ANSWER_BYTES: usize = 8 << 20;

/// Why a call did not give an answer.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// A value the caller gave cannot be used (the server's address, a name, the token): nothing was sent. The message holds no
    /// token.
    #[error("{0}")]
    Config(String),
    /// The server could not be reached or did not answer in time. `class` is a fixed phrase, never the transport's own message.
    #[error("cannot reach {origin}: {class}")]
    Unreachable {
        /// The server's origin (`https://host:port`), with no user name, path or query.
        origin: String,
        /// What went wrong, in a fixed set of phrases.
        class: &'static str,
    },
    /// The server answered with a status of 400 or more (or a redirect, which is never followed).
    #[error("{} {}", .0.status, .0.slug.as_deref().unwrap_or("no problem details"))]
    Problem(Box<Problem>),
    /// A success whose body is not what the API sends.
    #[error("the server's answer is not what the Platform API sends: {0}")]
    BadAnswer(String),
}

/// The precondition of a write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Condition {
    /// `If-None-Match: *`: create, and fail with `409 already-exists` if the name is taken.
    Create,
    /// `If-Match: <tag>`: replace or delete the object that has this tag.
    Replace(String),
}

/// A successful answer.
#[derive(Clone, Debug)]
pub struct Reply {
    /// The status: 200, 201, 202 or 204.
    pub status: u16,
    /// The `ETag` header, as it came (quotes included): what `If-Match` takes back.
    pub etag: Option<String>,
    /// The body.
    pub body: Vec<u8>,
}

impl Reply {
    /// The body as JSON.
    ///
    /// # Errors
    ///
    /// [`ClientError::BadAnswer`] when it is not JSON (the message holds no part of the body).
    pub fn json(&self) -> Result<Value, ClientError> {
        serde_json::from_slice(&self.body)
            .map_err(|e| ClientError::BadAnswer(format!("not JSON ({:?})", e.classify())))
    }

    /// The body as text.
    ///
    /// # Errors
    ///
    /// [`ClientError::BadAnswer`] when it is not UTF-8.
    pub fn text(&self) -> Result<String, ClientError> {
        String::from_utf8(self.body.clone())
            .map_err(|_| ClientError::BadAnswer("not UTF-8 text".to_owned()))
    }
}

/// How a [`Client`] is made.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Allow `http://` for a host that is not a loopback address. Off: the token would cross the network in the clear.
    pub allow_insecure_http: bool,
    /// Use the proxy the environment names (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`). Off in the crate's own tests.
    /// **Never used for an `http://` loopback address, whatever this says**: a proxy would be handed the bearer token in the clear
    /// for a server on the same machine (P-031).
    pub system_proxy: bool,
    /// How long to wait for the connection to be made.
    pub connect_timeout: Duration,
    /// How long a whole request may take, from the first byte sent to the last received.
    pub request_timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            allow_insecure_http: false,
            system_proxy: true,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
        }
    }
}

/// An address of a server in one spelling, for telling whether two names are the same server: the scheme and the host in lower
/// case, the default port left out, no trailing slash. `None` when it is not an address `http` or `https` can use.
pub fn normalize_server(address: &str) -> Option<String> {
    let url = Url::parse(address.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host().is_none() {
        return None;
    }
    let path = url.path().trim_end_matches('/');
    Some(format!("{}{path}", url.origin().ascii_serialization()))
}

/// A client of one Platform API, with one token.
pub struct Client {
    http: reqwest::Client,
    base: Url,
    /// Replaced when a refresh gave a new one ([`Client::replace_token`]).
    token: Mutex<Token>,
    trace: Mutex<Vec<String>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("server", &self.origin())
            .field("token", &"Token(…)")
            .finish_non_exhaustive()
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(a)) => a.is_loopback(),
        Some(Host::Ipv6(a)) => a.is_loopback(),
        None => false,
    }
}

/// Whether `name` can be one path segment of an object: what the API's names are (`[a-z][a-z0-9-]*`) and a little more, and never
/// a segment that means something to a URL.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

impl Client {
    /// A client for the API at `server` (`https://host[:port][/prefix]`).
    ///
    /// # Errors
    ///
    /// [`ClientError::Config`] for an address that is not `http` or `https`, has no host, holds a user name, a password, a query
    /// or a fragment, or is `http` for a host that is not a loopback address (unless allowed); or when the HTTP client cannot be
    /// built.
    pub fn new(server: &str, token: Token, options: Options) -> Result<Self, ClientError> {
        let bad =
            |why: &str| ClientError::Config(format!("the server address is not usable: {why}"));
        let base = Url::parse(server.trim()).map_err(|_| bad("it is not an address"))?;
        match base.scheme() {
            "https" => {}
            "http" => {
                if !is_loopback(&base) && !options.allow_insecure_http {
                    return Err(bad(
                        "http would send the token in the clear: use https (--insecure-http allows it)",
                    ));
                }
            }
            _ => return Err(bad("it is not http or https")),
        }
        if base.host().is_none() {
            return Err(bad("it has no host"));
        }
        if !base.username().is_empty() || base.password().is_some() {
            return Err(bad("it holds a user name or a password"));
        }
        if base.query().is_some() || base.fragment().is_some() {
            return Err(bad("it holds a query or a fragment"));
        }
        let mut builder = reqwest::Client::builder()
            // A redirect would carry the request, and the person's trust, somewhere the person did not name.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(options.connect_timeout)
            .timeout(options.request_timeout)
            .user_agent(concat!("aapctl/", env!("CARGO_PKG_VERSION")));
        // A proxy named by the environment is a third party: it would see the bearer token of a request that never leaves the
        // machine. So an `http://` loopback address is never sent through one.
        if !options.system_proxy || (base.scheme() == "http" && is_loopback(&base)) {
            builder = builder.no_proxy();
        }
        let http = builder
            .build()
            .map_err(|_| ClientError::Config("the HTTP client cannot be built".to_owned()))?;
        Ok(Self {
            http,
            base,
            token: Mutex::new(token),
            trace: Mutex::new(Vec::new()),
        })
    }

    /// Use another token from now on: what a refresh gave. The server it is for does not change.
    pub fn replace_token(&self, token: Token) {
        *self.token.lock().unwrap_or_else(PoisonError::into_inner) = token;
    }

    /// The token the next call sends (a refresh may have replaced the one the client was made with).
    pub fn current_token(&self) -> Token {
        self.token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The server's origin: scheme, host and port.
    pub fn origin(&self) -> String {
        self.base.origin().ascii_serialization()
    }

    /// What was sent and answered since the last call of this, one line each: method, path, the precondition and status. Never a
    /// header that holds a credential.
    pub fn take_trace(&self) -> Vec<String> {
        match self.trace.lock() {
            Ok(mut t) => std::mem::take(&mut *t),
            Err(_) => Vec::new(),
        }
    }

    fn note(&self, line: String) {
        if let Ok(mut t) = self.trace.lock() {
            t.push(line);
        }
    }

    /// One call.
    ///
    /// # Errors
    ///
    /// [`ClientError::Config`] for a `name` that is not a path segment, or a `name` missing or given where the call has none;
    /// [`ClientError::Unreachable`]; [`ClientError::Problem`] for a status of 300 or more.
    pub async fn call(
        &self,
        op: Op,
        name: Option<&str>,
        condition: Option<&Condition>,
        body: Option<&Value>,
    ) -> Result<Reply, ClientError> {
        if op.names_an_object() != name.is_some() {
            return Err(ClientError::Config(
                "a call needs the name of an object exactly when its path has one".to_owned(),
            ));
        }
        if let Some(n) = name
            && !valid_name(n)
        {
            return Err(ClientError::Config(format!(
                "{:?} is not the name of an object: letters, digits, '-', '_' and '.' only",
                clean(n)
            )));
        }
        let mut url = self.base.clone();
        {
            let mut segments = url.path_segments_mut().map_err(|()| {
                ClientError::Config("the server address cannot have a path".to_owned())
            })?;
            segments.pop_if_empty();
            segments.extend(op.segments(name));
        }
        let method = reqwest::Method::from_bytes(op.method().as_bytes())
            .map_err(|_| ClientError::Config("not a method".to_owned()))?;
        let mut request = self.http.request(method, url);
        let text = self
            .token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .expose()
            .to_owned();
        let mut auth = HeaderValue::from_str(&format!("Bearer {text}"))
            .map_err(|_| ClientError::Config("the token cannot be sent as a header".to_owned()))?;
        auth.set_sensitive(true);
        request = request.header(AUTHORIZATION, auth);
        let mut line = format!("> {} /{}", op.method(), op.segments(name).join("/"));
        match condition {
            Some(Condition::Create) => {
                request = request.header("if-none-match", "*");
                line.push_str("  If-None-Match: *");
            }
            Some(Condition::Replace(tag)) => {
                let value = HeaderValue::from_str(tag).map_err(|_| {
                    ClientError::Config("the ETag cannot be sent as a header".to_owned())
                })?;
                request = request.header("if-match", value);
                line.push_str(&format!("  If-Match: {}", clean(tag)));
            }
            None => {}
        }
        self.note(line);
        if let Some(body) = body {
            request = request.header(CONTENT_TYPE, "application/json").body(
                serde_json::to_vec(body).map_err(|_| {
                    ClientError::Config("the form cannot be written as JSON".to_owned())
                })?,
            );
        }
        let response = request.send().await.map_err(|e| self.unreachable(&e))?;
        let status = response.status().as_u16();
        self.note(format!("< {status}"));
        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let body = read_capped(response, MAX_ANSWER_BYTES)
            .await
            .map_err(|e| self.unreachable(&e))?;
        let Some(body) = body else {
            return Err(ClientError::BadAnswer(format!(
                "the answer is larger than {MAX_ANSWER_BYTES} bytes"
            )));
        };
        if status >= 300 {
            return Err(ClientError::Problem(Box::new(Problem::read(status, &body))));
        }
        Ok(Reply { status, etag, body })
    }

    fn unreachable(&self, error: &reqwest::Error) -> ClientError {
        ClientError::Unreachable {
            origin: self.origin(),
            class: classify(error),
        }
    }

    // ------------------------------------------------------------------ the calls

    /// `GET /v1/me`.
    ///
    /// # Errors
    ///
    /// As [`call`](Self::call).
    pub async fn me(&self) -> Result<Reply, ClientError> {
        self.call(Op::Me, None, None, None).await
    }

    /// `GET /v1/info`.
    ///
    /// # Errors
    ///
    /// As [`call`](Self::call).
    pub async fn info(&self) -> Result<Reply, ClientError> {
        self.call(Op::Info, None, None, None).await
    }

    /// `GET /v1/<collection>`.
    ///
    /// # Errors
    ///
    /// As [`call`](Self::call).
    pub async fn list(&self, collection: Collection) -> Result<Reply, ClientError> {
        self.call(Op::List(collection), None, None, None).await
    }

    /// `GET /v1/<collection>/{name}`.
    ///
    /// # Errors
    ///
    /// As [`call`](Self::call); a missing object is a [`Problem`] of status 404.
    pub async fn get(&self, collection: Collection, name: &str) -> Result<Reply, ClientError> {
        self.call(Op::Get(collection), Some(name), None, None).await
    }

    /// `PUT /v1/<collection>/{name}` with the form and the precondition.
    ///
    /// # Errors
    ///
    /// As [`call`](Self::call).
    pub async fn put(
        &self,
        collection: Collection,
        name: &str,
        form: &Value,
        condition: &Condition,
    ) -> Result<Reply, ClientError> {
        self.call(Op::Put(collection), Some(name), Some(condition), Some(form))
            .await
    }

    /// `DELETE /v1/<collection>/{name}` of the object that has this tag.
    ///
    /// # Errors
    ///
    /// As [`call`](Self::call).
    pub async fn delete(
        &self,
        collection: Collection,
        name: &str,
        etag: &str,
    ) -> Result<Reply, ClientError> {
        self.call(
            Op::Delete(collection),
            Some(name),
            Some(&Condition::Replace(etag.to_owned())),
            None,
        )
        .await
    }

    /// `GET /v1/agents/{name}/yaml`.
    ///
    /// # Errors
    ///
    /// As [`call`](Self::call).
    pub async fn agent_yaml(&self, name: &str) -> Result<Reply, ClientError> {
        self.call(Op::AgentYaml, Some(name), None, None).await
    }
}

/// What went wrong with a request, as a phrase from a fixed set: **read from the error and never printed from it**, because its text can
/// hold the URL (a query with a code in it, a host).
fn classify(error: &reqwest::Error) -> &'static str {
    let mut chain = String::new();
    let mut source: Option<&dyn std::error::Error> = Some(error);
    while let Some(e) = source {
        chain.push_str(&e.to_string().to_ascii_lowercase());
        chain.push('\n');
        source = e.source();
    }
    if chain.contains("certificate") || chain.contains("tls") {
        "the TLS handshake failed or the certificate was not accepted"
    } else if error.is_timeout() {
        "the server did not answer in time"
    } else if error.is_connect() {
        "the connection failed"
    } else {
        "the request failed"
    }
}

/// The body of an answer, up to `limit` bytes; `None` when there is more.
async fn read_capped(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Option<Vec<u8>>, reqwest::Error> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > limit {
            return Ok(None);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Some(body))
}
