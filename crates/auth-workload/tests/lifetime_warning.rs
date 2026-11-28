//! ANR-1374: the lifetime warning. One line per process, with both numbers, and **never the identity**; and a leaf the listener
//! refuses cannot spend the one warning, because the check runs after every refusal.
//!
//! A test binary of its own, as the crate's other logs are: `std::sync::Once` is per process, so the count is only meaningful where
//! nothing else in the binary has warned.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use aap_auth_workload::{WorkloadAuth, WorkloadClient, WorkloadClients};
use aap_ports::{Certificate, ClientAuthenticator};
use tracing_subscriber::layer::SubscriberExt;

const CA: &str = include_str!("fixtures/ca.pem");
const ACCEPTED: &str = include_str!("fixtures/accepted.pem");
const UNKNOWN: &str = include_str!("fixtures/unknown.pem");
const ORCHESTRATOR: &str = "spiffe://another/ns/test/sa/orchestrator";

/// The first certificate of a PEM, as the port's type.
fn der(pem: &str) -> Certificate {
    let (_, block) = x509_parser::pem::parse_x509_pem(pem.as_bytes()).expect("a PEM certificate");
    Certificate::new(block.contents)
}

/// Every event's fields, as `name=value` pairs on one line each.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<String>>>);

impl Captured {
    fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Captured {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        #[derive(Default)]
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                self.0.push_str(&format!("{}={value:?}", field.name()));
            }
        }
        let mut fields = Fields::default();
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }
}

fn authenticator(max: Option<i64>) -> WorkloadAuth {
    let clients = WorkloadClients {
        clients: vec![WorkloadClient {
            spiffe_id: ORCHESTRATOR.to_owned(),
            permissions: vec!["platform:agents.own".to_owned()],
        }],
        max_leaf_lifetime_secs: max,
    };
    WorkloadAuth::new(CA, &clients).expect("the test CA bundle and table")
}

#[tokio::test]
async fn a_long_lived_leaf_is_warned_about_once_and_the_identity_is_never_in_the_line() {
    let captured = Captured::default();
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(captured.clone()));
    // One second: the fixtures live for years, so every accepted leaf is "too long".
    let auth = authenticator(Some(1));

    // A leaf the listener refuses is refused for its identity, not for its length — and it must not spend the one warning.
    assert!(
        auth.authenticate(Some(&der(UNKNOWN))).await.is_err(),
        "the fixture is not a configured identity"
    );
    assert!(
        captured.lines().is_empty(),
        "a refused leaf warns about nothing: {:?}",
        captured.lines()
    );

    // An accepted one, twice: one line, whoever asks.
    assert!(
        auth.authenticate(Some(&der(ACCEPTED))).await.is_ok(),
        "a long-lived leaf is accepted"
    );
    assert!(auth.authenticate(Some(&der(ACCEPTED))).await.is_ok());
    let lines = captured.lines();
    assert_eq!(lines.len(), 1, "once per process: {lines:?}");
    assert!(lines[0].contains("leaf_lifetime_secs"), "{lines:?}");
    assert!(lines[0].contains("max_leaf_lifetime_secs"), "{lines:?}");
    assert!(
        !lines[0].contains("spiffe"),
        "the identity is never in the line: {lines:?}"
    );
}

#[tokio::test]
async fn with_no_maximum_nothing_is_warned_about() {
    let captured = Captured::default();
    let _guard =
        tracing::subscriber::set_default(tracing_subscriber::registry().with(captured.clone()));
    let auth = authenticator(None);
    assert!(auth.authenticate(Some(&der(ACCEPTED))).await.is_ok());
    assert!(captured.lines().is_empty(), "{:?}", captured.lines());
}
