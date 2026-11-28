//! The HTTP [`ActivitySource`] (AD-058 P2c): asks an agent service's `GET /internal/activity` what work it holds.
//!
//! The question goes to the service's cluster address, `http://<name>.<namespace>.svc:<port>/internal/activity`, with the
//! bearer the agents are configured with. **Nothing here can call an agent idle by accident**: a connection that fails, a
//! timeout, a refusal, a status other than 200, a body that is too large or is not `{busy, nextDueAt}` are all errors, and
//! the caller counts an error as busy.

mod time;

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use aap_ports::{Activity, ActivityError, ActivitySource, RuntimeId, UnavailableCause};

/// The most bytes of an answer that are read: the real one is a few dozen.
pub const MAX_BODY: usize = 4096;

/// The bearer token of the agents. It has no `Display` and its `Debug` prints nothing of it.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// A token. Empty or with a control character is refused: it could not be a header value.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Token`] when the text is empty after trimming, or holds a control character.
    pub fn new(text: &str) -> Result<Self, SettingsError> {
        let text = text.trim();
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(SettingsError::Token);
        }
        Ok(Self(text.to_owned()))
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// Why a source could not be built.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// The token is empty or holds a control character.
    #[error("the activity token is empty or holds a control character")]
    Token,
    /// The timeout is zero.
    #[error("the activity timeout must be at least one second")]
    Timeout,
    /// The HTTP client could not be built.
    #[error("the activity HTTP client could not be built")]
    Client(#[source] reqwest::Error),
}

type Resolver = dyn Fn(&RuntimeId) -> String + Send + Sync;

/// Where the bearer comes from, asked at each poll: a deployment that rotates its Secret needs no restart (ANR-1192).
/// `None` is no bearer, which an agent refuses.
pub type TokenSource = dyn Fn() -> Option<Token> + Send + Sync;

/// An [`ActivitySource`] over HTTP. Cheap to clone.
#[derive(Clone)]
pub struct HttpActivity {
    client: reqwest::Client,
    resolve: Arc<Resolver>,
    token: Option<Arc<TokenSource>>,
}

impl fmt::Debug for HttpActivity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpActivity")
            .field("token", &self.token.is_some())
            .finish_non_exhaustive()
    }
}

impl HttpActivity {
    /// A source that asks `http://<name>.<namespace>.svc:<port>/internal/activity` of each service, waiting at most
    /// `timeout`. `token` is sent as `Authorization: Bearer`; agents refuse a request without it.
    ///
    /// # Errors
    ///
    /// [`SettingsError`] for a zero timeout or a client that cannot be built.
    pub fn in_cluster(
        port: u16,
        timeout: Duration,
        token: Option<Token>,
    ) -> Result<Self, SettingsError> {
        Self::with_resolver(
            move |id| {
                format!(
                    "http://{}.{}.svc:{port}/internal/activity",
                    id.name(),
                    id.scope()
                )
            },
            timeout,
            token,
        )
    }

    /// A source that asks the URL `resolve` names for each service. For tests, and for a deployment whose agents are not
    /// at the cluster's service addresses.
    ///
    /// # Errors
    ///
    /// [`SettingsError`] for a zero timeout or a client that cannot be built.
    pub fn with_resolver(
        resolve: impl Fn(&RuntimeId) -> String + Send + Sync + 'static,
        timeout: Duration,
        token: Option<Token>,
    ) -> Result<Self, SettingsError> {
        if timeout.is_zero() {
            return Err(SettingsError::Timeout);
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(SettingsError::Client)?;
        let token = token.map(|token| {
            let token = Arc::new(token);
            Arc::new(move || Some(Token(token.0.clone()))) as Arc<TokenSource>
        });
        Ok(Self {
            client,
            resolve: Arc::new(resolve),
            token,
        })
    }

    /// The bearer from `source`, asked at each poll (ANR-1192): a rotated Secret needs no restart. It replaces whatever
    /// token the constructor was given.
    #[must_use]
    pub fn with_token_source(
        mut self,
        source: impl Fn() -> Option<Token> + Send + Sync + 'static,
    ) -> Self {
        self.token = Some(Arc::new(source));
        self
    }
}

impl ActivitySource for HttpActivity {
    async fn poll(&self, id: &RuntimeId) -> Result<Activity, ActivityError> {
        let url = (self.resolve)(id);
        let mut request = self.client.get(url);
        if let Some(token) = self.token.as_ref().and_then(|source| source()) {
            request = request.bearer_auth(&token.0);
        }
        // `reqwest::Error` can hold the URL: it names a service, never a credential, but the message is not made from it.
        let mut response = request.send().await.map_err(transport)?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(ActivityError::Refused {
                status: status.as_u16(),
            });
        }
        if !status.is_success() {
            let cause = if status.is_server_error() {
                UnavailableCause::ServerError
            } else {
                UnavailableCause::Other
            };
            return Err(ActivityError::Unavailable {
                cause,
                source: format!("the agent answered {status}").into(),
            });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(transport)? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(ActivityError::Invalid(format!(
                    "the answer is longer than {MAX_BODY} bytes"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        parse(&body)
    }
}

/// A request that failed on the wire, before the answer or in the middle of its body. The URL is taken out of it.
fn transport(e: reqwest::Error) -> ActivityError {
    let cause = if e.is_timeout() {
        UnavailableCause::Timeout
    } else if e.is_connect() {
        UnavailableCause::Connect
    } else {
        UnavailableCause::Other
    };
    ActivityError::Unavailable {
        cause,
        source: Box::new(e.without_url()),
    }
}

/// `{"busy": 2, "nextDueAt": "2026-11-26T01:00:00Z"}`: `busy` a whole number, `nextDueAt` absent or an RFC 3339 time.
fn parse(body: &[u8]) -> Result<Activity, ActivityError> {
    let value: serde_json::Value = serde_json::from_slice(body)
        .map_err(|_| ActivityError::Invalid("the answer is not JSON".into()))?;
    let busy = value
        .get("busy")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| ActivityError::Invalid("`busy` is missing or not a whole number".into()))?;
    let next_due_at = match value.get("nextDueAt") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) => {
            Some(time::parse_rfc3339(text).ok_or_else(|| {
                ActivityError::Invalid("`nextDueAt` is not an RFC 3339 time".into())
            })?)
        }
        Some(_) => return Err(ActivityError::Invalid("`nextDueAt` is not a string".into())),
    };
    Ok(Activity { busy, next_due_at })
}
