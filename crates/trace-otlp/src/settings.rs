//! The settings, from the standard `OTEL_*` variables (a strict subset) or from a configuration file's `telemetry.otlp`
//! section (ANR-962, AD-064).
//!
//! One pure function reads them, so both binaries share the rules and `--check` can run them. An error names the
//! variable (or the setting) and a fixed reason, never a value: a value may be a URL with a password.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use aap_trace::{ResourceKey, Sampler};
use serde::Deserialize;
use url::Url;

use crate::headers::Headers;

/// Why a variable was refused: its name and a fixed text. Never the value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{variable}: {reason}")]
pub struct SettingsError {
    /// The variable.
    pub variable: &'static str,
    /// What is wrong with it.
    pub reason: &'static str,
}

fn bad(variable: &'static str, reason: &'static str) -> SettingsError {
    SettingsError { variable, reason }
}

/// The default time an export may take.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(10_000);
/// The longest time an export may be given.
pub const MAX_TIMEOUT: Duration = Duration::from_millis(60_000);

/// Every variable this reader looks at.
pub const VARIABLES: [&str; 14] = [
    "OTEL_SDK_DISABLED",
    "OTEL_EXPORTER_OTLP_ENDPOINT",
    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
    "OTEL_EXPORTER_OTLP_PROTOCOL",
    "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
    "OTEL_EXPORTER_OTLP_HEADERS",
    "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
    "OTEL_EXPORTER_OTLP_TIMEOUT",
    "OTEL_EXPORTER_OTLP_CERTIFICATE",
    "OTEL_SERVICE_NAME",
    "OTEL_RESOURCE_ATTRIBUTES",
    "OTEL_TRACES_SAMPLER",
    "OTEL_TRACES_SAMPLER_ARG",
    "AAP_OTEL_HEADERS_FILE",
];

/// Whether at least one tracing variable is set (not empty): the process was asked something about tracing, so `/metrics`
/// says whether it is on.
pub fn any_set(lookup: impl Fn(&str) -> Option<String>) -> bool {
    VARIABLES
        .iter()
        .any(|name| lookup(name).is_some_and(|v| !v.is_empty()))
}

/// What the variables say, when tracing is on.
#[derive(Clone, Debug, PartialEq)]
pub struct TraceSettings {
    /// The URL spans are POSTed to, as the variables name it (no userinfo, query or fragment).
    pub endpoint: Url,
    /// How long an export may take.
    pub timeout: Duration,
    /// `service.name`.
    pub service_name: String,
    /// The closed keys a deployment may add to the resource.
    pub resource: Vec<(ResourceKey, String)>,
    /// The sampler.
    pub sampler: Sampler,
    /// `AAP_OTEL_HEADERS_FILE`: a file of `k=v,...`. It is read by [`OtlpJson::new`](crate::OtlpJson::new), not here.
    pub headers_file: Option<PathBuf>,
    /// The headers a configuration file's section named, each read from the reference that holds it (ANR-962): set by
    /// [`OtlpSection::settings`], never by [`from_env`](Self::from_env).
    pub headers: Option<Headers>,
    /// `OTEL_EXPORTER_OTLP_CERTIFICATE`: a PEM bundle that is the exporter's only trust.
    pub certificate: Option<PathBuf>,
}

/// Where a value the configuration file does not hold is read from (ANR-962, AD-064): a mounted file, or a variable.
///
/// The file says `{ file: /var/run/secrets/aap/registry/token }` or `{ env: NAME }` and nothing else: one key, one text.
/// A value is never written in the file, and an error names the reference, never what it holds.
#[derive(Clone, Debug, PartialEq, Eq, schemars::JsonSchema)]
pub enum SecretRef {
    /// A file, read once at start (a mounted Secret).
    File(PathBuf),
    /// A variable of the process.
    Env(String),
}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error as _;
        // The message never holds what the value was: a string here may be the token itself.
        let map = BTreeMap::<String, String>::deserialize(deserializer).map_err(|_| {
            D::Error::invalid_type(
                serde::de::Unexpected::Other("a value"),
                &"a reference: { file: /path } or { env: NAME }",
            )
        })?;
        let mut file = None;
        let mut env = None;
        for (key, value) in map {
            match key.as_str() {
                "file" => file = Some(PathBuf::from(value)),
                "env" => env = Some(value),
                other => {
                    return Err(D::Error::custom(format!(
                        "{other:?} is not a reference: file or env"
                    )));
                }
            }
        }
        match (file, env) {
            (Some(path), None) if !path.as_os_str().is_empty() => Ok(Self::File(path)),
            (None, Some(name)) if !name.trim().is_empty() => Ok(Self::Env(name)),
            (Some(_), Some(_)) => Err(D::Error::custom(
                "a reference is a file or a variable, not both",
            )),
            _ => Err(D::Error::custom("a reference names a file or a variable")),
        }
    }
}

impl SecretRef {
    /// The value, read now: a file's text without its trailing newline (a mounted Secret ends one as often as not), or
    /// the variable.
    ///
    /// # Errors
    ///
    /// The file cannot be read, or the variable is not set. The reason never holds the path, the name or the value.
    pub fn resolve(&self, lookup: impl Fn(&str) -> Option<String>) -> Result<String, &'static str> {
        match self {
            Self::File(path) => std::fs::read_to_string(path)
                .map(|text| text.trim_end_matches(['\n', '\r']).to_owned())
                .map_err(|_| "the file cannot be read"),
            Self::Env(name) => lookup(name).ok_or("the variable is not set"),
        }
    }
}

/// The `telemetry.otlp` section of a configuration file (ANR-962, AD-064): the settings the `OTEL_*` variables carry,
/// written as YAML. Absent: tracing is off, exactly as with no variable.
///
/// The file names the headers by reference (a mounted file per header, or a variable) and the certificate by path; none
/// of them is read by [`validate`](Self::validate), which is what `--check` runs.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OtlpSection {
    /// The full URL of the collector's traces endpoint (`https://collector:4318/v1/traces`). Absent or empty: off.
    #[serde(default)]
    pub endpoint: Option<String>,
    /// The headers the collector wants, by name: a token, usually. Each is read from its reference when the exporter is
    /// built, and a header needs an `https` endpoint (a token in clear is a token given away).
    #[serde(default)]
    pub headers: BTreeMap<String, SecretRef>,
    /// A PEM bundle that is the exporter's only trust, as a path.
    #[serde(default)]
    pub certificate: Option<PathBuf>,
    /// How long an export may take, in milliseconds (the unit of `OTEL_EXPORTER_OTLP_TIMEOUT`). 10 000 by default.
    #[serde(default)]
    pub timeout: Option<u64>,
    /// `always_on` (the default), `always_off` or `traceidratio` (with `samplerRatio`).
    #[serde(default)]
    pub sampler: Option<String>,
    /// The ratio of `traceidratio`, 0 to 1. Ignored by the other samplers.
    #[serde(default)]
    pub sampler_ratio: Option<f64>,
    /// `service.name`. `aap-operator` or `aap-api` when absent.
    #[serde(default)]
    pub service_name: Option<String>,
    /// The closed keys a deployment may add to the resource.
    #[serde(default)]
    pub resource_attributes: BTreeMap<String, String>,
}

impl OtlpSection {
    /// Whether the section says anything at all.
    #[must_use]
    pub fn is_set(&self) -> bool {
        self != &Self::default()
    }

    /// Whether the section is one the exporter can work with, without reading any file it names: the endpoint, the
    /// timeout, the sampler, the service name, the resource attributes, and the rule that a header needs `https`.
    ///
    /// # Errors
    ///
    /// The first setting that is wrong, named: never a value.
    pub fn validate(&self) -> Result<(), SettingsError> {
        // The service name's default is the binary's; this only asks whether what the section says is usable.
        self.parse("aap")?;
        Ok(())
    }

    /// What the section asks for, and the headers read from their references. `Ok(None)` is off: no endpoint.
    ///
    /// `default_service` is the `service.name` when the section names none. `lookup` reads the variables a reference may
    /// name (never the process's own here: a caller says).
    ///
    /// # Errors
    ///
    /// As [`validate`](Self::validate), and a reference that cannot be read.
    pub fn settings(
        &self,
        default_service: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<TraceSettings>, SettingsError> {
        let Some(mut settings) = self.parse(default_service)? else {
            return Ok(None);
        };
        if !self.headers.is_empty() {
            let mut headers = Headers::default();
            for (name, reference) in &self.headers {
                let value = reference
                    .resolve(&lookup)
                    .map_err(|reason| bad("telemetry.otlp.headers", reason))?;
                headers
                    .insert(name, &value)
                    .map_err(|reason| bad("telemetry.otlp.headers", reason))?;
            }
            settings.headers = Some(headers);
        }
        Ok(Some(settings))
    }

    /// The settings the section describes, with no header read: what [`validate`](Self::validate) asks of.
    fn parse(&self, default_service: &str) -> Result<Option<TraceSettings>, SettingsError> {
        let endpoint = match self
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|e| !e.is_empty())
        {
            None => return Ok(None),
            Some(text) => parse_url(text, "telemetry.otlp.endpoint")?,
        };
        let timeout = match self.timeout {
            None => DEFAULT_TIMEOUT,
            Some(ms) => {
                let t = Duration::from_millis(ms);
                if t.is_zero() || t > MAX_TIMEOUT {
                    return Err(bad(
                        "telemetry.otlp.timeout",
                        "must be between 1 and 60000 milliseconds",
                    ));
                }
                t
            }
        };
        let service_name = match self.service_name.as_deref() {
            None => default_service.to_owned(),
            Some(v) => {
                if v.is_empty()
                    || v.len() > 63
                    || !v
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                {
                    return Err(bad(
                        "telemetry.otlp.serviceName",
                        "must match [a-z0-9-]{1,63}",
                    ));
                }
                v.to_owned()
            }
        };
        let resource = resource_from_pairs(
            self.resource_attributes
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str())),
            "telemetry.otlp.resourceAttributes",
        )?;
        let ratio = self.sampler_ratio.map(|r| r.to_string());
        let sampler = parse_sampler(
            self.sampler.as_deref(),
            ratio.as_deref(),
            "telemetry.otlp.sampler",
            "telemetry.otlp.samplerRatio",
        )?;
        // A token that goes out in clear is a token given away: headers need a TLS endpoint.
        if !self.headers.is_empty() && endpoint.scheme() != "https" {
            return Err(bad(
                "telemetry.otlp.headers",
                "the headers would be sent in clear: the endpoint must be https",
            ));
        }
        Ok(Some(TraceSettings {
            endpoint,
            timeout,
            service_name,
            resource,
            sampler,
            headers_file: None,
            headers: None,
            certificate: self.certificate.clone(),
        }))
    }
}

impl TraceSettings {
    /// Read the variables through `lookup`. `Ok(None)` is off: `OTEL_SDK_DISABLED=true`, or no endpoint.
    /// `default_service` is the `service.name` when `OTEL_SERVICE_NAME` is unset.
    ///
    /// # Errors
    ///
    /// A variable this exporter does not support or that does not parse: `grpc` or `http/protobuf`,
    /// `OTEL_EXPORTER_OTLP_HEADERS` (headers come from a file), a URL with userinfo, a query or a fragment, a
    /// resource attribute outside the closed list, a sampler it does not know.
    pub fn from_env(
        default_service: &str,
        lookup: impl Fn(&str) -> Option<String>,
    ) -> Result<Option<Self>, SettingsError> {
        // An empty value is an unset one: the chart renders nothing, and a shell may export an empty name.
        let get = |name: &str| lookup(name).filter(|v| !v.is_empty());

        if let Some(v) = get("OTEL_SDK_DISABLED") {
            match v.to_ascii_lowercase().as_str() {
                "true" => return Ok(None),
                "false" => {}
                _ => return Err(bad("OTEL_SDK_DISABLED", "must be true or false")),
            }
        }
        for name in [
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
        ] {
            if get(name).is_some() {
                return Err(bad(
                    name,
                    "not supported: put the headers in the file AAP_OTEL_HEADERS_FILE names",
                ));
            }
        }
        for name in [
            "OTEL_EXPORTER_OTLP_PROTOCOL",
            "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
        ] {
            if let Some(v) = get(name)
                && v != "http/json"
            {
                return Err(bad(
                    name,
                    "only http/json is supported (a collector turns it into grpc or http/protobuf)",
                ));
            }
        }

        let timeout = match get("OTEL_EXPORTER_OTLP_TIMEOUT") {
            None => DEFAULT_TIMEOUT,
            Some(v) => {
                let ms: u64 = v
                    .parse()
                    .map_err(|_| bad("OTEL_EXPORTER_OTLP_TIMEOUT", "must be milliseconds"))?;
                let t = Duration::from_millis(ms);
                if t.is_zero() || t > MAX_TIMEOUT {
                    return Err(bad(
                        "OTEL_EXPORTER_OTLP_TIMEOUT",
                        "must be between 1 and 60000 milliseconds",
                    ));
                }
                t
            }
        };

        let service_name = match get("OTEL_SERVICE_NAME") {
            None => default_service.to_owned(),
            Some(v) => {
                if v.is_empty()
                    || v.len() > 63
                    || !v
                        .chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                {
                    return Err(bad("OTEL_SERVICE_NAME", "must match [a-z0-9-]{1,63}"));
                }
                v
            }
        };

        let resource = match get("OTEL_RESOURCE_ATTRIBUTES") {
            None => Vec::new(),
            Some(v) => parse_resource(&v)?,
        };
        let sampler = parse_sampler(
            get("OTEL_TRACES_SAMPLER").as_deref(),
            get("OTEL_TRACES_SAMPLER_ARG").as_deref(),
            "OTEL_TRACES_SAMPLER",
            "OTEL_TRACES_SAMPLER_ARG",
        )?;

        // Both are checked, whichever wins.
        let full = get("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
            .map(|v| parse_url(&v, "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT"))
            .transpose()?;
        let base = get("OTEL_EXPORTER_OTLP_ENDPOINT")
            .map(|v| parse_url(&v, "OTEL_EXPORTER_OTLP_ENDPOINT"))
            .transpose()?;
        let endpoint = full.or_else(|| {
            base.map(|mut url| {
                let mut path = url.path().trim_end_matches('/').to_owned();
                path.push_str("/v1/traces");
                url.set_path(&path);
                url
            })
        });
        let headers_file = get("AAP_OTEL_HEADERS_FILE").map(PathBuf::from);
        let certificate = get("OTEL_EXPORTER_OTLP_CERTIFICATE").map(PathBuf::from);
        let Some(endpoint) = endpoint else {
            return Ok(None);
        };
        // A token that goes out in clear is a token given away: headers need a TLS endpoint.
        if headers_file.is_some() && endpoint.scheme() != "https" {
            return Err(bad(
                "AAP_OTEL_HEADERS_FILE",
                "the headers would be sent in clear: the endpoint must be https",
            ));
        }
        Ok(Some(Self {
            endpoint,
            timeout,
            service_name,
            resource,
            sampler,
            headers_file,
            headers: None,
            certificate,
        }))
    }
}

fn parse_url(text: &str, variable: &'static str) -> Result<Url, SettingsError> {
    let url = Url::parse(text).map_err(|_| bad(variable, "is not a URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(bad(variable, "must be an http or https URL with a host"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(bad(variable, "must not hold a user name or a password"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(bad(variable, "must not hold a query or a fragment"));
    }
    Ok(url)
}

fn parse_resource(text: &str) -> Result<Vec<(ResourceKey, String)>, SettingsError> {
    let var = "OTEL_RESOURCE_ATTRIBUTES";
    let pairs: Vec<(&str, &str)> = text
        .split(',')
        .filter(|p| !p.trim().is_empty())
        .map(|pair| pair.split_once('=').ok_or(bad(var, "a pair has no `=`")))
        .collect::<Result<_, _>>()?;
    resource_from_pairs(pairs.into_iter(), var)
}

/// Resource attributes from name/value pairs, the rule of `OTEL_RESOURCE_ATTRIBUTES` and of the file's
/// `telemetry.otlp.resourceAttributes`: a closed list of keys, a value of `[A-Za-z0-9._-]{1,128}`, each key once.
fn resource_from_pairs<'a>(
    pairs: impl Iterator<Item = (&'a str, &'a str)>,
    var: &'static str,
) -> Result<Vec<(ResourceKey, String)>, SettingsError> {
    let mut out: Vec<(ResourceKey, String)> = Vec::new();
    for (k, v) in pairs {
        let key = ResourceKey::from_name(k.trim()).ok_or(bad(
            var,
            "only deployment.environment.name, k8s.cluster.name and service.namespace are allowed",
        ))?;
        let v = v.trim();
        if v.is_empty()
            || v.len() > 128
            || !v
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(bad(var, "a value must match [A-Za-z0-9._-]{1,128}"));
        }
        if out.iter().any(|(k, _)| *k == key) {
            return Err(bad(var, "a key is given twice"));
        }
        out.push((key, v.to_owned()));
    }
    Ok(out)
}

fn parse_sampler(
    name: Option<&str>,
    arg: Option<&str>,
    var: &'static str,
    var_arg: &'static str,
) -> Result<Sampler, SettingsError> {
    let Some(name) = name else {
        return Ok(Sampler::AlwaysOn);
    };
    // No parent is ever accepted (the API reads no traceparent), so a parent-based sampler is its root sampler.
    let base = name.to_ascii_lowercase();
    let base = base.strip_prefix("parentbased_").unwrap_or(&base);
    match base {
        "always_on" => Ok(Sampler::AlwaysOn),
        "always_off" => Ok(Sampler::AlwaysOff),
        "traceidratio" => {
            let ratio = match arg {
                None => 1.0,
                Some(a) => a
                    .parse::<f64>()
                    .map_err(|_| bad(var_arg, "must be a number"))?,
            };
            if !(0.0..=1.0).contains(&ratio) {
                return Err(bad(var_arg, "must be between 0 and 1"));
            }
            Ok(Sampler::TraceIdRatio(ratio))
        }
        _ => Err(bad(
            var,
            "must be always_on, always_off or traceidratio (or their parentbased_ forms)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    fn read(pairs: &[(&str, &str)]) -> Result<Option<TraceSettings>, SettingsError> {
        TraceSettings::from_env("aap-api", env(pairs))
    }

    const EP: (&str, &str) = (
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "http://c:4318/v1/traces",
    );

    #[test]
    fn off_without_an_endpoint_or_when_disabled() {
        assert_eq!(read(&[]).unwrap(), None);
        assert_eq!(read(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "")]).unwrap(), None);
        assert_eq!(read(&[EP, ("OTEL_SDK_DISABLED", "TRUE")]).unwrap(), None);
        assert!(read(&[EP, ("OTEL_SDK_DISABLED", "maybe")]).is_err());
    }

    #[test]
    fn the_traces_endpoint_is_used_as_written_and_the_base_gets_a_path() {
        let s = read(&[EP]).unwrap().unwrap();
        assert_eq!(s.endpoint.as_str(), "http://c:4318/v1/traces");
        assert_eq!(s.service_name, "aap-api");
        assert_eq!(s.timeout, DEFAULT_TIMEOUT);
        assert_eq!(s.sampler, Sampler::AlwaysOn);
        // The traces variable is not extended, whatever its path.
        let s = read(&[("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://c:4318/custom")])
            .unwrap()
            .unwrap();
        assert_eq!(s.endpoint.path(), "/custom");
        for (base, want) in [
            ("http://c:4318", "/v1/traces"),
            ("http://c:4318/", "/v1/traces"),
            ("https://c/otel/", "/otel/v1/traces"),
        ] {
            let s = read(&[("OTEL_EXPORTER_OTLP_ENDPOINT", base)])
                .unwrap()
                .unwrap();
            assert_eq!(s.endpoint.path(), want, "{base}");
        }
        // The traces variable wins.
        let s = read(&[EP, ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://other:1")])
            .unwrap()
            .unwrap();
        assert_eq!(s.endpoint.host_str(), Some("c"));
    }

    #[test]
    fn refusals_name_the_variable_and_never_the_value() {
        let secret = "u:hunter2pass@c";
        let cases: Vec<Vec<(&str, String)>> = vec![
            vec![(
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                format!("http://{secret}:4318"),
            )],
            // A credential as the user name alone.
            vec![(
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "http://hunter2@c:4318".into(),
            )],
            vec![(
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "http://c:4318?token=hunter2pass".into(),
            )],
            vec![(
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "http://c:4318#hunter2pass".into(),
            )],
            vec![("OTEL_EXPORTER_OTLP_ENDPOINT", "ftp://c".into())],
            vec![(
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "not a url hunter2pass".into(),
            )],
            vec![(
                "OTEL_EXPORTER_OTLP_HEADERS",
                "authorization=Bearer hunter2pass".into(),
            )],
            vec![("OTEL_EXPORTER_OTLP_TRACES_HEADERS", "a=hunter2pass".into())],
            vec![("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc".into())],
            vec![("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf".into())],
            vec![("OTEL_EXPORTER_OTLP_TIMEOUT", "0".into())],
            vec![("OTEL_EXPORTER_OTLP_TIMEOUT", "60001".into())],
            vec![("OTEL_EXPORTER_OTLP_TIMEOUT", "5s".into())],
            vec![("OTEL_SERVICE_NAME", "Has Space".into())],
            vec![("OTEL_RESOURCE_ATTRIBUTES", "user.email=a-b".into())],
            vec![("OTEL_RESOURCE_ATTRIBUTES", "k8s.cluster.name=a/b".into())],
            vec![(
                "OTEL_RESOURCE_ATTRIBUTES",
                "k8s.cluster.name=a,k8s.cluster.name=b".into(),
            )],
            vec![("OTEL_TRACES_SAMPLER", "jaeger_remote".into())],
            vec![
                ("OTEL_TRACES_SAMPLER", "traceidratio".into()),
                ("OTEL_TRACES_SAMPLER_ARG", "1.5".into()),
            ],
            vec![
                ("OTEL_TRACES_SAMPLER", "traceidratio".into()),
                ("OTEL_TRACES_SAMPLER_ARG", "x".into()),
            ],
        ];
        for case in cases {
            let mut pairs: Vec<(&str, &str)> = vec![EP];
            pairs.extend(case.iter().map(|(k, v)| (*k, v.as_str())));
            let e = read(&pairs).expect_err(&format!("{case:?}"));
            let shown = e.to_string();
            assert!(shown.starts_with(e.variable), "{shown}");
            assert!(
                !shown.contains("hunter2") && !shown.contains("c:4318"),
                "{shown}"
            );
        }
    }

    #[test]
    fn accepted_values() {
        let s = read(&[
            // https, because a headers file is set (headers are never sent in clear).
            (
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "https://c:4318/v1/traces",
            ),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json"),
            ("OTEL_EXPORTER_OTLP_TIMEOUT", "250"),
            ("OTEL_SERVICE_NAME", "my-api"),
            (
                "OTEL_RESOURCE_ATTRIBUTES",
                "deployment.environment.name=prod, k8s.cluster.name=c1.eu",
            ),
            ("OTEL_TRACES_SAMPLER", "parentbased_traceidratio"),
            ("OTEL_TRACES_SAMPLER_ARG", "0.25"),
            ("AAP_OTEL_HEADERS_FILE", "/etc/otel/headers"),
            ("OTEL_EXPORTER_OTLP_CERTIFICATE", "/etc/otel/ca.pem"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(s.timeout, Duration::from_millis(250));
        assert_eq!(s.service_name, "my-api");
        assert_eq!(s.resource.len(), 2);
        assert_eq!(s.sampler, Sampler::TraceIdRatio(0.25));
        assert_eq!(s.headers_file, Some(PathBuf::from("/etc/otel/headers")));
        assert_eq!(s.certificate, Some(PathBuf::from("/etc/otel/ca.pem")));
    }

    #[test]
    fn headers_over_plain_http_are_refused_by_name_and_https_or_no_headers_pass() {
        let e = read(&[EP, ("AAP_OTEL_HEADERS_FILE", "/etc/otel/headers")]).unwrap_err();
        assert_eq!(e.variable, "AAP_OTEL_HEADERS_FILE");
        assert!(e.reason.contains("clear"), "{e}");
        assert!(!e.to_string().contains("c:4318"));
        assert!(
            read(&[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "https://c:4318"),
                ("AAP_OTEL_HEADERS_FILE", "/etc/otel/headers"),
            ])
            .unwrap()
            .is_some()
        );
        assert!(read(&[EP]).unwrap().is_some());
        // No endpoint: tracing is off and there is nothing to send in clear.
        assert!(
            read(&[("AAP_OTEL_HEADERS_FILE", "/etc/otel/headers")])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn every_sampler_word_reads_as_itself() {
        let sampler = |name: &str, arg: Option<&str>| {
            let mut pairs = vec![EP, ("OTEL_TRACES_SAMPLER", name)];
            if let Some(a) = arg {
                pairs.push(("OTEL_TRACES_SAMPLER_ARG", a));
            }
            read(&pairs).unwrap().unwrap().sampler
        };
        assert_eq!(sampler("always_on", None), Sampler::AlwaysOn);
        assert_eq!(sampler("always_off", None), Sampler::AlwaysOff);
        assert_eq!(sampler("parentbased_always_off", None), Sampler::AlwaysOff);
        assert_eq!(sampler("ALWAYS_OFF", None), Sampler::AlwaysOff);
        assert_eq!(
            sampler("traceidratio", Some("0.5")),
            Sampler::TraceIdRatio(0.5)
        );
        // No argument is the whole of the traces.
        assert_eq!(sampler("traceidratio", None), Sampler::TraceIdRatio(1.0));
    }

    #[test]
    fn a_service_name_is_at_most_63_characters() {
        let name = |n: usize| "a".repeat(n);
        let with = |n: String| read(&[EP, ("OTEL_SERVICE_NAME", n.as_str())]);
        assert_eq!(with(name(63)).unwrap().unwrap().service_name, name(63));
        assert!(with(name(64)).is_err());
    }

    #[test]
    fn any_set_sees_the_variables_this_reader_knows_and_ignores_empty_ones() {
        assert!(!any_set(env(&[])));
        assert!(!any_set(env(&[("OTEL_TRACES_SAMPLER", "")])));
        assert!(!any_set(env(&[("OTEL_TRACES_EXPORTER", "otlp")])));
        // The list is written out here too: a variable dropped from `VARIABLES` is no longer "asked".
        let by_hand = [
            "OTEL_SDK_DISABLED",
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "OTEL_EXPORTER_OTLP_PROTOCOL",
            "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
            "OTEL_EXPORTER_OTLP_HEADERS",
            "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
            "OTEL_EXPORTER_OTLP_TIMEOUT",
            "OTEL_EXPORTER_OTLP_CERTIFICATE",
            "OTEL_SERVICE_NAME",
            "OTEL_RESOURCE_ATTRIBUTES",
            "OTEL_TRACES_SAMPLER",
            "OTEL_TRACES_SAMPLER_ARG",
            "AAP_OTEL_HEADERS_FILE",
        ];
        assert_eq!(VARIABLES, by_hand);
        for name in by_hand {
            assert!(any_set(env(&[(name, "x")])), "{name}");
        }
    }
}
