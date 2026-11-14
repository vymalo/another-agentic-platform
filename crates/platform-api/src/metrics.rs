//! Two counters in the Prometheus text format, in the style of the controller's (`aap-controller`, `metrics.rs`): no client
//! library, a counter is a number and the exposition format is lines of text.
//!
//! **Every label value is chosen by this crate from a closed set**, never taken from a request: the route is a template of
//! [`ROUTES`](crate::ROUTES) or `unmatched`, the method one of five words or `other`, the status a number, the reason one of
//! nine words. So the series are bounded, and no token, body, path, name or user id can reach them.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use aap_ports::{AuthError, InvalidToken};
use axum::http::Method;

use crate::routes;

/// Names of the series.
pub mod metric {
    /// Requests, by `route` (a template of the table, or `unmatched`), `method` and `status`.
    pub const REQUESTS: &str = "aap_api_requests_total";
    /// Requests whose token was refused or could not be checked, by `reason`.
    pub const AUTH_FAILURES: &str = "aap_api_auth_failures_total";
}

/// The `route` label of a request that matched no route of the table.
pub const UNMATCHED: &str = "unmatched";

fn help(series: &str) -> &'static str {
    match series {
        metric::REQUESTS => "Requests of the Platform API, by route template, method and status.",
        metric::AUTH_FAILURES => "Requests refused at authentication, by reason.",
        _ => "",
    }
}

/// The counters of the API. Share it in an `Arc`.
#[derive(Debug, Default)]
pub struct ApiMetrics {
    series: Mutex<BTreeMap<(&'static str, String), u64>>,
}

fn labels(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(k);
        out.push_str("=\"");
        for c in v.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    out
}

/// The closed word of a method.
fn method_label(method: &Method) -> &'static str {
    match *method {
        Method::GET => "GET",
        Method::PUT => "PUT",
        Method::DELETE => "DELETE",
        Method::POST => "POST",
        Method::HEAD => "HEAD",
        Method::OPTIONS => "OPTIONS",
        _ => "other",
    }
}

/// The closed word of an authentication failure.
pub(crate) fn reason_label(error: &AuthError) -> &'static str {
    match error {
        AuthError::Missing => "missing",
        AuthError::Unavailable(_) => "unavailable",
        AuthError::Invalid(why) => match why {
            InvalidToken::Malformed => "malformed",
            InvalidToken::BadSignature => "bad_signature",
            InvalidToken::UnknownKey => "unknown_key",
            InvalidToken::Expired => "expired",
            InvalidToken::WrongIssuer => "wrong_issuer",
            InvalidToken::WrongAudience => "wrong_audience",
            InvalidToken::MissingClaim => "missing_claim",
        },
    }
}

impl ApiMetrics {
    /// An empty set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn add(&self, series: &'static str, label_pairs: &[(&str, &str)]) {
        let mut map = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        *map.entry((series, labels(label_pairs))).or_insert(0) += 1;
    }

    /// Count one request. `template` is the matched path as the router saw it: it is looked up in the table, and anything the
    /// table does not have counts as [`UNMATCHED`].
    pub(crate) fn request(&self, template: Option<&str>, method: &Method, status: u16) {
        let route = template.and_then(routes::template).unwrap_or(UNMATCHED);
        self.add(
            metric::REQUESTS,
            &[
                ("route", route),
                ("method", method_label(method)),
                ("status", &status.to_string()),
            ],
        );
    }

    /// Count one authentication failure.
    pub(crate) fn auth_failure(&self, error: &AuthError) {
        self.add(metric::AUTH_FAILURES, &[("reason", reason_label(error))]);
    }

    /// The value of a series, 0 when it was never touched.
    #[must_use]
    pub fn get(&self, series: &'static str, label_pairs: &[(&str, &str)]) -> u64 {
        let map = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        map.get(&(series, labels(label_pairs)))
            .copied()
            .unwrap_or(0)
    }

    /// The text of a scrape.
    #[must_use]
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let map = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out = String::new();
        let mut last = "";
        for ((series, label_text), value) in map.iter() {
            if *series != last {
                let _ = write!(
                    out,
                    "# HELP {series} {}\n# TYPE {series} counter\n",
                    help(series)
                );
                last = series;
            }
            let _ = writeln!(out, "{series}{{{label_text}}} {value}");
        }
        out
    }
}
