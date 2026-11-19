//! A fake API server for the lease (K7a): a `tower` service behind a real `kube::Client`, so the election's
//! requests are the real ones (`GET`, `POST` and `PUT` on the lease path) and what it does with the answers is
//! the real code.
//!
//! It keeps the lease by path with a `resourceVersion` that every write moves, answers `409` to a write that
//! names a stale one and to a create of a name that is there, and `404` to a read of one that is not. It is not
//! an API server: no validation, no field ownership, no other kind, and no admission.
//!
//! A test can also make one method fail with a status ([`Fake::fail`]): that is how "a `409` leaves a follower"
//! is shown without racing another candidate.

#![allow(dead_code)] // each test file uses part of it

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use http::{Request, Response};
use kube::Client;
use kube::client::Body;
use serde_json::{Value, json};
use tower::service_fn;

/// The group and version of a `Lease`.
const GROUP: &str = "/apis/coordination.k8s.io/v1";

/// One request.
#[derive(Clone, Debug)]
pub struct Call {
    pub method: String,
    pub path: String,
    pub body: Option<Value>,
}

struct State {
    /// The leases, by path.
    objects: BTreeMap<String, Value>,
    calls: Vec<Call>,
    version: u64,
    /// One method answered with a status instead, while it is set.
    failing: Option<(String, u16)>,
}

/// The fake server. Clones share one state.
#[derive(Clone)]
pub struct Fake {
    state: Arc<Mutex<State>>,
}

impl Default for Fake {
    fn default() -> Self {
        Self::new()
    }
}

fn lock(s: &Mutex<State>) -> MutexGuard<'_, State> {
    s.lock().unwrap_or_else(PoisonError::into_inner)
}

fn path(ns: &str, name: &str) -> String {
    format!("{GROUP}/namespaces/{ns}/leases/{name}")
}

impl Fake {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                objects: BTreeMap::new(),
                calls: Vec::new(),
                version: 0,
                failing: None,
            })),
        }
    }

    /// A client whose requests go to this fake.
    pub fn client(&self) -> Client {
        let fake = self.clone();
        Client::new(
            service_fn(move |req: Request<Body>| {
                let fake = fake.clone();
                async move { Ok::<_, Infallible>(fake.handle(req).await) }
            }),
            "default",
        )
    }

    /// Plant a lease there, as somebody else's replica would have left it. The `resourceVersion` is the fake's.
    pub fn put(&self, ns: &str, name: &str, mut object: Value) {
        object["metadata"]["name"] = json!(name);
        object["metadata"]["namespace"] = json!(ns);
        let mut s = lock(&self.state);
        s.version += 1;
        object["metadata"]["resourceVersion"] = json!(s.version.to_string());
        s.objects.insert(path(ns, name), object);
    }

    /// The lease, as the server holds it.
    pub fn get(&self, ns: &str, name: &str) -> Option<Value> {
        lock(&self.state).objects.get(&path(ns, name)).cloned()
    }

    /// Every request, in order.
    pub fn calls(&self) -> Vec<Call> {
        lock(&self.state).calls.clone()
    }

    /// Answer `method` with `code` until [`clear_failures`](Self::clear_failures).
    pub fn fail(&self, method: &str, code: u16) {
        lock(&self.state).failing = Some((method.to_owned(), code));
    }

    /// Answer every method as usual again.
    pub fn clear_failures(&self) {
        lock(&self.state).failing = None;
    }

    async fn handle(&self, req: Request<Body>) -> Response<Body> {
        let (parts, body) = req.into_parts();
        let bytes = body.collect_bytes().await.unwrap_or_default();
        let call = Call {
            method: parts.method.as_str().to_owned(),
            path: parts.uri.path().to_owned(),
            body: serde_json::from_slice(&bytes).ok(),
        };
        let mut s = lock(&self.state);
        s.calls.push(call.clone());
        if let Some((method, code)) = &s.failing
            && *method == call.method
        {
            return status(*code, "the fake fails this method on purpose", "Conflict");
        }
        let Some(rest) = call.path.strip_prefix(GROUP) else {
            return status(404, "not a path of the fake", "NotFound");
        };
        let segments: Vec<&str> = rest.trim_start_matches('/').split('/').collect();
        let (ns, tail) = match segments.as_slice() {
            ["namespaces", ns, "leases", tail @ ..] => (*ns, tail),
            _ => return status(404, "not a path of the fake", "NotFound"),
        };
        match (call.method.as_str(), tail) {
            ("GET", [name]) => match s.objects.get(&path(ns, name)) {
                Some(lease) => respond(200, lease),
                None => not_found(name),
            },
            ("POST", []) => {
                let Some(lease) = call.body.clone() else {
                    return status(400, "no body", "BadRequest");
                };
                let Some(name) = lease["metadata"]["name"].as_str() else {
                    return status(422, "a lease has a name", "Invalid");
                };
                let key = path(ns, name);
                if s.objects.contains_key(&key) {
                    return status(409, "the lease is there", "AlreadyExists");
                }
                s.version += 1;
                let version = s.version.to_string();
                let mut stored = lease;
                stored["metadata"]["namespace"] = json!(ns);
                stored["metadata"]["resourceVersion"] = json!(version);
                stored["metadata"]["uid"] = json!(format!("uid-{version}"));
                s.objects.insert(key, stored.clone());
                respond(201, &stored)
            }
            ("PUT", [name]) => {
                let Some(mut lease) = call.body.clone() else {
                    return status(400, "no body", "BadRequest");
                };
                let key = path(ns, name);
                let Some(existing) = s.objects.get(&key) else {
                    return not_found(name);
                };
                let (uid, version) = (
                    existing["metadata"]["uid"].clone(),
                    existing["metadata"]["resourceVersion"].clone(),
                );
                if lease["metadata"]["resourceVersion"] != version {
                    return status(409, "the lease moved", "Conflict");
                }
                s.version += 1;
                let version = s.version.to_string();
                lease["metadata"]["name"] = json!(name);
                lease["metadata"]["namespace"] = json!(ns);
                lease["metadata"]["resourceVersion"] = json!(version);
                lease["metadata"]["uid"] = uid;
                s.objects.insert(key, lease.clone());
                respond(200, &lease)
            }
            _ => status(405, "not served by the fake", "MethodNotAllowed"),
        }
    }
}

fn respond(code: u16, body: &Value) -> Response<Body> {
    Response::builder()
        .status(code)
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap_or_default()))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

fn status(code: u16, message: &str, reason: &str) -> Response<Body> {
    respond(
        code,
        &json!({"apiVersion": "v1", "kind": "Status", "status": "Failure",
                "message": message, "reason": reason, "code": code}),
    )
}

/// What an API server answers for a lease that is not there: a `Status` whose `details` name it.
fn not_found(name: &str) -> Response<Body> {
    respond(
        404,
        &json!({"apiVersion": "v1", "kind": "Status", "status": "Failure", "metadata": {},
                "message": format!("leases.coordination.k8s.io \"{name}\" not found"),
                "reason": "NotFound", "code": 404,
                "details": {"name": name, "group": "coordination.k8s.io", "kind": "leases"}}),
    )
}
