//! The tracing variables, asked of the built binary (K5): `api --check` validates them with the configuration and reads none of
//! the files they name, and a running API **never stops for a tracing setting**: it warns once, by variable, and runs with
//! tracing off (platforms inject OTEL_* variables of their own; BuildKit does, into every RUN step).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// What BuildKit puts in the environment of a RUN step (seen 2026-11-15 in the log of pull request 52, run 43729018865).
const BUILDKIT: [(&str, &str); 3] = [
    ("OTEL_TRACES_EXPORTER", "otlp"),
    ("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL", "grpc"),
    (
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
        "unix:///dev/otel-grpc.sock",
    ),
];

/// A command for the API with none of the developer's own `OTEL_*` or `AAP_OTEL_*` variables, so a shell cannot fail these tests.
fn api() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_api"));
    // A proxy of the developer's or the runner's would make the kube client refuse to build (and is no business of these tests).
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        cmd.env_remove(name);
    }
    for (name, _) in std::env::vars() {
        if name.starts_with("OTEL_") || name.starts_with("AAP_OTEL_") {
            cmd.env_remove(name);
        }
    }
    cmd
}

fn config() -> String {
    format!("{}/config.example.yaml", env!("CARGO_MANIFEST_DIR"))
}

fn check(env: &[(&str, &str)]) -> (Option<i32>, String, String) {
    let out = api()
        .args(["--config", &config(), "--check"])
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
            "https://otel.invalid:4318/v1/traces",
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

/// `--check` is the place a rendered configuration is validated, so BuildKit's shape is still an error there.
#[test]
fn check_refuses_buildkits_variables_naming_the_variable_and_not_the_value() {
    let (code, _, stderr) = check(&BUILDKIT);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(
        stderr.contains("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL"),
        "{stderr}"
    );
    assert!(!stderr.contains("otel-grpc.sock"), "{stderr}");
}

#[test]
fn check_refuses_a_headers_file_with_a_plain_http_endpoint() {
    let (code, _, stderr) = check(&[
        ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://otel.invalid:4318"),
        ("AAP_OTEL_HEADERS_FILE", "/nonexistent/headers"),
    ]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("AAP_OTEL_HEADERS_FILE"), "{stderr}");
    assert!(!stderr.contains("otel.invalid"), "{stderr}");
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

fn free_addr() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

/// A kubeconfig that names a server nobody answers: the client is built (the API connects lazily), nothing is reachable.
fn kubeconfig(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aap-api-trace-env-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("kubeconfig");
    std::fs::write(
        &path,
        "apiVersion: v1\nkind: Config\nclusters:\n- name: c\n  cluster:\n    server: https://127.0.0.1:1\nusers:\n- name: u\n  user:\n    token: t\ncontexts:\n- name: x\n  context:\n    cluster: c\n    user: u\ncurrent-context: x\n",
    )
    .unwrap();
    path
}

fn http_get(addr: SocketAddr, path: &str) -> Option<String> {
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(200)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n"
    )
    .ok()?;
    let mut text = String::new();
    stream.read_to_string(&mut text).ok()?;
    Some(text)
}

struct Killed(Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `run` with BuildKit's variables: the API starts, serves `/metrics` with `aap_trace_enabled 0`, and has said once, on one line,
/// which variable was refused and not what it held.
#[test]
fn run_with_buildkits_variables_starts_warns_once_and_serves() {
    let (listen, health, metrics) = (free_addr(), free_addr(), free_addr());
    let kube = kubeconfig("run");
    let mut child = Killed(
        api()
            .args(["--config", &config()])
            .args(["--listen", &listen.to_string()])
            .args(["--health-addr", &health.to_string()])
            .args(["--metrics-addr", &metrics.to_string()])
            .env("KUBECONFIG", &kube)
            .env_remove("KUBERNETES_SERVICE_HOST")
            .envs(BUILDKIT)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let text = loop {
        if let Some(t) = http_get(metrics, "/metrics")
            && t.starts_with("HTTP/1.1 200")
        {
            break t;
        }
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "the API stopped for a tracing setting"
        );
        assert!(Instant::now() < deadline, "the API did not come up");
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(text.contains("aap_trace_enabled 0\n"), "{text}");
    assert!(!text.contains("aap_trace_spans"), "{text}");
    let _ = child.0.kill();
    let mut stderr = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    let warnings: Vec<&str> = stderr
        .lines()
        .filter(|l| l.contains("tracing is off"))
        .collect();
    assert_eq!(warnings.len(), 1, "{stderr}");
    assert!(warnings[0].contains("WARN"), "{stderr}");
    assert!(
        warnings[0].contains("OTEL_EXPORTER_OTLP_TRACES_PROTOCOL"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("otel-grpc.sock"),
        "the value is never printed: {stderr}"
    );
}

/// With no tracing variable at all the metrics text has no trace line: it is what K4 prints.
#[test]
fn run_with_no_tracing_variable_has_no_trace_text_on_metrics() {
    let (listen, health, metrics) = (free_addr(), free_addr(), free_addr());
    let kube = kubeconfig("plain");
    let child = Killed(
        api()
            .args(["--config", &config()])
            .args(["--listen", &listen.to_string()])
            .args(["--health-addr", &health.to_string()])
            .args(["--metrics-addr", &metrics.to_string()])
            .env("KUBECONFIG", &kube)
            .env_remove("KUBERNETES_SERVICE_HOST")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    let text = loop {
        if let Some(t) = http_get(metrics, "/metrics")
            && t.starts_with("HTTP/1.1 200")
        {
            break t;
        }
        assert!(Instant::now() < deadline, "the API did not come up");
        std::thread::sleep(Duration::from_millis(50));
    };
    drop(child);
    assert!(!text.contains("aap_trace_"), "{text}");
}
