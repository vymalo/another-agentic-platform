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
            ..Options::default()
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
            ..Options::default()
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

/// A server that accepts the connection and never answers: the timeouts are what ends the call.
async fn silent_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    url
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_never_answers_ends_at_the_request_timeout() {
    let url = silent_server().await;
    let c = Client::new(
        &url,
        Token::new(SECRET).unwrap(),
        Options {
            system_proxy: false,
            request_timeout: std::time::Duration::from_millis(300),
            ..Options::default()
        },
    )
    .unwrap();
    let started = std::time::Instant::now();
    let error = c.me().await.unwrap_err();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    match error {
        ClientError::Unreachable { class, .. } => {
            assert_eq!(class, "the server did not answer in time");
        }
        other => panic!("{other}"),
    }
}

#[test]
fn the_default_timeouts_are_ten_and_sixty_seconds() {
    let o = Options::default();
    assert_eq!(o.connect_timeout, std::time::Duration::from_secs(10));
    assert_eq!(o.request_timeout, std::time::Duration::from_secs(60));
    assert!(!o.allow_insecure_http && o.system_proxy);
}

#[test]
fn two_spellings_of_a_server_are_one_and_two_servers_are_two() {
    use aap_platform_client::normalize_server as n;
    assert_eq!(
        n("https://API.Example.com:443/"),
        n("https://api.example.com")
    );
    assert_eq!(n("HTTP://127.0.0.1:80"), n("http://127.0.0.1/"));
    assert_ne!(n("https://api.example.com"), n("https://other.example.com"));
    assert_ne!(n("https://api.example.com"), n("http://api.example.com"));
    assert_ne!(
        n("https://api.example.com"),
        n("https://api.example.com:8443")
    );
    assert_ne!(
        n("https://api.example.com/a"),
        n("https://api.example.com/b")
    );
    assert_eq!(n("ftp://x"), None);
    assert_eq!(n("not an address"), None);
}

#[test]
fn clean_replaces_controls_and_bidirectional_controls_and_json_escapes_them() {
    use aap_platform_client::{clean, has_terminal_hazard, json_for_terminal};
    let hostile =
        "a\u{1b}[31mb\nc\u{202e}d\u{2066}e\u{200f}f\u{061c}g\u{85}h\u{7f}i\u{2028}j\u{2029}k";
    let cleaned = clean(hostile);
    assert!(
        !has_terminal_hazard(&cleaned) && !cleaned.contains('\n'),
        "{cleaned:?}"
    );
    assert_eq!(cleaned.chars().count(), hostile.chars().count());
    assert!(has_terminal_hazard(hostile));
    assert!(!has_terminal_hazard("two lines\nand\ta tab"));
    assert!(has_terminal_hazard("cr\r"));

    let value = json!({ "title": hostile, "list": [hostile], hostile: 1 });
    let printed = json_for_terminal(&value);
    assert!(
        !printed
            .chars()
            .any(|c| c != '\n' && (c.is_control() || ('\u{202a}'..='\u{202e}').contains(&c))),
        "{printed:?}"
    );
    for c in ['\u{2066}', '\u{061c}', '\u{200f}', '\u{2028}', '\u{2029}'] {
        assert!(!printed.contains(c), "{c:?} is raw in {printed:?}");
    }
    // The same JSON value.
    assert_eq!(serde_json::from_str::<Value>(&printed).unwrap(), value);
}

#[test]
fn the_marks_of_right_to_left_text_pass_a_file_and_the_joiners_are_never_touched() {
    use aap_platform_client::{clean, clean_multiline, terminal_hazard_kind};
    // LRM, RLM and ALM are ordinary in Arabic and Hebrew: a file holds them, and a YAML string keeps them.
    let text = "\u{05e9}\u{05dc}\u{05d5}\u{05dd}\u{200f} \u{0627}\u{061c}\u{200e}x";
    assert_eq!(terminal_hazard_kind(text), None);
    assert_eq!(clean_multiline(text), text);
    // They are still replaced where a line must be one clean line.
    assert!(!clean(text).contains('\u{200f}'));
    // ZWNJ and ZWJ (Persian, Indic scripts, emoji) are never touched.
    let joiners = "a\u{200c}b\u{200d}c";
    assert_eq!(terminal_hazard_kind(joiners), None);
    assert_eq!(clean_multiline(joiners), joiners);
    assert_eq!(clean(joiners), joiners);
    // What a file refuses says its kind and never the text.
    assert_eq!(
        terminal_hazard_kind("a\u{1b}b"),
        Some("a control character")
    );
    assert_eq!(
        terminal_hazard_kind("a\u{202e}b"),
        Some("a bidirectional embedding, override or isolate")
    );
    assert!(
        terminal_hazard_kind("x\u{2028}y").is_none(),
        "U+2028 is cleaned, not refused"
    );
    assert_eq!(clean_multiline("x\u{2028}y\u{2029}z"), "x y z");
}
