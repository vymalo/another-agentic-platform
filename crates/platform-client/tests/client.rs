//! The client against the real router over the `Memory` ports, on a loopback socket.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use aap_forms::Defaults;
use aap_platform_api::{ApiState, router};
use aap_platform_client::{Client, ClientError, Collection, Condition, Options, Token};
use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
use serde_json::{Value, json};

const NS: &str = "another-agentic-system";
const SECRET: &str = "aapctl-client-test-token-5d8e1f7a93c2b604";

fn defaults() -> Defaults {
    serde_json::from_value(json!({
        "namespace": NS,
        "store": { "cnpg": { "instances": 1, "storage": { "size": "5Gi" } } },
        "a2aBearer": { "name": "agent-a2a", "key": "AGENT_REGISTRY_AGENT_TOKEN" },
        "allowFrom": [],
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

struct Server {
    url: String,
}

async fn serve(permissions: &[&str]) -> Server {
    let objects = MemoryObjects::in_namespace(NS);
    objects.offer("agent-models", "GATEWAY_API_KEY", "agent-models-es");
    let auth = MemoryAuthenticator::new();
    auth.grant(SECRET, "tester", permissions.iter().copied());
    let state = ApiState::new(objects.clone(), auth, defaults()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, router(state)).await.unwrap();
    });
    Server { url }
}

fn client(url: &str) -> Client {
    Client::new(
        url,
        Token::new(SECRET).unwrap(),
        Options {
            allow_insecure_http: false,
            system_proxy: false,
        },
    )
    .unwrap()
}

fn model(name: &str) -> Value {
    json!({
        "name": name,
        "title": "Gateway",
        "baseUrl": { "value": "https://gateway.example.com/v1" },
        "apiKey": { "secret": "agent-models", "key": "GATEWAY_API_KEY" },
        "models": ["chat-model"]
    })
}

fn all() -> Vec<&'static str> {
    aap_api::permission::ALL.to_vec()
}

fn problem(error: ClientError) -> aap_platform_client::Problem {
    match error {
        ClientError::Problem(p) => *p,
        other => panic!("not a problem: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_carry_their_precondition() {
    let server = serve(&all()).await;
    let c = client(&server.url);

    let created = c
        .put(
            Collection::Models,
            "gateway",
            &model("gateway"),
            &Condition::Create,
        )
        .await
        .unwrap();
    assert_eq!(created.status, 201);
    let tag = created.etag.clone().expect("an ETag");

    // A second create is a conflict, whatever the body: the client never turns it into a replace.
    let again = problem(
        c.put(
            Collection::Models,
            "gateway",
            &model("gateway"),
            &Condition::Create,
        )
        .await
        .unwrap_err(),
    );
    assert_eq!(
        (again.status, again.slug.as_deref()),
        (409, Some("already-exists"))
    );

    let mut changed = model("gateway");
    changed["title"] = json!("Gateway 2");
    let stale = problem(
        c.put(
            Collection::Models,
            "gateway",
            &changed,
            &Condition::Replace("\"not-the-tag\"".to_owned()),
        )
        .await
        .unwrap_err(),
    );
    assert_eq!((stale.status, stale.slug.as_deref()), (412, Some("stale")));

    let replaced = c
        .put(
            Collection::Models,
            "gateway",
            &changed,
            &Condition::Replace(tag),
        )
        .await
        .unwrap();
    assert_eq!(replaced.status, 200);
    let tag = replaced.etag.unwrap();

    assert_eq!(
        c.delete(Collection::Models, "gateway", &tag)
            .await
            .unwrap()
            .status,
        204
    );
    let gone = problem(c.get(Collection::Models, "gateway").await.unwrap_err());
    assert_eq!(gone.status, 404);

    let trace = c.take_trace().join("\n");
    assert!(
        trace.contains("> PUT /v1/models/gateway  If-None-Match: *"),
        "{trace}"
    );
    assert!(
        trace.contains("> DELETE /v1/models/gateway  If-Match: "),
        "{trace}"
    );
    assert!(!trace.contains(SECRET), "{trace}");
}

#[tokio::test(flavor = "multi_thread")]
async fn problems_are_read_into_their_members() {
    let server = serve(&["platform:agents.read", "platform:models.write"]).await;
    let c = client(&server.url);

    // 403 names the permission.
    let forbidden = problem(
        c.put(
            Collection::Models,
            "gateway",
            &model("gateway"),
            &Condition::Create,
        )
        .await
        .unwrap_err(),
    );
    assert_eq!(forbidden.status, 403);
    assert_eq!(
        forbidden.permission.as_deref(),
        Some("platform:secrets.pick")
    );

    // 422 names the form field.
    let mut bad = model("gateway");
    bad["baseUrl"] = json!({ "value": "not a url" });
    let server = serve(&all()).await;
    let invalid = problem(
        client(&server.url)
            .put(Collection::Models, "gateway", &bad, &Condition::Create)
            .await
            .unwrap_err(),
    );
    assert_eq!(invalid.status, 422);
    assert!(
        invalid
            .errors
            .iter()
            .any(|e| e.field.starts_with("baseUrl")),
        "{invalid:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_token_in_any_error_or_debug() {
    // Nothing listens on port 1.
    let c = client("http://127.0.0.1:1");
    let error = c.me().await.unwrap_err();
    let shown = format!("{error} {error:?}");
    assert!(!shown.contains(SECRET), "{shown}");
    assert_eq!(
        error.to_string(),
        "cannot reach http://127.0.0.1:1: the connection failed"
    );
    assert!(!format!("{c:?}").contains(SECRET));
    assert!(!format!("{:?}", Token::new(SECRET).unwrap()).contains(SECRET));
    let trace = c.take_trace().join("\n");
    assert!(!trace.contains(SECRET));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_address_that_would_leak_or_cheat_is_refused() {
    let t = || Token::new(SECRET).unwrap();
    for (address, why) in [
        ("http://api.example.com", "in the clear"),
        ("ftp://api.example.com", "not http"),
        ("https://user:pw@api.example.com", "password"),
        ("https://api.example.com/?code=1", "query"),
        ("https://api.example.com/#x", "fragment"),
        ("not an address", "not an address"),
    ] {
        let error = Client::new(address, t(), Options::default()).unwrap_err();
        assert!(matches!(error, ClientError::Config(_)), "{address}");
        let text = error.to_string();
        assert!(text.contains(why), "{address}: {text}");
        assert!(!text.contains("pw") && !text.contains("code=1"), "{text}");
    }
    // Allowed on purpose, and for a loopback address.
    Client::new(
        "http://api.example.com",
        t(),
        Options {
            allow_insecure_http: true,
            system_proxy: false,
        },
    )
    .unwrap();
    Client::new("http://127.0.0.1:8080", t(), Options::default()).unwrap();
    Client::new("http://[::1]:8080", t(), Options::default()).unwrap();
    Client::new("http://localhost", t(), Options::default()).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_that_is_not_a_segment_is_refused_before_anything_is_sent() {
    let c = client("http://127.0.0.1:1");
    for name in ["", "..", ".", "a/b", "a b", "a?x=1", "a%2fb"] {
        let error = c.get(Collection::Agents, name).await.unwrap_err();
        assert!(matches!(error, ClientError::Config(_)), "{name:?}: {error}");
    }
    assert!(c.take_trace().is_empty(), "something was sent");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redirect_is_not_followed() {
    let app = axum::Router::new().route(
        "/v1/me",
        axum::routing::get(|| async {
            (
                axum::http::StatusCode::FOUND,
                [("location", "http://127.0.0.1:1/elsewhere")],
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let p = problem(client(&url).me().await.unwrap_err());
    assert_eq!(p.status, 302);
    assert_eq!(p.slug, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_problem_that_is_not_the_apis_is_never_quoted() {
    let app = axum::Router::new().route(
        "/v1/me",
        axum::routing::get(|| async {
            (
                axum::http::StatusCode::BAD_GATEWAY,
                "<html>secret-proxy-page \u{1b}[31m</html>",
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let p = problem(client(&url).me().await.unwrap_err());
    assert_eq!(p.status, 502);
    assert!(p.slug.is_none() && p.detail.is_none());
    assert!(!format!("{p:?}").contains("secret-proxy-page"));
}

#[test]
fn a_token_with_white_space_or_a_line_break_is_refused() {
    for bad in ["", "   ", "a b", "a\nb", "a\r\nX-Evil: 1", "tökén"] {
        let error = Token::new(bad).unwrap_err().to_string();
        assert!(!error.contains("X-Evil"), "{error}");
    }
    assert_eq!(Token::new("  abc\n").unwrap().expose(), "abc");
}
