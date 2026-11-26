//! What both binaries do at start: read the variables, make the exporter, start the pipeline.

use aap_trace::{K8sName, Limits, Pipeline, Resource, Tracer};

use crate::exporter::OtlpJson;
use crate::settings::{OtlpSection, SettingsError, TraceSettings, any_set};

/// The variables, from the process's environment (`std::env::var`: a value that is not text is unset).
fn from_process(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Whether the variables of this process are usable: for `--check`, which needs no collector and reads **none of the files**
/// the variables name.
///
/// # Errors
///
/// As [`TraceSettings::from_env`].
pub fn check(default_service: &str) -> Result<(), SettingsError> {
    TraceSettings::from_env(default_service, from_process).map(|_| ())
}

/// What starting tracing came to. Tracing is not a security control, so **a setting that is refused never stops the process**:
/// the tracer is off and the reason is here for the caller to log once ([`Started::warn`]).
#[derive(Debug)]
pub struct Started {
    /// The tracer to use: off when nothing was asked, or when what was asked was refused (then it is
    /// [`asked`](Tracer::asked), which is what puts `aap_trace_enabled 0` on `/metrics`).
    pub tracer: Tracer,
    /// The refused setting: a variable name and a fixed reason, never a value.
    pub refused: Option<SettingsError>,
}

impl Started {
    /// Log the refusal, once, as one `warn` line naming the variable and the fixed reason. Nothing else is logged.
    pub fn warn(&self) {
        if let Some(e) = &self.refused {
            tracing::warn!(
                variable = e.variable,
                reason = e.reason,
                "tracing is off: a tracing setting was refused"
            );
        }
    }
}

/// [`start`] over a configuration file's `telemetry.otlp` section (ANR-962, AD-064): the headers and the certificate the
/// section names are read here, and a reference that cannot be read is a refusal like a bad variable (tracing off, one
/// warning, the operator runs). `POD_NAME` and `POD_NAMESPACE` still come from the environment (the downward API).
#[must_use]
pub fn start_section(
    default_service: &str,
    version: &str,
    section: Option<&OtlpSection>,
) -> Started {
    let configured = section.is_some_and(OtlpSection::is_set);
    let off = || {
        if configured {
            Tracer::off_but_asked()
        } else {
            Tracer::off()
        }
    };
    let built = section.map_or(Ok(None), |s| s.settings(default_service, from_process));
    match built.and_then(|settings| tracer_from(version, settings, from_process)) {
        Ok(tracer) if tracer.enabled() => Started {
            tracer,
            refused: None,
        },
        Ok(_) => Started {
            tracer: off(),
            refused: None,
        },
        Err(e) => Started {
            tracer: off(),
            refused: Some(e),
        },
    }
}

/// [`tracer`] for a process that must run whatever the settings say: an error becomes [`Started::refused`] and a tracer that
/// is off. Call it inside a Tokio runtime, after the log is set up, then [`Started::warn`].
#[must_use]
pub fn start(default_service: &str, version: &str) -> Started {
    start_with(default_service, version, from_process)
}

/// [`start`] over another source of variables.
#[must_use]
pub fn start_with(
    default_service: &str,
    version: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Started {
    let configured = any_set(&lookup);
    let off = || {
        if configured {
            Tracer::off_but_asked()
        } else {
            Tracer::off()
        }
    };
    match tracer_with(default_service, version, &lookup) {
        Ok(tracer) if tracer.enabled() => Started {
            tracer,
            refused: None,
        },
        Ok(_) => Started {
            tracer: off(),
            refused: None,
        },
        Err(e) => Started {
            tracer: off(),
            refused: Some(e),
        },
    }
}

/// The tracer these variables ask for: [`Tracer::off`] when they ask for none. Call it inside a Tokio runtime (the pipeline is
/// a task), **before** connecting to anything, so a bad variable stops the process at start and says which.
///
/// `POD_NAME` and `POD_NAMESPACE` (the downward API, which the chart sets) become `k8s.pod.name`, `service.instance.id` and
/// `k8s.namespace.name`; a value that is not a Kubernetes name is left out.
///
/// # Errors
///
/// A variable that is refused, or a headers or certificate file that is not usable.
pub fn tracer(default_service: &str, version: &str) -> Result<Tracer, SettingsError> {
    tracer_with(default_service, version, from_process)
}

/// [`tracer`] over another source of variables.
///
/// # Errors
///
/// As [`tracer`].
pub fn tracer_with(
    default_service: &str,
    version: &str,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Tracer, SettingsError> {
    let settings = TraceSettings::from_env(default_service, &lookup)?;
    tracer_from(version, settings, lookup)
}

/// The tracer these settings ask for: [`Tracer::off`] when they ask for none. `lookup` reads `POD_NAME` and
/// `POD_NAMESPACE` only (the downward API, which the chart sets): they become `k8s.pod.name`, `service.instance.id` and
/// `k8s.namespace.name`, and a value that is not a Kubernetes name is left out.
///
/// # Errors
///
/// A headers or certificate file that is not usable.
pub fn tracer_from(
    version: &str,
    settings: Option<TraceSettings>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Tracer, SettingsError> {
    let Some(settings) = settings else {
        return Ok(Tracer::off());
    };
    let exporter = OtlpJson::new(&settings)?;
    let name = |var: &str| lookup(var).filter(|v| K8sName::new(v).is_some());
    let mut resource = Resource::new(settings.service_name.clone(), version);
    resource.pod_name = name("POD_NAME");
    resource.namespace = name("POD_NAMESPACE");
    resource.extra = settings.resource.clone();
    Ok(Pipeline::start(
        exporter,
        Limits::default(),
        resource,
        settings.sampler,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            pairs
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[tokio::test]
    async fn no_endpoint_is_off_and_starts_nothing() {
        let t = tracer_with("aap-api", "0", lookup(&[])).unwrap();
        assert!(!t.enabled());
        assert!(t.stats().is_none());
    }

    #[tokio::test]
    async fn start_runs_with_tracing_off_when_a_setting_is_refused_and_says_which() {
        // BuildKit's shape: a traces protocol of grpc and an endpoint on a unix socket.
        let s = start_with(
            "aap-api",
            "0",
            lookup(&[
                ("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "grpc"),
                (
                    "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                    "unix:///dev/otel-grpc.sock",
                ),
                ("OTEL_TRACES_EXPORTER", "otlp"),
            ]),
        );
        assert!(!s.tracer.enabled());
        assert!(s.tracer.asked());
        let e = s.refused.unwrap();
        assert_eq!(e.variable, "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL");
        assert!(!format!("{e}").contains("unix"));
    }

    #[tokio::test]
    async fn start_with_no_variable_is_off_and_not_configured() {
        let s = start_with("aap-api", "0", lookup(&[]));
        assert!(!s.tracer.enabled() && !s.tracer.asked() && s.refused.is_none());
        // An empty value is an unset one.
        let s = start_with("aap-api", "0", lookup(&[("OTEL_SERVICE_NAME", "")]));
        assert!(!s.tracer.asked());
        // A variable that is set and ignored (no endpoint) is still "configured but off".
        let s = start_with("aap-api", "0", lookup(&[("OTEL_SERVICE_NAME", "x")]));
        assert!(s.tracer.asked() && !s.tracer.enabled() && s.refused.is_none());
    }

    #[tokio::test]
    async fn a_refused_variable_stops_the_start_by_name() {
        let e = tracer_with(
            "aap-api",
            "0",
            lookup(&[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://c:4318"),
                ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
            ]),
        )
        .unwrap_err();
        assert_eq!(e.variable, "OTEL_EXPORTER_OTLP_PROTOCOL");
    }

    #[tokio::test]
    async fn an_endpoint_starts_a_pipeline_that_names_the_pod() {
        let t = tracer_with(
            "aap-api",
            "0.1.0",
            lookup(&[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:1"),
                ("POD_NAME", "api-0"),
                ("POD_NAMESPACE", "NOT A NAME"),
            ]),
        )
        .unwrap();
        assert!(t.enabled());
        t.shutdown(std::time::Duration::from_millis(100)).await;
    }
}
