//! The adapter's settings: plain Rust, validated. No YAML and no environment here (ANR-1103 reads the file).

use std::path::PathBuf;
use std::time::Duration;

use url::Url;

/// A setting that is refused. It names the setting and never a value.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("usage OTLP setting `{setting}`: {reason}")]
pub struct SettingsError {
    /// The field.
    pub setting: &'static str,
    /// Why, in fixed words.
    pub reason: &'static str,
}

fn bad(setting: &'static str, reason: &'static str) -> SettingsError {
    SettingsError { setting, reason }
}

/// What the sink is told.
#[derive(Clone, Debug)]
pub struct UsageOtlpSettings {
    /// The collector's base URL (`http://collector:4318`); `/v1/logs` is appended. `http` is allowed: the collector is in
    /// the cluster behind a NetworkPolicy, and AD-069 names no header.
    pub endpoint: Url,
    /// A PEM bundle that is the **only** trust for an `https` endpoint. None: the platform roots.
    pub ca: Option<PathBuf>,
    /// Events the queue holds before `send` refuses (default 4096).
    pub queue: usize,
    /// Events in one POST (default 512).
    pub batch: usize,
    /// How long a part batch waits before it is sent (default 5 s).
    pub interval: Duration,
    /// One POST's time limit (default 10 s).
    pub timeout: Duration,
    /// `service.name` of the resource (default `aap-operator`).
    pub service_name: String,
    /// The first retry's wait, doubling (default 1 s).
    pub backoff_min: Duration,
    /// The longest wait between retries (default 60 s).
    pub backoff_max: Duration,
    /// At most one warning this often while a batch is being retried (default 60 s).
    pub warn_every: Duration,
}

impl UsageOtlpSettings {
    /// The defaults, for this endpoint.
    #[must_use]
    pub fn new(endpoint: Url) -> Self {
        Self {
            endpoint,
            ca: None,
            queue: 4096,
            batch: 512,
            interval: Duration::from_secs(5),
            timeout: Duration::from_secs(10),
            service_name: "aap-operator".into(),
            backoff_min: Duration::from_secs(1),
            backoff_max: Duration::from_secs(60),
            warn_every: Duration::from_secs(60),
        }
    }

    /// Checks the bounds.
    ///
    /// # Errors
    ///
    /// The first setting that is out of bounds.
    pub fn validate(&self) -> Result<(), SettingsError> {
        let e = &self.endpoint;
        if !matches!(e.scheme(), "http" | "https") {
            return Err(bad("endpoint", "the scheme is not http or https"));
        }
        if e.host_str().is_none() {
            return Err(bad("endpoint", "there is no host"));
        }
        if !e.username().is_empty() || e.password().is_some() {
            return Err(bad("endpoint", "it carries a user name or a password"));
        }
        if e.query().is_some() || e.fragment().is_some() {
            return Err(bad("endpoint", "it carries a query or a fragment"));
        }
        if !(1..=1_000_000).contains(&self.queue) {
            return Err(bad("queue", "not between 1 and 1000000"));
        }
        if !(1..=10_000).contains(&self.batch) {
            return Err(bad("batch", "not between 1 and 10000"));
        }
        let secs = |d: Duration| d.as_secs_f64();
        if !(0.01..=3600.0).contains(&secs(self.interval)) {
            return Err(bad("interval", "not between 10 ms and 1 h"));
        }
        if !(0.01..=300.0).contains(&secs(self.timeout)) {
            return Err(bad("timeout", "not between 10 ms and 5 min"));
        }
        if self.backoff_min.is_zero() || self.backoff_max < self.backoff_min {
            return Err(bad("backoff", "min is zero or above max"));
        }
        if self.warn_every.is_zero() {
            return Err(bad("warnEvery", "is zero"));
        }
        let n = &self.service_name;
        let ok = !n.is_empty()
            && n.len() <= 128
            && n.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
        if !ok {
            return Err(bad("serviceName", "not 1 to 128 of [A-Za-z0-9._-]"));
        }
        Ok(())
    }

    /// The URL events are POSTed to.
    pub(crate) fn logs_url(&self) -> String {
        format!("{}/v1/logs", self.endpoint.as_str().trim_end_matches('/'))
    }
}
