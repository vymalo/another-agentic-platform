//! The traces, asked of the built binary (K5, ANR-970): the file's `telemetry.otlp` section, not the `OTEL_*` variables.
//! `--check` validates the section with the configuration and reads none of the files it names; a running API **never stops
//! for a tracing setting**: it warns once and runs with tracing off.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A command for the API, with a proxy of the developer's or the runner's removed (it would make the kube client refuse to
/// build, and is no business of these tests) and no `RUST_LOG`: the log's filter is the file's.
fn api() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_api"));
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
        "RUST_LOG",
    ] {
        cmd.env_remove(name);
    }
    cmd
}

const EXAMPLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/config.example.yaml");

/// The example, with each top-level key of `extra` set (replacing one the example has) and, when `ports` is given, the three
/// listeners.
fn document(extra: &str, ports: Option<(SocketAddr, SocketAddr, SocketAddr)>) -> serde_yaml::Value {
    let mut doc: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(EXAMPLE).unwrap()).unwrap();
    let map = doc.as_mapping_mut().unwrap();
    if let Some((listen, health, metrics)) = ports {
        map.insert("listen".into(), listen.to_string().into());
        map.insert("healthListen".into(), health.to_string().into());
        map.insert("metricsListen".into(), metrics.to_string().into());
    }
    if !extra.is_empty() {
        let more: serde_yaml::Value = serde_yaml::from_str(extra).unwrap();
        for (key, value) in more.as_mapping().unwrap() {
            map.insert(key.clone(), value.clone());
        }
    }
    doc
}

/// A configuration file of its own, so a test never writes over the example.
fn write(tag: &str, doc: &serde_yaml::Value) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aap-api-telemetry-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.yaml");
    std::fs::write(&path, serde_yaml::to_string(doc).unwrap()).unwrap();
    path
}

fn check(path: &std::path::Path) -> (Option<i32>, String, String) {
    let out = api()
        .args(["--config"])
        .arg(path)
        .arg("--check")
        .output()
        .unwrap();
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn check_accepts_the_example_and_a_telemetry_section() {
    let (code, stdout, stderr) = check(std::path::Path::new(EXAMPLE));
    assert_eq!(code, Some(0), "{stdout} {stderr}");
    // The headers file and the certificate are not read by `--check`: they do not exist.
    let path = write(
        "ok",
        &document(
            "telemetry:\n  otlp:\n    endpoint: https://otel.invalid:4318/v1/traces\n    sampler: traceidratio\n    samplerRatio: 0.5\n    headers:\n      authorization:\n        file: /nonexistent/headers\n    certificate: /nonexistent/ca.pem\n",
            None,
        ),
    );
    let (code, stdout, stderr) = check(&path);
    assert_eq!(code, Some(0), "{stdout} {stderr}");
}

/// ANR-970's acceptance: an unknown key is refused at the top of the section and nested, and a header that is a plain string
/// is refused without the text ever repeating it.
#[test]
fn check_refuses_what_the_telemetry_section_does_not_say() {
    for (what, key, extra) in [
        ("the section", "otpl", "telemetry:\n  otpl: {}\n"),
        (
            "nested",
            "typo",
            "telemetry:\n  otlp:\n    endpoint: https://otel.invalid:4318/v1/traces\n    typo: x\n",
        ),
    ] {
        let path = write("unknown", &document(extra, None));
        let (code, _, stderr) = check(&path);
        assert_eq!(code, Some(1), "{what}: {stderr}");
        assert!(stderr.contains(key), "{what}: {stderr}");
    }
    let path = write(
        "string",
        &document(
            "telemetry:\n  otlp:\n    endpoint: https://otel.invalid:4318/v1/traces\n    headers:\n      authorization: HUNTER2-TOKEN\n",
            None,
        ),
    );
    let (code, _, stderr) = check(&path);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("a reference"), "{stderr}");
    assert!(
        !stderr.contains("HUNTER2"),
        "the value is never printed: {stderr}"
    );
}

/// Three addresses no listener holds. All three are bound **before** any is released: two `bind(0)` calls in a row can be
/// handed the same port, and a child that cannot bind its second listener exits (ANR-1065).
fn free_addrs() -> (SocketAddr, SocketAddr, SocketAddr) {
    let held: Vec<TcpListener> = (0..3)
        .map(|_| TcpListener::bind("127.0.0.1:0").unwrap())
        .collect();
    let at = |i: usize| held[i].local_addr().unwrap();
    (at(0), at(1), at(2))
}

/// Start the API on a file and wait for `/metrics`. A child that **exits** before it serves is retried on three fresh
/// addresses, but **only** when it says the port was taken: any other exit is the test's answer. Every failure says what the
/// child wrote to stderr (ANR-1065).
fn serve_metrics(tag: &str, extra: &str, env: &[(&str, &str)]) -> (Killed, String) {
    // The running tests start in parallel and each releases its ports before its child binds them: one at a time.
    static START: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _starting = START
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut why = String::new();
    for attempt in 1..=3 {
        let (listen, health, metrics) = free_addrs();
        let config = write(
            &format!("{tag}-{attempt}"),
            &document(extra, Some((listen, health, metrics))),
        );
        let kube = kubeconfig(tag);
        let mut child = Killed(
            api()
                .args(["--config"])
                .arg(&config)
                .envs(env.iter().copied())
                .env("KUBECONFIG", &kube)
                .env_remove("KUBERNETES_SERVICE_HOST")
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(text) = http_get(metrics, "/metrics")
                && text.starts_with("HTTP/1.1 200")
            {
                return (child, text);
            }
            if let Some(status) = child.0.try_wait().unwrap() {
                let tail = stderr_of(&mut child);
                let said = format!("attempt {attempt}: the API exited ({status}): {tail}");
                assert!(tail.contains("Address already in use"), "{said}");
                if !why.is_empty() {
                    why.push_str("; ");
                }
                why.push_str(&said);
                eprintln!("{said}; retrying on fresh ports");
                break;
            }
            if Instant::now() >= deadline {
                let tail = stderr_of(&mut child);
                panic!(
                    "the API did not come up in 20 s on {listen}, {health} and {metrics}: {tail}"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    panic!("the API never came up: {why}");
}

fn stderr_of(child: &mut Killed) -> String {
    let _ = child.0.kill();
    let _ = child.0.wait();
    let mut bytes = Vec::new();
    if let Some(mut err) = child.0.stderr.take() {
        let _ = err.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A kubeconfig that names a server nobody answers: the client is built (the API connects lazily), nothing is reachable.
fn kubeconfig(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("aap-api-telemetry-{}-{tag}", std::process::id()));
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

/// With no `telemetry` in the file the metrics text has no trace line: it is what K4 prints.
#[test]
fn a_running_api_with_no_telemetry_makes_no_span() {
    let (_child, text) = serve_metrics("plain", "", &[]);
    assert!(!text.contains("aap_trace_"), "{text}");
}

/// A collector that is not there is a tracing setting the API warns about and runs without: it never stops for one.
#[test]
fn a_running_api_with_an_unreachable_collector_still_serves() {
    let (_child, text) = serve_metrics(
        "unreachable",
        "telemetry:\n  otlp:\n    endpoint: http://127.0.0.1:1/v1/traces\n",
        &[],
    );
    assert!(text.contains("aap_trace_enabled 1\n"), "{text}");
}

/// ANR-970 kept the rule for the file that the variables had: a reference the pod does not have never stops the API. `--check` reads
/// no file, so it accepts the section; a running API warns once, serves, and says tracing is off.
#[test]
fn a_reference_the_pod_does_not_have_turns_tracing_off_and_stops_nothing() {
    let unreadable = "telemetry:\n  otlp:\n    endpoint: https://otel.invalid:4318/v1/traces\n    headers:\n      authorization:\n        file: /nonexistent/headers\n";
    let (code, stdout, stderr) = check(&write("unreadable", &document(unreadable, None)));
    assert_eq!(code, Some(0), "{stdout} {stderr}");
    let (mut child, text) = serve_metrics("unreadable", unreadable, &[]);
    let log = stderr_of(&mut child);
    assert!(text.contains("aap_trace_enabled 0\n"), "{text}");
    assert!(log.contains("tracing is off"), "{log}");
}

/// The variables the cutover removed are read by nothing: a shell that still sets them changes no setting, and the API serves.
#[test]
fn the_removed_variables_are_ignored() {
    let (_child, text) = serve_metrics(
        "ignored",
        "",
        &[
            ("API_LISTEN_ADDR", "garbage"),
            ("HEALTH_ADDR", "garbage"),
            ("METRICS_ADDR", "garbage"),
            ("API_CONFIG_FILE", "/nonexistent/config.yaml"),
            ("RUST_LOG", "="),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
            (
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "unix:///dev/otel-grpc.sock",
            ),
        ],
    );
    assert!(!text.contains("aap_trace_"), "{text}");
}
