//! CBOR as a second media type (AD-061): one layer in front of the handlers, so that no handler knows it.
//!
//! * A request body sent as `application/cbor` is checked by [`walk`] and turned into JSON, and the handler reads the JSON
//!   it always read ([`decode_request`], inside the authorization layer).
//! * An answer is encoded as CBOR when `Accept` prefers it ([`encode_response`], outside the authorization layer, so that a
//!   refusal is negotiated too). No `Accept`, `*/*` or JSON is JSON; neither listed is `406`. `Vary: Accept` is on every
//!   negotiated answer.
//! * The data model is the JSON one: text keys, no byte strings, no tags, no `undefined`, no indefinite length. A body that
//!   is not is `400`, said without quoting a value. `GET /v1/openapi.json` and the YAML export are not negotiated.
//! * An `ETag` is computed over the stored object, never over these bytes, so it is the same in both encodings.

use axum::body::{Body, to_bytes};
use axum::extract::{MatchedPath, Request};
use axum::http::header::{ACCEPT, CONTENT_LENGTH, CONTENT_TYPE, VARY};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::Value;

use crate::body::{MAX_BODY_BYTES, bytes};
use crate::problem::Problem;

/// The media type of a CBOR body.
pub const CBOR: &str = "application/cbor";

/// The most nesting a CBOR body may have. A form is a few levels deep; this keeps the walk and the decode off the stack's edge.
const MAX_DEPTH: usize = 32;

/// The most an answer may hold when it is read back to be encoded: far above any answer of the API.
const MAX_ANSWER_BYTES: usize = 64 * 1024 * 1024;

/// What the caller wants to be answered with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wanted {
    Json,
    Cbor,
}

/// The routes whose answer is not a JSON document of the data model.
fn is_exempt(path: Option<&MatchedPath>) -> bool {
    path.is_some_and(|p| p.as_str().ends_with("/openapi.json") || p.as_str().ends_with("/yaml"))
}

/// `Accept`, read: `None` when no medium of this API is acceptable.
fn wanted(headers: &HeaderMap) -> Option<Wanted> {
    let joined = headers
        .get_all(ACCEPT)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .collect::<Vec<_>>()
        .join(",");
    if joined.trim().is_empty() {
        return Some(Wanted::Json);
    }
    // The best quality, and whether it came from the medium's own name, for each of the two.
    let (mut json, mut cbor) = ((0u32, false), (0u32, false));
    for range in joined.split(',') {
        let mut parts = range.split(';');
        let essence = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
        let q = parts
            .filter_map(|p| p.trim().strip_prefix("q=").map(str::trim))
            .find_map(|q| q.parse::<f32>().ok())
            .map_or(1000, |q| (q.clamp(0.0, 1.0) * 1000.0) as u32);
        let better = |slot: &mut (u32, bool), exact: bool| {
            if (q, exact) > *slot {
                *slot = (q, exact);
            }
        };
        match essence.as_str() {
            "application/json" | "application/problem+json" => better(&mut json, true),
            CBOR => better(&mut cbor, true),
            "application/*" | "*/*" => {
                better(&mut json, false);
                better(&mut cbor, false);
            }
            _ => {}
        }
    }
    match (json.0, cbor.0) {
        (0, 0) => None,
        (j, c) if c > j || (c == j && cbor.1 && !json.1) => Some(Wanted::Cbor),
        _ => Some(Wanted::Json),
    }
}

/// Whether the request declares a CBOR body.
fn is_cbor(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return false;
    };
    value.to_str().is_ok_and(|text| {
        text.split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .eq_ignore_ascii_case(CBOR)
    })
}

/// Turns a CBOR body into the JSON the handlers read.
pub(crate) async fn decode_request(request: Request, next: Next) -> Response {
    if !is_cbor(request.headers()) {
        return next.run(request).await;
    }
    let (mut parts, body) = request.into_parts();
    let raw = match bytes(&parts.headers, body, MAX_BODY_BYTES).await {
        Ok(raw) => raw,
        Err(problem) => return problem.into_response(),
    };
    let json = if raw.is_empty() {
        raw
    } else {
        match to_json(&raw) {
            Ok(json) => json,
            Err(problem) => return problem.into_response(),
        }
    };
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    parts.headers.remove(CONTENT_LENGTH);
    next.run(Request::from_parts(parts, Body::from(json))).await
}

/// Encodes the answer as the caller accepts.
pub(crate) async fn encode_response(request: Request, next: Next) -> Response {
    let exempt = is_exempt(request.extensions().get::<MatchedPath>());
    let wanted = wanted(request.headers());
    if exempt {
        return next.run(request).await;
    }
    let Some(wanted) = wanted else {
        return vary(Problem::not_acceptable().into_response());
    };
    let response = next.run(request).await;
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|t| {
            t.starts_with("application/json") || t.starts_with("application/problem+json")
        });
    if !is_json {
        return response;
    }
    if wanted == Wanted::Json {
        return vary(response);
    }
    let (mut parts, body) = response.into_parts();
    let encoded = match to_bytes(body, MAX_ANSWER_BYTES).await {
        Ok(raw) => serde_json::from_slice::<Value>(&raw)
            .ok()
            .and_then(|value| {
                let mut out = Vec::with_capacity(raw.len());
                ciborium::into_writer(&value, &mut out).ok().map(|()| out)
            }),
        Err(_) => None,
    };
    let Some(encoded) = encoded else {
        tracing::error!("an answer could not be written as CBOR");
        return vary(Problem::internal().into_response());
    };
    parts
        .headers
        .insert(CONTENT_TYPE, HeaderValue::from_static(CBOR));
    parts.headers.remove(CONTENT_LENGTH);
    vary(Response::from_parts(parts, Body::from(encoded)))
}

fn vary(mut response: Response) -> Response {
    response
        .headers_mut()
        .append(VARY, HeaderValue::from_static("Accept"));
    response
}

/// The JSON of a CBOR body, or a `400` that quotes nothing of it.
fn to_json(raw: &[u8]) -> Result<Vec<u8>, Problem> {
    walk(raw).map_err(|why| {
        Problem::invalid_body(format!("the body is not CBOR of the data model: {why}"))
    })?;
    let value: Value = ciborium::from_reader(raw).map_err(|_| {
        Problem::invalid_body("the body is not CBOR of the data model: a value of the wrong shape")
    })?;
    serde_json::to_vec(&value).map_err(|_| Problem::invalid_body("the body could not be read"))
}

/// Checks `raw` is exactly one item of the data model: definite lengths, text keys said once, no byte string, tag or simple
/// value but `false`, `true` and `null`, nothing after it, at most [`MAX_DEPTH`] levels. It allocates nothing but the keys of
/// one map at a time, and reads no item it has not the bytes for.
fn walk(raw: &[u8]) -> Result<(), &'static str> {
    let mut at = 0;
    item(raw, &mut at, 0)?;
    if at == raw.len() {
        Ok(())
    } else {
        Err("bytes after the document")
    }
}

fn head(raw: &[u8], at: &mut usize) -> Result<(u8, u8, u64), &'static str> {
    let first = *raw.get(*at).ok_or("the document ends early")?;
    *at += 1;
    let (major, info) = (first >> 5, first & 31);
    let width = match info {
        0..=23 => return Ok((major, info, u64::from(info))),
        24 => 1,
        25 => 2,
        26 => 4,
        27 => 8,
        31 => return Err("indefinite length"),
        _ => return Err("a reserved encoding"),
    };
    let end = at
        .checked_add(width)
        .filter(|e| *e <= raw.len())
        .ok_or("the document ends early")?;
    let argument = raw[*at..end]
        .iter()
        .fold(0u64, |n, b| (n << 8) | u64::from(*b));
    *at = end;
    Ok((major, info, argument))
}

fn item(raw: &[u8], at: &mut usize, depth: usize) -> Result<(), &'static str> {
    if depth > MAX_DEPTH {
        return Err("nested too deep");
    }
    let (major, info, n) = head(raw, at)?;
    match major {
        0 | 1 => Ok(()),
        2 => Err("a byte string"),
        3 => {
            let end = usize::try_from(n)
                .ok()
                .and_then(|n| at.checked_add(n))
                .filter(|e| *e <= raw.len())
                .ok_or("the document ends early")?;
            std::str::from_utf8(&raw[*at..end]).map_err(|_| "text that is not UTF-8")?;
            *at = end;
            Ok(())
        }
        4 => {
            // Each item is at least one byte: a count the rest cannot hold is refused before any loop.
            if n > (raw.len() - *at) as u64 {
                return Err("the document ends early");
            }
            for _ in 0..n {
                item(raw, at, depth + 1)?;
            }
            Ok(())
        }
        5 => {
            if n > (raw.len() - *at) as u64 {
                return Err("the document ends early");
            }
            let mut keys: Vec<&[u8]> = Vec::new();
            for _ in 0..n {
                let start = *at;
                if raw.get(start).map(|b| b >> 5) != Some(3) {
                    return Err("a key that is not text");
                }
                item(raw, at, depth + 1)?;
                keys.push(&raw[start..*at]);
                item(raw, at, depth + 1)?;
            }
            keys.sort_unstable();
            if keys.windows(2).any(|w| w[0] == w[1]) {
                return Err("a member said twice");
            }
            Ok(())
        }
        6 => Err("a tag"),
        // Major type 7: `false`, `true` and `null` (20 to 22) and the three float widths (25 to 27).
        _ => match info {
            20..=22 | 25..=27 => Ok(()),
            _ => Err("a simple value that JSON has not"),
        },
    }
}
