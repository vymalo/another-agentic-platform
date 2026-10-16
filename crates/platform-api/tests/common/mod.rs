//! A router over the `Memory` ports, and the requests the tests make of it.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use aap_api::permission;
use aap_forms::Defaults;
use aap_platform_api::{ApiState, CorsOrigins, router};
use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

pub const NS: &str = "another-agentic-system";

/// A token, a sentinel that must be in no answer and no log line.
pub const TOKEN_SENTINEL: &str = "tok-sentinel-6f2a9d1c";

/// The deployment's defaults, as the example configuration file has them.
pub fn defaults() -> Defaults {
    serde_json::from_value(json!({
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
    }))
    .unwrap()
}

/// The five permissions of the dashboard, and one that is not (an orchestrator role).
pub fn every_permission() -> Vec<&'static str> {
    permission::ALL.to_vec()
}

pub struct Api {
    pub router: Router,
    pub objects: MemoryObjects,
    pub auth: MemoryAuthenticator,
}

impl Api {
    pub fn new() -> Self {
        Self::with(CorsOrigins::none())
    }

    pub fn with(cors: CorsOrigins) -> Self {
        Self::build(
            MemoryObjects::in_namespace(NS),
            MemoryAuthenticator::new(),
            cors,
        )
    }

    /// A router built elsewhere (over another authenticator); `auth` is then a table nobody consults.
    pub fn from_router(router: Router, objects: MemoryObjects) -> Self {
        Self {
            router,
            objects,
            auth: MemoryAuthenticator::new(),
        }
    }

    pub fn build(objects: MemoryObjects, auth: MemoryAuthenticator, cors: CorsOrigins) -> Self {
        Self::over(objects, auth, defaults(), cors)
    }

    /// A router over these ports with these defaults: the same store behind another deployment's configuration.
    pub fn over(
        objects: MemoryObjects,
        auth: MemoryAuthenticator,
        defaults: aap_forms::Defaults,
        cors: CorsOrigins,
    ) -> Self {
        let state = ApiState::new(objects.clone(), auth.clone(), defaults)
            .unwrap()
            .with_cors(cors);
        Self {
            router: router(state),
            objects,
            auth,
        }
    }

    /// A token for `subject` that holds `permissions`.
    pub fn token(&self, subject: &str, permissions: &[&str]) -> String {
        let token = format!("token-of-{subject}");
        self.auth
            .grant(&token, subject, permissions.iter().copied());
        token
    }

    /// A token that holds all five permissions.
    pub fn admin(&self) -> String {
        self.token("admin", &every_permission())
    }

    pub fn request(&self, method: Method, path: &str) -> Call<'_> {
        Call {
            api: self,
            method,
            path: path.to_owned(),
            token: None,
            headers: Vec::new(),
            body: None,
            content_type: None,
        }
    }

    pub fn get(&self, path: &str) -> Call<'_> {
        self.request(Method::GET, path)
    }

    pub fn put(&self, path: &str) -> Call<'_> {
        self.request(Method::PUT, path)
    }

    pub fn delete(&self, path: &str) -> Call<'_> {
        self.request(Method::DELETE, path)
    }
}

pub struct Call<'a> {
    api: &'a Api,
    method: Method,
    path: String,
    token: Option<String>,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    content_type: Option<String>,
}

impl Call<'_> {
    #[must_use]
    pub fn token(mut self, token: &str) -> Self {
        self.token = Some(token.to_owned());
        self
    }

    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    #[must_use]
    pub fn json(mut self, body: &Value) -> Self {
        self.body = Some(serde_json::to_vec(body).unwrap());
        self.content_type
            .get_or_insert_with(|| "application/json".to_owned());
        self
    }

    #[must_use]
    pub fn raw(mut self, body: &[u8], content_type: Option<&str>) -> Self {
        self.body = Some(body.to_vec());
        self.content_type = content_type.map(str::to_owned);
        self
    }

    #[must_use]
    pub fn create(self) -> Self {
        self.header("if-none-match", "*")
    }

    #[must_use]
    pub fn replace(self, etag: &str) -> Self {
        self.header("if-match", etag)
    }

    pub async fn send(self) -> Reply {
        let mut builder = Request::builder().method(self.method).uri(&self.path);
        if let Some(token) = &self.token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        if let Some(ct) = &self.content_type {
            builder = builder.header("content-type", ct);
        }
        for (k, v) in &self.headers {
            builder = builder.header(k, v);
        }
        let body = self.body.map_or_else(Body::empty, Body::from);
        let response = self
            .api
            .router
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap();
        let (parts, body) = response.into_parts();
        let body = axum::body::to_bytes(body, 8 << 20).await.unwrap().to_vec();
        Reply {
            status: parts.status,
            headers: parts.headers,
            body,
        }
    }
}

#[derive(Debug)]
pub struct Reply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn code(&self) -> u16 {
        self.status.as_u16()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body)
            .unwrap_or_else(|e| panic!("not JSON ({e}): {}", self.text()))
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    /// The `ETag` header, as a client echoes it.
    pub fn etag(&self) -> String {
        self.header("etag")
            .unwrap_or_else(|| panic!("no ETag on {self:?}"))
            .to_owned()
    }

    pub fn is_problem(&self, slug: &str) -> bool {
        self.header("content-type") == Some("application/problem+json")
            && self.json()["type"] == format!("urn:vymalo:agents:problem:{slug}")
    }
}

// ----------------------------------------------------------------- the bodies

pub fn model_body(name: &str) -> Value {
    json!({
        "name": name,
        "title": "Gateway",
        "baseUrl": { "value": "https://gateway.example.com/v1" },
        "apiKey": { "secret": "agent-models", "key": "GATEWAY_API_KEY" },
        "models": ["chat-model", "coding-model"]
    })
}

pub fn tool_body(name: &str) -> Value {
    json!({
        "name": name,
        "title": "Web search",
        "url": "https://search.example.com/mcp",
        "headers": { "Authorization": { "prefix": "Bearer ", "secret": { "secret": "agent-tools", "key": "SEARCH_TOKEN" } } },
        "tools": [],
        "optional": true
    })
}

/// A tool server that names no secret.
pub fn open_tool_body(name: &str) -> Value {
    json!({ "name": name, "title": "Docs", "url": "https://docs.example.com/mcp", "optional": true })
}

pub fn folder_body(name: &str) -> Value {
    json!({
        "name": name,
        "title": "Chat",
        "tags": ["chat"],
        "model": { "endpoint": "gateway", "model": "chat-model" },
        "audience": ["*"],
        "agent": { "kind": "folder", "files": { "instructions.md": "---\nname: chat\n---\nYour name is Chat.\n" } }
    })
}

pub fn coder_body(name: &str) -> Value {
    json!({
        "name": name,
        "title": "Coder",
        "model": { "endpoint": "gateway", "model": "coding-model" },
        "toolServers": ["docs"],
        "audience": [format!("agent.use:{name}")],
        "agent": {
            "kind": "coder",
            "github": {
                "appId": "Iv1.example",
                "privateKey": { "secret": "github-app", "key": "private-key.pem" },
                "owners": ["octocat"]
            },
            "workers": 2
        }
    })
}

/// The keys the bodies above name, offered by ExternalSecrets.
pub fn offer_everything(objects: &MemoryObjects) {
    objects.offer("agent-models", "GATEWAY_API_KEY", "agent-models-es");
    objects.offer("agent-tools", "SEARCH_TOKEN", "agent-tools-es");
    objects.offer("github-app", "private-key.pem", "github-app-es");
}

impl Api {
    /// An API with the keys offered and the model `gateway` and the tool server `docs` made, and a token that holds
    /// everything.
    pub async fn seeded() -> (Self, String) {
        let api = Self::new();
        offer_everything(&api.objects);
        let token = api.admin();
        let r = api
            .put("/v1/models/gateway")
            .token(&token)
            .create()
            .json(&model_body("gateway"))
            .send()
            .await;
        assert_eq!(r.code(), 201, "{}", r.text());
        let r = api
            .put("/v1/tool-servers/docs")
            .token(&token)
            .create()
            .json(&open_tool_body("docs"))
            .send()
            .await;
        assert_eq!(r.code(), 201, "{}", r.text());
        (api, token)
    }
}
