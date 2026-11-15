//! The tracing variables, asked of the built binary (K5): a variable this build does not support stops `operator run` at
//! once, naming the variable, before anything is asked of a cluster.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

fn run(env: &[(&str, &str)]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_operator"))
        .arg("run")
        // No cluster, however the machine is set up: if the variable were not checked first, this is what would fail.
        .env("KUBECONFIG", "/nonexistent/kubeconfig")
        .env_remove("KUBERNETES_SERVICE_HOST")
        .env_remove("OTEL_SDK_DISABLED")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn a_protocol_this_exporter_does_not_speak_stops_the_start_by_name() {
    for protocol in ["grpc", "http/protobuf"] {
        let (code, stderr) = run(&[
            (
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                "http://collector.invalid:4318",
            ),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", protocol),
        ]);
        assert_eq!(code, Some(1), "{stderr}");
        assert!(stderr.contains("OTEL_EXPORTER_OTLP_PROTOCOL"), "{stderr}");
        assert!(!stderr.contains("collector.invalid"), "{stderr}");
        assert!(
            !stderr.to_lowercase().contains("cluster"),
            "the variable is checked before the cluster is asked anything: {stderr}"
        );
    }
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
    assert!(stderr.contains("OTEL_EXPORTER_OTLP_HEADERS"), "{stderr}");
    assert!(stderr.contains("AAP_OTEL_HEADERS_FILE"), "{stderr}");
    assert!(!stderr.contains("HUNTER2"), "{stderr}");
}

#[test]
fn off_is_the_default_and_the_first_failure_is_the_cluster() {
    let (code, stderr) = run(&[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(!stderr.contains("OTEL_"), "{stderr}");
}
