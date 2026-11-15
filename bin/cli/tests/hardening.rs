//! `aapctl` over the Platform API, after the review of K9b-1: each test says the rule it holds. Same harness as `remote.rs` (the
//! built binary against the real router over the `Memory` ports), plus a scripted server (`remote/stub.rs`) for the answers the
//! router never gives.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "remote/api.rs"]
mod api;
mod common;
#[path = "remote/stub.rs"]
mod stub;

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use aap_forms::{AgentForm, to_resources};
use aap_platform_client::{Client, Collection, Options, Token};
use aap_ports::{AgentObjects, Kind, Precondition, SharedKind};
use api::{
    Api, NS, TOKEN, agent_docs, agent_docs_with, defaults, file, folder_form, model_doc,
    model_form, run_binary, run_binary_env, seed_gateway_and_chat, tool_doc,
};
use common::set;
use serde_json::{Value, json};
use stub::{Answer, Stub, json_headers, problem};

fn client(api: &Api) -> Client {
    Client::new(
        &api.url,
        Token::new(TOKEN).unwrap(),
        Options {
            system_proxy: false,
            ..Options::default()
        },
    )
    .unwrap()
}

fn api_body(api: &Api, collection: Collection, name: &str) -> Value {
    api.block(client(api).get(collection, name))
        .unwrap()
        .json()
        .unwrap()
}

fn api_etag(api: &Api, name: &str) -> String {
    api.block(client(api).get(Collection::Agents, name))
        .unwrap()
        .etag
        .unwrap()
}

fn seed_gateway(api: &Api) {
    api.block(api.objects.put_shared(
        SharedKind::Model,
        &model_form("gateway").draft(),
        &Precondition::DoesNotExist,
    ))
    .unwrap();
}

/// The documents of an agent with no `metadata.namespace`: the API's is meant, and the check of it (which reads `/v1/info`) is not made.
fn without_namespaces(mut docs: Vec<Value>) -> Vec<Value> {
    for d in &mut docs {
        set(d, "/metadata/namespace", Value::Null);
    }
    docs
}

fn cfg(api: &Api) -> String {
    api.config().to_str().unwrap().to_owned()
}

/// A listener that counts the connections made to it and drops them: a proxy that is never to be asked, or that must be.
fn counting_listener() -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
            seen.fetch_add(1, Ordering::SeqCst);
        }
    });
    (url, count)
}

const HOSTILE: &str = "red\u{1b}[31m evil\u{202e}gnp.exe\u{85}x\u{7f}y\u{2066}z";

fn has_hazard(text: &str) -> bool {
    text.chars().any(|c| {
        (c.is_control() && c != '\n' && c != '\t')
            || ('\u{202a}'..='\u{202e}').contains(&c)
            || ('\u{2066}'..='\u{2069}').contains(&c)
            || matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}')
    })
}

// ------------------------------------------------------------------------------------------ A1: --if-match

#[test]
fn if_match_changes_only_the_tag_the_write_carries() {
    let api = Api::start("if-match-checks");
    seed_gateway_and_chat(&api, "Chat");
    let tag = api_etag(&api, "chat");

    // The deployment part of the file differs from the object's: refused, as without a tag, and nothing is written.
    let mut docs = agent_docs("chat", "Chat");
    set(&mut docs[0], "/spec/security/runAsUser", json!(20000));
    let other = file(&api.dir, "other.yaml", &docs);
    let out = api.ctl(
        &[
            "apply",
            "-f",
            other.to_str().unwrap(),
            "--if-match",
            &tag,
            "--verbose",
        ],
        Some(TOKEN),
    );
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("security.runAsUser"), "{}", out.stderr);
    assert!(!out.stderr.contains("> PUT"), "{}", out.stderr);
    assert_eq!(api_etag(&api, "chat"), tag);

    // The same object, unchanged: nothing is written, whatever tag is given.
    let same = file(&api.dir, "same.yaml", &agent_docs("chat", "Chat"));
    let out = api.ctl(
        &[
            "apply",
            "-f",
            same.to_str().unwrap(),
            "--if-match",
            &tag,
            "--verbose",
        ],
        Some(TOKEN),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "agent/chat unchanged\n");
    assert!(!out.stderr.contains("> PUT"), "{}", out.stderr);

    // A change is written with the tag given, which is what the trace shows.
    let changed = file(&api.dir, "changed.yaml", &agent_docs("chat", "Renamed"));
    let out = api.ctl(
        &[
            "apply",
            "-f",
            changed.to_str().unwrap(),
            "--if-match",
            &tag,
            "--verbose",
        ],
        Some(TOKEN),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stderr
            .contains(&format!("> PUT /v1/agents/chat  If-Match: {tag}")),
        "{}",
        out.stderr
    );

    // A tag for an object that is not there is a 404 of the read, not a create.
    let missing = file(&api.dir, "missing.yaml", &agent_docs("ghost", "Ghost"));
    let out = api.ctl(
        &["apply", "-f", missing.to_str().unwrap(), "--if-match", &tag],
        Some(TOKEN),
    );
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("404"), "{}", out.stderr);
    assert!(api.agent("ghost").is_none());
}

// ------------------------------------------------------------------------------------------ A2: namespaces

#[test]
fn every_document_says_the_apis_namespace_or_none_and_a_service_agrees_with_its_config() {
    let api = Api::start("namespaces");

    // A service that says none and a config in another namespace: the config's is never the API's.
    let mut docs = agent_docs("chat", "Chat");
    set(
        &mut docs[0],
        "/metadata/namespace",
        json!("another-namespace"),
    );
    set(&mut docs[1], "/metadata/namespace", Value::Null);
    docs.push(model_doc("gateway"));
    let path = file(&api.dir, "config-elsewhere.yaml", &docs);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("AgentConfig/chat"), "{}", out.stderr);
    assert!(out.stderr.contains("another-namespace"), "{}", out.stderr);
    assert!(out.stderr.contains(NS), "{}", out.stderr);
    assert!(api.agent("chat").is_none() && api.model("gateway").is_none());

    // Two namespaces for one agent: said before anything is asked of the server.
    let mut docs = agent_docs("chat", "Chat");
    set(&mut docs[0], "/metadata/namespace", json!("ns-b"));
    set(&mut docs[1], "/metadata/namespace", json!("ns-a"));
    let path = file(&api.dir, "two.yaml", &docs);
    let before = api.request_count();
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(
        out.stderr.contains("ns-a")
            && out.stderr.contains("ns-b")
            && out.stderr.contains("one namespace"),
        "{}",
        out.stderr
    );
    assert_eq!(api.request_count(), before, "a request was sent");

    // A model in another namespace is refused the same way (exit 2, naming it and both namespaces).
    let mut model = model_doc("gateway");
    set(
        &mut model,
        "/metadata/namespace",
        json!("another-namespace"),
    );
    let path = file(&api.dir, "model.yaml", &[model]);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(
        out.stderr.contains("ModelEndpoint/gateway"),
        "{}",
        out.stderr
    );

    // None at all, or the API's on one of the two, is fine.
    seed_gateway(&api);
    let mut docs = agent_docs("chat", "Chat");
    set(&mut docs[0], "/metadata/namespace", Value::Null);
    set(&mut docs[1], "/metadata/namespace", json!(NS));
    let path = file(&api.dir, "fine.yaml", &docs);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "agent/chat created\n");
}

// ------------------------------------------------------------------------------------------ A3: version skew

/// A stub that serves the real detail of `chat` as it is after `mutate`, and answers every write with a 500 that would show.
fn serve_agent_detail(api: &Api, mutate: impl Fn(&mut Value)) -> Stub {
    let mut body = api_body(api, Collection::Agents, "chat");
    mutate(&mut body);
    let body = body.to_string();
    let tag = api_etag(api, "chat");
    Stub::start(move |line, _| match line {
        "GET /v1/agents/chat" => {
            let mut headers = json_headers();
            headers.push(("ETag".to_owned(), tag.clone()));
            Answer::Say(200, headers, body.clone())
        }
        _ => Answer::Say(500, json_headers(), problem(500, "internal", "no", "")),
    })
}

#[test]
fn a_form_this_binary_does_not_read_whole_is_not_written_from() {
    let api = Api::start("skew-forms");
    seed_gateway_and_chat(&api, "Chat");
    let new_title = file(
        &api.dir,
        "new.yaml",
        &without_namespaces(agent_docs("chat", "A new title")),
    );
    let run = |stub: &Stub| {
        run_binary(
            &[
                "apply",
                "-f",
                new_title.to_str().unwrap(),
                "--server",
                &stub.url,
                "--config",
                &cfg(&api),
            ],
            Some(TOKEN),
            None,
            &api.dir,
        )
    };

    // A newer server's form has a member this binary does not know: a write from the form would reset it.
    let newer = serve_agent_detail(&api, |b| {
        b["representation"]["form"]["form"]["aMemberOfALaterRelease"] =
            json!("the-value-is-not-printed");
    });
    let out = run(&newer);
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(
        out.stderr.contains("aMemberOfALaterRelease"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr
            .contains("use the aapctl of the server's release"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("agent/chat"), "{}", out.stderr);
    assert!(
        !out.stderr.contains("the-value-is-not-printed"),
        "{}",
        out.stderr
    );
    assert_eq!(newer.count("PUT"), 0, "{:?}", newer.lines());

    // An older server does not send a member this binary writes (it would fill in its own default and compare against that).
    let older = serve_agent_detail(&api, |b| {
        b["representation"]["form"]["form"]
            .as_object_mut()
            .unwrap()
            .remove("suspend");
    });
    let out = run(&older);
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(out.stderr.contains("suspend"), "{}", out.stderr);
    assert_eq!(older.count("PUT"), 0, "{:?}", older.lines());

    // An optional member that is absent is not a difference: the file is the object, so nothing is written and nothing is refused.
    let same = file(
        &api.dir,
        "same.yaml",
        &without_namespaces(agent_docs("chat", "Chat")),
    );
    let optional_absent = serve_agent_detail(&api, |b| {
        b["representation"]["form"]["form"]
            .as_object_mut()
            .unwrap()
            .remove("description");
    });
    let out = run_binary(
        &[
            "apply",
            "-f",
            same.to_str().unwrap(),
            "--server",
            &optional_absent.url,
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "agent/chat unchanged\n");

    // The deployment part is read the same way.
    let deployment = serve_agent_detail(&api, |b| {
        b["representation"]["form"]["deployment"]["fromTheFuture"] = json!({ "x": 1 });
    });
    let out = run(&deployment);
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(
        out.stderr.contains("deployment.fromTheFuture"),
        "{}",
        out.stderr
    );
}

#[test]
fn a_model_form_of_another_release_is_not_written_from_either() {
    let api = Api::start("skew-model");
    seed_gateway(&api);
    let mut body = api_body(&api, Collection::Models, "gateway");
    body["representation"]["form"]["laterMember"] = json!(1);
    let body = body.to_string();
    let stub = Stub::start(move |line, _| match line {
        "GET /v1/models/gateway" => {
            let mut h = json_headers();
            h.push(("ETag".to_owned(), "\"x\"".to_owned()));
            Answer::Say(200, h, body.clone())
        }
        _ => Answer::Say(500, json_headers(), problem(500, "internal", "no", "")),
    });
    let mut changed = model_doc("gateway");
    set(&mut changed, "/metadata/namespace", Value::Null);
    set(&mut changed, "/spec/title", json!("Changed"));
    let path = file(&api.dir, "model.yaml", &[changed]);
    let out = run_binary(
        &[
            "apply",
            "-f",
            path.to_str().unwrap(),
            "--server",
            &stub.url,
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(out.stderr.contains("laterMember"), "{}", out.stderr);
    assert_eq!(stub.count("PUT"), 0);
}

#[test]
fn defaults_of_another_release_stop_a_create_before_it_writes() {
    let api = Api::start("skew-info");
    let info = api.block(client(&api).info()).unwrap().json().unwrap();
    let path = file(&api.dir, "chat.yaml", &agent_docs("chat", "Chat"));
    for (what, mutate, member) in [
        (
            "older",
            Box::new(|v: &mut Value| {
                v["defaults"]
                    .as_object_mut()
                    .unwrap()
                    .remove("maxAccessTier");
            }) as Box<dyn Fn(&mut Value)>,
            "maxAccessTier",
        ),
        (
            "newer",
            Box::new(|v: &mut Value| v["defaults"]["fromTheFuture"] = json!({ "a": 1 })),
            "fromTheFuture",
        ),
    ] {
        let mut served = info.clone();
        mutate(&mut served);
        let served = served.to_string();
        let stub = Stub::start(move |line, _| match line {
            "GET /v1/agents/chat" => {
                Answer::Say(404, json_headers(), problem(404, "not-found", "no", ""))
            }
            "GET /v1/info" => Answer::Say(200, json_headers(), served.clone()),
            _ => Answer::Say(500, json_headers(), problem(500, "internal", "no", "")),
        });
        let out = run_binary(
            &[
                "apply",
                "-f",
                path.to_str().unwrap(),
                "--server",
                &stub.url,
                "--config",
                &cfg(&api),
            ],
            Some(TOKEN),
            None,
            &api.dir,
        );
        assert_eq!(out.code, 5, "{what}: {}", out.stderr);
        assert!(
            out.stderr.contains(&format!("defaults.{member}")),
            "{what}: {}",
            out.stderr
        );
        assert!(
            out.stderr.contains("aapctl of the server's release"),
            "{what}: {}",
            out.stderr
        );
        assert_eq!(stub.count("PUT"), 0, "{what}: {:?}", stub.lines());
    }
}

#[test]
fn a_coder_agent_reads_back_as_the_server_sent_it() {
    // No false alarm for the members only a coder has, and for the optional ones that are absent.
    let api = Api::start("skew-coder");
    seed_gateway(&api);
    api.objects
        .offer("github-app", "private-key.pem", "github-app-es");
    let form: AgentForm = serde_json::from_value(json!({
        "name": "coder-me",
        "title": "Coder",
        "tags": ["coding"],
        "toolServers": [],
        "model": { "endpoint": "gateway", "model": "coding-model" },
        "audience": ["agent.use:coder-me"],
        "agent": {
            "kind": "coder",
            "github": {
                "appId": "Iv1.example",
                "privateKey": { "secret": "github-app", "key": "private-key.pem" },
                "owners": ["octocat"]
            },
            "workers": 2,
            "prDraft": true
        }
    }))
    .unwrap();
    let resources = to_resources(
        &form,
        &defaults().deployment_for(form.kind(), "coder-me"),
        NS,
    );
    let docs = vec![
        serde_json::to_value(&resources.config).unwrap(),
        serde_json::to_value(&resources.service).unwrap(),
    ];
    let path = file(&api.dir, "coder.yaml", &docs);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "agent/coder-me created\n");
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "agent/coder-me unchanged\n");
}

// ------------------------------------------------------------------------------------------ A4: the token follows its server

#[test]
fn the_saved_token_goes_only_to_the_server_it_was_saved_for() {
    let api = Api::start("token-server");
    let out = api.ctl_in(&["login", "--token-stdin"], None, Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let config = cfg(&api);

    // Another server by --server: the saved token is not sent, and the person is told why.
    let other = Stub::answering(200, "{}");
    let out = run_binary(
        &["get", "agents", "--server", &other.url, "--config", &config],
        None,
        None,
        &api.dir,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("saved in the configuration file is for"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains(&api.url), "{}", out.stderr);
    assert!(out.stderr.contains("AAPCTL_TOKEN"), "{}", out.stderr);
    assert!(other.seen().is_empty(), "{:?}", other.seen());
    assert!(!out.stderr.contains(TOKEN));

    // The same server in another spelling is the same server.
    for spelling in [
        format!("{}/", api.url),
        api.url.replace("http://", "HTTP://"),
    ] {
        let out = run_binary(
            &["get", "agents", "--server", &spelling, "--config", &config],
            None,
            None,
            &api.dir,
        );
        assert_eq!(out.code, 0, "{spelling}: {}", out.stderr);
    }

    // A token the person put in the environment is theirs to send wherever they point it.
    let out = run_binary(
        &["get", "agents", "--server", &other.url, "--config", &config],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let seen = other.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(
        seen[0].authorization.as_deref(),
        Some(&*format!("Bearer {TOKEN}"))
    );
}

// ------------------------------------------------------------------------------------------ A5: no proxy for loopback http

#[test]
fn a_plain_http_loopback_server_is_never_reached_through_a_proxy() {
    let api = Api::start("proxy-rule");
    let (proxy, connections) = counting_listener();
    let environment = [
        ("HTTP_PROXY", proxy.as_str()),
        ("http_proxy", proxy.as_str()),
        ("HTTPS_PROXY", proxy.as_str()),
        ("ALL_PROXY", proxy.as_str()),
        // Nothing is excepted: the rule is the client's, not the environment's.
        ("NO_PROXY", ""),
    ];
    let out = run_binary_env(
        &["whoami", "--server", &api.url, "--config", &cfg(&api)],
        Some(TOKEN),
        None,
        &api.dir,
        &environment,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "the bearer token went to the proxy"
    );

    // What still uses the proxy and needs no TLS: an `http://` address that is not a loopback one, allowed on purpose.
    let out = run_binary_env(
        &[
            "get",
            "agents",
            "--server",
            "http://api.example.test:8080",
            "--insecure-http",
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
        &environment,
    );
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(
        connections.load(Ordering::SeqCst) >= 1,
        "the system proxy is not used for a remote address"
    );
}

// ------------------------------------------------------------------------------------------ A6: annotations and labels

#[test]
fn an_annotation_or_a_label_the_api_would_not_write_is_refused_naming_the_key_and_never_the_value()
{
    let api = Api::start("metadata");
    for (name, pointer, value, key) in [
        (
            "annotation",
            "/metadata/annotations",
            json!({ "agents.vymalo.com/note": "the-value-is-not-printed" }),
            "agents.vymalo.com/note",
        ),
        (
            "label",
            "/metadata/labels",
            json!({ "team": "the-value-is-not-printed" }),
            "team",
        ),
        (
            "platform label",
            "/metadata/labels",
            json!({ "agents.vymalo.com/team": "the-value-is-not-printed" }),
            "agents.vymalo.com/team",
        ),
    ] {
        let mut model = model_doc("gateway");
        set(&mut model, pointer, value);
        let path = file(&api.dir, "metadata.yaml", &[model]);
        let out = api.ctl(
            &["apply", "-f", path.to_str().unwrap(), "--verbose"],
            Some(TOKEN),
        );
        assert_eq!(out.code, 2, "{name}: {}", out.stderr);
        assert!(out.stderr.contains(key), "{name}: {}", out.stderr);
        assert!(
            !out.stderr.contains("the-value-is-not-printed"),
            "{name}: {}",
            out.stderr
        );
        assert!(!out.stderr.contains("> "), "{name}: a request was sent");
        assert!(api.model("gateway").is_none(), "{name}");
    }
}

#[test]
fn an_export_is_read_back_unchanged_even_when_the_agent_holds_labels_and_annotations_the_api_does_not_write()
 {
    let api = Api::start("export-metadata");
    seed_gateway_and_chat(&api, "Chat");
    api.dress_agents(
        ("team", "p-the-label-value"),
        ("agents.vymalo.com/note", "p-the-annotation-value"),
    );
    // The API serves them: they are the person's own and the platform's.
    let served = api
        .block(client(&api).agent_yaml("chat"))
        .unwrap()
        .text()
        .unwrap();
    assert!(
        served.contains("team") && served.contains("agents.vymalo.com/note"),
        "{served}"
    );

    let out = api.ctl(&["export", "agent", "chat"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        !out.stdout.contains("team") && !out.stdout.contains("p-the-"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("app.kubernetes.io/managed-by"),
        "{}",
        out.stdout
    );
    assert!(out.stderr.contains("label team"), "{}", out.stderr);
    assert!(
        out.stderr.contains("annotation agents.vymalo.com/note"),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains("p-the-"), "{}", out.stderr);

    let exported = api.dir.join("exported.yaml");
    std::fs::write(&exported, &out.stdout).unwrap();
    let back = api.ctl(&["apply", "-f", exported.to_str().unwrap()], Some(TOKEN));
    assert_eq!(back.code, 0, "{}", back.stderr);
    assert_eq!(back.stdout, "agent/chat unchanged\n");
}

// ------------------------------------------------------------------------------------------ A7: what reaches the terminal

#[test]
fn what_the_server_says_never_reaches_the_terminal_as_a_control() {
    let api = Api::start("terminal");
    seed_gateway_and_chat(&api, HOSTILE);
    let body = api_body(&api, Collection::Agents, "chat");
    assert_eq!(body["title"], HOSTILE, "the store holds the hostile title");

    // -o json: the same JSON value, and no raw control in the bytes.
    let out = api.ctl(&["get", "agent", "chat", "-o", "json"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!has_hazard(&out.stdout), "{:?}", out.stdout);
    assert!(!out.stdout.contains('\u{1b}') && !out.stdout.contains('\u{202e}'));
    assert_eq!(serde_json::from_str::<Value>(&out.stdout).unwrap(), body);
    let list = api.ctl(&["get", "agents", "-o", "json"], Some(TOKEN));
    assert!(!has_hazard(&list.stdout), "{:?}", list.stdout);

    // The other outputs.
    for args in [
        vec!["get", "agent", "chat", "-o", "yaml"],
        vec!["get", "agents"],
        vec!["get", "agents", "-o", "etag"],
        vec!["export", "agent", "chat"],
    ] {
        let out = api.ctl(&args, Some(TOKEN));
        assert!(!has_hazard(&out.stdout), "{args:?}: {:?}", out.stdout);
        assert!(!has_hazard(&out.stderr), "{args:?}: {:?}", out.stderr);
    }
}

#[test]
fn an_export_that_holds_control_characters_is_refused_and_none_of_it_is_printed() {
    let api = Api::start("export-hostile");
    for text in [
        "kind: AgentService\ntitle: \u{1b}[31mred\n",
        "kind: AgentService\ntitle: a\u{202e}b\n",
        "kind: AgentService\ntitle: a\u{85}b\n",
        "kind: AgentService\ntitle: a\u{7f}b\n",
        "kind: AgentService\ntitle: a\rb\n",
    ] {
        let text = text.to_owned();
        let stub = Stub::start(move |_, _| {
            Answer::Say(
                200,
                vec![("Content-Type".to_owned(), "application/yaml".to_owned())],
                text.clone(),
            )
        });
        let out = run_binary(
            &[
                "export",
                "agent",
                "chat",
                "--server",
                &stub.url,
                "--config",
                &cfg(&api),
            ],
            Some(TOKEN),
            None,
            &api.dir,
        );
        assert_eq!(out.code, 5, "{:?}: {}", stub.lines(), out.stderr);
        assert!(
            out.stderr.contains("the answer holds control characters"),
            "{}",
            out.stderr
        );
        assert!(out.stdout.is_empty(), "{:?}", out.stdout);
    }
    // Line feeds and tabs are a file.
    let stub = Stub::start(|_, _| Answer::Say(200, vec![], "a:\n\tb\n".to_owned()));
    let out = run_binary(
        &[
            "export",
            "agent",
            "chat",
            "--server",
            &stub.url,
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "a:\n\tb\n");
}

// ------------------------------------------------------------------------------------------ A8: one config, one service

#[test]
fn two_services_naming_one_config_are_refused_before_anything_is_written() {
    let api = Api::start("shared-config");
    let mut docs = agent_docs("chat", "Chat");
    let mut twin = docs[1].clone();
    set(&mut twin, "/metadata/name", json!("chat-twin"));
    docs.push(twin);
    docs.push(model_doc("gateway"));
    let path = file(&api.dir, "twins.yaml", &docs);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(
        out.stderr.contains("agent/chat") && out.stderr.contains("agent/chat-twin"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("both name the AgentConfig"),
        "{}",
        out.stderr
    );
    assert!(api.model("gateway").is_none() && api.agent("chat").is_none());
    assert_eq!(api.request_count(), 0);
}

// ------------------------------------------------------------------------------------------ B9 to B12: the file and the token

#[test]
fn a_config_file_that_leaks_does_not_stop_a_command_that_does_not_use_it() {
    let api = Api::start("file-unused");
    assert_eq!(
        api.ctl_in(&["login", "--token-stdin"], None, Some(TOKEN))
            .code,
        0
    );
    std::fs::set_permissions(api.config(), std::fs::Permissions::from_mode(0o644)).unwrap();
    let before = std::fs::read(api.config()).unwrap();

    // The token and the server are both given: the file is not read.
    let out = api.ctl(&["whoami"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    // The server comes from the file when it is not given: the file is needed, and refused.
    let out = run_binary(
        &["whoami", "--config", &cfg(&api)],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(
        out.stderr.contains("readable by group or others"),
        "{}",
        out.stderr
    );

    // `login` and `logout` need the file, and refuse one that leaks; it is left as it was (not rewritten in place).
    for args in [&["logout"][..], &["login", "--token-stdin"][..]] {
        let out = api.ctl_in(args, None, Some(TOKEN));
        assert_eq!(out.code, 2, "{args:?}: {}", out.stderr);
        assert!(
            out.stderr.contains("readable by group or others"),
            "{}",
            out.stderr
        );
    }
    assert_eq!(std::fs::read(api.config()).unwrap(), before);
    let mode = std::fs::metadata(api.config())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644);
}

#[test]
fn a_config_file_is_not_followed_through_a_link_and_must_be_ours() {
    let api = Api::start("file-link");
    assert_eq!(
        api.ctl_in(&["login", "--token-stdin"], None, Some(TOKEN))
            .code,
        0
    );
    let link = api.dir.join("link.yaml");
    std::os::unix::fs::symlink(api.config(), &link).unwrap();
    let out = run_binary(
        &[
            "whoami",
            "--server",
            &api.url,
            "--config",
            link.to_str().unwrap(),
        ],
        None,
        None,
        &api.dir,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("symbolic link"), "{}", out.stderr);

    // Owned by somebody else: only a process that may give a file away can make one, which is root.
    if rustix::process::geteuid().is_root() {
        std::os::unix::fs::chown(api.config(), Some(65534), None).unwrap();
        let out = api.ctl(&["whoami"], None);
        assert_eq!(out.code, 2, "{}", out.stderr);
        assert!(out.stderr.contains("not owned by you"), "{}", out.stderr);
    }
}

#[test]
fn login_keeps_the_token_exactly_and_refuses_what_is_not_one() {
    let api = Api::start("login-input");

    // The token is kept as it is: no line break, nothing around it.
    let out = api.ctl_in(
        &["login", "--token-stdin"],
        None,
        Some(&format!("  {TOKEN}\r\n\n")),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let kept: Value =
        serde_yaml::from_str(&std::fs::read_to_string(api.config()).unwrap()).unwrap();
    assert_eq!(kept["accessToken"], TOKEN);
    std::fs::remove_file(api.config()).unwrap();

    // Nothing on standard input, or only white space: nothing is made.
    for input in ["", "  \n\t\n"] {
        let out = api.ctl_in(&["login", "--token-stdin"], None, Some(input));
        assert_eq!(out.code, 2, "{input:?}: {}", out.stderr);
        assert!(!api.config().exists(), "{input:?}: a file was made");
    }

    // More than a token can be is refused, not cut to something that might pass.
    let long = "a".repeat(65 * 1024 + 1);
    let out = api.ctl_in(&["login", "--token-stdin"], None, Some(&long));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("more than"), "{}", out.stderr);
    assert!(!api.config().exists());

    // A token with white space inside, in the environment: usage error, and no part of it in the message.
    let out = api.ctl(&["whoami"], Some("ey.sentinel-first part-second.sig"));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(
        !out.stderr.contains("sentinel") && !out.stderr.contains("second"),
        "{}",
        out.stderr
    );
}

#[test]
fn a_refused_token_says_to_sign_in_again() {
    let api = Api::start("token-401");
    let out = api.ctl(&["get", "agents"], Some("ey.nobody-knows-this-token"));
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(out.stderr.contains("401"), "{}", out.stderr);
    assert!(
        out.stderr.contains("refused or has expired"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("aapctl login --token-stdin"),
        "{}",
        out.stderr
    );
}

// ------------------------------------------------------------------------------------------ C: apply and delete

#[test]
fn a_refused_object_stops_the_apply_and_says_what_was_applied_before_it() {
    let api = Api::start("stops");
    let mut docs = agent_docs_with("chat", "Chat", &["docs"]);
    docs.push(tool_doc("docs"));
    docs.push(model_doc("gateway"));
    let path = file(&api.dir, "all.yaml", &docs);

    // The model is written, the tool server is refused (no permission), and the agent is never sent.
    let token = api.token_with(&[
        "platform:agents.read",
        "platform:agents.write",
        "platform:models.write",
        "platform:secrets.pick",
    ]);
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--verbose"],
        Some(&token),
    );
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert_eq!(out.stdout, "model/gateway created\n");
    assert!(
        out.stderr
            .contains("applied before this (1): model/gateway"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("platform:toolproviders.write"),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains("> PUT /v1/agents"), "{}", out.stderr);
    assert!(
        api.model("gateway").is_some() && api.tool("docs").is_none() && api.agent("chat").is_none()
    );

    // The first object refused: nothing was applied before it.
    let fresh = Api::start("stops-first");
    let token = fresh.token_with(&["platform:agents.read"]);
    let out = fresh.ctl(&["apply", "-f", path.to_str().unwrap()], Some(&token));
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("nothing was applied before this"),
        "{}",
        out.stderr
    );
}

#[test]
fn the_retry_of_a_moving_object_is_bounded() {
    let api = Api::start("bounded-retry");
    seed_gateway_and_chat(&api, "Chat");
    let moved = Arc::new(AtomicUsize::new(0));
    let counter = moved.clone();
    // Only the tag moves, after every read, for ever: a retry that is not bounded never ends.
    api.after_every_read(move |objects| {
        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
        objects.set_status(
            Kind::AgentService,
            "chat",
            json!({ "observedGeneration": n }),
        );
    });
    let path = file(
        &api.dir,
        "new-title.yaml",
        &agent_docs("chat", "A new title"),
    );
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--verbose"],
        Some(TOKEN),
    );
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(out.stderr.contains("kept changing"), "{}", out.stderr);
    // `ATTEMPTS` of `remote.rs`.
    assert_eq!(out.stderr.matches("> PUT").count(), 3, "{}", out.stderr);
}

#[test]
fn a_dry_run_makes_no_request_at_all() {
    let api = Api::start("dry-run-silent");
    let mut docs = agent_docs("chat", "Chat");
    docs.push(model_doc("gateway"));
    let path = file(&api.dir, "all.yaml", &docs);
    let out = api.ctl(
        &[
            "apply",
            "-f",
            path.to_str().unwrap(),
            "--dry-run",
            "--verbose",
        ],
        Some(TOKEN),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(!out.stderr.contains("> "), "{}", out.stderr);
    assert_eq!(api.request_count(), 0, "the router was asked");
}

#[test]
fn an_object_on_the_server_that_no_form_says_is_refused_by_the_client_before_any_write() {
    let api = Api::start("server-yaml-only");
    seed_gateway(&api);
    let form = folder_form("chat", "Chat");
    let mut draft =
        to_resources(&form, &defaults().deployment_for(form.kind(), "chat"), NS).draft();
    draft.service["scaling"] = json!({ "topology": "combined", "workers": 3 });
    api.block(api.objects.put_agent(&draft, &Precondition::DoesNotExist))
        .unwrap();
    let path = file(&api.dir, "chat.yaml", &agent_docs("chat", "Chat"));
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--verbose"],
        Some(TOKEN),
    );
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("yaml-only"), "{}", out.stderr);
    assert!(
        out.stderr
            .contains("the object on the server has fields no form says"),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains("> PUT"), "{}", out.stderr);
}

#[test]
fn a_create_that_cannot_read_the_defaults_says_which_object_and_why() {
    let api = Api::start("info-refused");
    let stub = Stub::start(|line, _| match line {
        "GET /v1/agents/chat" => {
            Answer::Say(404, json_headers(), problem(404, "not-found", "no", ""))
        }
        "GET /v1/info" => Answer::Say(
            403,
            json_headers(),
            problem(
                403,
                "forbidden",
                "no",
                r#","permission":"platform:agents.read""#,
            ),
        ),
        _ => Answer::Say(500, json_headers(), problem(500, "internal", "no", "")),
    });
    let path = file(
        &api.dir,
        "chat.yaml",
        &without_namespaces(agent_docs("chat", "Chat")),
    );
    let out = run_binary(
        &[
            "apply",
            "-f",
            path.to_str().unwrap(),
            "--server",
            &stub.url,
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("agent/chat: cannot be created"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("platform:agents.read"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("403"), "{}", out.stderr);
    assert_eq!(stub.count("PUT"), 0);
}

#[test]
fn a_delete_the_operator_has_not_finished_says_what_an_orphan_is() {
    let api = Api::start("delete-orphan");
    seed_gateway_and_chat(&api, "Chat");
    api.objects.seed_finalizer("chat");
    let out = api.ctl(&["delete", "agent", "chat"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("agent/chat deleting"), "{}", out.stdout);
    assert!(out.stdout.contains("orphan"), "{}", out.stdout);
    assert!(
        out.stdout.contains("next create of the name takes it over"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("no route to list or delete"),
        "{}",
        out.stdout
    );
}

/// The detail of `chat` as the real API gives it, for a stub to serve.
fn chat_detail(api: &Api) -> (String, String) {
    (
        api_body(api, Collection::Agents, "chat").to_string(),
        api_etag(api, "chat"),
    )
}

#[test]
fn a_wait_survives_a_failing_read_and_ends_when_the_agent_is_gone() {
    let api = Api::start("wait-transient");
    seed_gateway_and_chat(&api, "Chat");
    let (body, tag) = chat_detail(&api);
    let stub = Stub::start(move |line, nth| match (line, nth) {
        ("GET /v1/agents/chat", 1) => {
            let mut h = json_headers();
            h.push(("ETag".to_owned(), tag.clone()));
            Answer::Say(200, h, body.clone())
        }
        ("DELETE /v1/agents/chat", _) => Answer::Say(202, json_headers(), "{}".to_owned()),
        // The reads that follow: a 503, an answer that is not the API's, one that is still there, and then it is gone.
        ("GET /v1/agents/chat", 2) => {
            Answer::Say(503, json_headers(), problem(503, "unavailable", "down", ""))
        }
        ("GET /v1/agents/chat", 3) => {
            Answer::Say(502, vec![], "<html>bad gateway</html>".to_owned())
        }
        ("GET /v1/agents/chat", 4) => {
            let mut h = json_headers();
            h.push(("ETag".to_owned(), tag.clone()));
            Answer::Say(200, h, body.clone())
        }
        _ => Answer::Say(404, json_headers(), problem(404, "not-found", "gone", "")),
    });
    let out = run_binary(
        &[
            "delete",
            "agent",
            "chat",
            "--wait",
            "--poll-ms",
            "20",
            "--timeout",
            "30",
            "--server",
            &stub.url,
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("agent/chat gone"), "{}", out.stdout);
    assert_eq!(stub.count("GET /v1/agents/chat"), 5, "{:?}", stub.lines());
}

#[test]
fn a_wait_ends_with_its_timeout_even_when_the_server_never_answers() {
    let api = Api::start("wait-hang");
    seed_gateway_and_chat(&api, "Chat");
    let (body, tag) = chat_detail(&api);
    let stub = Stub::start(move |line, nth| match (line, nth) {
        ("GET /v1/agents/chat", 1) => {
            let mut h = json_headers();
            h.push(("ETag".to_owned(), tag.clone()));
            Answer::Say(200, h, body.clone())
        }
        ("DELETE /v1/agents/chat", _) => Answer::Say(202, json_headers(), "{}".to_owned()),
        _ => Answer::Hang,
    });
    let started = std::time::Instant::now();
    let out = run_binary(
        &[
            "delete",
            "agent",
            "chat",
            "--wait",
            "--poll-ms",
            "20",
            "--timeout",
            "2",
            "--server",
            &stub.url,
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
    );
    let took = started.elapsed();
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(
        out.stderr.contains("still being deleted after 2 s"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("the last read failed"),
        "{}",
        out.stderr
    );
    assert!(
        took < std::time::Duration::from_secs(8),
        "the wait took {took:?} for a timeout of 2 s"
    );
}

#[test]
fn a_wait_does_not_outlast_a_refusal_that_waiting_cannot_change() {
    let api = Api::start("wait-forbidden");
    seed_gateway_and_chat(&api, "Chat");
    let (body, tag) = chat_detail(&api);
    let stub = Stub::start(move |line, nth| match (line, nth) {
        ("GET /v1/agents/chat", 1) => {
            let mut h = json_headers();
            h.push(("ETag".to_owned(), tag.clone()));
            Answer::Say(200, h, body.clone())
        }
        ("DELETE /v1/agents/chat", _) => Answer::Say(202, json_headers(), "{}".to_owned()),
        _ => Answer::Say(403, json_headers(), problem(403, "forbidden", "no", "")),
    });
    let out = run_binary(
        &[
            "delete",
            "agent",
            "chat",
            "--wait",
            "--poll-ms",
            "20",
            "--timeout",
            "30",
            "--server",
            &stub.url,
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert_eq!(stub.count("GET /v1/agents/chat"), 2);
}

// ------------------------------------------------------------------------------------------ C20: the exit codes and the words

#[test]
fn export_names_agents_only() {
    let api = Api::start("export-noun");
    let out = api.ctl(&["export", "model", "gateway"], Some(TOKEN));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert_eq!(api.request_count(), 0);
}

#[test]
fn a_server_that_fails_is_exit_5_whatever_it_says() {
    let api = Api::start("server-fails");
    for stub in [
        Stub::answering(500, &problem(500, "internal", "broke", "")),
        Stub::answering(503, &problem(503, "unavailable", "down", "")),
        Stub::answering(502, "<html>secret-proxy-page</html>"),
    ] {
        let out = run_binary(
            &[
                "get",
                "agents",
                "--server",
                &stub.url,
                "--config",
                &cfg(&api),
            ],
            Some(TOKEN),
            None,
            &api.dir,
        );
        assert_eq!(out.code, 5, "{}", out.stderr);
        assert!(!out.stderr.contains("secret-proxy-page"), "{}", out.stderr);
    }
}

#[test]
fn a_forbidden_answer_names_the_permission_even_when_the_detail_does_not() {
    let api = Api::start("forbidden-permission");
    let stub = Stub::answering(
        403,
        &problem(
            403,
            "forbidden",
            "not allowed",
            r#","permission":"platform:agents.write""#,
        ),
    );
    let out = run_binary(
        &[
            "get",
            "agents",
            "--server",
            &stub.url,
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("the permission it needs is platform:agents.write"),
        "{}",
        out.stderr
    );
}

#[test]
fn the_words_of_a_problem_are_one_clean_line() {
    let api = Api::start("problem-words");
    let stub = Stub::answering(
        409,
        &problem(
            409,
            "stale",
            "bad\u{1b}[31m\nsecond line\u{202e}evil\u{2066}",
            "",
        ),
    );
    let out = run_binary(
        &[
            "get",
            "agents",
            "--server",
            &stub.url,
            "--config",
            &cfg(&api),
        ],
        Some(TOKEN),
        None,
        &api.dir,
    );
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(!has_hazard(&out.stderr), "{:?}", out.stderr);
    assert_eq!(out.stderr.lines().count(), 1, "{:?}", out.stderr);
    assert!(
        out.stderr.contains("bad") && out.stderr.contains("second line"),
        "{}",
        out.stderr
    );
}

// ------------------------------------------------------------------------------------------ D23: phase 1 stays offline

#[test]
fn validate_and_render_never_touch_a_proxy_or_the_token() {
    let (proxy, connections) = counting_listener();
    let dir = common::scratch("phase-1-offline");
    let example = common::example("chat.yaml");
    let example = example.to_str().unwrap();
    for command in ["validate", "render"] {
        let args = [
            command,
            "-f",
            example,
            "--default-agent-image",
            common::IMAGE,
        ];
        let plain = common::aapctl(&args, None);
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_aapctl"));
        c.args(args)
            .env("AAPCTL_TOKEN", TOKEN)
            .env("HTTPS_PROXY", &proxy)
            .env("HTTP_PROXY", &proxy)
            .env("ALL_PROXY", &proxy)
            .env("XDG_CONFIG_HOME", dir.join("nowhere"))
            .env("HOME", &dir);
        let o = c.output().unwrap();
        let (code, stdout, stderr) = (
            o.status.code().unwrap(),
            String::from_utf8(o.stdout).unwrap(),
            String::from_utf8(o.stderr).unwrap(),
        );
        assert_eq!(
            (plain.code, plain.stdout, plain.stderr),
            (code, stdout, stderr),
            "{command}"
        );
    }
    assert_eq!(connections.load(Ordering::SeqCst), 0);
}
