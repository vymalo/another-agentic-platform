//! The testkit (feature `testkit`): an OTLP/JSON **logs** receiver on a local port that checks the shape of what it is
//! sent, can be told to fail in every way a collector can, and decodes what it took back into events; and
//! [`ObservedOtlp`], the wrapper that lets `aap_usage::testkit::run` judge the real sink.
//!
//! The server is a few lines of HTTP/1.1 on a raw socket (one request per connection, `Connection: close`) so that it
//! can also hang and cut a connection, which a router cannot.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aap_usage::testkit::Observed;
use aap_usage::{PodKind, PodMinuteEvent, UsageError, UsageSink};
use serde_json::Value as Json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::OtlpUsageSink;

/// What the collector answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Behaviour {
    /// `200` and `{}`, after recording the body.
    Accept,
    /// This status, with no body, recording nothing.
    Status(u16),
    /// Read the request and never answer.
    Hang,
    /// Read the request and cut the connection, recording nothing.
    Close,
    /// Record the body, then cut the connection without answering: the sink cannot know it was taken, so it sends it
    /// again and the collector sees it twice.
    LoseAnswer,
    /// `302` to `/elsewhere`.
    Redirect,
    /// `200` with `partialSuccess.rejectedLogRecords` of this, after recording the body.
    Reject(u32),
    /// `413` to a request with more than this many records, recording nothing; `200` (recorded) otherwise.
    MaxRecords(usize),
}

#[derive(Default)]
struct Inner {
    bodies: Mutex<Vec<Json>>,
    raw: Mutex<Vec<Vec<u8>>>,
    behaviour: Mutex<Option<Behaviour>>,
    elsewhere: AtomicUsize,
}

/// An OTLP/JSON logs receiver. It answers `400` to a body that does not have the shape of `examples/logs.json` of
/// opentelemetry-proto (see [`check_shape`]) or that does not carry exactly AD-069's attributes.
pub struct TestCollector {
    addr: SocketAddr,
    inner: Arc<Inner>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestCollector {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_request(sock: &mut TcpStream) -> Option<(String, Vec<(String, String)>, Vec<u8>)> {
    let mut data = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?.to_owned();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    let mut body = data[head_end..].to_vec();
    while body.len() < length {
        let n = sock.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(length);
    Some((request_line, headers, body))
}

async fn respond(sock: &mut TcpStream, status: u16, extra: &str, body: &str) {
    let text = format!(
        "HTTP/1.1 {status} X\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(text.as_bytes()).await;
    let _ = sock.shutdown().await;
}

async fn serve(mut sock: TcpStream, s: Arc<Inner>) {
    let Some((line, headers, body)) = read_request(&mut sock).await else {
        return;
    };
    let path = line.split(' ').nth(1).unwrap_or("");
    if path == "/elsewhere" {
        s.elsewhere.fetch_add(1, Ordering::SeqCst);
        respond(&mut sock, 200, "", "").await;
        return;
    }
    let behaviour =
        (*s.behaviour.lock().unwrap_or_else(PoisonError::into_inner)).unwrap_or(Behaviour::Accept);
    s.raw
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(body.clone());
    match behaviour {
        Behaviour::Hang => std::future::pending::<()>().await,
        Behaviour::Close => {}
        Behaviour::Status(n) => respond(&mut sock, n, "", "").await,
        Behaviour::Redirect => respond(&mut sock, 302, "Location: /elsewhere\r\n", "").await,
        Behaviour::Accept
        | Behaviour::LoseAnswer
        | Behaviour::Reject(_)
        | Behaviour::MaxRecords(_) => {
            let content_type_ok = headers
                .iter()
                .any(|(k, v)| k == "content-type" && v == "application/json");
            let json = serde_json::from_slice::<Json>(&body).ok();
            let valid = path == "/v1/logs"
                && content_type_ok
                && json.as_ref().is_some_and(|j| check_shape(j).is_ok());
            let Some(json) = json.filter(|_| valid) else {
                respond(&mut sock, 400, "", "").await;
                return;
            };
            if let Behaviour::MaxRecords(max) = behaviour
                && records(&json).len() > max
            {
                respond(&mut sock, 413, "", "").await;
                return;
            }
            s.bodies
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(json);
            match behaviour {
                Behaviour::LoseAnswer => {}
                Behaviour::Reject(n) => {
                    let answer = format!(
                        r#"{{"partialSuccess":{{"rejectedLogRecords":"{n}","errorMessage":"no"}}}}"#
                    );
                    respond(
                        &mut sock,
                        200,
                        "Content-Type: application/json\r\n",
                        &answer,
                    )
                    .await;
                }
                _ => respond(&mut sock, 200, "Content-Type: application/json\r\n", "{}").await,
            }
        }
    }
}

impl TestCollector {
    /// Start on `127.0.0.1:0`. Not `async`, so a conformance `make` closure can call it.
    ///
    /// # Panics
    ///
    /// When the port cannot be bound, or outside a Tokio runtime.
    pub fn start() -> Self {
        let inner = Arc::new(Inner::default());
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        std_listener.set_nonblocking(true).expect("non-blocking");
        let listener = TcpListener::from_std(std_listener).expect("a tokio listener");
        let addr = listener.local_addr().expect("a bound address");
        let state = inner.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve(sock, state.clone()));
            }
        });
        Self { addr, inner, task }
    }

    /// The base URL: what the settings' `endpoint` is.
    #[must_use]
    pub fn base_url(&self) -> url::Url {
        url::Url::parse(&format!("http://{}", self.addr)).expect("a URL")
    }

    /// The address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Answer as `behaviour` says from now on.
    pub fn set_behaviour(&self, behaviour: Behaviour) {
        *self
            .inner
            .behaviour
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(behaviour);
    }

    /// The bodies accepted (and recorded), in order.
    #[must_use]
    pub fn requests(&self) -> Vec<Json> {
        self.inner
            .bodies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The raw bytes of **every** request the collector received on any path but `/elsewhere`, before the path is
    /// checked and whatever it was answered: a request to a wrong path is recorded here too.
    #[must_use]
    pub fn raw_requests(&self) -> Vec<Vec<u8>> {
        self.inner
            .raw
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many times `/elsewhere` was asked for: a redirect that was followed.
    #[must_use]
    pub fn elsewhere_hits(&self) -> usize {
        self.inner.elsewhere.load(Ordering::SeqCst)
    }

    /// Every event of every accepted request, in order, decoded; a record that does not decode is a panic.
    ///
    /// # Panics
    ///
    /// On a record [`decode_record`] refuses (the collector would have answered `400` to it).
    #[must_use]
    pub fn events(&self) -> Vec<PodMinuteEvent> {
        self.requests()
            .iter()
            .flat_map(|r| records(r).into_iter().cloned().collect::<Vec<_>>())
            .map(|r| decode_record(&r).expect("a record the shape check let through decodes"))
            .collect()
    }
}

/// The log records of a request body.
#[must_use]
pub fn records(body: &Json) -> Vec<&Json> {
    body["resourceLogs"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|rl| rl["scopeLogs"].as_array().into_iter().flatten())
        .flat_map(|sl| sl["logRecords"].as_array().into_iter().flatten())
        .collect()
}

fn is_nanos(v: &Json) -> bool {
    v.as_str()
        .is_some_and(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()))
}

fn check_attributes(v: &Json) -> Result<(), &'static str> {
    for a in v.as_array().ok_or("attributes is not an array")? {
        if a["key"].as_str().is_none() {
            return Err("an attribute has no key");
        }
        let value = a["value"].as_object().ok_or("an attribute has no value")?;
        let ok = match (value.len(), value.iter().next()) {
            (1, Some((k, v))) => match k.as_str() {
                "stringValue" => v.is_string(),
                "intValue" => is_nanos(v),
                "boolValue" => v.is_boolean(),
                _ => false,
            },
            _ => false,
        };
        if !ok {
            return Err(
                "an attribute value is not stringValue, intValue (a decimal string) or boolValue",
            );
        }
    }
    Ok(())
}

/// Whether `body` has the shape of `examples/logs.json` of opentelemetry-proto (`resourceLogs`, `scopeLogs`,
/// `logRecords`; times in decimal strings, `severityNumber` an integer, tagged values) **and** each record decodes
/// as a pod-minute event ([`decode_record`]).
///
/// # Errors
///
/// A fixed text that says which rule it broke.
pub fn check_shape(body: &Json) -> Result<(), &'static str> {
    let resource_logs = body["resourceLogs"].as_array().ok_or("no resourceLogs")?;
    for rl in resource_logs {
        check_attributes(&rl["resource"]["attributes"])?;
        for sl in rl["scopeLogs"].as_array().ok_or("no scopeLogs")? {
            for r in sl["logRecords"].as_array().ok_or("no logRecords")? {
                if !is_nanos(&r["timeUnixNano"]) || !is_nanos(&r["observedTimeUnixNano"]) {
                    return Err("a time is not a decimal string");
                }
                if !r["severityNumber"].is_u64() {
                    return Err("severityNumber is not an integer");
                }
                if r["body"]["stringValue"].as_str().is_none() {
                    return Err("body is not a stringValue");
                }
                check_attributes(&r["attributes"])?;
                decode_record(r)?;
            }
        }
    }
    Ok(())
}

/// A record back into the event it carries, refusing anything AD-069 does not say: every attribute of the right
/// type, no attribute twice, none unknown, the body and `type` the event type, severity 9, both times equal.
///
/// # Errors
///
/// A fixed text that says which rule it broke.
pub fn decode_record(r: &Json) -> Result<PodMinuteEvent, &'static str> {
    let attrs = r["attributes"].as_array().ok_or("no attributes")?;
    let mut text = std::collections::BTreeMap::new();
    let mut ints = std::collections::BTreeMap::new();
    for a in attrs {
        let key = a["key"].as_str().ok_or("no key")?;
        let value = &a["value"];
        let dup = if let Some(s) = value["stringValue"].as_str() {
            text.insert(key, s.to_owned()).is_some()
        } else if let Some(s) = value["intValue"].as_str() {
            ints.insert(
                key,
                s.parse::<u64>().map_err(|_| "intValue is not a number")?,
            )
            .is_some()
        } else {
            return Err("an attribute is neither string nor int");
        };
        if dup {
            return Err("an attribute is repeated");
        }
    }
    let known = [
        "type",
        "event_id",
        "source",
        "subject",
        "weightQuarters",
        "nodeType",
        "podKind",
        "agentService",
        "run",
    ];
    if text.keys().chain(ints.keys()).any(|k| !known.contains(k)) {
        return Err("an unknown attribute");
    }
    if ints.keys().any(|k| *k != "weightQuarters") || text.contains_key("weightQuarters") {
        return Err("an attribute has the wrong type");
    }
    let get = |k: &'static str| {
        text.get(k)
            .cloned()
            .ok_or("a required attribute is missing")
    };
    if get("type")? != aap_usage::EVENT_TYPE || r["body"]["stringValue"] != aap_usage::EVENT_TYPE {
        return Err("the type or the body is not the event type");
    }
    if r["severityNumber"] != 9 {
        return Err("severityNumber is not 9");
    }
    if r["timeUnixNano"] != r["observedTimeUnixNano"] {
        return Err("the two times differ");
    }
    let nanos: u64 = r["timeUnixNano"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or("the time is not a number")?;
    if !nanos.is_multiple_of(60_000_000_000) || nanos == 0 {
        return Err("the time is not the end of a minute");
    }
    let pod_kind = match get("podKind")?.as_str() {
        "run" => PodKind::Run,
        "agent" => PodKind::Agent,
        _ => return Err("podKind is neither run nor agent"),
    };
    let event = PodMinuteEvent {
        source: get("source")?,
        event_id: get("event_id")?,
        minute: nanos / 60_000_000_000 - 1,
        subject: get("subject")?,
        weight_quarters: u32::try_from(*ints.get("weightQuarters").ok_or("no weightQuarters")?)
            .map_err(|_| "weightQuarters is too large")?,
        node_type: get("nodeType")?,
        pod_kind,
        agent_service: get("agentService")?,
        run: text.get("run").cloned(),
    };
    event
        .validate()
        .map_err(|_| "the event does not validate")?;
    Ok(event)
}

/// A sink and its collector, for `aap_usage::testkit::run`: `send` sends and then flushes, so that `taken` reads what
/// the collector has received.
pub struct ObservedOtlp {
    /// The real sink.
    pub sink: OtlpUsageSink,
    /// The collector it sends to.
    pub collector: TestCollector,
}

impl UsageSink for ObservedOtlp {
    async fn send(&self, event: PodMinuteEvent) -> Result<(), UsageError> {
        self.sink.send(event).await?;
        if self.sink.flush(Duration::from_secs(10)).await {
            Ok(())
        } else {
            Err(UsageError::Unavailable)
        }
    }

    fn outcomes(&self) -> Option<Vec<aap_usage::Outcome>> {
        self.sink.outcomes()
    }
}

impl Observed for ObservedOtlp {
    fn taken(&self) -> Vec<PodMinuteEvent> {
        self.collector.events()
    }
}
