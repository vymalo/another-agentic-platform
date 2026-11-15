//! The tracing variables, asked of the built binary (K5): a setting this build does not support **never stops `operator run`**
//! (tracing is not a security control, and platforms inject OTEL_* variables of their own: BuildKit does, into every RUN step).
//! It warns once, naming the variable and never the value, runs with tracing off, and goes on to the cluster.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

/// What BuildKit puts in the environment of a RUN step (seen 2026-11-15 in the log of pull request 52, run 43729018865).
const BUILDKIT: [(&str, &str); 3] = [
    ("OTEL_TRACES_EXPORTER", "otlp"),
    ("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "grpc"),
    (
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "unix:///dev/otel-grpc.sock",
    ),
];

fn run(env: &[(&str, &str)]) -> (Option<i32>, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_operator"));
    cmd.arg("run")
        // No cluster, however the machine is set up: the run goes on to the cluster and fails there.
        .env("KUBECONFIG", "/nonexistent/kubeconfig")
        .env_remove("KUBERNETES_SERVICE_HOST");
    // None of the developer's own tracing variables: a shell cannot fail these tests.
    for (name, _) in std::env::vars() {
        if name.starts_with("OTEL_") || name.starts_with("AAP_OTEL_") {
            cmd.env_remove(name);
        }
    }
    let out = cmd.envs(env.iter().copied()).output().unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn warnings(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.contains("tracing is off"))
        .collect()
}

#[test]
fn a_protocol_this_exporter_does_not_speak_warns_once_by_name_and_the_start_goes_on() {
    for protocol in ["grpc", "http/protobuf"] {
        let (code, stderr) = run(&[
            (
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "http://collector.invalid:4318",
            ),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", protocol),
        ]);
        // The cluster is what fails, after the tracing setting was warned about.
        assert_eq!(code, Some(1), "{stderr}");
        let warned = warnings(&stderr);
        assert_eq!(warned.len(), 1, "{stderr}");
        assert!(
            warned[0].contains("OTEL_EXPORTER_OTLP_PROTOCOL"),
            "{stderr}"
        );
        assert!(!stderr.contains("collector.invalid"), "{stderr}");
        assert!(
            stderr.to_lowercase().contains("cluster"),
            "the start went on to the cluster: {stderr}"
        );
    }
}

#[test]
fn buildkits_variables_do_not_stop_the_operator() {
    let (code, stderr) = run(&BUILDKIT);
    assert_eq!(code, Some(1), "{stderr}");
    let warned = warnings(&stderr);
    assert_eq!(warned.len(), 1, "{stderr}");
    assert!(
        warned[0].contains("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL"),
        "{stderr}"
    );
    assert!(!stderr.contains("otel-grpc.sock"), "{stderr}");
    assert!(
        stderr.to_lowercase().contains("cluster"),
        "the start went on to the cluster: {stderr}"
    );
}

#[test]
fn headers_are_refused_as_a_variable_and_the_value_is_never_printed() {
    let (code, stderr) = run(&[
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "http://collector.invalid:4318",
        ),
        (
            "OTEL_EXPORTER_OTLP_HEADERS",
            "authorization=Bearer HUNTER2-TOKEN",
        ),
    ]);
    assert_eq!(code, Some(1), "{stderr}");
    let warned = warnings(&stderr);
    assert_eq!(warned.len(), 1, "{stderr}");
    assert!(warned[0].contains("OTEL_EXPORTER_OTLP_HEADERS"), "{stderr}");
    assert!(!stderr.contains("HUNTER2"), "{stderr}");
}

#[test]
fn a_headers_file_with_a_plain_http_endpoint_is_refused_and_the_start_goes_on() {
    let (code, stderr) = run(&[
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "http://collector.invalid:4318",
        ),
        ("AAP_OTEL_HEADERS_FILE", "/nonexistent/headers"),
    ]);
    assert_eq!(code, Some(1), "{stderr}");
    let warned = warnings(&stderr);
    assert_eq!(warned.len(), 1, "{stderr}");
    assert!(warned[0].contains("AAP_OTEL_HEADERS_FILE"), "{stderr}");
    assert!(!stderr.contains("collector.invalid"), "{stderr}");
}

#[test]
fn off_is_the_default_and_the_first_failure_is_the_cluster() {
    let (code, stderr) = run(&[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(!stderr.contains("OTEL_"), "{stderr}");
    assert!(warnings(&stderr).is_empty(), "{stderr}");
}
