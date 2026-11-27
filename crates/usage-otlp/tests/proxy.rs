#![allow(clippy::unwrap_used, clippy::expect_used)]
//! "No proxy": the sink does not read `HTTP_PROXY`, `HTTPS_PROXY` or `ALL_PROXY`.
//!
//! In a binary of its own that re-runs itself as a child with the variables set (`set_var` is `unsafe` since edition
//! 2024, and a process-wide variable would reach the other tests). The parent is the "proxy": a listener that counts
//! connections. The child makes one request with a plain `reqwest` client, which goes to the proxy (the control: the
//! variables are read), and then sends one event through the sink, which must reach the collector and not the proxy.

use std::net::TcpListener;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use aap_usage::UsageSink;
use aap_usage::testkit::event;
use aap_usage_otlp::testkit::TestCollector;
use aap_usage_otlp::{OtlpUsageSink, UsageOtlpSettings};

const CHILD: &str = "AAP_USAGE_NO_PROXY_TEST_CHILD";

#[test]
fn the_sink_does_not_read_the_proxy_variables() {
    if std::env::var_os(CHILD).is_some() {
        return;
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", listener.local_addr().unwrap());
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    std::thread::spawn(move || {
        // Accept and drop: the client sees a connection that closes.
        while let Ok((stream, _)) = listener.accept() {
            counter.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    let exe = std::env::current_exe().unwrap();
    let out = Command::new(exe)
        .args(["--exact", "child", "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .env("HTTP_PROXY", &proxy)
        .env("http_proxy", &proxy)
        .env("HTTPS_PROXY", &proxy)
        .env("https_proxy", &proxy)
        .env("ALL_PROXY", &proxy)
        .env("all_proxy", &proxy)
        .env_remove("NO_PROXY")
        .env_remove("no_proxy")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Exactly one connection: the control's. The sink made none.
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn child() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let collector = TestCollector::start();
    // The control: a default client does use the variables, so the proxy is in the way for it.
    let control = reqwest::Client::new()
        .post(format!("{}/v1/logs", collector.base_url()))
        .body("{}")
        .send()
        .await;
    assert!(
        control.is_err(),
        "the default client should have gone through the proxy"
    );
    let sink = OtlpUsageSink::start(UsageOtlpSettings::new(collector.base_url())).unwrap();
    let e = event("m1", "pod-1", 3);
    sink.send(e.clone()).await.unwrap();
    assert!(sink.flush(Duration::from_secs(10)).await);
    assert_eq!(collector.events(), vec![e]);
}
