//! The HTTP [`ActivitySource`] (AD-058, P2c): asks an agent `GET /internal/activity` and reads `{busy, nextDueAt}`.
//!
//! A failed or malformed answer is an error, never an idle answer, so the caller counts the agent as busy: a refused
//! connection, a timeout, a non-2xx status (a `401` included), a redirect, a body over [`MAX_BODY`] bytes, text that is not
//! the document, or a document with a field of the wrong type. The bearer token is the deployment's, held in a type with no
//! `Debug` or `Display` of its text, and no error holds a URL, the token or a response body.

#![warn(missing_docs)]

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use aap_ports::{Activity, ActivityError, ActivitySource, RuntimeId};
use serde::Deserialize;

/// The most bytes of an answer read. The document is two small fields.
pub const MAX_BODY: usize = 4096;

/// The path asked of an agent, on its health listener.
pub const PATH: &str = "/internal/activity";

/// The port adam's health listener serves the route on.
pub const DEFAULT_PORT: u16 = 8080;

/// A bearer token. It cannot be printed.
#[derive(Clone)]
pub struct Bearer(Arc<str>);

impl Bearer {
    /// A token from its text; surrounding whitespace (a trailing newline of a mounted file) is dropped.
    pub fn new(text: &str) -> Option<Self> {
        let text = text.trim();
        (!text.is_empty()).then(|| Self(text.into()))
    }
}

impl fmt::Debug for Bearer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Bearer(..)")
    }
}

type Resolver = Arc<dyn Fn(&RuntimeId) -> String + Send + Sync>;

/// Asks agents over HTTP.
#[derive(Clone)]
pub struct HttpActivity {
    client: reqwest::Client,
    base: Resolver,
    bearer: Option<Bearer>,
}

impl fmt::Debug for HttpActivity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpActivity").finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Wire {
    busy: u32,
    #[serde(default)]
    next_due_at: Option<i64>,
}

impl HttpActivity {
    /// A source that asks `http://<name>.<scope>.svc:<port>`, answering within `timeout`.
    pub fn in_cluster(
        port: u16,
        timeout: Duration,
        bearer: Option<Bearer>,
    ) -> Result<Self, ActivityError> {
        Self::with_base(
            move |id: &RuntimeId| format!("http://{}.{}.svc:{port}", id.name(), id.scope()),
            timeout,
            bearer,
        )
    }

    /// A source whose base URL (scheme, host, port; no path) `base` gives for each service. For tests and for a
    /// deployment that fronts its agents differently.
    pub fn with_base(
        base: impl Fn(&RuntimeId) -> String + Send + Sync + 'static,
        timeout: Duration,
        bearer: Option<Bearer>,
    ) -> Result<Self, ActivityError> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|_| ActivityError::Invalid("the HTTP client could not be built".into()))?;
        Ok(Self {
            client,
            base: Arc::new(base),
            bearer,
        })
    }
}

impl ActivitySource for HttpActivity {
    async fn poll(&self, id: &RuntimeId) -> Result<Activity, ActivityError> {
        let url = format!("{}{PATH}", (self.base)(id));
        let mut request = self.client.get(url);
        if let Some(Bearer(token)) = &self.bearer {
            request = request.bearer_auth(token);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| ActivityError::Unavailable("the request failed".into()))?;
        let status = response.status();
        if !status.is_success() {
            return Err(ActivityError::Invalid(format!(
                "status {}",
                status.as_u16()
            )));
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ActivityError::Unavailable("the answer was cut".into()))?
        {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(ActivityError::Invalid("the answer is too large".into()));
            }
            body.extend_from_slice(&chunk);
        }
        let wire: Wire = serde_json::from_slice(&body).map_err(|_| {
            ActivityError::Invalid("the answer is not the activity document".into())
        })?;
        Ok(Activity {
            busy: wire.busy,
            next_due_at: wire.next_due_at,
        })
    }
}
