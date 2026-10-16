//! The log of keys that cannot be fetched says so when it starts and when it ends, and not once per request.
//!
//! In a test binary of its own, as `tests/logs.rs` is: `tracing` caches a callsite that another test of the same process hit
//! first while no subscriber was installed.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use std::io::Write;
use std::sync::{Arc, Mutex};

use common::*;
use tracing_subscriber::fmt::MakeWriter;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

impl Capture {
    /// The lines that say the keys are (un)available: the request lines say `request` and are not counted.
    fn lines_about_keys(&self) -> Vec<String> {
        String::from_utf8(self.0.lock().unwrap().clone())
            .unwrap()
            .lines()
            .filter(|l| l.contains("issuer's keys"))
            .map(str::to_owned)
            .collect()
    }
}

#[tokio::test]
async fn keys_that_are_down_are_one_warning_and_their_return_is_one_line() {
    let capture = Capture::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(capture.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let api = Api::new();
    let token = api.admin();
    assert_eq!(api.get("/v1/me").token(&token).send().await.code(), 200);
    assert!(capture.lines_about_keys().is_empty(), "nothing to say yet");

    api.auth.set_unavailable(true);
    for _ in 0..25 {
        assert_eq!(api.get("/v1/me").token(&token).send().await.code(), 503);
    }
    // A request with no token needs no keys, and does not say they are back.
    assert_eq!(api.get("/v1/me").send().await.code(), 401);
    let lines = capture.lines_about_keys();
    assert_eq!(lines.len(), 1, "25 requests, one warning: {}", lines.len());
    assert!(
        lines[0].contains("WARN") && lines[0].contains("unavailable"),
        "{lines:?}"
    );

    api.auth.set_unavailable(false);
    assert_eq!(api.get("/v1/me").token(&token).send().await.code(), 200);
    assert_eq!(api.get("/v1/me").token(&token).send().await.code(), 200);
    let lines = capture.lines_about_keys();
    assert_eq!(lines.len(), 2, "and one line when they are back: {lines:?}");
    assert!(
        lines[1].contains("INFO") && lines[1].contains("available again"),
        "{lines:?}"
    );

    // Down again is a change, and says so again.
    api.auth.set_unavailable(true);
    for _ in 0..5 {
        assert_eq!(api.get("/v1/me").token(&token).send().await.code(), 503);
    }
    let lines = capture.lines_about_keys();
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[2].contains("WARN"), "{lines:?}");
}
