//! A fake API server: a `tower` service behind a real `kube::Client`, so the crate's requests are the real ones
//! and what it does with the answers is the real code.
//!
//! It keeps objects by path and implements what `AgentObjects` and its harness use, **as a kube-apiserver
//! v1.35.8 does it** (every behaviour below was observed on one, see the crate documentation and
//! `tests/fixtures`):
//!
//! * **server-side apply** by field manager, with `force`, `dryRun`, ownership of the leaf fields and the
//!   conflict a field owned by another manager is, a `metadata.resourceVersion` that must be the object's (a `409`;
//!   ignored for an object that is not there), a `metadata.uid` (a `422` for another object, a `409` for none),
//!   the defaults of the schema (`suspend`, `scaling`, `conditions: []`, ...) written into what is stored, the
//!   `status` of the main resource ignored, and `metadata.managedFields` in the answer;
//! * the `status` subresource, JSON merge patches (an *update*: it takes the fields it changes from the others),
//!   `POST`, `GET` and `LIST` with a label selector, and `DELETE` with the options in its body (`dryRun`,
//!   `preconditions`), the query parameter ignored;
//! * the answers of the rules a body can break, as the `Status` bodies captured from the real thing
//!   (`tests/fixtures/status`);
//! * a kind that is not served (ExternalSecrets, until they are) answers the plain text `404 page not found`.
//!
//! It is not an API server: there is no authorization (`calls()` is how a test shows what was asked), no
//! finalizers, no watches, no paging, no validation but the few rules above, and the defaults are the few the
//! tests need. What a cluster does is `tests/cluster.rs`'s.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

pub mod harness;
pub mod intercept;

use std::collections::{BTreeMap, BTreeSet};
use std::convert::Infallible;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use http::{Request, Response};
use kube::Client;
use kube::client::Body;
use serde_json::{Map, Value, json};
use tower::service_fn;

type Path = Vec<String>;

/// One request the crate made.
#[derive(Clone, Debug)]
pub struct Call {
    /// Which client asked: `dashboard` is the crate under test, `admin` the harness.
    pub who: &'static str,
    pub method: String,
    pub path: String,
    pub query: BTreeMap<String, String>,
    pub content_type: String,
    pub body: Option<Value>,
    /// The status the fake answered.
    pub code: u16,
}

impl Call {
    /// The collection: `agentservices`, `externalsecrets`, `secrets`.
    pub fn plural(&self) -> &str {
        let segments: Vec<&str> = self.path.split('/').collect();
        segments
            .iter()
            .position(|s| *s == "namespaces")
            .and_then(|i| segments.get(i + 2))
            .copied()
            .unwrap_or_default()
    }

    /// The object's name, for a call on one object.
    pub fn name(&self) -> Option<&str> {
        let segments: Vec<&str> = self.path.split('/').collect();
        let at = segments.iter().position(|s| *s == "namespaces")? + 3;
        segments.get(at).copied()
    }

    pub fn is_subresource(&self) -> bool {
        self.path.ends_with("/status")
    }

    /// Whether the call is a dry run. A `DELETE` takes its options from its body when it has one, and the
    /// query parameter `dryRun` is then ignored (as the real API server does).
    pub fn dry_run(&self) -> bool {
        match (self.method.as_str(), &self.body) {
            ("DELETE", Some(body)) => body
                .get("dryRun")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty()),
            _ => self.query.contains_key("dryRun"),
        }
    }

    pub fn is_apply(&self) -> bool {
        self.method == "PATCH" && self.content_type.contains("apply-patch")
    }

    /// Whether the call changes something, if it is accepted.
    pub fn writes(&self) -> bool {
        matches!(self.method.as_str(), "PATCH" | "POST" | "PUT" | "DELETE")
    }

    /// Whether the call changes something for real: a write that is not a dry run.
    pub fn changes(&self) -> bool {
        self.writes() && !self.dry_run()
    }

    pub fn succeeded(&self) -> bool {
        self.code < 300
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Apply,
    Update,
}

/// What one manager owns of one object.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    manager: String,
    op: Op,
    sub: Option<String>,
    paths: BTreeSet<Path>,
}

#[derive(Clone, Debug)]
struct Obj {
    value: Value,
    entries: Vec<Entry>,
}

/// A hook that runs once, before the next real apply of an object: somebody else getting there first.
struct Hook {
    plural: String,
    name: String,
    run: Box<dyn FnOnce(&mut State) + Send>,
}

#[derive(Default)]
pub struct State {
    objects: BTreeMap<String, Obj>,
    calls: Vec<Call>,
    next_version: u64,
    next_uid: u64,
    fail_with: Option<u16>,
    unserved: BTreeSet<String>,
    hooks: Vec<Hook>,
    delete_hooks: Vec<Hook>,
}

/// The fake server. Clones share one state.
#[derive(Clone, Default)]
pub struct Fake {
    state: Arc<Mutex<State>>,
}

fn lock(s: &Mutex<State>) -> MutexGuard<'_, State> {
    s.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Fake {
    pub fn new() -> Self {
        let fake = Self::default();
        // The External Secrets Operator is not installed until a test installs it.
        lock(&fake.state)
            .unserved
            .insert("externalsecrets".to_owned());
        fake
    }

    /// A client whose requests go to this fake.
    pub fn client(&self) -> Client {
        self.client_as("client")
    }

    /// A client whose requests go to this fake and are recorded as `who`'s.
    pub fn client_as(&self, who: &'static str) -> Client {
        let fake = self.clone();
        let service = service_fn(move |req: Request<Body>| {
            let fake = fake.clone();
            async move { Ok::<_, Infallible>(fake.handle(req, who).await) }
        });
        Client::new(service, "default")
    }

    /// A client whose every request fails in the transport, as a refused connection does.
    pub fn unreachable_client() -> Client {
        let service = service_fn(|_req: Request<Body>| async move {
            Err::<Response<Body>, tower::BoxError>("connection refused".into())
        });
        Client::new(service, "default")
    }

    /// Serve (or stop serving) a kind: the External Secrets Operator's CRD is installed or not.
    pub fn serve(&self, plural: &str, served: bool) {
        let mut s = lock(&self.state);
        if served {
            s.unserved.remove(plural);
        } else {
            s.unserved.insert(plural.to_owned());
        }
    }

    /// Answer every request with this HTTP status, or go back to answering.
    pub fn fail_with(&self, code: Option<u16>) {
        lock(&self.state).fail_with = code;
    }

    pub fn calls(&self) -> Vec<Call> {
        lock(&self.state).calls.clone()
    }

    pub fn clear_calls(&self) {
        lock(&self.state).calls.clear();
    }

    /// The object as stored (without the bookkeeping the server adds to an answer), or `None`.
    pub fn stored(&self, plural: &str, name: &str) -> Option<Value> {
        lock(&self.state)
            .objects
            .get(&key(plural, name))
            .map(|o| o.value.clone())
    }

    /// Change an object as somebody else would, now.
    pub fn edit(&self, edit: impl FnOnce(&mut State)) {
        edit(&mut lock(&self.state));
    }

    /// Run `edit` once, just before the next delete that is not a dry run of the object.
    pub fn before_the_next_delete_of(
        &self,
        plural: &str,
        name: &str,
        edit: impl FnOnce(&mut State) + Send + 'static,
    ) {
        lock(&self.state).delete_hooks.push(Hook {
            plural: plural.to_owned(),
            name: name.to_owned(),
            run: Box::new(edit),
        });
    }

    /// Run `edit` once, just before the next apply that is not a dry run of the object: somebody else got there
    /// first, between the dry runs and the real writes.
    pub fn before_the_next_real_apply(
        &self,
        plural: &str,
        name: &str,
        edit: impl FnOnce(&mut State) + Send + 'static,
    ) {
        lock(&self.state).hooks.push(Hook {
            plural: plural.to_owned(),
            name: name.to_owned(),
            run: Box::new(edit),
        });
    }

    async fn handle(&self, req: Request<Body>, who: &'static str) -> Response<Body> {
        let (parts, body) = req.into_parts();
        let bytes = body.collect_bytes().await.unwrap_or_default();
        let mut call = Call {
            who,
            method: parts.method.as_str().to_owned(),
            path: parts.uri.path().to_owned(),
            query: url::form_urlencoded::parse(parts.uri.query().unwrap_or_default().as_bytes())
                .into_owned()
                .collect(),
            content_type: parts
                .headers
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned(),
            body: serde_json::from_slice(&bytes).ok(),
            code: 0,
        };
        let mut s = lock(&self.state);
        let (code, body) = s.route(&call);
        call.code = code;
        s.calls.push(call);
        reply(code, &body)
    }
}

fn key(plural: &str, name: &str) -> String {
    format!("{plural}/{name}")
}

fn reply(code: u16, body: &Value) -> Response<Body> {
    // The plain text of an API the server does not have.
    let (content_type, bytes) = match body {
        Value::String(text) => ("text/plain; charset=utf-8", text.clone().into_bytes()),
        other => (
            "application/json",
            serde_json::to_vec(other).unwrap_or_default(),
        ),
    };
    Response::builder()
        .status(code)
        .header("content-type", content_type)
        .body(Body::from(bytes))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

// ------------------------------------------------------------------ the Status bodies

fn status(code: u16, reason: &str, message: &str, details: Value) -> (u16, Value) {
    (
        code,
        json!({ "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
                "message": message, "reason": reason, "details": details, "code": code }),
    )
}

fn not_found(plural: &str, name: &str) -> (u16, Value) {
    status(
        404,
        "NotFound",
        &format!("{plural}.agents.vymalo.com \"{name}\" not found"),
        json!({ "name": name, "group": "agents.vymalo.com", "kind": plural }),
    )
}

fn conflict(plural: &str, name: &str, message: &str) -> (u16, Value) {
    status(
        409,
        "Conflict",
        &format!(
            "Operation cannot be fulfilled on {plural}.agents.vymalo.com \"{name}\": {message}"
        ),
        json!({ "name": name, "group": "agents.vymalo.com", "kind": plural }),
    )
}

/// A captured body, for the object `name` (the fixtures were captured for `x`).
fn fixture(text: &str, name: &str) -> (u16, Value) {
    let mut body: Value = serde_json::from_str(text).expect("a fixture is JSON");
    let code = u16::try_from(body["code"].as_u64().unwrap_or(500)).unwrap_or(500);
    if let Some(message) = body["message"].as_str() {
        body["message"] = json!(
            message
                .replace("\"x\"", &format!("\"{name}\""))
                .replace("(/x;", &format!("(/{name};"))
        );
    }
    if body["details"]["name"].is_string() {
        body["details"]["name"] = json!(name);
    }
    (code, body)
}

// ------------------------------------------------------------------ the rules a body can break

/// The rules the fake knows. The first that matches answers; the bodies are the captured ones.
fn refuse(plural: &str, name: &str, patch: &Value) -> Option<(u16, Value)> {
    let spec = patch.get("spec");
    if let Some(spec) = spec
        && !spec.is_object()
        && !spec.is_null()
    {
        return Some(fixture(
            include_str!("../fixtures/status/apply-string-spec.500.json"),
            name,
        ));
    }
    match plural {
        "agentservices" => {
            if patch
                .pointer("/spec/suspend")
                .is_some_and(|v| !v.is_boolean())
            {
                return Some(fixture(
                    include_str!("../fixtures/status/apply-type-mismatch.500.json"),
                    name,
                ));
            }
            let audience = patch
                .pointer("/spec/access/audience")
                .and_then(Value::as_array);
            if audience.is_some_and(|a| a.len() > 1 && a.iter().any(|v| v == "*")) {
                return Some(fixture(
                    include_str!("../fixtures/status/apply-cel-audience.422.json"),
                    name,
                ));
            }
        }
        "agentconfigs" => {
            let model = patch.pointer("/spec/model");
            let has = |field: &str| model.is_some_and(|m| m.get(field).is_some());
            if has("endpointRef") && (has("baseUrl") || has("apiKeySecretRef")) {
                return Some(fixture(
                    include_str!("../fixtures/status/apply-cel-model-exclusive.422.json"),
                    name,
                ));
            }
        }
        _ => {}
    }
    None
}

/// The defaults the schemas of the CRDs write into every object, for the fields the tests set.
fn defaults(plural: &str, value: &mut Value) {
    let mut put = |path: &[&str], default: Value| {
        let path: Path = path.iter().map(|s| (*s).to_owned()).collect();
        if get_path(value, &path).is_none() {
            let parent = &path[..path.len() - 1];
            if parent.is_empty() || get_path(value, parent).is_some_and(Value::is_object) {
                set_path(value, &path, default);
            }
        }
    };
    match plural {
        "agentservices" => {
            put(&["spec", "suspend"], json!(false));
            put(&["spec", "deletionPolicy"], json!("Retain"));
            put(
                &["spec", "scaling"],
                json!({ "topology": "combined", "workers": 1 }),
            );
            put(&["spec", "access", "allowFrom"], json!([]));
            put(&["spec", "interfaces", "mcp"], json!({ "enabled": false }));
        }
        "agentconfigs" => {
            put(&["spec", "environment", "volumes"], json!([]));
            put(&["spec", "extraEnv"], json!({}));
            put(&["spec", "security"], json!({}));
        }
        _ => {}
    }
    if matches!(plural, "agentservices" | "agentconfigs") {
        put(&["status", "conditions"], json!([]));
    }
}

// ------------------------------------------------------------------ paths in a JSON document

fn get_path<'a>(v: &'a Value, path: &[String]) -> Option<&'a Value> {
    path.iter().try_fold(v, |v, k| v.get(k))
}

fn set_path(v: &mut Value, path: &[String], new: Value) {
    let Some((last, parents)) = path.split_last() else {
        *v = new;
        return;
    };
    let mut node = v;
    for k in parents {
        if !node.get(k).is_some_and(Value::is_object) {
            node[k] = json!({});
        }
        node = &mut node[k];
    }
    node[last] = new;
}

fn remove_path(v: &mut Value, path: &[String]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut node = v;
    for k in parents {
        match node.get_mut(k) {
            Some(next) => node = next,
            None => return,
        }
    }
    if let Some(map) = node.as_object_mut() {
        map.remove(last);
    }
}

/// The leaves of a configuration: scalars, lists and empty maps are leaves (a list is atomic).
fn leaves(v: &Value, prefix: &mut Path, out: &mut BTreeSet<Path>) {
    match v {
        Value::Object(map) if !map.is_empty() => {
            for (k, child) in map {
                prefix.push(k.clone());
                leaves(child, prefix, out);
                prefix.pop();
            }
        }
        _ => {
            out.insert(prefix.clone());
        }
    }
}

/// The fields of an apply that are owned: not the identity of the object, nor its version.
fn owned_leaves(patch: &Value, only: Option<&str>) -> BTreeSet<Path> {
    let mut out = BTreeSet::new();
    for (top, value) in patch.as_object().into_iter().flatten() {
        if matches!(top.as_str(), "apiVersion" | "kind") || only.is_some_and(|o| o != top) {
            continue;
        }
        if top == "status" && only.is_none() {
            continue;
        }
        let mut prefix = vec![top.clone()];
        if top == "metadata" {
            // Only what a person sets: the labels and the annotations.
            for sub in ["labels", "annotations"] {
                if let Some(map) = value.get(sub) {
                    let mut p = vec![top.clone(), sub.to_owned()];
                    leaves(map, &mut p, &mut out);
                }
            }
            continue;
        }
        leaves(value, &mut prefix, &mut out);
    }
    out
}

/// `metadata.managedFields[].fieldsV1`.
fn fields_v1(paths: &BTreeSet<Path>) -> Value {
    let mut root = Map::new();
    for path in paths {
        let mut node = &mut root;
        for segment in path {
            let entry = node
                .entry(format!("f:{segment}"))
                .or_insert_with(|| Value::Object(Map::new()));
            node = entry.as_object_mut().expect("a node is an object");
        }
    }
    Value::Object(root)
}

// ------------------------------------------------------------------ the server

struct Target<'a> {
    plural: &'a str,
    name: Option<&'a str>,
    sub: Option<&'a str>,
}

fn target(path: &str) -> Option<Target<'_>> {
    let segments: Vec<&str> = path.split('/').collect();
    let at = segments.iter().position(|s| *s == "namespaces")? + 2;
    Some(Target {
        plural: segments.get(at).copied()?,
        name: segments.get(at + 1).copied(),
        sub: segments.get(at + 2).copied(),
    })
}

fn kind_of(plural: &str) -> (&'static str, &'static str) {
    match plural {
        "agentservices" => ("agents.vymalo.com/v1alpha1", "AgentService"),
        "agentconfigs" => ("agents.vymalo.com/v1alpha1", "AgentConfig"),
        "modelendpoints" => ("agents.vymalo.com/v1alpha1", "ModelEndpoint"),
        "toolproviders" => ("agents.vymalo.com/v1alpha1", "ToolProvider"),
        "externalsecrets" => ("external-secrets.io/v1", "ExternalSecret"),
        "secrets" => ("v1", "Secret"),
        _ => ("v1", "Unknown"),
    }
}

impl State {
    fn route(&mut self, call: &Call) -> (u16, Value) {
        if let Some(code) = self.fail_with {
            return status(
                code,
                "InternalError",
                "the fake is failing on purpose",
                json!({}),
            );
        }
        let Some(t) = target(&call.path) else {
            return status(404, "NotFound", "not a path of the fake", json!({}));
        };
        if self.unserved.contains(t.plural) {
            return (404, Value::String("404 page not found\n".to_owned()));
        }
        match (call.method.as_str(), t.name) {
            ("GET", None) => self.list(t.plural, call),
            ("GET", Some(name)) => match self.objects.get(&key(t.plural, name)) {
                Some(o) => (200, self.answer(t.plural, name, o)),
                None => not_found(t.plural, name),
            },
            ("POST", None) => self.create(t.plural, call),
            ("PATCH", Some(name)) if call.is_apply() => self.apply(t.plural, name, t.sub, call),
            ("PATCH", Some(name)) => self.merge(t.plural, name, call),
            ("DELETE", Some(name)) => self.delete(t.plural, name, call),
            _ => status(
                405,
                "MethodNotAllowed",
                "not supported by the fake",
                json!({}),
            ),
        }
    }

    /// The object as the server answers: the stored value and the bookkeeping.
    fn answer(&self, plural: &str, name: &str, o: &Obj) -> Value {
        let _ = name;
        let (api_version, kind) = kind_of(plural);
        let mut v = o.value.clone();
        v["apiVersion"] = json!(api_version);
        v["kind"] = json!(kind);
        let managed: Vec<Value> = o
            .entries
            .iter()
            .map(|e| {
                let mut m = json!({
                    "manager": e.manager,
                    "operation": if e.op == Op::Apply { "Apply" } else { "Update" },
                    "apiVersion": api_version,
                    "fieldsType": "FieldsV1",
                    "fieldsV1": fields_v1(&e.paths),
                });
                if let Some(sub) = &e.sub {
                    m["subresource"] = json!(sub);
                }
                m
            })
            .collect();
        v["metadata"]["managedFields"] = Value::Array(managed);
        v
    }

    fn list(&self, plural: &str, call: &Call) -> (u16, Value) {
        let selector: Vec<(String, String)> = call
            .query
            .get("labelSelector")
            .map(|s| {
                s.split(',')
                    .filter_map(|kv| kv.split_once('='))
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect()
            })
            .unwrap_or_default();
        let prefix = format!("{plural}/");
        let items: Vec<Value> = self
            .objects
            .iter()
            .filter_map(|(k, o)| Some((k.strip_prefix(&prefix)?, o)))
            .filter(|(_, o)| {
                selector
                    .iter()
                    .all(|(k, v)| o.value["metadata"]["labels"][k] == json!(v))
            })
            .map(|(name, o)| self.answer(plural, name, o))
            .collect();
        let (api_version, kind) = kind_of(plural);
        (
            200,
            json!({ "apiVersion": api_version, "kind": format!("{kind}List"),
                    "metadata": { "resourceVersion": self.next_version.to_string() }, "items": items }),
        )
    }

    fn bump(&mut self) -> String {
        self.next_version += 1;
        (100 + self.next_version).to_string()
    }

    fn create(&mut self, plural: &str, call: &Call) -> (u16, Value) {
        let Some(mut object) = call.body.clone() else {
            return status(400, "BadRequest", "no body", json!({}));
        };
        let Some(name) = object["metadata"]["name"].as_str().map(str::to_owned) else {
            return status(422, "Invalid", "no name", json!({}));
        };
        if self.objects.contains_key(&key(plural, &name)) {
            return status(
                409,
                "AlreadyExists",
                &format!("{plural} \"{name}\" already exists"),
                json!({ "name": name, "kind": plural }),
            );
        }
        let manager = call
            .query
            .get("fieldManager")
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned());
        let mut paths = BTreeSet::new();
        leaves(
            &object["metadata"]["labels"].clone(),
            &mut vec!["metadata".to_owned(), "labels".to_owned()],
            &mut paths,
        );
        for top in ["spec", "data", "stringData"] {
            if let Some(v) = object.get(top) {
                leaves(v, &mut vec![top.to_owned()], &mut paths);
            }
        }
        if !call.dry_run() {
            let version = self.bump();
            self.next_uid += 1;
            object["metadata"]["uid"] = json!(format!("uid-{}", self.next_uid));
            object["metadata"]["resourceVersion"] = json!(version);
            object["metadata"]["generation"] = json!(1);
            self.objects.insert(
                key(plural, &name),
                Obj {
                    value: object.clone(),
                    entries: vec![Entry {
                        manager,
                        op: Op::Update,
                        sub: None,
                        paths,
                    }],
                },
            );
        }
        (201, object)
    }

    /// A JSON merge patch: an update, which takes the fields it changes from the other managers.
    fn merge(&mut self, plural: &str, name: &str, call: &Call) -> (u16, Value) {
        let Some(patch) = call.body.clone() else {
            return status(400, "BadRequest", "no body", json!({}));
        };
        let manager = call
            .query
            .get("fieldManager")
            .cloned()
            .unwrap_or_else(|| "unknown".to_owned());
        let Some(mut obj) = self.objects.get(&key(plural, name)).cloned() else {
            return not_found(plural, name);
        };
        let mut changed = BTreeSet::new();
        let mut removed = Vec::new();
        merge_patch(
            &mut obj.value,
            &patch,
            &mut Vec::new(),
            &mut changed,
            &mut removed,
        );
        for e in &mut obj.entries {
            e.paths
                .retain(|p| !changed.contains(p) && !removed.iter().any(|r| p.starts_with(r)));
        }
        obj.entries.retain(|e| !e.paths.is_empty());
        let mut mine = Entry {
            manager: manager.clone(),
            op: Op::Update,
            sub: None,
            paths: changed,
        };
        if let Some(i) = obj
            .entries
            .iter()
            .position(|e| e.manager == manager && e.op == Op::Update && e.sub.is_none())
        {
            mine.paths.extend(obj.entries.remove(i).paths);
        }
        obj.entries.push(mine);
        self.commit(plural, name, obj, call.dry_run())
    }

    fn delete(&mut self, plural: &str, name: &str, call: &Call) -> (u16, Value) {
        if !call.dry_run()
            && let Some(i) = self
                .delete_hooks
                .iter()
                .position(|h| h.plural == plural && h.name == name)
        {
            let hook = self.delete_hooks.remove(i);
            (hook.run)(self);
        }
        let Some(obj) = self.objects.get(&key(plural, name)) else {
            return not_found(plural, name);
        };
        let pre = call.body.as_ref().map(|b| &b["preconditions"]);
        if let Some(rv) = pre.and_then(|p| p["resourceVersion"].as_str())
            && Some(rv) != obj.value["metadata"]["resourceVersion"].as_str()
        {
            return conflict(
                plural,
                name,
                &format!(
                    "the ResourceVersion in the precondition ({rv}) does not match the ResourceVersion in record. The object might have been modified"
                ),
            );
        }
        if let Some(uid) = pre.and_then(|p| p["uid"].as_str())
            && Some(uid) != obj.value["metadata"]["uid"].as_str()
        {
            return conflict(
                plural,
                name,
                &format!(
                    "the UID in the precondition ({uid}) does not match the UID in record. The object might have been deleted and then recreated"
                ),
            );
        }
        if !call.dry_run() {
            self.objects.remove(&key(plural, name));
        }
        (
            200,
            json!({ "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Success",
                    "details": { "name": name, "group": "agents.vymalo.com", "kind": plural } }),
        )
    }

    /// Store `obj` as the new state of the object, with a new version when anything it holds changed.
    fn commit(&mut self, plural: &str, name: &str, mut obj: Obj, dry_run: bool) -> (u16, Value) {
        let before = self.objects.get(&key(plural, name)).cloned();
        let created = before.is_none();
        let changed = before
            .as_ref()
            .is_none_or(|b| b.value != obj.value || b.entries != obj.entries);
        if changed {
            let spec_changed = before
                .as_ref()
                .is_none_or(|b| b.value.get("spec") != obj.value.get("spec"));
            let generation = before.as_ref().map_or(0, |b| {
                b.value["metadata"]["generation"].as_i64().unwrap_or(1)
            });
            obj.value["metadata"]["generation"] = json!(if created {
                1
            } else {
                generation + i64::from(spec_changed)
            });
            if !dry_run {
                obj.value["metadata"]["resourceVersion"] = json!(self.bump());
            }
        }
        let answer = self.answer(plural, name, &obj);
        if !dry_run {
            self.objects.insert(key(plural, name), obj);
        }
        (if created { 201 } else { 200 }, answer)
    }

    fn apply(&mut self, plural: &str, name: &str, sub: Option<&str>, call: &Call) -> (u16, Value) {
        if !call.dry_run()
            && let Some(i) = self
                .hooks
                .iter()
                .position(|h| h.plural == plural && h.name == name)
        {
            let hook = self.hooks.remove(i);
            (hook.run)(self);
        }
        let Some(patch) = call.body.clone() else {
            return status(400, "BadRequest", "no body", json!({}));
        };
        let Some(manager) = call.query.get("fieldManager").cloned() else {
            return status(
                422,
                "Invalid",
                "PatchOptions.meta.k8s.io \"\" is invalid: fieldManager: Required value: is required for apply patch",
                json!({}),
            );
        };
        let force = call.query.get("force").is_some_and(|f| f == "true");
        if let Some(refusal) = refuse(plural, name, &patch) {
            return refusal;
        }
        let existing = self.objects.get(&key(plural, name)).cloned();
        if sub.is_some() && existing.is_none() {
            return not_found(plural, name);
        }

        // The uid names an object: another one is immutable, and none at all is a conflict.
        if let Some(uid) = patch["metadata"]["uid"].as_str() {
            match &existing {
                None => {
                    return conflict(
                        plural,
                        name,
                        &format!(
                            "uid mismatch: the provided object specified uid {uid}, and no existing object was found"
                        ),
                    );
                }
                Some(o) if o.value["metadata"]["uid"] != json!(uid) => {
                    return fixture(
                        include_str!("../fixtures/status/apply-uid-immutable.422.json"),
                        name,
                    );
                }
                Some(_) => {}
            }
        }

        let applied = owned_leaves(&patch, sub);
        let mut obj = existing.clone().unwrap_or_else(|| {
            self.next_uid += 1;
            Obj {
                value: json!({ "metadata": { "name": name, "namespace": "aap-test", "uid": format!("uid-{}", self.next_uid) } }),
                entries: Vec::new(),
            }
        });
        let me = |e: &Entry| e.manager == manager && e.op == Op::Apply && e.sub.as_deref() == sub;

        // A field that another manager owns and that this apply would change.
        let conflicts: Vec<(&String, &Path)> = obj
            .entries
            .iter()
            .filter(|e| !me(e))
            .flat_map(|e| e.paths.iter().map(move |p| (&e.manager, p)))
            .filter(|(_, p)| applied.contains(*p))
            .filter(|(_, p)| get_path(&obj.value, p) != get_path(&patch, p))
            .collect();
        if !conflicts.is_empty() && !force {
            let fields: Vec<String> = conflicts
                .iter()
                .map(|(_, p)| format!(".{}", p.join(".")))
                .collect();
            let (owner, _) = conflicts[0];
            return (
                409,
                json!({ "kind": "Status", "apiVersion": "v1", "metadata": {}, "status": "Failure",
                        "message": format!("Apply failed with {} conflict(s): conflict with \"{owner}\" using {}: {}", conflicts.len(), kind_of(plural).0, fields.join(", ")),
                        "reason": "Conflict",
                        "details": { "causes": conflicts.iter().map(|(m, p)| json!({
                            "reason": "FieldManagerConflict",
                            "message": format!("conflict with \"{m}\" using {}", kind_of(plural).0),
                            "field": format!(".{}", p.join(".")) })).collect::<Vec<_>>() },
                        "code": 409 }),
            );
        }
        // The version of an object that is there must be the one named; for none that is there it is ignored.
        if let (Some(rv), Some(_)) = (patch["metadata"]["resourceVersion"].as_str(), &existing)
            && Some(rv) != obj.value["metadata"]["resourceVersion"].as_str()
        {
            return conflict(
                plural,
                name,
                "the object has been modified; please apply your changes to the latest version and try again",
            );
        }

        if force {
            let taken: BTreeSet<Path> = conflicts.iter().map(|(_, p)| (*p).clone()).collect();
            for e in obj.entries.iter_mut().filter(|e| !me(e)) {
                e.paths.retain(|p| !taken.contains(p));
            }
        }
        let before: BTreeSet<Path> = obj
            .entries
            .iter()
            .find(|e| me(e))
            .map(|e| e.paths.clone())
            .unwrap_or_default();
        // What this manager owned and no longer names goes, unless somebody else holds it.
        for gone in before.difference(&applied) {
            let held = obj.entries.iter().any(|e| !me(e) && e.paths.contains(gone));
            if !held {
                remove_path(&mut obj.value, gone);
            }
        }
        for path in &applied {
            if let Some(v) = get_path(&patch, path) {
                set_path(&mut obj.value, path, v.clone());
            }
        }
        obj.entries.retain(|e| !me(e));
        obj.entries.retain(|e| !e.paths.is_empty());
        obj.entries.push(Entry {
            manager,
            op: Op::Apply,
            sub: sub.map(str::to_owned),
            paths: applied,
        });
        defaults(plural, &mut obj.value);
        if sub.is_none() && existing.is_none() {
            obj.value["metadata"]["creationTimestamp"] = json!("2026-10-16T00:00:00Z");
        }
        self.commit(plural, name, obj, call.dry_run())
    }

    /// Edit an object as somebody else (for a hook): a merge patch by `manager`.
    pub fn merge_as(&mut self, manager: &str, plural: &str, name: &str, patch: Value) {
        let call = Call {
            who: "somebody else",
            method: "PATCH".to_owned(),
            path: format!("/apis/agents.vymalo.com/v1alpha1/namespaces/aap-test/{plural}/{name}"),
            query: BTreeMap::from([("fieldManager".to_owned(), manager.to_owned())]),
            content_type: "application/merge-patch+json".to_owned(),
            body: Some(patch),
            code: 0,
        };
        let (code, body) = self.merge(plural, name, &call);
        assert_eq!(code, 200, "{body}");
    }

    /// Apply an object as somebody else (for a hook): it creates the object when it is not there.
    pub fn apply_as(&mut self, manager: &str, force: bool, plural: &str, name: &str, body: Value) {
        let mut query = BTreeMap::from([("fieldManager".to_owned(), manager.to_owned())]);
        if force {
            query.insert("force".to_owned(), "true".to_owned());
        }
        let call = Call {
            who: "somebody else",
            method: "PATCH".to_owned(),
            path: format!("/apis/agents.vymalo.com/v1alpha1/namespaces/aap-test/{plural}/{name}"),
            query,
            content_type: "application/apply-patch+yaml".to_owned(),
            body: Some(body),
            code: 0,
        };
        let (code, body) = self.apply(plural, name, None, &call);
        assert!(code < 300, "{code} {body}");
    }

    /// Write the status of an object as its controller would (for a hook).
    pub fn apply_status_as(&mut self, manager: &str, plural: &str, name: &str, status: Value) {
        let query = BTreeMap::from([
            ("fieldManager".to_owned(), manager.to_owned()),
            ("force".to_owned(), "true".to_owned()),
        ]);
        let call = Call {
            who: "somebody else",
            method: "PATCH".to_owned(),
            path: format!(
                "/apis/agents.vymalo.com/v1alpha1/namespaces/aap-test/{plural}/{name}/status"
            ),
            query,
            content_type: "application/apply-patch+yaml".to_owned(),
            body: Some(json!({ "metadata": { "name": name }, "status": status })),
            code: 0,
        };
        let (code, body) = self.apply(plural, name, Some("status"), &call);
        assert!(code < 300, "{code} {body}");
    }

    /// Delete an object as somebody else (for a hook).
    pub fn remove(&mut self, plural: &str, name: &str) {
        self.objects.remove(&key(plural, name));
    }
}

/// RFC 7386 on `target`, noting the leaf paths it sets and the paths it removes.
fn merge_patch(
    target: &mut Value,
    patch: &Value,
    at: &mut Path,
    set: &mut BTreeSet<Path>,
    removed: &mut Vec<Path>,
) {
    let Value::Object(p) = patch else {
        *target = patch.clone();
        set.insert(at.clone());
        return;
    };
    if !target.is_object() {
        *target = json!({});
    }
    for (k, v) in p {
        at.push(k.clone());
        if v.is_null() {
            if let Some(map) = target.as_object_mut() {
                map.remove(k);
            }
            removed.push(at.clone());
        } else if v.is_object() {
            merge_patch(&mut target[k.as_str()], v, at, set, removed);
        } else {
            target[k.as_str()] = v.clone();
            set.insert(at.clone());
        }
        at.pop();
    }
}
