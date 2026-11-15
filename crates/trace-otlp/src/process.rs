//! What both binaries do at start: read the variables, make the exporter, start the pipeline.

use aap_trace::{K8sName, Limits, Pipeline, Resource, Tracer};

use crate::exporter::OtlpJson;
use crate::settings::{SettingsError, TraceSettings};

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
    let Some(settings) = TraceSettings::from_env(default_service, &lookup)? else {
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
