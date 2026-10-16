//! Reading the body of a write: the media type, the size, and a JSON error that quotes nothing.
//!
//! The order is the cheap checks first: from the headers, `400` for two `Content-Type` headers and `415` for a media type
//! that is not JSON; `413` from `Content-Length` before a byte is read (and from the count of bytes read, for a body that has
//! no length or lies); then `400` for what is not JSON of the form.

use axum::body::Body;
use axum::http::HeaderMap;
use axum::http::header::{CONTENT_LENGTH, CONTENT_TYPE};
use futures::StreamExt;
use serde::de::DeserializeOwned;

use crate::problem::Problem;

/// The most a body may hold: 2 MiB. §60a lets a folder agent's files be 1 MiB; their JSON is a little larger (quotes, line
/// breaks and non-ASCII characters are escaped), and 2 MiB leaves room for it. The API server's own limit for one object
/// (etcd's 1.5 MiB by default) is what a larger folder meets next, as a `422` of its own.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Whether the request says its body is JSON: `application/json`, with a `charset` of `utf-8` or none.
fn is_json(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let (Some(value), None) = (values.next(), values.next()) else {
        return false;
    };
    let Ok(text) = value.to_str() else {
        return false;
    };
    let mut parts = text.split(';');
    let essence = parts.next().unwrap_or_default().trim();
    if !essence.eq_ignore_ascii_case("application/json") {
        return false;
    }
    parts.all(|p| {
        let p = p.trim();
        match p.split_once('=') {
            Some((name, value)) if name.trim().eq_ignore_ascii_case("charset") => {
                value.trim().trim_matches('"').eq_ignore_ascii_case("utf-8")
            }
            _ => true,
        }
    })
}

/// The `415` check alone, for a handler that wants it before it looks at other headers.
///
/// # Errors
///
/// `400` for two `Content-Type` headers (which one the sender meant is not for the server to pick, as with two
/// `Authorization` or two `If-Match`), `415` when the body is not declared as JSON.
pub fn require_json(headers: &HeaderMap) -> Result<(), Problem> {
    if headers.get_all(CONTENT_TYPE).iter().nth(1).is_some() {
        return Err(Problem::invalid_request("send one Content-Type header"));
    }
    if is_json(headers) {
        Ok(())
    } else {
        Err(Problem::unsupported_media_type())
    }
}

/// The body, whole, if it is at most `limit` bytes.
async fn bytes(headers: &HeaderMap, body: Body, limit: usize) -> Result<Vec<u8>, Problem> {
    let declared = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<usize>().ok());
    if declared.is_some_and(|n| n > limit) {
        return Err(Problem::too_large(limit));
    }
    let mut out = Vec::new();
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| Problem::invalid_request("the body could not be read"))?;
        if out.len() + chunk.len() > limit {
            return Err(Problem::too_large(limit));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// The form in the body.
///
/// # Errors
///
/// `415` for a body that is not declared as JSON, `413` for one over `limit`, `400` for one that is not JSON or is not the
/// form (a member the form does not have, a missing member, a wrong type). The `400` names a member and never quotes the
/// body: `serde_json`'s own sentences quote values (`invalid type: string "…"`), which a person may have typed where a
/// secret does not belong.
pub async fn json<T: DeserializeOwned>(
    headers: &HeaderMap,
    body: Body,
    limit: usize,
) -> Result<T, Problem> {
    require_json(headers)?;
    let raw = bytes(headers, body, limit).await?;
    if raw.is_empty() {
        return Err(Problem::invalid_body("the body is empty"));
    }
    serde_json::from_slice(&raw).map_err(|e| Problem::invalid_body(describe(&e)))
}

/// What is wrong with the body, in words that cannot hold a value of it.
fn describe(error: &serde_json::Error) -> String {
    use serde_json::error::Category;
    let at = format!("line {}, column {}", error.line(), error.column());
    match error.classify() {
        Category::Syntax | Category::Eof => format!("the body is not JSON ({at})"),
        Category::Io => "the body could not be read".to_owned(),
        Category::Data => {
            let message = error.to_string();
            // `unknown field `x`, expected …` and `missing field `x``: a member's name, not a value.
            let member = |marker: &str| {
                message
                    .strip_prefix(marker)
                    .and_then(|rest| rest.split_once('`'))
                    .map(|(name, _)| name)
                    .filter(|name| is_member_name(name))
            };
            if let Some(name) = member("unknown field `") {
                format!(
                    "the form has no member {name}: a body with a member the form does not have is not a form ({at})"
                )
            } else if let Some(name) = member("missing field `") {
                format!("the form needs the member {name} ({at})")
            } else if message.starts_with("duplicate field") {
                format!("a member is said twice ({at})")
            } else {
                format!("a member has a value of the wrong type or shape ({at})")
            }
        }
    }
}

/// A member name as the forms have them, so that what is echoed is a name and nothing else.
fn is_member_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn with(content_type: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = content_type {
            h.insert(CONTENT_TYPE, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn only_json_in_utf_8_is_json() {
        for ok in [
            "application/json",
            "Application/JSON",
            "application/json; charset=utf-8",
            "application/json;charset=\"UTF-8\"",
        ] {
            assert!(is_json(&with(Some(ok))), "{ok}");
        }
        for bad in [
            "text/plain",
            "application/json-patch+json",
            "application/yaml",
            "application/json; charset=latin1",
            "application/jsonx",
        ] {
            assert!(!is_json(&with(Some(bad))), "{bad}");
        }
        assert!(!is_json(&with(None)));
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Form {
        #[allow(dead_code)]
        suspend: bool,
        #[allow(dead_code)]
        name: String,
    }

    #[test]
    fn the_error_names_a_member_and_never_quotes_a_value() {
        const SENTINEL: &str = "sentinel-value-71c3";
        for body in [
            format!(r#"{{"suspend": "{SENTINEL}", "name": "a"}}"#),
            format!(r#"{{"suspend": true, "name": "a", "password": "{SENTINEL}"}}"#),
            format!(r#"{{"suspend": true, "name": 7, "{SENTINEL}": 1}}"#),
            format!(r#"{{"suspend": true, "{SENTINEL}"#),
            format!(r#"{{"name": "{SENTINEL}"}}"#),
            format!(r#"["{SENTINEL}"]"#),
        ] {
            let e = serde_json::from_str::<Form>(&body).unwrap_err();
            let said = describe(&e);
            assert!(!said.contains(SENTINEL), "{said}");
        }
        let e = serde_json::from_str::<Form>(r#"{"suspend": true, "name": "a", "password": "x"}"#)
            .unwrap_err();
        assert!(describe(&e).contains("password"), "{}", describe(&e));
    }
}
