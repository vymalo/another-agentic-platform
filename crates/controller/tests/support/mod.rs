//! A fake API server for the controller: a `tower` service behind a real `kube::Client`, so the
//! controller's requests are the real ones (the finalizer helper's JSON patches, the server-side
//! apply of the status, the Events, and the list and watch of kube's `Controller`) and what it does
//! with the answers is the real code.
//!
//! It keeps the custom resources (the two kinds of the operator and the two shared kinds of the
//! dashboard, whose status it never touches) by path, applies JSON patches to `metadata.finalizers` as the
//! API server does (a failed `test` is a 422; an object that is being deleted goes away when the last
//! finalizer does), replaces `status` on an apply of the `/status` subresource, answers a list, streams
//! a watch (every change of an object is an `ADDED`, `MODIFIED` or `DELETED` line to whoever watches
//! its kind), and records every call and every Event. A kind can be made *absent* ([`Fake::absent`]):
//! every request for it, list, watch and get, is the 404 a real API server answers when the CRD is not
//! installed, and a missing object is the 404 that names it (both *verified 2026-10-15* against a bare
//! kube-apiserver v1.35.8: the first is the plain text `404 page not found`, which kube turns into a
//! `Status` with no `details`; the second is a `Status` whose `details.name` is the object's). A request can
//! be made to **never be answered** ([`Fake::stall_next`]): the shape of a call queued behind a watch on a
//! pooled connection (*seen 2026-10-16* on a real cluster: hyper 1.11.1, see the controller's README). It is
//! not an API server: no CRD schema, no CEL, no resource versions to resume from, no other kinds.

#![allow(dead_code)] // each test file uses part of it

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use futures::StreamExt;
use http::{Request, Response};
use http_body::Frame;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt, Full, StreamBody};
use kube::Client;
use kube::client::Body;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tower::service_fn;

pub mod scripted;

const GROUP: &str = "/apis/agents.vymalo.com/v1alpha1";

type Resp = Response<UnsyncBoxBody<Bytes, Box<dyn std::error::Error + Send + Sync>>>;

/// One request.
#[derive(Clone, Debug)]
pub struct Call {
    pub method: String,
    pub path: String,
    pub query: String,
    pub content_type: String,
    pub body: Option<Value>,
}

/// A change of an object, as a watcher is told.
#[derive(Clone, Debug)]
struct Change {
    plural: String,
    ns: String,
    kind: &'static str,
    object: Value,
}

struct State {
    objects: BTreeMap<String, Value>,
    calls: Vec<Call>,
    events: Vec<Value>,
    version: u64,
    fail_with: Option<u16>,
    watchers: BTreeMap<String, usize>,
    /// The kinds (by plural) whose CRD is not installed.
    absent: BTreeSet<String>,
    /// Requests that are received and never answered, while `remaining` lasts.
    stalls: Vec<Stall>,
}

/// A request the fake takes and never answers.
struct Stall {
    method: String,
    path_contains: String,
    remaining: usize,
}

#[derive(Clone)]
pub struct Fake {
    state: Arc<Mutex<State>>,
    changes: broadcast::Sender<Change>,
}

impl Default for Fake {
    fn default() -> Self {
        Self::new()
    }
}

fn lock(s: &Mutex<State>) -> MutexGuard<'_, State> {
    s.lock().unwrap_or_else(PoisonError::into_inner)
}

fn path(plural: &str, ns: &str, name: &str) -> String {
    format!("{GROUP}/namespaces/{ns}/{plural}/{name}")
}

fn kind_of(plural: &str) -> &'static str {
    match plural {
        "agentservices" => "AgentService",
        "modelendpoints" => "ModelEndpoint",
        "toolproviders" => "ToolProvider",
        _ => "AgentConfig",
    }
}

impl Fake {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                objects: BTreeMap::new(),
                calls: Vec::new(),
                events: Vec::new(),
                version: 0,
                fail_with: None,
                watchers: BTreeMap::new(),
                absent: BTreeSet::new(),
                stalls: Vec::new(),
            })),
            changes: broadcast::channel(1024).0,
        }
    }

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

    fn announce(&self, kind: &'static str, plural: &str, object: &Value) {
        // Nobody watching is not a failure.
        let _ = self.changes.send(Change {
            plural: plural.to_owned(),
            ns: object["metadata"]["namespace"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            kind,
            object: object.clone(),
        });
    }

    /// Create or replace an object, as a person's `kubectl apply` would: a new uid and generation.
    pub fn put(&self, plural: &str, mut object: Value) {
        let ns = object["metadata"]["namespace"]
            .as_str()
            .unwrap_or("default")
            .to_owned();
        let name = object["metadata"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let mut s = lock(&self.state);
        s.version += 1;
        let key = path(plural, &ns, &name);
        let (uid, generation, kind) = match s.objects.get(&key) {
            Some(old) => (
                old["metadata"]["uid"].clone(),
                old["metadata"]["generation"].as_i64().unwrap_or(1) + 1,
                "MODIFIED",
            ),
            None => (json!(format!("uid-{name}")), 1, "ADDED"),
        };
        object["metadata"]["uid"] = uid;
        object["metadata"]["generation"] = json!(generation);
        object["metadata"]["resourceVersion"] = json!(s.version.to_string());
        // A person's apply does not carry the controller's status, the finalizers, or the deletion.
        if let Some(old) = s.objects.get(&key) {
            for field in ["finalizers", "deletionTimestamp"] {
                if let Some(v) = old["metadata"].get(field) {
                    object["metadata"][field] = v.clone();
                }
            }
            if let Some(v) = old.get("status") {
                object["status"] = v.clone();
            }
        }
        s.objects.insert(key, object.clone());
        drop(s);
        self.announce(kind, plural, &object);
    }

    pub fn get(&self, plural: &str, ns: &str, name: &str) -> Option<Value> {
        lock(&self.state)
            .objects
            .get(&path(plural, ns, name))
            .cloned()
    }

    /// `kubectl delete`: with finalizers the object stays, with a `deletionTimestamp`.
    pub fn delete(&self, plural: &str, ns: &str, name: &str) {
        let mut s = lock(&self.state);
        let key = path(plural, ns, name);
        let (object, gone) = match s.objects.get_mut(&key) {
            Some(o)
                if o["metadata"]["finalizers"]
                    .as_array()
                    .is_some_and(|f| !f.is_empty()) =>
            {
                o["metadata"]["deletionTimestamp"] = json!("2026-10-05T10:00:00Z");
                (o.clone(), false)
            }
            Some(o) => (o.clone(), true),
            None => return,
        };
        if gone {
            s.objects.remove(&key);
        }
        drop(s);
        self.announce(if gone { "DELETED" } else { "MODIFIED" }, plural, &object);
    }

    pub fn calls(&self) -> Vec<Call> {
        lock(&self.state).calls.clone()
    }

    pub fn clear_calls(&self) {
        lock(&self.state).calls.clear();
    }

    pub fn writes(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| c.method != "GET")
            .collect()
    }

    /// The Events the controller published.
    pub fn events(&self) -> Vec<Value> {
        lock(&self.state).events.clone()
    }

    /// Answer every request with this HTTP status, or go back to answering.
    pub fn fail_with(&self, code: Option<u16>) {
        lock(&self.state).fail_with = code;
    }

    /// The kinds (by plural, `modelendpoints`) whose CRD is not installed: every request for them is the
    /// 404 an API server answers for a resource it does not serve. Replaces the earlier set; an empty
    /// one is the CRDs being installed.
    pub fn absent(&self, plurals: &[&str]) {
        lock(&self.state).absent = plurals.iter().map(|p| (*p).to_owned()).collect();
    }

    /// The next `times` requests with this method whose path contains `path_contains` are received (and
    /// recorded in [`calls`](Self::calls)) and **never answered**: the client waits for ever, as it does for
    /// a request queued behind a watch on a connection that is still streaming it.
    pub fn stall_next(&self, method: &str, path_contains: &str, times: usize) {
        lock(&self.state).stalls.push(Stall {
            method: method.to_owned(),
            path_contains: path_contains.to_owned(),
            remaining: times,
        });
    }

    /// How many watches of this kind are open.
    pub fn watchers(&self, plural: &str) -> usize {
        lock(&self.state).watchers.get(plural).copied().unwrap_or(0)
    }

    /// Whether `call` is one to leave unanswered (and, then, record it and use one of the stall's turns).
    fn stalls(&self, call: &Call) -> bool {
        let mut s = lock(&self.state);
        let Some(stall) = s.stalls.iter_mut().find(|st| {
            st.remaining > 0 && st.method == call.method && call.path.contains(&st.path_contains)
        }) else {
            return false;
        };
        stall.remaining -= 1;
        s.calls.push(call.clone());
        true
    }

    async fn handle(&self, req: Request<Body>) -> Resp {
        let (parts, body) = req.into_parts();
        let bytes = body.collect_bytes().await.unwrap_or_default();
        let call = Call {
            method: parts.method.as_str().to_owned(),
            path: parts.uri.path().to_owned(),
            query: parts.uri.query().unwrap_or_default().to_owned(),
            content_type: parts
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
            body: serde_json::from_slice(&bytes).ok(),
        };
        if self.stalls(&call) {
            return std::future::pending().await;
        }
        let mut s = lock(&self.state);
        s.calls.push(call.clone());
        if let Some(code) = s.fail_with {
            return failure(code, "the fake is failing on purpose");
        }

        if call.path.contains("/events") && !call.path.starts_with(GROUP) {
            if let Some(body) = &call.body {
                s.events.push(body.clone());
            }
            return respond(201, call.body.as_ref().unwrap_or(&json!({})));
        }

        // `/apis/agents.vymalo.com/v1alpha1/[namespaces/<ns>/]<plural>[/<name>[/status]]`
        let Some(rest) = call
            .path
            .strip_prefix(GROUP)
            .map(|r| r.trim_start_matches('/'))
        else {
            return failure(404, "not a path of the fake");
        };
        let segments: Vec<&str> = rest.split('/').collect();
        let (ns, plural, tail) = match segments.as_slice() {
            ["namespaces", ns, plural, tail @ ..] => (Some(*ns), *plural, tail),
            [plural, tail @ ..] => (None, *plural, tail),
            [] => return failure(404, "no resource"),
        };

        if s.absent.contains(plural) {
            return not_served();
        }
        match (call.method.as_str(), tail) {
            ("GET", []) => {
                let watching = call.query.split('&').any(|p| p == "watch=true");
                if watching {
                    *s.watchers.entry(plural.to_owned()).or_insert(0) += 1;
                    drop(s);
                    return self.watch(plural, ns);
                }
                let prefix = match ns {
                    Some(ns) => format!("{GROUP}/namespaces/{ns}/{plural}/"),
                    None => String::new(),
                };
                let items: Vec<&Value> = s
                    .objects
                    .iter()
                    .filter(|(p, _)| {
                        if ns.is_some() {
                            p.starts_with(&prefix)
                        } else {
                            p.rsplit('/').nth(1) == Some(plural)
                        }
                    })
                    .map(|(_, o)| o)
                    .collect();
                respond(
                    200,
                    &json!({
                        "apiVersion": "agents.vymalo.com/v1alpha1",
                        "kind": format!("{}List", kind_of(plural)),
                        "metadata": {"resourceVersion": s.version.to_string()},
                        "items": items,
                    }),
                )
            }
            ("GET", [name]) => match ns.and_then(|ns| s.objects.get(&path(plural, ns, name))) {
                Some(o) => respond(200, o),
                None => not_found(plural, name),
            },
            ("PATCH", [name, rest @ ..]) => {
                let is_status = rest == ["status"];
                let Some(patch) = call.body.clone() else {
                    return failure(400, "no body");
                };
                let Some(ns) = ns else {
                    return failure(404, "a patch is namespaced");
                };
                let key = path(plural, ns, name);
                s.version += 1;
                let version = s.version.to_string();
                let Some(object) = s.objects.get_mut(&key) else {
                    return not_found(plural, name);
                };
                if call.content_type.contains("json-patch") {
                    if let Err(why) = json_patch(object, &patch) {
                        return failure(422, &why);
                    }
                } else if call.content_type.contains("apply-patch") && is_status {
                    // A server-side apply of the status by its only manager: it becomes the status.
                    object["status"] = patch["status"].clone();
                } else {
                    return failure(415, "the fake serves json-patch and the apply of /status");
                }
                object["metadata"]["resourceVersion"] = json!(version);
                let out = object.clone();
                let finished = out["metadata"]["deletionTimestamp"].is_string()
                    && out["metadata"]["finalizers"]
                        .as_array()
                        .is_none_or(Vec::is_empty);
                if finished {
                    s.objects.remove(&key);
                }
                drop(s);
                self.announce(if finished { "DELETED" } else { "MODIFIED" }, plural, &out);
                respond(200, &out)
            }
            _ => failure(405, "not served by the fake"),
        }
    }

    /// An open watch: a line for each change of the kind, until the client hangs up.
    fn watch(&self, plural: &str, ns: Option<&str>) -> Resp {
        let rx = self.changes.subscribe();
        let (plural, ns) = (plural.to_owned(), ns.map(str::to_owned));
        let stream = futures::stream::unfold(rx, move |mut rx| {
            let (plural, ns) = (plural.clone(), ns.clone());
            async move {
                loop {
                    match rx.recv().await {
                        Ok(c) if c.plural == plural && ns.as_ref().is_none_or(|n| *n == c.ns) => {
                            let mut line =
                                serde_json::to_vec(&json!({"type": c.kind, "object": c.object}))
                                    .unwrap_or_default();
                            line.push(b'\n');
                            let frame: Result<
                                Frame<Bytes>,
                                Box<dyn std::error::Error + Send + Sync>,
                            > = Ok(Frame::data(Bytes::from(line)));
                            return Some((frame, rx));
                        }
                        Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => return None,
                    }
                }
            }
        })
        .boxed();
        Response::builder()
            .status(200)
            .header("content-type", "application/json")
            .body(StreamBody::new(stream).boxed_unsync())
            .unwrap_or_else(|_| failure(500, "could not build the watch"))
    }
}

/// The three operations the finalizer helper sends.
fn json_patch(object: &mut Value, ops: &Value) -> Result<(), String> {
    for op in ops.as_array().ok_or("a JSON patch is an array")? {
        let path = op["path"].as_str().ok_or("no path")?;
        match op["op"].as_str() {
            Some("test") => {
                let found = object.pointer(path).cloned().unwrap_or(Value::Null);
                if found != op["value"] {
                    return Err(format!("test failed at {path}: {found} != {}", op["value"]));
                }
            }
            Some("add") => match path.rsplit_once('/') {
                Some((parent, "-")) => object
                    .pointer_mut(parent)
                    .and_then(Value::as_array_mut)
                    .ok_or("no array to append to")?
                    .push(op["value"].clone()),
                Some((parent, key)) => {
                    object
                        .pointer_mut(parent)
                        .and_then(Value::as_object_mut)
                        .ok_or("no object to add to")?
                        .insert(key.to_owned(), op["value"].clone());
                }
                None => return Err("bad path".into()),
            },
            Some("remove") => {
                let (parent, index) = path.rsplit_once('/').ok_or("bad path")?;
                let at: usize = index.parse().map_err(|_| "remove wants an index")?;
                object
                    .pointer_mut(parent)
                    .and_then(Value::as_array_mut)
                    .filter(|a| at < a.len())
                    .ok_or("nothing to remove")?
                    .remove(at);
            }
            other => return Err(format!("the fake does not know op {other:?}")),
        }
    }
    Ok(())
}

fn respond(code: u16, body: &Value) -> Resp {
    Response::builder()
        .status(code)
        .header("content-type", "application/json")
        .body(
            Full::new(Bytes::from(serde_json::to_vec(body).unwrap_or_default()))
                .map_err(|never| match never {})
                .boxed_unsync(),
        )
        .unwrap_or_else(|_| {
            Response::new(
                Full::new(Bytes::new())
                    .map_err(|never| match never {})
                    .boxed_unsync(),
            )
        })
}

/// What an API server answers for an object that is not there: a `Status` whose `details` name it.
fn not_found(plural: &str, name: &str) -> Resp {
    respond(
        404,
        &json!({"apiVersion": "v1", "kind": "Status", "status": "Failure", "metadata": {},
                "message": format!("{plural}.agents.vymalo.com \"{name}\" not found"),
                "reason": "NotFound", "code": 404,
                "details": {"name": name, "group": "agents.vymalo.com", "kind": plural}}),
    )
}

/// What an API server answers for a resource it does not serve, a CRD that is not installed: not a
/// `Status` but the plain text `404 page not found`, which kube turns into a `Status` with no details.
fn not_served() -> Resp {
    Response::builder()
        .status(404)
        .header("content-type", "text/plain; charset=utf-8")
        .body(
            Full::new(Bytes::from_static(b"404 page not found\n"))
                .map_err(|never| match never {})
                .boxed_unsync(),
        )
        .unwrap_or_else(|_| failure(404, "not served"))
}

fn failure(code: u16, message: &str) -> Resp {
    let reason = match code {
        404 => "NotFound",
        422 => "Invalid",
        403 => "Forbidden",
        503 => "ServiceUnavailable",
        _ => "Failure",
    };
    respond(
        code,
        &json!({"apiVersion": "v1", "kind": "Status", "status": "Failure",
                "message": message, "reason": reason, "code": code}),
    )
}
