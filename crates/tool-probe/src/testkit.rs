//! A stub MCP server for the tests of this crate, the controller's, and the kind end-to-end (`examples/test-mcp.rs`).
//!
//! It speaks the Streamable HTTP transport the probe speaks: `initialize`, `notifications/initialized` and `tools/list`
//! (paginated when `page_size` is set). A case asks it to require a header, fail with a status, answer after a delay,
//! answer as SSE, answer a body over 1 MiB, redirect, answer a JSON-RPC error, or list no tools. It records the methods
//! it was asked, so a test can show that a probe it expects not to happen made no request.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

/// How the stub answers. The defaults are a server with no tools that answers every request at once.
#[derive(Clone, Debug, Default)]
pub struct StubConfig {
    /// The tool names `tools/list` lists, in this order.
    pub tools: Vec<String>,
    /// When set, `tools/list` lists this many names per page and a `nextCursor` until the last.
    pub page_size: Option<usize>,
    /// A header (`name`, `value`) the server requires on every request; a request without it is `401`.
    pub require_header: Option<(String, String)>,
    /// A status every request answers with instead of a result.
    pub fail_status: Option<u16>,
    /// How long every request waits before it answers.
    pub delay: Option<Duration>,
    /// Answer as `text/event-stream` instead of `application/json`.
    pub sse: bool,
    /// Answer the SSE event and keep the stream open (5 s), as a proxy that holds the connection does.
    pub sse_hold: bool,
    /// Answer a body over 1 MiB.
    pub huge: bool,
    /// Answer a `302` to `redirect_to` (a URL that answers MCP, so a probe that follows it would be `Probed`).
    pub redirect_to: Option<String>,
    /// Answer a 200 whose body is not JSON.
    pub garbage: bool,
    /// Answer a JSON-RPC `error` instead of a `result`.
    pub jsonrpc_error: bool,
    /// Answer with the id `99` instead of the request's.
    pub wrong_id: bool,
    /// Answer with the request's id as a string (`"1"`), as some servers echo it.
    pub string_id: bool,
    /// Name a session (`Mcp-Session-Id: stub-session`) on `initialize` and require it, and the protocol version, on every
    /// later request.
    pub session: bool,
    /// `tools/list` answers an empty list (a reachable server with no tools).
    pub no_tools: bool,
}

#[derive(Clone)]
struct Stub {
    config: Arc<StubConfig>,
    requests: Arc<Mutex<Vec<String>>>,
}

/// A running stub server. Dropping it stops the server.
pub struct StubMcp {
    url: String,
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl StubMcp {
    /// Start a stub on a free port of `127.0.0.1`.
    ///
    /// # Panics
    ///
    /// The port cannot be bound.
    pub async fn start(config: StubConfig) -> Self {
        Self::start_at("127.0.0.1:0".parse().expect("an address"), config).await
    }

    /// Start a stub at `addr` (`0.0.0.0:0` for a cluster that reaches this machine).
    ///
    /// # Panics
    ///
    /// The address cannot be bound.
    pub async fn start_at(addr: SocketAddr, config: StubConfig) -> Self {
        let stub = Stub {
            config: Arc::new(config),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route("/mcp", post(mcp).get(mcp_get).delete(mcp_delete))
            .with_state(stub.clone());
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .expect("the stub's port is free");
        let addr = listener.local_addr().expect("the stub has an address");
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        let host = if addr.ip().is_unspecified() {
            "127.0.0.1".to_owned()
        } else {
            addr.ip().to_string()
        };
        Self {
            url: format!("http://{host}:{}/mcp", addr.port()),
            addr,
            requests: stub.requests,
            task,
        }
    }

    /// The URL a client on this machine calls.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The address it listens on.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The methods it was asked, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

impl Drop for StubMcp {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// A `GET /mcp`: only a probe that followed a redirect makes one. It answers a valid JSON-RPC result (with a tool) and records
/// `get`, so a test can show that no redirect was followed.
async fn mcp_get(State(stub): State<Stub>) -> Response {
    if let Ok(mut requests) = stub.requests.lock() {
        requests.push("get".to_owned());
    }
    Json(json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "protocolVersion": "2025-06-18",
            "tools": [{ "name": "redirected" }]
        }
    }))
    .into_response()
}

async fn mcp(State(stub): State<Stub>, headers: HeaderMap, Json(body): Json<Value>) -> Response {
    let method = body
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if let Ok(mut requests) = stub.requests.lock() {
        requests.push(method.clone());
    }
    if stub.config.session
        && method != "initialize"
        && (headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) != Some("stub-session")
            || headers
                .get("mcp-protocol-version")
                .and_then(|v| v.to_str().ok())
                != Some("2025-06-18"))
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Some((name, value)) = &stub.config.require_header
        && headers.get(name.as_str()).and_then(|v| v.to_str().ok()) != Some(value.as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if let Some(status) = stub.config.fail_status {
        return StatusCode::from_u16(status)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            .into_response();
    }
    if let Some(delay) = stub.config.delay {
        tokio::time::sleep(delay).await;
    }
    if let Some(target) = &stub.config.redirect_to {
        return (StatusCode::FOUND, [(header::LOCATION, target.as_str())]).into_response();
    }
    if stub.config.garbage {
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            "this is not json",
        )
            .into_response();
    }
    let id = if stub.config.wrong_id {
        json!(99)
    } else if stub.config.string_id {
        json!(body["id"].as_u64().unwrap_or(0).to_string())
    } else {
        body.get("id").cloned().unwrap_or(Value::Null)
    };
    if stub.config.jsonrpc_error && method != "notifications/initialized" {
        return Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": -32601, "message": "the stub says no" }
        }))
        .into_response();
    }
    let answer = match method.as_str() {
        "initialize" => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "stub", "version": "0" }
            }
        }),
        "notifications/initialized" => return StatusCode::ACCEPTED.into_response(),
        "tools/list" => {
            let start = body["params"]["cursor"]
                .as_str()
                .and_then(|c| c.parse::<usize>().ok())
                .unwrap_or(0);
            let total = if stub.config.no_tools {
                0
            } else {
                stub.config.tools.len()
            };
            let page = stub.config.page_size.unwrap_or_else(|| total.max(1)).max(1);
            let end = (start + page).min(total);
            let tools = if start < end {
                stub.config.tools[start..end]
                    .iter()
                    .map(|name| {
                        json!({
                            "name": name,
                            "description": "a stub tool",
                            "inputSchema": { "type": "object" }
                        })
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let mut result = json!({ "tools": tools });
            if end < total {
                result["nextCursor"] = json!(end.to_string());
            }
            json!({ "jsonrpc": "2.0", "id": id, "result": result })
        }
        _ => return StatusCode::BAD_REQUEST.into_response(),
    };
    if stub.config.huge {
        // A valid answer with a tool, so that a probe that did not cap the body would report `Probed` and not `NotMcp`.
        let huge = "x".repeat(1024 * 1024 + 64);
        return Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "tools": [{ "name": "capped" }], "padding": huge }
        }))
        .into_response();
    }
    if stub.config.sse {
        let body = format!("event: message\ndata: {answer}\n\n");
        if stub.config.sse_hold {
            return held_stream(body);
        }
        return (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            body,
        )
            .into_response();
    }
    let mut response = Json(answer).into_response();
    if stub.config.session && method == "initialize" {
        response.headers_mut().insert(
            HeaderName::from_static("mcp-session-id"),
            HeaderValue::from_static("stub-session"),
        );
    }
    response
}

/// The end of a session: the probe's `DELETE`, recorded as `delete`.
async fn mcp_delete(State(stub): State<Stub>, headers: HeaderMap) -> Response {
    if let Ok(mut requests) = stub.requests.lock() {
        requests.push("delete".to_owned());
    }
    if stub.config.session
        && headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) != Some("stub-session")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    StatusCode::NO_CONTENT.into_response()
}

/// An SSE answer that keeps the stream open after it (as a proxy that holds the connection does).
fn held_stream(body: String) -> Response {
    let stream = futures::stream::unfold((Some(body), false), |(body, held)| async move {
        match (body, held) {
            (Some(body), false) => Some((
                Ok::<_, std::io::Error>(bytes::Bytes::from(body)),
                (None, true),
            )),
            (None, true) => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                None
            }
            _ => None,
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .body(axum::body::Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
