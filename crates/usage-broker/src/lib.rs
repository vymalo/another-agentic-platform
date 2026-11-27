//! [`BrokerAccounts`] (ANR-1103, AD-069): the [`AccountSource`] of the pod-minute meter for a **run pod**. It asks the
//! orchestrator's broker `GET <base>/internal/runs/<run>/account` with the meter's bearer and reads `{ "accountId": … }`,
//! the person's opaque id (the IdP `sub`, never an e-mail).
//!
//! * **One path segment.** The run id is percent-encoded as exactly one segment: every byte but `[A-Za-z0-9_-]` is
//!   escaped, so a `/` or `:` or `.` in it cannot add a segment or reach another route. A run id that is `.` or `..` (a
//!   dot segment, which the URL parser would resolve away) or that breaks the text rule of a usage line is **never asked**:
//!   the answer is `None`, no account.
//! * **Nothing is logged here**, and no error holds the run id, the account, the URL or the token: the meter's metrics are
//!   how an operator learns what happened.
//! * **What each answer means.** `200` with a valid `accountId` is the account. A `404` whose body `code` is
//!   `run_not_bound` (a run the broker does not know, or forgot after its 24 h) and a `422` whose `code` is
//!   `account_not_opaque` (the person has no opaque account) are `None`: the minutes are counted as unattributed. The
//!   meter asks again on every pass until the minutes pass its backfill bound. Every other answer is
//!   [`AccountUnavailable`] and the minute is held: any other `404` or `422` (the broker answers 404 to every caller it
//!   does not accept, so a wrong identity or base path must not look like "no account"), `400`, `401`, `403`, `429`,
//!   `5xx`, a redirect, a connection that fails, a body that is too long or not what the contract says.
//! * **An agent pod has no source yet** (ANR-1100): it answers `None`.

use std::fmt;
use std::time::Duration;

use aap_usage::{AccountSource, AccountUnavailable, PodRef, is_line_text};
use url::Url;

/// The most bytes of an answer that are read: the real one is a few dozen.
pub const MAX_BODY: usize = 4096;

/// The meter's bearer token. It has no `Display` and its `Debug` prints nothing of it.
#[derive(Clone)]
pub struct Token(String);

impl Token {
    /// A token. Empty after trimming, or with a control character, is refused: it could not be a header value.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Token`].
    pub fn new(text: &str) -> Result<Self, SettingsError> {
        let text = text.trim();
        if text.is_empty() || text.chars().any(char::is_control) {
            return Err(SettingsError::Token);
        }
        Ok(Self(text.to_owned()))
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// Why a source could not be built. Fixed words: no value.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// The token is empty or holds a control character.
    #[error("the broker token is empty or holds a control character")]
    Token,
    /// The timeout is zero.
    #[error("the broker timeout must be at least one second")]
    Timeout,
    /// The base URL is not `http` or `https`, has no host, or carries a user name, a password, a query or a fragment.
    #[error(
        "the broker URL must be http or https with a host, and no credentials, query or fragment"
    )]
    Url,
    /// The HTTP client could not be built.
    #[error("the broker HTTP client could not be built")]
    Client(#[source] reqwest::Error),
}

/// Checks a base URL the way [`BrokerAccounts::new`] does, for the configuration file's `--check`.
///
/// # Errors
///
/// [`SettingsError::Url`].
pub fn check_base(base: &Url) -> Result<(), SettingsError> {
    let ok = matches!(base.scheme(), "http" | "https")
        && base.host_str().is_some()
        && base.username().is_empty()
        && base.password().is_none()
        && base.query().is_none()
        && base.fragment().is_none();
    if ok { Ok(()) } else { Err(SettingsError::Url) }
}

/// The [`AccountSource`] over the broker's HTTP route. Cheap to clone.
#[derive(Clone)]
pub struct BrokerAccounts {
    client: reqwest::Client,
    base: String,
    token: Token,
}

impl fmt::Debug for BrokerAccounts {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No URL, no token.
        f.write_str("BrokerAccounts")
    }
}

impl BrokerAccounts {
    /// A source that asks `<base>/internal/runs/<run>/account`, waiting at most `timeout`. No redirect is followed, and
    /// no proxy is used: the broker is in the cluster.
    ///
    /// # Errors
    ///
    /// [`SettingsError`] for a base URL that is refused, a zero timeout, or a client that cannot be built.
    pub fn new(base: &Url, token: Token, timeout: Duration) -> Result<Self, SettingsError> {
        check_base(base)?;
        if timeout.is_zero() {
            return Err(SettingsError::Timeout);
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(SettingsError::Client)?;
        Ok(Self {
            client,
            base: base.as_str().trim_end_matches('/').to_owned(),
            token,
        })
    }

    /// The URL asked for `run`, or `None` for a run id that is never asked.
    fn url(&self, run: &str) -> Option<String> {
        let segment = segment(run)?;
        Some(format!("{}/internal/runs/{segment}/account", self.base))
    }
}

/// `run` as exactly one path segment, or `None` when it is not a run id this source asks about.
fn segment(run: &str) -> Option<String> {
    if !is_line_text(run) || run == "." || run == ".." {
        return None;
    }
    let mut out = String::with_capacity(run.len());
    for b in run.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    Some(out)
}

impl AccountSource for BrokerAccounts {
    async fn account(&self, pod: &PodRef) -> Result<Option<String>, AccountUnavailable> {
        let run = match pod {
            PodRef::Run { run, .. } => run,
            // ANR-1100: no account source for an agent's own pod yet.
            PodRef::Agent { .. } => return Ok(None),
        };
        let Some(url) = self.url(run) else {
            return Ok(None);
        };
        let mut response = self
            .client
            .get(url)
            .bearer_auth(&self.token.0)
            .send()
            .await
            .map_err(|_| AccountUnavailable)?;
        let status = response.status().as_u16();
        if !matches!(status, 200 | 404 | 422) {
            // Every other answer, 400 included, is a fault to be seen (a wrong path, a wrong identity): held.
            return Err(AccountUnavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| AccountUnavailable)? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(AccountUnavailable);
            }
            body.extend_from_slice(&chunk);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&body).map_err(|_| AccountUnavailable)?;
        if status != 200 {
            // The broker answers 404 `run_not_bound` for an unknown run and also for every other caller: only
            // the two codes that say "no account for this run" are no account; any other 404 or 422 is held.
            let code = value.get("code").and_then(serde_json::Value::as_str);
            return match (status, code) {
                (404, Some("run_not_bound")) | (422, Some("account_not_opaque")) => Ok(None),
                _ => Err(AccountUnavailable),
            };
        }
        let account = value
            .get("accountId")
            .and_then(serde_json::Value::as_str)
            .ok_or(AccountUnavailable)?;
        // The alphabet has no `@`: an e-mail is refused here too, and held (the broker is wrong), never sent on.
        if is_line_text(account) {
            Ok(Some(account.to_owned()))
        } else {
            Err(AccountUnavailable)
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn source() -> BrokerAccounts {
        BrokerAccounts::new(
            &Url::parse("http://broker.example:8080/base/").unwrap(),
            Token::new("t").unwrap(),
            Duration::from_secs(1),
        )
        .unwrap()
    }

    #[test]
    fn a_run_id_is_one_percent_encoded_segment() {
        let s = source();
        assert_eq!(
            s.url("run-1_a").as_deref(),
            Some("http://broker.example:8080/base/internal/runs/run-1_a/account")
        );
        // `/`, `:` and `.` are escaped, so none adds a segment or a dot segment.
        assert_eq!(
            s.url("a/b:c.d").as_deref(),
            Some("http://broker.example:8080/base/internal/runs/a%2Fb%3Ac%2Ed/account")
        );
    }

    #[test]
    fn what_cannot_be_one_segment_is_never_asked() {
        let s = source();
        for run in ["", ".", "..", "a b", "a@b", "a?b", "a#b", "a%2Fb", "é"] {
            assert_eq!(s.url(run), None, "{run:?}");
        }
        assert_eq!(s.url(&"a".repeat(129)), None);
    }

    #[test]
    fn the_token_and_the_url_are_never_printed() {
        let s = source();
        let text = format!("{s:?} {:?}", Token::new("sekret").unwrap());
        assert!(
            !text.contains("sekret") && !text.contains("broker.example"),
            "{text}"
        );
    }

    #[test]
    fn bad_settings_are_refused_with_fixed_words() {
        let t = || Token::new("t").unwrap();
        for url in [
            "ftp://h",
            "http://u:p@h",
            "http://h/?q=1",
            "http://h/#f",
            "http://h?x",
        ] {
            let e = BrokerAccounts::new(&Url::parse(url).unwrap(), t(), Duration::from_secs(1));
            assert!(matches!(e, Err(SettingsError::Url)), "{url}");
        }
        let ok = Url::parse("https://h").unwrap();
        assert!(matches!(
            BrokerAccounts::new(&ok, t(), Duration::ZERO),
            Err(SettingsError::Timeout)
        ));
        assert!(Token::new(" ").is_err() && Token::new("a\nb").is_err());
    }
}
