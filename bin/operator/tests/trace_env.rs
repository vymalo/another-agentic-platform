//! The traces of the operator, asked of the built binary (K5, ANR-962): the settings are the file's `telemetry.otlp`
//! section, and the `OTEL_*` variables are not read any more. A section the exporter cannot use is refused with the file
//! (`EX_CONFIG`); a **file** the section names (a headers file, a certificate) that cannot be read never stops `operator
//! run`: it warns once, naming the setting and never the value, runs with tracing off, and goes on to the cluster.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::process::Command;

/// A configuration file with this text, in a directory of this test's own.
fn file(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aap-trace-env-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a directory");
    let path = dir.join(name);
    std::fs::write(&path, text).expect("writes");
    path
}

/// `run` on a file, with no cluster: it goes on to the cluster and fails there (exit 1) unless the file stopped it.
fn run(name: &str, text: &str, env: &[(&str, &str)]) -> (Option<i32>, String) {
    let path = file(name, text);
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_operator"));
    cmd.arg("run")
        .arg("--config")
        .arg(&path)
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
fn a_section_the_exporter_does_not_speak_is_refused_with_the_file() {
    // The variables of a platform (BuildKit puts these in every RUN step) say nothing to this binary any more.
    let (code, stderr) = run(
        "ignored.yaml",
        "version: 1\n",
        &[
            ("OTEL_TRACES_EXPORTER", "otlp"),
            ("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "grpc"),
            (
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "unix:///dev/otel-grpc.sock",
            ),
        ],
    );
    assert_eq!(code, Some(1), "{stderr}");
    assert!(warnings(&stderr).is_empty(), "{stderr}");
    assert!(stderr.contains("connecting to the cluster"), "{stderr}");

    // The same setting in the file is refused, by name, before the cluster.
    for (body, named) in [
        (
            "endpoint: 'unix:///dev/otel-grpc.sock'",
            "telemetry.otlp.endpoint",
        ),
        (
            "endpoint: 'http://c:4318/v1/traces'\n    sampler: grpc",
            "telemetry.otlp.sampler",
        ),
        (
            "endpoint: 'http://u:p@c:4318/v1/traces'",
            "telemetry.otlp.endpoint",
        ),
    ] {
        let (code, stderr) = run(
            "refused.yaml",
            &format!("version: 1\ntelemetry:\n  otlp:\n    {body}\n"),
            &[],
        );
        assert_eq!(code, Some(78), "{body}: {stderr}");
        assert!(stderr.contains(named), "{body}: {stderr}");
        assert!(!stderr.contains("connecting to the cluster"), "{stderr}");
        assert!(!stderr.contains("otel-grpc.sock"), "{stderr}");
    }
}

#[test]
fn a_headers_file_that_cannot_be_read_warns_once_by_name_and_the_start_goes_on() {
    let (code, stderr) = run(
        "headers.yaml",
        "version: 1\ntelemetry:\n  otlp:\n    endpoint: https://collector.invalid:4318/v1/traces\n    headers:\n      authorization:\n        file: /nonexistent/headers\n",
        &[],
    );
    // The cluster is what fails, after the header was warned about.
    assert_eq!(code, Some(1), "{stderr}");
    let warned = warnings(&stderr);
    assert_eq!(warned.len(), 1, "{stderr}");
    assert!(warned[0].contains("telemetry.otlp.headers"), "{stderr}");
    assert!(!stderr.contains("collector.invalid"), "{stderr}");
    assert!(
        stderr.to_lowercase().contains("cluster"),
        "the start went on to the cluster: {stderr}"
    );
}

#[test]
fn a_certificate_that_cannot_be_read_warns_once_by_name_and_the_start_goes_on() {
    let (code, stderr) = run(
        "certificate.yaml",
        "version: 1\ntelemetry:\n  otlp:\n    endpoint: https://collector.invalid:4318/v1/traces\n    certificate: /nonexistent/ca.pem\n",
        &[],
    );
    assert_eq!(code, Some(1), "{stderr}");
    let warned = warnings(&stderr);
    assert_eq!(warned.len(), 1, "{stderr}");
    assert!(!stderr.contains("collector.invalid"), "{stderr}");
    assert!(
        stderr.to_lowercase().contains("cluster"),
        "the start went on to the cluster: {stderr}"
    );
}
