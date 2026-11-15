//! The testkit (feature `testkit`): an OTLP/JSON receiver on a local port that checks the shape of what it is sent.

#![allow(clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde_json::Value as Json;

/// What the collector answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// `200` and `{}`.
    Accept,
    /// This status, with no body.
    Status(u16),
    /// `200` with `partialSuccess.rejectedSpans` of this.
    Reject(u32),
    /// Never answer.
    Hang,
    /// `302` to `/elsewhere`.
    Redirect,
}

#[derive(Default)]
struct Inner {
    bodies: Mutex<Vec<Json>>,
    headers: Mutex<Vec<HeaderMap>>,
    behaviour: Mutex<Option<Behaviour>>,
    elsewhere: AtomicUsize,
}

/// An OTLP/JSON receiver. It answers `400` to a body whose shape is not the one of `trace.json`, the example of
/// opentelemetry-proto (see [`check_shape`]).
pub struct TestCollector {
    addr: SocketAddr,
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestCollector {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn traces(State(s): State<Arc<Inner>>, headers: HeaderMap, body: Bytes) -> impl IntoResponse {
    let behaviour =
        (*s.behaviour.lock().unwrap_or_else(PoisonError::into_inner)).unwrap_or(Behaviour::Accept);
    s.headers
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(headers.clone());
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok());
    let Ok(json) = serde_json::from_slice::<Json>(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    if ct != Some("application/json") || check_shape(&json).is_err() {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match behaviour {
        Behaviour::Hang => std::future::pending().await,
        Behaviour::Status(n) => StatusCode::from_u16(n)
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            .into_response(),
        Behaviour::Redirect => {
            (StatusCode::FOUND, [(header::LOCATION, "/elsewhere")]).into_response()
        }
        Behaviour::Accept | Behaviour::Reject(_) => {
            s.bodies
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(json);
            match behaviour {
                Behaviour::Reject(n) => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    format!(r#"{{"partialSuccess":{{"rejectedSpans":"{n}","errorMessage":"not stored"}}}}"#),
                )
                    .into_response(),
                _ => (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    "{}",
                )
                    .into_response(),
            }
        }
    }
}

impl TestCollector {
    /// Start on `127.0.0.1:0`.
    ///
    /// # Panics
    ///
    /// When the port cannot be bound.
    pub async fn start() -> Self {
        let inner = Arc::new(Inner::default());
        let router = Router::new()
            .route("/v1/traces", post(traces))
            .route(
                "/elsewhere",
                get(|State(s): State<Arc<Inner>>| async move {
                    s.elsewhere.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }),
            )
            .with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a local port");
        let addr = listener.local_addr().expect("a bound address");
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self { addr, inner, task }
    }

    /// The URL to export to.
    #[must_use]
    pub fn endpoint(&self) -> String {
        format!("http://{}/v1/traces", self.addr)
    }

    /// The address, for a test that wants the port.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Answer as `behaviour` says from now on.
    pub fn set_behaviour(&self, behaviour: Behaviour) {
        *self
            .inner
            .behaviour
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(behaviour);
    }

    /// The bodies accepted, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<Json> {
        self.inner
            .bodies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The headers of every request, accepted or not.
    #[must_use]
    pub fn request_headers(&self) -> Vec<HeaderMap> {
        self.inner
            .headers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many times `/elsewhere` was asked for: a redirect that was followed.
    #[must_use]
    pub fn elsewhere_hits(&self) -> usize {
        self.inner.elsewhere.load(Ordering::SeqCst)
    }

    /// Every span of every accepted request.
    #[must_use]
    pub fn spans(&self) -> Vec<Json> {
        self.requests()
            .iter()
            .flat_map(|r| {
                r["resourceSpans"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|rs| rs["scopeSpans"].as_array().into_iter().flatten())
                    .flat_map(|ss| ss["spans"].as_array().cloned().unwrap_or_default())
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The span ids delivered, in order.
    #[must_use]
    pub fn span_ids(&self) -> Vec<[u8; 8]> {
        self.spans()
            .iter()
            .filter_map(|s| s["spanId"].as_str().and_then(unhex8))
            .collect()
    }

    /// The `service.name` of the resource of every accepted request.
    #[must_use]
    pub fn service_names(&self) -> Vec<String> {
        self.requests()
            .iter()
            .flat_map(|r| r["resourceSpans"].as_array().cloned().unwrap_or_default())
            .flat_map(|rs| {
                rs["resource"]["attributes"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            })
            .filter(|a| a["key"] == "service.name")
            .filter_map(|a| a["value"]["stringValue"].as_str().map(str::to_owned))
            .collect()
    }
}

fn unhex8(s: &str) -> Option<[u8; 8]> {
    if s.len() != 16 {
        return None;
    }
    let mut out = [0u8; 8];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn is_nanos(v: &Json) -> bool {
    v.as_str()
        .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

fn check_attributes(v: &Json) -> Result<(), &'static str> {
    for a in v.as_array().ok_or("attributes is not an array")? {
        if a["key"].as_str().is_none() {
            return Err("an attribute has no key");
        }
        let value = a["value"].as_object().ok_or("an attribute has no value")?;
        let ok = match (value.len(), value.iter().next()) {
            (1, Some((k, v))) => match k.as_str() {
                "stringValue" => v.is_string(),
                "intValue" => is_nanos(v),
                "boolValue" => v.is_boolean(),
                _ => false,
            },
            _ => false,
        };
        if !ok {
            return Err(
                "an attribute value is not stringValue, intValue (a decimal string) or boolValue",
            );
        }
    }
    Ok(())
}

/// Whether `body` has the shape of `examples/trace.json` of opentelemetry-proto: ids in hex, nanoseconds in decimal
/// strings, `kind` and `status.code` integers, values tagged.
///
/// # Errors
///
/// A fixed text that says which rule it broke.
pub fn check_shape(body: &Json) -> Result<(), &'static str> {
    let resource_spans = body["resourceSpans"].as_array().ok_or("no resourceSpans")?;
    for rs in resource_spans {
        check_attributes(&rs["resource"]["attributes"])?;
        for ss in rs["scopeSpans"].as_array().ok_or("no scopeSpans")? {
            if ss["scope"]["name"].as_str().is_none() {
                return Err("a scope has no name");
            }
            for s in ss["spans"].as_array().ok_or("no spans")? {
                if !s["traceId"].as_str().is_some_and(|t| is_hex(t, 32)) {
                    return Err("traceId is not 32 hex digits");
                }
                if !s["spanId"].as_str().is_some_and(|t| is_hex(t, 16)) {
                    return Err("spanId is not 16 hex digits");
                }
                if s.get("parentSpanId")
                    .is_some_and(|p| !p.as_str().is_some_and(|p| is_hex(p, 16)))
                {
                    return Err("parentSpanId is not 16 hex digits");
                }
                if s["name"].as_str().is_none() {
                    return Err("a span has no name");
                }
                if !is_nanos(&s["startTimeUnixNano"]) || !is_nanos(&s["endTimeUnixNano"]) {
                    return Err("a time is not a decimal string");
                }
                if !s["kind"].is_u64() {
                    return Err("kind is not an integer");
                }
                if let Some(status) = s.get("status")
                    && !status["code"].is_u64()
                {
                    return Err("status.code is not an integer");
                }
                check_attributes(&s["attributes"])?;
                for forbidden in ["events", "links"] {
                    if s.get(forbidden)
                        .is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()))
                    {
                        return Err("a span has events or links");
                    }
                }
            }
        }
    }
    Ok(())
}
