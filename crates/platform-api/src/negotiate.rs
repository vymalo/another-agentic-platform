//! CBOR as the second media type (AD-061): one layer, outside the guard, so that no handler knows it.
//!
//! A request with `Content-Type: application/cbor` is turned into the JSON the handlers read; an answer is turned into CBOR
//! when `Accept` asks for it. Both go through `serde_json::Value`, so the same request in either encoding gives the same
//! values, and the `ETag` (computed over the stored object, not the wire bytes) is the same. JSON is the default.
//!
//! `Accept` names `application/json` (or `application/problem+json`, `*/*`, `application/*`) or `application/cbor`; the first
//! supported entry wins and an entry with `q=0` is skipped. A header that lists types and none of them is supported is `406`.
//! A problem in CBOR is `application/cbor` with the same members. `GET /v1/openapi.json` stays JSON.

use axum::body::{Body, to_bytes};
use axum::extract::Request;
use axum::http::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, VARY};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::body::MAX_BODY_BYTES;
use crate::problem::Problem;

/// The media type of a CBOR body or answer.
pub const CBOR: &str = "application/cbor";

/// The most a JSON answer may be to be turned into CBOR.
const MAX_ANSWER_BYTES: usize = 32 * 1024 * 1024;

/// Nesting deeper than this is refused, not recursed into.
const RECURSION_LIMIT: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Wire {
    Json,
    Cbor,
}

fn essence(value: &str) -> String {
    value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// What the caller asked to be answered in, or `None` for `406`.
fn accepted(headers: &HeaderMap) -> Option<Wire> {
    let mut any = false;
    for value in headers.get_all(ACCEPT) {
        let Ok(text) = value.to_str() else { continue };
        for entry in text.split(',') {
            let entry = entry.trim();
            if entry.is_empty() {
                continue;
            }
            any = true;
            let zero = entry.split(';').skip(1).any(|p| {
                p.trim()
                    .strip_prefix("q=")
                    .or_else(|| p.trim().strip_prefix("Q="))
                    .is_some_and(|q| q.trim().parse::<f32>().is_ok_and(|q| q <= 0.0))
            });
            if zero {
                continue;
            }
            match essence(entry).as_str() {
                CBOR => return Some(Wire::Cbor),
                "application/json" | "application/problem+json" | "application/*" | "*/*" => {
                    return Some(Wire::Json);
                }
                _ => {}
            }
        }
    }
    if any { None } else { Some(Wire::Json) }
}

fn is_cbor_request(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    match (values.next(), values.next()) {
        (Some(v), None) => v.to_str().is_ok_and(|v| essence(v) == CBOR),
        _ => false,
    }
}

/// CBOR to the JSON the handlers read. The error says nothing of the body's values.
fn to_json(raw: &[u8]) -> Result<Vec<u8>, Problem> {
    let bad = || {
        Problem::invalid_body(
            "the body is not CBOR of the form's shape (text keys, no byte strings)",
        )
    };
    let value: serde_json::Value =
        ciborium::de::from_reader_with_recursion_limit(raw, RECURSION_LIMIT).map_err(|_| bad())?;
    serde_json::to_vec(&value).map_err(|_| bad())
}

fn encode(response: &mut Response, bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return false;
    };
    let mut out = Vec::new();
    if ciborium::ser::into_writer(&value, &mut out).is_err() {
        return false;
    }
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(CBOR));
    headers.remove(CONTENT_LENGTH);
    *response.body_mut() = Body::from(out);
    true
}

fn vary(response: &mut Response) {
    response
        .headers_mut()
        .append(VARY, HeaderValue::from_static("Accept"));
}

/// The layer.
pub(crate) async fn layer(mut request: Request, next: Next) -> Response {
    let Some(wire) = accepted(request.headers()) else {
        let mut response = Problem::not_acceptable().into_response();
        vary(&mut response);
        return response;
    };
    let path = request.uri().path().to_owned();
    let wire = if path == "/v1/openapi.json" {
        Wire::Json
    } else {
        wire
    };

    if is_cbor_request(request.headers()) {
        let declared = request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<usize>().ok());
        let too_large = declared.is_some_and(|n| n > MAX_BODY_BYTES);
        let (mut parts, body) = request.into_parts();
        let converted = if too_large {
            Err(Problem::too_large(MAX_BODY_BYTES))
        } else {
            match to_bytes(body, MAX_BODY_BYTES).await {
                Err(_) => Err(Problem::too_large(MAX_BODY_BYTES)),
                Ok(raw) if raw.is_empty() => Ok(Vec::new()),
                Ok(raw) => to_json(&raw),
            }
        };
        match converted {
            Err(problem) => {
                let mut response = problem.into_response();
                finish(&mut response, wire).await;
                return response;
            }
            Ok(json) => {
                parts
                    .headers
                    .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
                parts
                    .headers
                    .insert(CONTENT_LENGTH, HeaderValue::from(json.len()));
                request = Request::from_parts(parts, Body::from(json));
            }
        }
    }

    let mut response = next.run(request).await;
    finish(&mut response, wire).await;
    response
}

async fn finish(response: &mut Response, wire: Wire) {
    vary(response);
    if wire != Wire::Cbor || response.status() == StatusCode::NO_CONTENT {
        return;
    }
    let json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(essence)
        .is_some_and(|e| e == "application/json" || e == "application/problem+json");
    if !json {
        return;
    }
    let body = std::mem::replace(response.body_mut(), Body::empty());
    match to_bytes(body, MAX_ANSWER_BYTES).await {
        Ok(bytes) if bytes.is_empty() => {}
        Ok(bytes) => {
            if !encode(response, &bytes) {
                *response.body_mut() = Body::from(bytes);
            }
        }
        Err(_) => *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(accept: Option<&str>) -> HeaderMap {
        let mut m = HeaderMap::new();
        if let Some(a) = accept {
            m.insert(ACCEPT, HeaderValue::from_str(a).unwrap());
        }
        m
    }

    #[test]
    fn accept_rules() {
        assert!(accepted(&h(None)) == Some(Wire::Json));
        assert!(accepted(&h(Some("*/*"))) == Some(Wire::Json));
        assert!(accepted(&h(Some("application/cbor"))) == Some(Wire::Cbor));
        assert!(accepted(&h(Some("text/html, application/cbor;q=0.5"))) == Some(Wire::Cbor));
        assert!(accepted(&h(Some("application/cbor;q=0, application/json"))) == Some(Wire::Json));
        assert!(accepted(&h(Some("text/html"))).is_none());
        assert!(accepted(&h(Some("application/xml"))).is_none());
    }
}
