//! The settings, from the standard `OTEL_*` variables (a strict subset) and one of ours.
//!
//! One pure function reads them, so both binaries share the rules and `--check` can run them. An error names the
//! variable and a fixed reason, never a value: a value may be a URL with a password.

use std::path::PathBuf;
use std::time::Duration;

use aap_trace::{ResourceKey, Sampler};
use url::Url;

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
    /// `OTEL_EXPORTER_OTLP_CERTIFICATE`: a PEM bundle that is the exporter's only trust.
    pub certificate: Option<PathBuf>,
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
        Ok(Some(Self {
            endpoint,
            timeout,
            service_name,
            resource,
            sampler,
            headers_file,
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
    let mut out: Vec<(ResourceKey, String)> = Vec::new();
    for pair in text.split(',').filter(|p| !p.trim().is_empty()) {
        let (k, v) = pair.split_once('=').ok_or(bad(var, "a pair has no `=`"))?;
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

fn parse_sampler(name: Option<&str>, arg: Option<&str>) -> Result<Sampler, SettingsError> {
    let var = "OTEL_TRACES_SAMPLER";
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
                    .map_err(|_| bad("OTEL_TRACES_SAMPLER_ARG", "must be a number"))?,
            };
            if !(0.0..=1.0).contains(&ratio) {
                return Err(bad("OTEL_TRACES_SAMPLER_ARG", "must be between 0 and 1"));
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
            EP,
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
}
