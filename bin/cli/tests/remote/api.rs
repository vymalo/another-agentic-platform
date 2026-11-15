//! The real router of the Platform API over the `Memory` ports, on `127.0.0.1:0`, and the built `aapctl` run against it.
//!
//! The server lives on a runtime of its own that the `Api` owns: a test is plain synchronous code that runs a child process, and
//! the server stops when the test ends. **The child's proxy variables are removed and `NO_PROXY` is set to the loopback**: the
//! machine that wrote these tests sets `HTTPS_PROXY`, and a request to the test's own server must not go through it.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aap_forms::{AgentForm, Defaults, ModelEndpointForm, ToolServerForm, to_resources};
use aap_platform_api::{ApiState, router};
use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
use aap_ports::{
    Agent, AgentDraft, AgentEvents, AgentObjects, ETag, ObjectsError, OfferedSecret, Precondition,
    SharedDraft, SharedKind, Stored,
};
use serde_json::{Value, json};

pub const NS: &str = "another-agentic-system";

/// A bearer token that is long and unlike anything else in an answer: a test that finds any 16 characters of it in a stream has
/// found a leak.
pub const TOKEN: &str = "ey.aapctl-sentinel-9f3c71d2b84e60a5-c1d8e2f7a39b4650.sig";

/// A command stops after this long.
const CHILD_LIMIT: Duration = Duration::from_secs(60);

pub use crate::common::Out;

pub fn defaults_json() -> Value {
    json!({
        "namespace": NS,
        "store": { "cnpg": { "instances": 1, "storage": { "size": "5Gi" } } },
        "a2aBearer": { "name": "agent-a2a", "key": "AGENT_REGISTRY_AGENT_TOKEN" },
        "allowFrom": [
            { "namespaceSelector": { "matchLabels": { "kubernetes.io/metadata.name": NS } } }
        ],
        "security": { "runAsUser": 10001, "runAsGroup": 10001, "fsGroup": 10001 },
        "coder": {
            "gitAuthor": { "name": "bot", "email": "bot@users.noreply.github.com" },
            "allowedRepoHosts": ["github.com"],
            "githubApiUrl": "https://api.github.com",
            "githubMcp": { "sidecar": true, "port": 8082 }
        },
        "folder": {}
    })
}

pub fn defaults() -> Defaults {
    serde_json::from_value(defaults_json()).unwrap()
}

pub fn all_permissions() -> Vec<&'static str> {
    aap_api::permission::ALL.to_vec()
}

// ----------------------------------------------------------------------- a store that can be made to move

type Hook = Box<dyn FnOnce(&MemoryObjects) + Send>;

/// `MemoryObjects` that runs one armed closure right after the next read of an agent or a shared object has been answered to the
/// API, and before the API answers the client: the moment a second writer, or the operator's status write, can get in between a
/// client's read and its write.
#[derive(Clone)]
pub struct Moving {
    inner: MemoryObjects,
    hook: Arc<Mutex<Option<Hook>>>,
}

impl Moving {
    fn fire(&self) {
        let hook = self.hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook(&self.inner);
        }
    }
}

impl AgentObjects for Moving {
    async fn list_agents(&self) -> Result<Vec<Agent>, ObjectsError> {
        self.inner.list_agents().await
    }
    async fn get_agent(&self, name: &str) -> Result<Option<Agent>, ObjectsError> {
        let answer = self.inner.get_agent(name).await;
        self.fire();
        answer
    }
    async fn put_agent(
        &self,
        draft: &AgentDraft,
        when: &Precondition,
    ) -> Result<Agent, ObjectsError> {
        self.inner.put_agent(draft, when).await
    }
    async fn delete_agent(&self, name: &str, when: &ETag) -> Result<(), ObjectsError> {
        self.inner.delete_agent(name, when).await
    }
    async fn list_orphans(&self) -> Result<Vec<Stored>, ObjectsError> {
        self.inner.list_orphans().await
    }
    async fn list_shared(&self, kind: SharedKind) -> Result<Vec<Stored>, ObjectsError> {
        self.inner.list_shared(kind).await
    }
    async fn get_shared(
        &self,
        kind: SharedKind,
        name: &str,
    ) -> Result<Option<Stored>, ObjectsError> {
        let answer = self.inner.get_shared(kind, name).await;
        self.fire();
        answer
    }
    async fn put_shared(
        &self,
        kind: SharedKind,
        draft: &SharedDraft,
        when: &Precondition,
    ) -> Result<Stored, ObjectsError> {
        self.inner.put_shared(kind, draft, when).await
    }
    async fn delete_shared(
        &self,
        kind: SharedKind,
        name: &str,
        when: &ETag,
    ) -> Result<(), ObjectsError> {
        self.inner.delete_shared(kind, name, when).await
    }
    async fn offered_secrets(&self) -> Result<Vec<OfferedSecret>, ObjectsError> {
        self.inner.offered_secrets().await
    }
    async fn agent_events(&self, name: &str) -> Result<AgentEvents, ObjectsError> {
        self.inner.agent_events(name).await
    }
}

// ----------------------------------------------------------------------- the harness

pub struct Api {
    rt: tokio::runtime::Runtime,
    pub url: String,
    pub objects: MemoryObjects,
    pub auth: MemoryAuthenticator,
    hook: Arc<Mutex<Option<Hook>>>,
    pub dir: PathBuf,
}

impl Api {
    /// A server with the keys of `model()` and `tool()` offered, and a token (`TOKEN`) that holds every permission.
    pub fn start(name: &str) -> Self {
        let dir = crate::common::scratch(name);
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let objects = MemoryObjects::in_namespace(NS);
        objects.offer("agent-models", "GATEWAY_API_KEY", "agent-models-es");
        objects.offer("agent-tools", "SEARCH_TOKEN", "agent-tools-es");
        let auth = MemoryAuthenticator::new();
        auth.grant(TOKEN, "tester", all_permissions());
        let hook = Arc::new(Mutex::new(None));
        let moving = Moving {
            inner: objects.clone(),
            hook: hook.clone(),
        };
        let state = ApiState::new(moving, auth.clone(), defaults()).unwrap();
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        rt.spawn(async move {
            axum::serve(listener, router(state)).await.unwrap();
        });
        Self {
            rt,
            url,
            objects,
            auth,
            hook,
            dir,
        }
    }

    /// Run `f` once, after the next read the API makes of an agent or a shared object.
    pub fn after_next_read(&self, f: impl FnOnce(&MemoryObjects) + Send + 'static) {
        *self.hook.lock().unwrap() = Some(Box::new(f));
    }

    /// Await something of the store from a test.
    pub fn block<F: std::future::Future>(&self, f: F) -> F::Output {
        self.rt.block_on(f)
    }

    pub fn agent(&self, name: &str) -> Option<Agent> {
        self.block(self.objects.get_agent(name)).unwrap()
    }

    pub fn model(&self, name: &str) -> Option<Stored> {
        self.block(self.objects.get_shared(SharedKind::Model, name))
            .unwrap()
    }

    pub fn tool(&self, name: &str) -> Option<Stored> {
        self.block(self.objects.get_shared(SharedKind::ToolServer, name))
            .unwrap()
    }

    /// A token for `perms` (a name made up from them), granted on the server.
    pub fn token_with(&self, perms: &[&str]) -> String {
        // A token is a visible string without white space; the permissions are in its text only so that two differ.
        let token = format!("ey.aapctl-limited.{}", perms.join("+").replace(':', "_"));
        self.auth.grant(&token, "limited", perms.iter().copied());
        token
    }

    pub fn config(&self) -> PathBuf {
        self.dir.join("config.yaml")
    }

    /// `aapctl <args> --server <url> --config <dir>/config.yaml`, with `AAPCTL_TOKEN` set to `token`.
    pub fn ctl(&self, args: &[&str], token: Option<&str>) -> Out {
        self.ctl_in(args, token, None)
    }

    pub fn ctl_in(&self, args: &[&str], token: Option<&str>, stdin: Option<&str>) -> Out {
        let config = self.config();
        let mut full: Vec<&str> = args.to_vec();
        full.extend(["--server", &self.url, "--config", config.to_str().unwrap()]);
        run_binary(&full, token, stdin, &self.dir)
    }
}

/// Run the built `aapctl` with the proxy variables removed and `NO_PROXY` set to the loopback, a home and a config home of the test's
/// own, and a limit of one minute.
pub fn run_binary(args: &[&str], token: Option<&str>, stdin: Option<&str>, home: &Path) -> Out {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aapctl"));
    command.args(args);
    for var in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
        "AAPCTL_TOKEN",
        "XDG_CONFIG_HOME",
    ] {
        command.env_remove(var);
    }
    command.env("NO_PROXY", "127.0.0.1,localhost");
    command.env("HOME", home);
    if let Some(token) = token {
        command.env("AAPCTL_TOKEN", token);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut input = child.stdin.take().unwrap();
        if let Some(text) = stdin {
            input.write_all(text.as_bytes()).unwrap();
        }
    }
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_thread = std::thread::spawn(move || {
        let mut s = String::new();
        stdout.read_to_string(&mut s).unwrap();
        s
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        stderr.read_to_string(&mut s).unwrap();
        s
    });
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            started.elapsed() < CHILD_LIMIT,
            "aapctl {args:?} did not finish in {CHILD_LIMIT:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    Out {
        code: status.code().unwrap(),
        stdout: out_thread.join().unwrap(),
        stderr: err_thread.join().unwrap(),
    }
}

// ----------------------------------------------------------------------- the documents of a file

pub fn model_form(name: &str) -> ModelEndpointForm {
    serde_json::from_value(json!({
        "name": name,
        "title": "Gateway",
        "baseUrl": { "value": "https://gateway.example.com/v1" },
        "apiKey": { "secret": "agent-models", "key": "GATEWAY_API_KEY" },
        "models": ["chat-model", "coding-model"]
    }))
    .unwrap()
}

pub fn tool_form(name: &str) -> ToolServerForm {
    serde_json::from_value(json!({
        "name": name,
        "title": "Docs",
        "url": "https://docs.example.com/mcp",
        "optional": true
    }))
    .unwrap()
}

pub fn folder_form(name: &str, title: &str) -> AgentForm {
    folder_form_with(name, title, &[])
}

pub fn folder_form_with(name: &str, title: &str, tools: &[&str]) -> AgentForm {
    serde_json::from_value(json!({
        "name": name,
        "title": title,
        "tags": ["chat"],
        "toolServers": tools,
        "model": { "endpoint": "gateway", "model": "chat-model" },
        "audience": ["*"],
        "agent": { "kind": "folder", "files": { "instructions.md": "---\nname: chat\n---\nYour name is Chat.\n" } }
    }))
    .unwrap()
}

pub fn model_doc(name: &str) -> Value {
    serde_json::to_value(model_form(name).to_resource(NS)).unwrap()
}

pub fn tool_doc(name: &str) -> Value {
    serde_json::to_value(tool_form(name).to_resource(NS)).unwrap()
}

/// The two objects of a folder agent as the dashboard would write them with the deployment's defaults, which are what a create
/// by the API writes.
pub fn agent_docs(name: &str, title: &str) -> Vec<Value> {
    agent_docs_with(name, title, &[])
}

pub fn agent_docs_with(name: &str, title: &str, tools: &[&str]) -> Vec<Value> {
    let form = folder_form_with(name, title, tools);
    let deployment = defaults().deployment_for(form.kind(), name);
    let resources = to_resources(&form, &deployment, NS);
    vec![
        serde_json::to_value(&resources.config).unwrap(),
        serde_json::to_value(&resources.service).unwrap(),
    ]
}

/// A file of these documents in the test's directory.
pub fn file(dir: &Path, name: &str, docs: &[Value]) -> PathBuf {
    let path = dir.join(name);
    crate::common::write_docs(&path, docs);
    path
}

/// Put the model `gateway` and the agent `chat` there, through the API's own ports, the way an earlier apply would have.
pub fn seed_gateway_and_chat(api: &Api, title: &str) {
    let d = model_form("gateway").draft();
    api.block(
        api.objects
            .put_shared(SharedKind::Model, &d, &Precondition::DoesNotExist),
    )
    .unwrap();
    let form = folder_form("chat", title);
    let deployment = defaults().deployment_for(form.kind(), "chat");
    let draft = to_resources(&form, &deployment, NS).draft();
    api.block(api.objects.put_agent(&draft, &Precondition::DoesNotExist))
        .unwrap();
}

/// Drive a future that is ready at once (every call of `MemoryObjects` is: it holds a `std` mutex and awaits nothing) from code that
/// is already inside a runtime and cannot block on it, such as an armed hook.
pub fn now<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, Waker};
    let mut f = std::pin::pin!(f);
    match f.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("the in-memory store awaited something"),
    }
}

/// `{key: value}` of a table's rows: the first column is the key (tests read rows by their name).
pub fn rows(table: &str) -> BTreeMap<String, Vec<String>> {
    let mut lines = table.lines();
    let _header = lines.next();
    lines
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let cols: Vec<String> = l.split_whitespace().map(str::to_owned).collect();
            (cols[0].clone(), cols)
        })
        .collect()
}
