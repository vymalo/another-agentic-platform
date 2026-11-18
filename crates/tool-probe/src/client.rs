//! The Streamable HTTP probe: one `initialize`, the initialized notification, `tools/list` (paginated, bounded), and the
//! session's end.

use std::time::Duration;

use serde_json::{Value, json};

/// The most tool names a [`ProbeOutcome`] lists. The status of a server that lists hundreds must stay a bounded size
/// (every change of it is sent to every watcher); the rest are counted in `tools_more`.
pub const MAX_TOOLS: usize = 64;

/// The longest tool name a [`ProbeOutcome`] lists, in characters. A longer one (or one with a control character) is not a
/// name a model can call, and is left out rather than written into a status that must stay small.
pub const MAX_TOOL_NAME: usize = 256;

/// The MCP protocol version the probe speaks. A server that speaks another one answers with its own in `initialize` and
/// the probe goes on with it (the version is a negotiation, and the probe only reads what the server lists).
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The most `tools/list` pages the probe reads, so that a server that keeps answering with a cursor cannot hold it. When
/// the last page still names a cursor, the outcome says it is `incomplete`.
const MAX_PAGES: usize = 5;

/// The most of an answer the probe reads (a body over it is `NotMcp`).
const MAX_BODY: usize = 1024 * 1024;

/// Why a probe ended as it did. A closed set: it reaches a condition's `reason`, a log line and a test, never a body,
/// a URL or a header (AD-053, AD-024).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeReason {
    /// The server answered `initialize` and `tools/list`.
    Probed,
    /// The server names a header of the object, which the operator does not read (AD-024). The controller decides this
    /// before it calls [`probe`].
    NeedsHeader,
    /// The object is invalid, so there is nothing to probe. The controller decides this before it calls [`probe`].
    Invalid,
    /// The server did not answer, answered a 401/403 or a 5xx, or the TLS or the connection failed.
    Unreachable,
    /// The server answered, but not as an MCP Streamable HTTP endpoint: a 4xx other than 401/403, a redirect, a body
    /// that is not JSON-RPC, a JSON-RPC error, an answer with another request's id, or a body over 1 MiB.
    NotMcp,
    /// The server did not answer within the deployment's deadline.
    Timeout,
}

impl ProbeReason {
    /// The word a condition's `reason` and a log line hold.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Probed => "Probed",
            Self::NeedsHeader => "NeedsHeader",
            Self::Invalid => "Invalid",
            Self::Unreachable => "Unreachable",
            Self::NotMcp => "NotMcp",
            Self::Timeout => "Timeout",
        }
    }
}

/// What a probe found: a reason, the tool names the server listed (sorted, each once, at most [`MAX_TOOLS`]), how many
/// more there were, and whether the list was cut by the page bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeOutcome {
    /// Why the probe ended as it did.
    pub reason: ProbeReason,
    /// The tool names, sorted and de-duplicated, at most [`MAX_TOOLS`].
    pub tools: Vec<String>,
    /// How many tool names are not in [`tools`](Self::tools).
    pub tools_more: usize,
    /// The server still named a `nextCursor` after [`MAX_PAGES`] pages: there may be more tools than those read, so the
    /// count is a lower bound.
    pub incomplete: bool,
}

impl ProbeOutcome {
    /// An outcome with `reason` and no tools.
    #[must_use]
    pub fn new(reason: ProbeReason) -> Self {
        Self {
            reason,
            tools: Vec::new(),
            tools_more: 0,
            incomplete: false,
        }
    }

    /// Whether the server answered `initialize` and `tools/list`.
    #[must_use]
    pub fn reachable(&self) -> bool {
        self.reason == ProbeReason::Probed
    }
}

/// Probe the MCP server at `url` within `timeout`.
///
/// The caller decides whether to call it: the object is valid, the deployment turned probing on, and the server names no
/// header of the object. No error or log from here holds `url`.
pub async fn probe(url: &str, timeout: Duration) -> ProbeOutcome {
    match tokio::time::timeout(timeout, discover(url)).await {
        Err(_) => ProbeOutcome::new(ProbeReason::Timeout),
        Ok(Err(reason)) => ProbeOutcome::new(reason),
        Ok(Ok((mut tools, incomplete))) => {
            tools.sort();
            tools.dedup();
            let more = tools.len().saturating_sub(MAX_TOOLS);
            tools.truncate(MAX_TOOLS);
            ProbeOutcome {
                reason: ProbeReason::Probed,
                tools,
                tools_more: more,
                incomplete,
            }
        }
    }
}

/// `initialize`, the notification, then every `tools/list` page (bounded), then the session's end.
async fn discover(url: &str) -> Result<(Vec<String>, bool), ProbeReason> {
    let client = reqwest::Client::builder()
        // A redirect would leave the host the deployment named: never follow one.
        .redirect(reqwest::redirect::Policy::none())
        // The operator reaches the server through its NetworkPolicy, not a proxy variable.
        .no_proxy()
        .build()
        .map_err(|_| ProbeReason::Unreachable)?;

    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "clientInfo": { "name": "aap-operator", "version": env!("CARGO_PKG_VERSION") }
        }
    });
    let (session, answer) = post(&client, url, &initialize, None, None, 1).await?;
    let version = answer["result"]["protocolVersion"]
        .as_str()
        .unwrap_or(PROTOCOL_VERSION)
        .to_owned();

    let initialized = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
    notify(&client, url, &initialized, session.as_deref(), &version).await?;

    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    let mut incomplete = false;
    for page in 0..MAX_PAGES {
        let body = match &cursor {
            Some(c) => json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": { "cursor": c }
            }),
            None => json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {} }),
        };
        let (_, answer) = post(&client, url, &body, session.as_deref(), Some(&version), 2).await?;
        let result = answer.get("result").ok_or(ProbeReason::NotMcp)?;
        let list = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or(ProbeReason::NotMcp)?;
        for tool in list {
            if let Some(name) = tool.get("name").and_then(Value::as_str)
                && is_listable(name)
            {
                tools.push(name.to_owned());
            }
        }
        cursor = result
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
        if page + 1 == MAX_PAGES {
            incomplete = true;
        }
    }
    // The session's end, best effort and on a task of its own: it must not delay the answer, and a server that does not
    // implement it (or holds the request) must not turn a `Probed` into a `Timeout`.
    if let Some(session) = session {
        terminate(client, url.to_owned(), session);
    }
    Ok((tools, incomplete))
}

/// Whether a name is one a status lists: 1 to [`MAX_TOOL_NAME`] characters, none of them a control character.
fn is_listable(name: &str) -> bool {
    !name.is_empty() && name.chars().count() <= MAX_TOOL_NAME && !name.chars().any(char::is_control)
}

/// One JSON-RPC request and its answer (a JSON body or an SSE event with the request's id).
async fn post(
    client: &reqwest::Client,
    url: &str,
    body: &Value,
    session: Option<&str>,
    version: Option<&str>,
    id: u64,
) -> Result<(Option<String>, Value), ProbeReason> {
    let mut request = client
        .post(url)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string());
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    if let Some(version) = version {
        request = request.header("mcp-protocol-version", version);
    }
    let response = request.send().await.map_err(|_| ProbeReason::Unreachable)?;
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 | 403 | 500..=599 => ProbeReason::Unreachable,
            _ => ProbeReason::NotMcp,
        });
    }
    let session = response
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let sse = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("text/event-stream"));
    let answer = if sse {
        read_sse(response, id).await?
    } else {
        let bytes = read_body(response).await?;
        serde_json::from_slice(&bytes).map_err(|_| ProbeReason::NotMcp)?
    };
    if answer.get("error").is_some()
        || answer.get("result").is_none()
        || !answer_matches(&answer, id)
    {
        return Err(ProbeReason::NotMcp);
    }
    Ok((session, answer))
}

/// The initialized notification: no id, and no answer to read.
async fn notify(
    client: &reqwest::Client,
    url: &str,
    body: &Value,
    session: Option<&str>,
    version: &str,
) -> Result<(), ProbeReason> {
    let mut request = client
        .post(url)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header("mcp-protocol-version", version)
        .body(body.to_string());
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = request.send().await.map_err(|_| ProbeReason::Unreachable)?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(ProbeReason::NotMcp)
    }
}

/// End the session on a task of its own, best effort (MCP Streamable HTTP's `DELETE`).
fn terminate(client: reqwest::Client, url: String, session: String) {
    tokio::spawn(async move {
        let _ = client
            .delete(&url)
            .header("mcp-session-id", &session)
            .send()
            .await;
    });
}

/// Whether `answer` carries the id of the request `id` (a number, or the string form some servers echo).
fn answer_matches(answer: &Value, id: u64) -> bool {
    match answer.get("id") {
        Some(Value::Number(number)) => number.as_u64() == Some(id),
        Some(Value::String(text)) => text == &id.to_string(),
        _ => false,
    }
}

/// At most [`MAX_BODY`] bytes of a body, read as it arrives.
async fn read_body(mut response: reqwest::Response) -> Result<Vec<u8>, ProbeReason> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_BODY as u64)
    {
        return Err(ProbeReason::NotMcp);
    }
    let mut out = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProbeReason::Unreachable)?
    {
        if out.len() + chunk.len() > MAX_BODY {
            return Err(ProbeReason::NotMcp);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// The SSE event whose `data` is the JSON-RPC answer with `id`, read **as it arrives**: a server that writes the answer
/// and keeps the stream open is answered, not timed out.
async fn read_sse(mut response: reqwest::Response, id: u64) -> Result<Value, ProbeReason> {
    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ProbeReason::Unreachable)?
    {
        if buffer.len() + chunk.len() > MAX_BODY {
            return Err(ProbeReason::NotMcp);
        }
        buffer.extend_from_slice(&chunk);
        while let Some(end) = event_end(&buffer) {
            let event: Vec<u8> = buffer.drain(..end).collect();
            if let Some(value) = sse_event(&event, id)? {
                return Ok(value);
            }
        }
    }
    Err(ProbeReason::NotMcp)
}

/// The end of the first complete SSE event in `bytes`, after its blank line (`\n\n` or `\r\n\r\n`).
fn event_end(bytes: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\n' && bytes.get(i + 1) == Some(&b'\n') {
            return Some(i + 2);
        }
        if bytes[i] == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
            if bytes.get(i + 2) == Some(&b'\n') {
                return Some(i + 3);
            }
            if bytes.get(i + 2) == Some(&b'\r') && bytes.get(i + 3) == Some(&b'\n') {
                return Some(i + 4);
            }
        }
        i += 1;
    }
    None
}

/// The JSON-RPC answer in one SSE event, if its `data` is one and it carries `id`.
fn sse_event(event: &[u8], id: u64) -> Result<Option<Value>, ProbeReason> {
    let text = std::str::from_utf8(event).map_err(|_| ProbeReason::NotMcp)?;
    let data = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");
    if data.is_empty() {
        return Ok(None);
    }
    let Ok(value) = serde_json::from_str::<Value>(&data) else {
        return Ok(None);
    };
    Ok(answer_matches(&value, id).then_some(value))
}
