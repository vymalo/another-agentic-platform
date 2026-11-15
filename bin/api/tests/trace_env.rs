//! The tracing variables, asked of the built binary (K5): `api --check` validates them with the configuration, and reads none
//! of the files they name.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;

fn check(env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    let config = concat!(env!("CARGO_MANIFEST_DIR"), "/config.example.yaml");
    let out = Command::new(env!("CARGO_BIN_EXE_api"))
        .args(["--config", config, "--check"])
        .env_remove("OTEL_SDK_DISABLED")
        .envs(env.iter().copied())
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn check_is_ok_with_no_tracing_and_with_usable_variables() {
    assert_eq!(check(&[]).0, Some(0));
    let (code, stdout, stderr) = check(&[
        (
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "http://otel.invalid:4318/v1/traces",
        ),
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json"),
        ("OTEL_TRACES_SAMPLER", "traceidratio"),
        ("OTEL_TRACES_SAMPLER_ARG", "0.5"),
        // The file is not read by `--check`: this one does not exist.
        ("AAP_OTEL_HEADERS_FILE", "/nonexistent/headers"),
        ("OTEL_EXPORTER_OTLP_CERTIFICATE", "/nonexistent/ca.pem"),
    ]);
    assert_eq!(code, Some(0), "{stdout} {stderr}");
}

#[test]
fn check_refuses_a_protocol_it_does_not_speak_naming_the_variable() {
    for protocol in ["grpc", "http/protobuf"] {
        let (code, _, stderr) = check(&[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://otel.invalid:4318"),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", protocol),
        ]);
        assert_eq!(code, Some(1), "{stderr}");
        assert!(stderr.contains("OTEL_EXPORTER_OTLP_PROTOCOL"), "{stderr}");
        assert!(!stderr.contains("otel.invalid"), "{stderr}");
    }
}

#[test]
fn check_refuses_a_url_with_a_password_without_printing_it() {
    let (code, _, stderr) = check(&[(
        "OTEL_EXPORTER_OTLP_ENDPOINT",
        "http://user:HUNTER2-PASSWORD@otel.invalid:4318",
    )]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("OTEL_EXPORTER_OTLP_ENDPOINT"), "{stderr}");
    assert!(
        !stderr.contains("HUNTER2") && !stderr.contains("otel.invalid"),
        "{stderr}"
    );
}
