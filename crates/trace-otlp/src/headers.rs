//! The headers a collector wants (a token, usually): read from a file, kept in a type that cannot print them.

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

/// The largest file `AAP_OTEL_HEADERS_FILE` may name.
pub const MAX_HEADERS_FILE: usize = 4096;

/// Headers the file may not set: the exporter owns them (the body's, the connection's), or they are hop-by-hop or carry a
/// credential of someone else's (a cookie, a proxy's), which a collector token never is.
const FORBIDDEN: [&str; 14] = [
    "host",
    "content-type",
    "content-length",
    "traceparent",
    "content-encoding",
    "connection",
    "transfer-encoding",
    "te",
    "upgrade",
    "proxy-authorization",
    "cookie",
    "user-agent",
    "accept-encoding",
    "expect",
];

/// Parsed headers. There is no `Display`, and `Debug` says how many, never what.
#[derive(Clone, Default)]
pub struct Headers(HeaderMap);

impl std::fmt::Debug for Headers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Headers({} hidden)", self.0.len())
    }
}

impl Headers {
    /// Parse the `k=v,k2=v2` syntax of `OTEL_EXPORTER_OTLP_HEADERS`. The error is a fixed text: it never holds a
    /// name or a value of the file.
    ///
    /// # Errors
    ///
    /// The text is too long, a pair has no `=`, a name is not an HTTP token or is one the exporter owns, or a value is
    /// not a header value.
    pub fn parse(text: &str) -> Result<Self, &'static str> {
        if text.len() > MAX_HEADERS_FILE {
            return Err("the file is longer than 4 KiB");
        }
        let mut map = HeaderMap::new();
        for pair in text.trim().split(',').filter(|p| !p.trim().is_empty()) {
            let (name, value) = pair.split_once('=').ok_or("a pair has no `=`")?;
            let name = HeaderName::from_bytes(name.trim().as_bytes())
                .map_err(|_| "a header name is not an HTTP token")?;
            if FORBIDDEN.contains(&name.as_str()) {
                return Err("a header is one the exporter sets itself");
            }
            let mut value =
                HeaderValue::from_str(value.trim()).map_err(|_| "a header value is not valid")?;
            value.set_sensitive(true);
            map.append(name, value);
        }
        Ok(Self(map))
    }

    /// The map to put on a request.
    #[must_use]
    pub fn map(&self) -> &HeaderMap {
        &self.0
    }

    /// How many headers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pairs_and_marks_values_sensitive() {
        let h = Headers::parse("authorization=Bearer abc, x-team = blue\n").unwrap();
        assert_eq!(h.len(), 2);
        assert!(h.map().get("authorization").unwrap().is_sensitive());
        assert_eq!(h.map().get("x-team").unwrap(), "blue");
    }

    #[test]
    fn refuses_what_it_must_without_quoting_it() {
        for (bad, why) in [
            ("host=evil", "sets itself"),
            ("Content-Type=text/plain", "sets itself"),
            ("traceparent=00-x", "sets itself"),
            ("Connection=close", "sets itself"),
            ("transfer-encoding=chunked", "sets itself"),
            ("te=trailers", "sets itself"),
            ("upgrade=h2c", "sets itself"),
            ("Proxy-Authorization=Basic x", "sets itself"),
            ("cookie=a=b", "sets itself"),
            ("user-agent=x", "sets itself"),
            ("accept-encoding=gzip", "sets itself"),
            ("expect=100-continue", "sets itself"),
            ("no-equals", "no `=`"),
            ("bad name=1", "token"),
            ("a=b\u{1}c", "not valid"),
        ] {
            let e = Headers::parse(bad).unwrap_err();
            assert!(e.contains(why), "{e}");
            assert!(!e.contains("evil") && !e.contains("00-x"));
        }
        assert!(Headers::parse(&"a=".repeat(MAX_HEADERS_FILE)).is_err());
        assert!(Headers::parse(&format!("a={}", "b".repeat(MAX_HEADERS_FILE - 2))).is_ok());
        assert!(Headers::parse(&format!("a={}", "b".repeat(MAX_HEADERS_FILE - 1))).is_err());
    }

    #[test]
    fn debug_shows_no_value() {
        let h = Headers::parse("authorization=SECRET-VALUE").unwrap();
        let shown = format!("{h:?}");
        assert!(
            !shown.contains("SECRET") && !shown.contains("authorization"),
            "{shown}"
        );
    }
}
