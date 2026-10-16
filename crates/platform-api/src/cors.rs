//! CORS for the deployment's origins (§60a, *Clients without a web server*).
//!
//! The Platform API is called by the web as a single-page app and by the native shells, from origins the deployment names.
//! **No wildcard, no credentials**: the token travels in `Authorization`, which a script on any page cannot read, and a
//! credentialed mode would need `Access-Control-Allow-Credentials`, which is never sent. An origin is matched
//! **exactly** (tower-http compares the `Origin` header with each entry byte for byte), so a list is the whole policy.
//! An empty list is no CORS at all: no header is added, and a browser from another origin cannot read an answer.
//!
//! Rate limits are the edge's, not this crate's.

use std::time::Duration;

use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, ETAG, IF_MATCH, IF_NONE_MATCH};
use axum::http::{HeaderValue, Method};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// How long a browser may keep the answer to a preflight: ten minutes.
pub const PREFLIGHT_MAX_AGE: Duration = Duration::from_secs(600);

/// A list of origins that was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum CorsError {
    /// `*` (or an entry that contains one): any origin may read the API.
    #[error("cors.allowedOrigins: a wildcard is refused: name each origin")]
    Wildcard,
    /// `null`, the origin of a sandboxed frame or a local file: every such page has it.
    #[error("cors.allowedOrigins: `null` is refused: it is the origin of any sandboxed page")]
    Null,
    /// Not an origin as a browser writes one.
    #[error(
        "cors.allowedOrigins: entry {index} is not an origin (scheme://host[:port], lower case, no path, no trailing slash)"
    )]
    NotAnOrigin {
        /// Which entry, from 0.
        index: usize,
    },
}

/// The origins allowed to call the API from a browser, each validated and written the way a browser writes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CorsOrigins(Vec<String>);

impl CorsOrigins {
    /// No origin: no CORS.
    pub fn none() -> Self {
        Self::default()
    }

    /// The origins of a configuration file.
    ///
    /// # Errors
    ///
    /// [`CorsError`] for a wildcard, `null`, and anything that is not `scheme://host[:port]`.
    pub fn new<I, S>(origins: I) -> Result<Self, CorsError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out: Vec<String> = Vec::new();
        for (index, origin) in origins.into_iter().enumerate() {
            let origin = origin.as_ref();
            if origin.contains('*') {
                return Err(CorsError::Wildcard);
            }
            if origin.eq_ignore_ascii_case("null") {
                return Err(CorsError::Null);
            }
            if !is_origin(origin) {
                return Err(CorsError::NotAnOrigin { index });
            }
            if !out.iter().any(|o| o == origin) {
                out.push(origin.to_owned());
            }
        }
        Ok(Self(out))
    }

    /// The origins, once each, in the order given.
    pub fn as_slice(&self) -> &[String] {
        &self.0
    }

    /// Whether there is none.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The layer, or `None` for no origin.
    pub(crate) fn layer(&self) -> Option<CorsLayer> {
        if self.0.is_empty() {
            return None;
        }
        // Validated characters only: always a header value.
        let origins: Vec<HeaderValue> = self
            .0
            .iter()
            .filter_map(|o| HeaderValue::from_str(o).ok())
            .collect();
        Some(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list(origins))
                .allow_methods([Method::GET, Method::PUT, Method::DELETE, Method::OPTIONS])
                .allow_headers([AUTHORIZATION, CONTENT_TYPE, IF_MATCH, IF_NONE_MATCH])
                .expose_headers([ETAG])
                .max_age(PREFLIGHT_MAX_AGE),
        )
    }
}

/// `scheme "://" host [ ":" port ]` as a browser serialises an origin (RFC 6454 §6.2): the scheme and the host in lower
/// case, no user information, no path (not even `/`), no query, no fragment.
fn is_origin(text: &str) -> bool {
    let Some((scheme, authority)) = text.split_once("://") else {
        return false;
    };
    let scheme_ok = scheme
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase())
        && scheme.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.')
        });
    if !scheme_ok {
        return false;
    }
    let (host, port) = match authority.strip_prefix('[') {
        // An IPv6 literal: `[::1]` or `[::1]:8080`.
        Some(rest) => match rest.split_once(']') {
            Some((inner, "")) => (Some(inner), None),
            Some((inner, tail)) => (Some(inner), tail.strip_prefix(':')),
            None => (None, None),
        },
        None => match authority.split_once(':') {
            Some((host, port)) => (Some(host), Some(port)),
            None => (Some(authority), None),
        },
    };
    let host_ok = host.is_some_and(|h| {
        !h.is_empty()
            && h.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b':')
            })
    });
    let port_ok = port.is_none_or(|p| {
        !p.is_empty()
            && p.len() <= 5
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u32>().is_ok_and(|n| (1..=65535).contains(&n))
    });
    host_ok && port_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_as_a_browser_writes_them() {
        let ok = CorsOrigins::new([
            "https://agents.example.com",
            "http://localhost:3000",
            "tauri://localhost",
            "http://tauri.localhost",
            "https://agents.example.com",
            "http://[::1]:8080",
        ])
        .unwrap();
        assert_eq!(ok.as_slice().len(), 5, "a repeated origin is one");
        assert!(CorsOrigins::new(Vec::<String>::new()).unwrap().is_empty());
    }

    #[test]
    fn a_wildcard_null_and_anything_else_is_refused() {
        for (bad, why) in [
            ("*", CorsError::Wildcard),
            ("https://*.example.com", CorsError::Wildcard),
            ("null", CorsError::Null),
            ("NULL", CorsError::Null),
        ] {
            assert_eq!(CorsOrigins::new([bad]).unwrap_err(), why, "{bad}");
        }
        for bad in [
            "agents.example.com",
            "https://agents.example.com/",
            "https://agents.example.com/app",
            "https://AGENTS.example.com",
            "https://user@agents.example.com",
            "https://agents.example.com:0",
            "https://agents.example.com:99999",
            "https://agents.example.com:",
            "https://agents.example.com?x=1",
            "https://",
            "://host",
            "",
            " https://agents.example.com",
        ] {
            assert!(
                matches!(
                    CorsOrigins::new(["https://ok.example.com", bad]),
                    Err(CorsError::NotAnOrigin { index: 1 })
                ),
                "{bad}"
            );
        }
    }
}
