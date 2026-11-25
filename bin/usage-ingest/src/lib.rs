//! The usage ingest (ANR-730, AD-060): the HTTP face of a [`UsageStore`].
//!
//! * `POST /v1/events` takes CloudEvents 1.0 structured JSON (one event, or a batch) from the meter's identity only.
//!   A new event is `201`, a duplicate `200`; a batch is `200` with the two counts. A refusal names the field and
//!   never its value.
//! * `POST /v1/spend` answers the ledger, and only the ledger's identity: weighted minutes per account and node type
//!   by **ingestion** time, over a window that ended at least the configured lag ago.
//!
//! Who is calling is a [`CallerVerifier`], a port: the binary composes a Kubernetes `TokenReview` adapter, the tests
//! a static one. No token, event value or account is ever logged.

use std::future::Future;
use std::sync::Arc;

use aap_usage::{EVENT_TYPE, Inserted, PodMinuteEvent, SpendQuery, UsageError, UsageStore, Window};
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::{Value, json};

pub mod tokenreview;

/// Who a verified caller is. Closed on purpose: a new kind must fail to compile in every route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caller {
    /// The pod meter: may post events.
    Meter,
    /// The ledger: may ask for spend.
    Ledger,
}

/// Why a token was not accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthFailure {
    /// Not a known identity: `401`.
    Refused,
    /// The check could not be made: `503`, never an allow.
    Unavailable,
}

/// Turns a bearer token into a [`Caller`]. A token is an argument and is never carried by an error.
pub trait CallerVerifier: Send + Sync {
    /// The caller the token proves, or why not.
    fn verify(&self, token: &str) -> impl Future<Output = Result<Caller, AuthFailure>> + Send;
}

/// The most events in one batch.
pub const MAX_BATCH: usize = 100;
/// The largest body, in bytes.
pub const MAX_BODY: usize = 256 * 1024;

/// What the router needs: a store, a verifier, the clock and the lag.
pub struct IngestState<S, V> {
    /// Where events go.
    pub store: S,
    /// Who may call.
    pub verifier: V,
    /// Seconds since the Unix epoch.
    pub now: Box<dyn Fn() -> u64 + Send + Sync>,
    /// A spend window must end at least this many seconds before now.
    pub lag_secs: u64,
}

/// The router over any store and verifier.
pub fn router<S, V>(state: Arc<IngestState<S, V>>) -> Router
where
    S: UsageStore + 'static,
    V: CallerVerifier + 'static,
{
    Router::new()
        .route("/v1/events", post(events::<S, V>))
        .route("/v1/spend", post(spend::<S, V>))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY))
        .with_state(state)
}

fn json_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn problem(status: StatusCode, code: &'static str, field: Option<&'static str>) -> Response {
    let mut body = json!({ "error": code });
    if let Some(field) = field {
        body["field"] = json!(field);
    }
    json_response(status, body)
}

async fn authorize<S, V: CallerVerifier>(
    state: &IngestState<S, V>,
    headers: &HeaderMap,
    want: Caller,
) -> Result<(), Response> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| problem(StatusCode::UNAUTHORIZED, "unauthorized", None))?;
    match state.verifier.verify(token).await {
        Ok(caller) if caller == want => Ok(()),
        Ok(_) => Err(problem(StatusCode::FORBIDDEN, "forbidden", None)),
        Err(AuthFailure::Refused) => Err(problem(StatusCode::UNAUTHORIZED, "unauthorized", None)),
        Err(AuthFailure::Unavailable) => Err(problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            None,
        )),
    }
}

fn content_type(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    Some(raw.split(';').next()?.trim().to_ascii_lowercase())
}

fn text(obj: &serde_json::Map<String, Value>, key: &'static str) -> Result<String, &'static str> {
    match obj.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(key),
    }
}

/// One CloudEvent to the port's event; the error is the contract's name of the first field at fault.
pub fn parse_event(value: &Value) -> Result<PodMinuteEvent, &'static str> {
    let obj = value.as_object().ok_or("event")?;
    if text(obj, "specversion")? != "1.0" {
        return Err("specversion");
    }
    if text(obj, "type")? != EVENT_TYPE {
        return Err("type");
    }
    let data = obj.get("data").and_then(Value::as_object).ok_or("data")?;
    let weight = data
        .get("weight")
        .and_then(Value::as_u64)
        .and_then(|w| u32::try_from(w).ok())
        .ok_or("weight")?;
    let run = match data.get("run") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err("run"),
    };
    let event = PodMinuteEvent {
        source: text(obj, "source")?,
        id: text(obj, "id")?,
        subject: text(obj, "subject")?,
        weight,
        node_type: text(data, "nodeType").map_err(|_| "nodeType")?,
        pod_kind: text(data, "podKind").map_err(|_| "podKind")?,
        agent_service: text(data, "agentService").map_err(|_| "agentService")?,
        run,
    };
    event.validate().map_err(|e| match e {
        UsageError::Invalid { field } => field,
        _ => "event",
    })?;
    Ok(event)
}

async fn events<S, V>(
    State(state): State<Arc<IngestState<S, V>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response
where
    S: UsageStore + 'static,
    V: CallerVerifier + 'static,
{
    if let Err(denied) = authorize(&state, &headers, Caller::Meter).await {
        return denied;
    }
    let batch = match content_type(&headers).as_deref() {
        Some("application/cloudevents+json") => false,
        Some("application/cloudevents-batch+json") => true,
        _ => {
            return problem(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                None,
            );
        }
    };
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return problem(StatusCode::BAD_REQUEST, "invalid_json", None);
    };
    let items: Vec<Value> = if batch {
        match value {
            Value::Array(items) if !items.is_empty() && items.len() <= MAX_BATCH => items,
            _ => return problem(StatusCode::BAD_REQUEST, "invalid_batch", None),
        }
    } else {
        vec![value]
    };
    // A batch is checked whole before anything is stored, so a refusal stores nothing.
    let mut parsed = Vec::with_capacity(items.len());
    for item in &items {
        match parse_event(item) {
            Ok(event) => parsed.push(event),
            Err(field) => return problem(StatusCode::BAD_REQUEST, "invalid_event", Some(field)),
        }
    }
    let at = (state.now)();
    let (mut new, mut duplicate) = (0u32, 0u32);
    for event in parsed {
        match state.store.insert(event, at).await {
            Ok(Inserted::New) => new += 1,
            Ok(Inserted::Duplicate) => duplicate += 1,
            Err(UsageError::Invalid { field }) => {
                return problem(StatusCode::BAD_REQUEST, "invalid_event", Some(field));
            }
            Err(_) => return problem(StatusCode::SERVICE_UNAVAILABLE, "unavailable", None),
        }
    }
    if batch {
        json_response(
            StatusCode::OK,
            json!({ "new": new, "duplicate": duplicate }),
        )
    } else if new == 1 {
        StatusCode::CREATED.into_response()
    } else {
        StatusCode::OK.into_response()
    }
}

async fn spend<S, V>(
    State(state): State<Arc<IngestState<S, V>>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response
where
    S: UsageStore + 'static,
    V: CallerVerifier + 'static,
{
    if let Err(denied) = authorize(&state, &headers, Caller::Ledger).await {
        return denied;
    }
    let Ok(Value::Object(req)) = serde_json::from_slice::<Value>(&body) else {
        return problem(StatusCode::BAD_REQUEST, "invalid_json", None);
    };
    let Some(from) = req.get("from").and_then(Value::as_u64) else {
        return problem(StatusCode::BAD_REQUEST, "invalid_query", Some("from"));
    };
    let Some(to) = req.get("to").and_then(Value::as_u64) else {
        return problem(StatusCode::BAD_REQUEST, "invalid_query", Some("to"));
    };
    let account = match req.get("account") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(_) => return problem(StatusCode::BAD_REQUEST, "invalid_query", Some("account")),
    };
    if from >= to {
        return problem(StatusCode::BAD_REQUEST, "empty_window", None);
    }
    // Only a window that no late event can still enter has an answer that cannot change.
    if to > (state.now)().saturating_sub(state.lag_secs) {
        return problem(StatusCode::UNPROCESSABLE_ENTITY, "window_too_recent", None);
    }
    let query = SpendQuery {
        window: Window { from, to },
        account,
    };
    match state.store.spend(query).await {
        Ok(lines) => {
            let lines: Vec<Value> = lines
                .into_iter()
                .map(|l| {
                    json!({
                        "account": l.account,
                        "nodeType": l.node_type,
                        "weightedMinutes": l.weighted_minutes,
                    })
                })
                .collect();
            json_response(StatusCode::OK, json!({ "lines": lines }))
        }
        Err(UsageError::EmptyWindow) => problem(StatusCode::BAD_REQUEST, "empty_window", None),
        Err(UsageError::Overflow) => problem(StatusCode::UNPROCESSABLE_ENTITY, "overflow", None),
        Err(_) => problem(StatusCode::SERVICE_UNAVAILABLE, "unavailable", None),
    }
}
