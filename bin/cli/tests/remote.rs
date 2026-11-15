//! `aapctl` over the Platform API (K9b-1): the built binary against the real router over the `Memory` ports.
//!
//! Each test says what it holds. The shape of every one is: files in, a command, the exit code, what was written to the store.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "remote/api.rs"]
mod api;
mod common;

use std::os::unix::fs::PermissionsExt;

use aap_platform_client::{Client, Collection, Options, Token};
use aap_ports::testkit::Foreign;
use aap_ports::{AgentObjects, Kind, Precondition, SharedKind};
use api::{
    Api, TOKEN, agent_docs, agent_docs_with, file, folder_form, model_doc, model_form, now, rows,
    seed_gateway_and_chat, tool_doc, tool_form,
};
use common::set;
use serde_json::{Value, json};

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

/// The API's own answer, to compare with what `get -o json` printed.
fn api_body(api: &Api, collection: Collection, name: Option<&str>) -> Value {
    let c = client(api);
    let reply = match name {
        Some(n) => api.block(c.get(collection, n)),
        None => api.block(c.list(collection)),
    };
    reply.unwrap().json().unwrap()
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

fn title_of(api: &Api, name: &str) -> String {
    api.agent(name).unwrap().service.spec["registry"]["title"]
        .as_str()
        .unwrap()
        .to_owned()
}

// ------------------------------------------------------------------------------------------ get

#[test]
fn get_lists_and_reads() {
    let api = Api::start("get");
    seed_gateway_and_chat(&api, "Chat");
    api.block(api.objects.put_shared(
        SharedKind::ToolServer,
        &tool_form("docs").draft(),
        &Precondition::DoesNotExist,
    ))
    .unwrap();

    let out = api.ctl(&["get", "agents"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.starts_with("NAME"), "{}", out.stdout);
    let header: Vec<&str> = out
        .stdout
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .collect();
    assert_eq!(
        header,
        ["NAME", "KIND", "STATE", "MANAGED-BY", "SUSPENDED", "TITLE"]
    );
    let table = rows(&out.stdout);
    assert_eq!(
        table["chat"],
        ["chat", "folder", "saving", "dashboard", "false", "Chat"]
    );

    let out = api.ctl(&["get", "models"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let header: Vec<&str> = out
        .stdout
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .collect();
    assert_eq!(header, ["NAME", "TITLE", "MANAGED-BY", "USED-BY"]);
    assert_eq!(
        rows(&out.stdout)["gateway"],
        ["gateway", "Gateway", "dashboard", "1"]
    );

    // The aliases, and the tool servers.
    for args in [
        ["get", "agent", "chat"],
        ["get", "model", "gateway"],
        ["get", "tools", "docs"],
        ["get", "tool-server", "docs"],
        ["get", "tool-servers", "docs"],
    ] {
        let out = api.ctl(&args, Some(TOKEN));
        assert_eq!(out.code, 0, "{args:?}: {}", out.stderr);
        assert_eq!(
            out.stdout.lines().count(),
            2,
            "{args:?}: header and one row"
        );
    }

    // `-o json` is the API's body, unchanged; `-o yaml` says the same.
    for (args, collection, name) in [
        (
            vec!["get", "agents", "-o", "json"],
            Collection::Agents,
            None,
        ),
        (
            vec!["get", "agents", "chat", "-o", "json"],
            Collection::Agents,
            Some("chat"),
        ),
        (
            vec!["get", "models", "-o", "json"],
            Collection::Models,
            None,
        ),
        (
            vec!["get", "model", "gateway", "-o", "json"],
            Collection::Models,
            Some("gateway"),
        ),
        (
            vec!["get", "tools", "-o", "json"],
            Collection::ToolServers,
            None,
        ),
    ] {
        let out = api.ctl(&args, Some(TOKEN));
        assert_eq!(out.code, 0, "{args:?}: {}", out.stderr);
        let printed: Value = serde_json::from_str(&out.stdout).unwrap();
        assert_eq!(printed, api_body(&api, collection, name), "{args:?}");
    }
    let yaml = api.ctl(&["get", "agent", "chat", "-o", "yaml"], Some(TOKEN));
    let from_yaml: Value = serde_yaml::from_str(&yaml.stdout).unwrap();
    assert_eq!(from_yaml, api_body(&api, Collection::Agents, Some("chat")));

    // `-o etag` is the tag and nothing else.
    let tag = api.ctl(&["get", "agent", "chat", "-o", "etag"], Some(TOKEN));
    assert_eq!(tag.code, 0);
    assert_eq!(tag.stdout, format!("{}\n", api_etag(&api, "chat")));
    // There is no tag for a list.
    let list = api.ctl(&["get", "agents", "-o", "etag"], Some(TOKEN));
    assert_eq!(list.code, 2, "{}", list.stderr);

    // A name that is not there is not-found (exit 1: the request names nothing).
    let missing = api.ctl(&["get", "agent", "nope"], Some(TOKEN));
    assert_eq!(missing.code, 1);
    assert!(
        missing.stderr.contains("404 not-found"),
        "{}",
        missing.stderr
    );
}

// ------------------------------------------------------------------------------------------ apply

#[test]
fn apply_creates_then_unchanged() {
    let api = Api::start("apply-create");
    // The agent first and the model second in the file: the order the file is written in is not the order of the writes.
    let mut docs = agent_docs("chat", "Chat");
    docs.push(model_doc("gateway"));
    let path = file(&api.dir, "all.yaml", &docs);
    let f = path.to_str().unwrap();

    let first = api.ctl(&["apply", "-f", f, "--verbose"], Some(TOKEN));
    assert_eq!(first.code, 0, "{}", first.stderr);
    assert_eq!(first.stdout, "model/gateway created\nagent/chat created\n");
    assert!(api.model("gateway").is_some() && api.agent("chat").is_some());
    let puts: Vec<&str> = first
        .stderr
        .lines()
        .filter(|l| l.starts_with("> PUT"))
        .collect();
    assert_eq!(
        puts,
        [
            "> PUT /v1/models/gateway  If-None-Match: *",
            "> PUT /v1/agents/chat  If-None-Match: *"
        ],
        "{}",
        first.stderr
    );
    let before = api_etag(&api, "chat");

    // Again: nothing is written, so the tag does not move and no PUT is sent.
    let second = api.ctl(&["apply", "-f", f, "--verbose"], Some(TOKEN));
    assert_eq!(second.code, 0, "{}", second.stderr);
    assert_eq!(
        second.stdout,
        "model/gateway unchanged\nagent/chat unchanged\n"
    );
    assert!(!second.stderr.contains("> PUT"), "{}", second.stderr);
    assert_eq!(api_etag(&api, "chat"), before);
}

#[test]
fn apply_orders_tool_servers_before_the_agents_that_name_them() {
    let api = Api::start("apply-order");
    let mut docs = agent_docs_with("chat", "Chat", &["docs"]);
    docs.push(tool_doc("docs"));
    docs.push(model_doc("gateway"));
    let path = file(&api.dir, "all.yaml", &docs);
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--verbose"],
        Some(TOKEN),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "model/gateway created\ntool-server/docs created\nagent/chat created\n"
    );
}

#[test]
fn apply_replaces() {
    let api = Api::start("apply-replace");
    let mut docs = agent_docs("chat", "Chat");
    docs.push(model_doc("gateway"));
    let first = file(&api.dir, "first.yaml", &docs);
    assert_eq!(
        api.ctl(&["apply", "-f", first.to_str().unwrap()], Some(TOKEN))
            .code,
        0
    );

    let mut docs = agent_docs("chat", "Chat, renamed");
    docs.push(model_doc("gateway"));
    let second = file(&api.dir, "second.yaml", &docs);
    let out = api.ctl(&["apply", "-f", second.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "model/gateway unchanged\nagent/chat configured\n"
    );
    assert_eq!(title_of(&api, "chat"), "Chat, renamed");
    let body = api_body(&api, Collection::Agents, Some("chat"));
    assert_eq!(
        body["representation"]["form"]["form"]["title"],
        "Chat, renamed"
    );
}

#[test]
fn apply_stale_if_match() {
    let api = Api::start("apply-stale");
    seed_gateway_and_chat(&api, "Chat");
    let old = api_etag(&api, "chat");

    // Somebody else saves.
    let c = client(&api);
    let mut form = folder_form("chat", "Somebody else's title");
    form.tags = vec!["chat".into()];
    let body = serde_json::to_value(&form).unwrap();
    let put = api
        .block(c.put(
            Collection::Agents,
            "chat",
            &body,
            &aap_platform_client::Condition::Replace(old.clone()),
        ))
        .unwrap();
    assert_eq!(put.status, 200);

    let path = file(&api.dir, "mine.yaml", &agent_docs("chat", "My title"));
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--if-match", &old],
        Some(TOKEN),
    );
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(out.stderr.contains("412 stale"), "{}", out.stderr);
    assert!(out.stderr.contains("agent/chat"), "{}", out.stderr);
    assert_eq!(title_of(&api, "chat"), "Somebody else's title");

    // With the tag that is current, the same command writes.
    let current = api_etag(&api, "chat");
    let out = api.ctl(
        &[
            "apply",
            "-f",
            path.to_str().unwrap(),
            "--if-match",
            &current,
        ],
        Some(TOKEN),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(title_of(&api, "chat"), "My title");
}

#[test]
fn if_match_takes_one_object() {
    let api = Api::start("if-match-two");
    let mut docs = agent_docs("chat", "Chat");
    docs.push(model_doc("gateway"));
    let path = file(&api.dir, "two.yaml", &docs);
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--if-match", "\"x\""],
        Some(TOKEN),
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("--if-match"), "{}", out.stderr);
}

#[test]
fn apply_retry_on_status_only_change() {
    let api = Api::start("apply-retry");
    seed_gateway_and_chat(&api, "Chat");
    let path = file(
        &api.dir,
        "new-title.yaml",
        &agent_docs("chat", "A new title"),
    );

    // The operator writes its status between the read and the write: the tag moves, the object does not.
    api.after_next_read(|objects| {
        objects.set_status(
            Kind::AgentService,
            "chat",
            json!({ "observedGeneration": 1 }),
        );
    });
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--verbose"],
        Some(TOKEN),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "agent/chat configured\n");
    assert_eq!(title_of(&api, "chat"), "A new title");
    let sent: Vec<&str> = out
        .stderr
        .lines()
        .filter(|l| l.starts_with("< ") && !l.starts_with("< 404"))
        .collect();
    assert!(out.stderr.contains("< 412"), "{}", out.stderr);
    assert_eq!(
        out.stderr.matches("> PUT").count(),
        2,
        "{sent:?}\n{}",
        out.stderr
    );
}

#[test]
fn apply_does_not_retry_when_the_object_changed() {
    let api = Api::start("apply-no-retry");
    seed_gateway_and_chat(&api, "Chat");
    let path = file(&api.dir, "mine.yaml", &agent_docs("chat", "My title"));

    // Between the read and the write somebody changes the title: the form is not what was read.
    api.after_next_read(|objects| {
        let form = folder_form("chat", "Somebody else's title");
        let deployment = api::defaults().deployment_for(form.kind(), "chat");
        let draft = aap_forms::to_resources(&form, &deployment, api::NS).draft();
        let agent = now(objects.get_agent("chat")).unwrap().unwrap();
        now(objects.put_agent(&draft, &Precondition::Matches(agent.etag()))).unwrap();
    });
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("changed on the server since it was read"),
        "{}",
        out.stderr
    );
    assert_eq!(title_of(&api, "chat"), "Somebody else's title");
}

#[test]
fn apply_yaml_only_refused() {
    let api = Api::start("apply-yaml-only");
    let mut docs = agent_docs("chat", "Chat");
    // The form has no scaling: the object says something the dashboard cannot.
    set(
        &mut docs[1],
        "/spec/scaling",
        json!({ "topology": "combined", "workers": 3 }),
    );
    docs.push(model_doc("gateway"));
    let path = file(&api.dir, "yaml-only.yaml", &docs);
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--verbose"],
        Some(TOKEN),
    );
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(
        out.stderr.contains("agent/chat: yaml-only"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("AgentService spec.scaling"),
        "{}",
        out.stderr
    );
    // Nothing was sent and nothing written, the model that was fine included: a file is judged whole.
    assert!(
        !out.stderr.contains("> "),
        "a request was sent: {}",
        out.stderr
    );
    assert!(out.stdout.is_empty());
    assert!(api.agent("chat").is_none() && api.model("gateway").is_none());
}

#[test]
fn apply_deployment_differs() {
    let api = Api::start("apply-deployment");
    seed_gateway(&api);

    // A create writes the server's defaults and not the file's: a file that says otherwise is refused, naming the field.
    let mut docs = agent_docs("chat", "Chat");
    set(&mut docs[0], "/spec/security/runAsUser", json!(20000));
    let path = file(&api.dir, "other-security.yaml", &docs);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("security.runAsUser"), "{}", out.stderr);
    assert!(
        api.agent("chat").is_none(),
        "the create wrote the server's defaults"
    );

    // A replace never moves it either.
    seed_gateway_and_chat_agent_only(&api);
    let before = api_etag(&api, "chat");
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("security.runAsUser"), "{}", out.stderr);
    assert_eq!(api_etag(&api, "chat"), before);
}

fn seed_gateway_and_chat_agent_only(api: &Api) {
    let form = folder_form("chat", "Chat");
    let deployment = api::defaults().deployment_for(form.kind(), "chat");
    let draft = aap_forms::to_resources(&form, &deployment, api::NS).draft();
    api.block(api.objects.put_agent(&draft, &Precondition::DoesNotExist))
        .unwrap();
}

#[test]
fn apply_gitops_refused() {
    let api = Api::start("apply-gitops");
    seed_gateway(&api);
    let docs = agent_docs("chat", "Chat");
    api.objects.seed_foreign(
        Kind::AgentConfig,
        "chat",
        docs[0]["spec"].clone(),
        Foreign::Unlabelled,
    );
    api.objects.seed_foreign(
        Kind::AgentService,
        "chat",
        docs[1]["spec"].clone(),
        Foreign::Unlabelled,
    );
    let changed = agent_docs("chat", "A change GitOps would undo");
    let path = file(&api.dir, "mine.yaml", &changed);
    let before = api_etag(&api, "chat");
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--verbose"],
        Some(TOKEN),
    );
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(out.stderr.contains("managed-by-gitops"), "{}", out.stderr);
    assert!(!out.stderr.contains("> PUT"), "{}", out.stderr);
    assert_eq!(api_etag(&api, "chat"), before);
}

#[test]
fn create_race_already_exists() {
    let api = Api::start("create-race");
    seed_gateway(&api);
    let path = file(&api.dir, "mine.yaml", &agent_docs("chat", "Mine"));

    // The agent is made between the read (404) and the write: the create is a conflict and never a replace.
    api.after_next_read(|objects| {
        let form = folder_form("chat", "Made first by somebody else");
        let deployment = api::defaults().deployment_for(form.kind(), "chat");
        let draft = aap_forms::to_resources(&form, &deployment, api::NS).draft();
        now(objects.put_agent(&draft, &Precondition::DoesNotExist)).unwrap();
    });
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(out.stderr.contains("409 already-exists"), "{}", out.stderr);
    assert_eq!(title_of(&api, "chat"), "Made first by somebody else");
}

#[test]
fn forbidden_named() {
    let api = Api::start("forbidden");
    seed_gateway(&api);
    let path = file(&api.dir, "chat.yaml", &agent_docs("chat", "Chat"));
    let token = api.token_with(&["platform:agents.read"]);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(&token));
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(out.stderr.contains("403 forbidden"), "{}", out.stderr);
    assert!(
        out.stderr.contains("platform:agents.write"),
        "{}",
        out.stderr
    );
    assert!(api.agent("chat").is_none());

    // A token the server does not know is 401, and that is exit 4 too.
    let out = api.ctl(&["get", "agents"], Some("ey.nobody-knows-this-token"));
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(out.stderr.contains("401"), "{}", out.stderr);

    // No token at all: said before anything is sent.
    let out = api.ctl(&["get", "agents", "--verbose"], None);
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(out.stderr.contains("not signed in"), "{}", out.stderr);
    assert!(!out.stderr.contains("> "), "{}", out.stderr);
}

#[test]
fn secret_needs_pick() {
    let api = Api::start("secret-pick");
    let path = file(&api.dir, "model.yaml", &[model_doc("gateway")]);
    let token = api.token_with(&["platform:agents.read", "platform:models.write"]);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(&token));
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("platform:secrets.pick"),
        "{}",
        out.stderr
    );
    assert!(api.model("gateway").is_none());
}

#[test]
fn apply_refuses_what_the_api_would_drop_and_what_is_not_paired() {
    let api = Api::start("apply-local");
    // A label the API does not write.
    let mut model = model_doc("gateway");
    set(&mut model, "/metadata/labels/team", json!("platform"));
    let path = file(&api.dir, "label.yaml", &[model]);
    let out = api.ctl(
        &["apply", "-f", path.to_str().unwrap(), "--verbose"],
        Some(TOKEN),
    );
    // A file that says what the API cannot keep is a usage error (exit 2), not a refusal of content.
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("label team"), "{}", out.stderr);
    assert!(!out.stderr.contains("> "), "{}", out.stderr);

    // An annotation, and a namespace that is not the API's.
    let mut model = model_doc("gateway");
    set(
        &mut model,
        "/metadata/annotations/example.com~1note",
        json!("x"),
    );
    let path = file(&api.dir, "annotation.yaml", &[model]);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("annotation"), "{}", out.stderr);

    let mut model = model_doc("gateway");
    set(
        &mut model,
        "/metadata/namespace",
        json!("another-namespace"),
    );
    let path = file(&api.dir, "namespace.yaml", &[model]);
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(
        out.stderr.contains("another-namespace") && out.stderr.contains(api::NS),
        "{}",
        out.stderr
    );
    assert!(api.model("gateway").is_none());

    // A service without its config, and a config without its service: a read error (exit 2).
    let docs = agent_docs("chat", "Chat");
    let service_only = file(&api.dir, "service.yaml", &docs[1..]);
    let out = api.ctl(
        &["apply", "-f", service_only.to_str().unwrap()],
        Some(TOKEN),
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("AgentConfig"), "{}", out.stderr);
    let config_only = file(&api.dir, "config.yaml", &docs[..1]);
    let out = api.ctl(&["apply", "-f", config_only.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("no AgentService"), "{}", out.stderr);
}

#[test]
fn apply_a_value_the_api_refuses_is_exit_1_with_the_field() {
    let api = Api::start("apply-422");
    // The agent names a model endpoint that is not there.
    let path = file(&api.dir, "chat.yaml", &agent_docs("chat", "Chat"));
    let out = api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN));
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("422 validation"), "{}", out.stderr);
    assert!(out.stderr.contains("model.endpoint"), "{}", out.stderr);
}

#[test]
fn dry_run_offline() {
    let dir = common::scratch("dry-run");
    let mut docs = agent_docs("chat", "Chat");
    docs.push(model_doc("gateway"));
    let path = file(&dir, "all.yaml", &docs);
    // No server, no token, no config: nothing is asked of a network.
    let out = api::run_binary(
        &["apply", "-f", path.to_str().unwrap(), "--dry-run"],
        None,
        None,
        &dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let printed: Value = serde_json::from_str(&out.stdout).unwrap();
    let items = printed.as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["kind"], "ModelEndpoint");
    assert_eq!(items[0]["name"], "gateway");
    assert_eq!(items[0]["form"]["title"], "Gateway");
    assert_eq!(items[1]["kind"], "AgentService");
    assert_eq!(items[1]["name"], "chat");
    assert_eq!(items[1]["form"]["title"], "Chat");
    assert!(items[1]["deployment"]["security"].is_object());
    assert!(out.stderr.contains("dry run"), "{}", out.stderr);

    // The checks that need no server are still made.
    let mut bad = agent_docs("chat", "Chat");
    set(
        &mut bad[1],
        "/spec/scaling",
        json!({ "topology": "combined", "workers": 3 }),
    );
    let path = file(&dir, "bad.yaml", &bad);
    let out = api::run_binary(
        &["apply", "-f", path.to_str().unwrap(), "--dry-run"],
        None,
        None,
        &dir,
    );
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stdout.is_empty());
}

// ------------------------------------------------------------------------------------------ delete, export

#[test]
fn delete_if_match() {
    let api = Api::start("delete");
    seed_gateway_and_chat(&api, "Chat");

    let out = api.ctl(&["delete", "agent", "chat", "--verbose"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "agent/chat deleted\n");
    assert!(
        out.stderr.contains("> DELETE /v1/agents/chat  If-Match: "),
        "{}",
        out.stderr
    );
    assert!(api.agent("chat").is_none());

    // A model an agent names is refused, by the API: exit 1.
    seed_gateway_and_chat_agent_only(&api);
    let out = api.ctl(&["delete", "model", "gateway"], Some(TOKEN));
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("409 referenced"), "{}", out.stderr);
    assert!(api.model("gateway").is_some());

    // Not there: exit 1.
    let out = api.ctl(&["delete", "agent", "nope"], Some(TOKEN));
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.contains("404 not-found"), "{}", out.stderr);

    // A tool server.
    api.block(api.objects.put_shared(
        SharedKind::ToolServer,
        &tool_form("docs").draft(),
        &Precondition::DoesNotExist,
    ))
    .unwrap();
    let out = api.ctl(&["delete", "tool-server", "docs"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(api.tool("docs").is_none());
}

#[test]
fn delete_retries_a_status_write_and_not_an_edit() {
    let api = Api::start("delete-retry");
    seed_gateway_and_chat(&api, "Chat");
    api.after_next_read(|objects| {
        objects.set_status(
            Kind::AgentService,
            "chat",
            json!({ "observedGeneration": 1 }),
        );
    });
    let out = api.ctl(&["delete", "agent", "chat", "--verbose"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("< 412"), "{}", out.stderr);
    assert!(api.agent("chat").is_none());

    // The object's form changed between the read and the delete: it is not the one the person named.
    seed_gateway_and_chat_agent_only(&api);
    api.after_next_read(|objects| {
        let form = folder_form("chat", "Edited meanwhile");
        let deployment = api::defaults().deployment_for(form.kind(), "chat");
        let draft = aap_forms::to_resources(&form, &deployment, api::NS).draft();
        let agent = now(objects.get_agent("chat")).unwrap().unwrap();
        now(objects.put_agent(&draft, &Precondition::Matches(agent.etag()))).unwrap();
    });
    let out = api.ctl(&["delete", "agent", "chat"], Some(TOKEN));
    assert_eq!(out.code, 3, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("changed on the server since it was read"),
        "{}",
        out.stderr
    );
    assert!(api.agent("chat").is_some());
}

#[test]
fn delete_wait_returns_when_the_agent_is_gone() {
    let api = Api::start("delete-wait");
    seed_gateway_and_chat(&api, "Chat");
    // The operator's finalizer is on the service: the delete answers 202 until it has run.
    api.objects.seed_finalizer("chat");

    let objects = api.objects.clone();
    let finisher = std::thread::spawn(move || {
        // Bounded: 10 s at most.
        for _ in 0..200 {
            if now(objects.get_agent("chat"))
                .unwrap()
                .is_some_and(|a| a.deleting())
            {
                objects.run_finalizer("chat");
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        false
    });
    let out = api.ctl(
        &[
            "delete",
            "agent",
            "chat",
            "--wait",
            "--poll-ms",
            "50",
            "--timeout",
            "30",
        ],
        Some(TOKEN),
    );
    assert!(
        finisher.join().unwrap(),
        "the delete never reached the store"
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("agent/chat deleting"), "{}", out.stdout);
    assert!(out.stdout.contains("agent/chat gone"), "{}", out.stdout);
    assert!(api.agent("chat").is_none());
}

#[test]
fn delete_wait_gives_up_after_its_timeout() {
    let api = Api::start("delete-wait-timeout");
    seed_gateway_and_chat(&api, "Chat");
    api.objects.seed_finalizer("chat");
    // Nobody runs the finalizer.
    let out = api.ctl(
        &[
            "delete",
            "agent",
            "chat",
            "--wait",
            "--poll-ms",
            "50",
            "--timeout",
            "1",
        ],
        Some(TOKEN),
    );
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(out.stderr.contains("still being deleted"), "{}", out.stderr);
}

#[test]
fn export_apply_round_trip() {
    let api = Api::start("export");
    let mut docs = agent_docs("chat", "Chat");
    docs.push(model_doc("gateway"));
    let path = file(&api.dir, "all.yaml", &docs);
    assert_eq!(
        api.ctl(&["apply", "-f", path.to_str().unwrap()], Some(TOKEN))
            .code,
        0
    );

    let first = api.ctl(&["export", "agent", "chat"], Some(TOKEN));
    assert_eq!(first.code, 0, "{}", first.stderr);
    assert!(
        first.stdout.contains("kind: AgentConfig") && first.stdout.contains("kind: AgentService")
    );
    // It is what the API serves, unchanged.
    let served = api
        .block(client(&api).agent_yaml("chat"))
        .unwrap()
        .text()
        .unwrap();
    assert_eq!(first.stdout, served);

    let exported = api.dir.join("exported.yaml");
    std::fs::write(&exported, &first.stdout).unwrap();
    let back = api.ctl(&["apply", "-f", exported.to_str().unwrap()], Some(TOKEN));
    assert_eq!(back.code, 0, "{}", back.stderr);
    assert_eq!(back.stdout, "agent/chat unchanged\n");

    let second = api.ctl(&["export", "agent", "chat"], Some(TOKEN));
    assert_eq!(second.stdout, first.stdout);
}

// ------------------------------------------------------------------------------------------ the token

#[test]
fn login_token_stdin_then_whoami_logout() {
    let api = Api::start("login");
    // The plain login is the browser's (K9b-2), and it needs an issuer to send the person to.
    let out = api.ctl(&["login"], None);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(
        out.stderr.contains("no issuer") && out.stderr.contains("--issuer"),
        "{}",
        out.stderr
    );

    // A token the server refuses is not stored.
    let out = api.ctl_in(&["login", "--token-stdin"], None, Some("ey.not-a-token\n"));
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        !api.config().exists(),
        "a token the server refused was stored"
    );

    // One it accepts is, in a file only the owner can read.
    let out = api.ctl_in(
        &["login", "--token-stdin"],
        None,
        Some(&format!("{TOKEN}\n")),
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("Signed in to") && out.stdout.contains("platform:agents.read"),
        "{}",
        out.stdout
    );
    let mode = std::fs::metadata(api.config())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);

    // No environment, no flag: the file's token is used.
    let who = api.ctl(&["whoami"], None);
    assert_eq!(who.code, 0, "{}", who.stderr);
    assert!(who.stdout.contains(&api.url), "{}", who.stdout);
    assert!(
        who.stdout.contains("platform:agents.write"),
        "{}",
        who.stdout
    );
    assert!(who.stdout.contains("expires"), "{}", who.stdout);
    let seen = api.ctl(&["get", "agents"], None);
    assert_eq!(seen.code, 0, "{}", seen.stderr);

    // The environment wins over the file, and is not written to it.
    let bad = api.ctl(
        &["whoami"],
        Some("ey.another-token-the-server-does-not-know"),
    );
    assert_eq!(bad.code, 4, "{}", bad.stderr);
    let text = std::fs::read_to_string(api.config()).unwrap();
    assert!(
        !text.contains("another-token"),
        "the environment's token reached the file"
    );

    // Logout removes the token and keeps the server.
    let out = api.ctl(&["logout"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let text = std::fs::read_to_string(api.config()).unwrap();
    assert!(!text.contains(TOKEN) && text.contains("server"), "{text}");
    let who = api.ctl(&["whoami"], None);
    assert_eq!(who.code, 4, "{}", who.stderr);
    assert!(who.stderr.contains("not signed in"), "{}", who.stderr);
}

#[test]
fn a_config_file_others_can_read_is_refused() {
    let api = Api::start("config-mode");
    let out = api.ctl_in(&["login", "--token-stdin"], None, Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
    std::fs::set_permissions(api.config(), std::fs::Permissions::from_mode(0o644)).unwrap();
    for args in [&["whoami"][..], &["get", "agents"][..]] {
        let out = api.ctl(args, None);
        assert_eq!(out.code, 2, "{args:?}: {}", out.stderr);
        assert!(
            out.stderr.contains("readable by group or others"),
            "{}",
            out.stderr
        );
        assert!(!out.stderr.contains(TOKEN));
    }
    // With a token in the environment **and** the server on the command line (as `ctl` gives it) the file is not read at all, so it
    // does not stop the command (`hardening.rs` holds this and the case where the file is needed).
    let out = api.ctl(&["whoami"], Some(TOKEN));
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn unreachable_and_insecure_addresses() {
    let dir = common::scratch("unreachable");
    let config = dir.join("config.yaml");
    let config = config.to_str().unwrap();
    // Nothing listens on port 1: exit 5.
    let out = api::run_binary(
        &[
            "get",
            "agents",
            "--server",
            "http://127.0.0.1:1",
            "--config",
            config,
        ],
        Some(TOKEN),
        None,
        &dir,
    );
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(
        out.stderr.contains("cannot reach http://127.0.0.1:1"),
        "{}",
        out.stderr
    );
    // A cleartext address that is not the loopback: refused (exit 2) before the token goes anywhere.
    let out = api::run_binary(
        &[
            "get",
            "agents",
            "--server",
            "http://api.example.com",
            "--config",
            config,
        ],
        Some(TOKEN),
        None,
        &dir,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("in the clear"), "{}", out.stderr);
    // No server named anywhere.
    let out = api::run_binary(
        &["get", "agents", "--config", config],
        Some(TOKEN),
        None,
        &dir,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("no server"), "{}", out.stderr);
}

/// Every command, with `--verbose`, successful and failing: the token is in none of stdout or stderr, nor is any 16 characters of it.
#[test]
fn no_token_anywhere() {
    let api = Api::start("no-token");
    let mut docs = agent_docs("chat", "Chat");
    docs.push(model_doc("gateway"));
    let all = file(&api.dir, "all.yaml", &docs);
    let all = all.to_str().unwrap();

    // (arguments, the token in the environment, standard input, the exit code that makes the run a real one: a command that is
    // not there at all would print nothing and leak nothing)
    type Run<'a> = (Vec<&'a str>, Option<&'a str>, Option<&'a str>, i32);
    let runs: Vec<Run> = vec![
        (
            vec!["login", "--token-stdin", "--verbose"],
            None,
            Some(TOKEN),
            0,
        ),
        (vec!["whoami", "--verbose"], None, None, 0),
        (vec!["apply", "-f", all, "--verbose"], None, None, 0),
        (vec!["apply", "-f", all, "--verbose"], Some(TOKEN), None, 0),
        (vec!["get", "agents", "--verbose"], Some(TOKEN), None, 0),
        (
            vec!["get", "model", "gateway", "-o", "json", "--verbose"],
            None,
            None,
            0,
        ),
        (vec!["get", "agent", "nope", "--verbose"], None, None, 1),
        (vec!["export", "agent", "chat", "--verbose"], None, None, 0),
        (
            vec!["delete", "model", "gateway", "--verbose"],
            None,
            None,
            1,
        ),
        (vec!["delete", "agent", "chat", "--verbose"], None, None, 0),
        (vec!["logout", "--verbose"], None, None, 0),
        (vec!["whoami", "--verbose"], Some(TOKEN), None, 0),
        (vec!["login", "--verbose"], None, None, 2),
        (
            vec!["apply", "-f", "/nonexistent/file.yaml", "--verbose"],
            Some(TOKEN),
            None,
            2,
        ),
    ];
    let mut spoke = 0;
    for (args, env, stdin, code) in runs {
        let out = api.ctl_in(&args, env, stdin);
        assert_eq!(out.code, code, "{args:?}: {}", out.stderr);
        spoke += out.stdout.len() + out.stderr.len();
        for (stream, text) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
            leaks(&format!("{args:?} {stream}"), text);
        }
    }
    assert!(
        spoke > 500,
        "the commands said almost nothing: {spoke} bytes"
    );
    // The ones that fail in the transport, and on the address.
    let dir = &api.dir;
    let config = api.config();
    let config = config.to_str().unwrap();
    for server in [
        "http://127.0.0.1:1",
        "http://api.example.com",
        "ftp://x",
        "https://u:p@x/?code=1",
    ] {
        let out = api::run_binary(
            &[
                "get",
                "agents",
                "--verbose",
                "--server",
                server,
                "--config",
                config,
            ],
            Some(TOKEN),
            None,
            dir,
        );
        leaks(server, &out.stdout);
        leaks(server, &out.stderr);
    }
}

fn leaks(what: &str, text: &str) {
    let chars: Vec<char> = TOKEN.chars().collect();
    for window in chars.windows(16) {
        let w: String = window.iter().collect();
        assert!(
            !text.contains(&w),
            "{what}: {w:?} of the token is in\n{text}"
        );
    }
}

#[test]
fn the_config_file_is_written_whole_or_not_at_all() {
    let api = Api::start("config-write");
    // A directory that does not exist is made, for the owner alone.
    let nested = api.dir.join("nested").join("aapctl").join("config.yaml");
    let out = api::run_binary(
        &[
            "login",
            "--token-stdin",
            "--server",
            &api.url,
            "--config",
            nested.to_str().unwrap(),
        ],
        None,
        Some(TOKEN),
        &api.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let dir_mode = std::fs::metadata(nested.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    // No temporary file is left beside it.
    let left: Vec<_> = std::fs::read_dir(nested.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, ["config.yaml"]);
}

// ------------------------------------------------------------------------------------------ phase 1 is untouched

/// The environment's proxy is honoured for a server that is not a plain-http loopback one (the harness removes the proxy variables
/// from every child so that a machine that sets `HTTPS_PROXY` does not route the test's own server through it), and **never for an
/// `http://` loopback server**, whatever `NO_PROXY` says: a proxy would be handed the bearer token in the clear for a server on the
/// same machine (P-031). `hardening.rs` counts the connections a proxy sees; this test shows the request still succeeds with
/// nothing listening at the proxy it names.
#[test]
fn a_loopback_server_is_reached_even_when_the_proxy_the_environment_names_is_down() {
    let api = Api::start("proxy");
    let run = |no_proxy: Option<&str>| {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_aapctl"));
        c.args([
            "whoami",
            "--server",
            &api.url,
            "--config",
            api.config().to_str().unwrap(),
        ]);
        for v in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            c.env_remove(v);
        }
        // Nothing listens on port 1: a request that goes through this proxy cannot succeed.
        c.env("HTTP_PROXY", "http://127.0.0.1:1")
            .env("AAPCTL_TOKEN", TOKEN)
            .env("HOME", &api.dir);
        if let Some(n) = no_proxy {
            c.env("NO_PROXY", n);
        }
        let o = c.output().unwrap();
        (
            o.status.code().unwrap(),
            String::from_utf8(o.stderr).unwrap(),
        )
    };
    for no_proxy in [Some("127.0.0.1"), None] {
        let (code, err) = run(no_proxy);
        assert_eq!(code, 0, "{no_proxy:?}: {err}");
    }
}

#[test]
fn validate_and_render_read_no_environment_and_no_network() {
    let dir = common::scratch("phase-1");
    let example = common::example("chat.yaml");
    let path = example.to_str().unwrap();
    let plain = api::run_binary(
        &[
            "validate",
            "-f",
            path,
            "--default-agent-image",
            common::IMAGE,
        ],
        None,
        None,
        &dir,
    );
    let with = {
        let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_aapctl"));
        c.args([
            "validate",
            "-f",
            path,
            "--default-agent-image",
            common::IMAGE,
        ])
        .env("AAPCTL_TOKEN", TOKEN)
        .env("HTTPS_PROXY", "http://127.0.0.1:1")
        .env("XDG_CONFIG_HOME", "/nonexistent");
        let o = c.output().unwrap();
        (
            o.status.code().unwrap(),
            String::from_utf8(o.stdout).unwrap(),
            String::from_utf8(o.stderr).unwrap(),
        )
    };
    assert_eq!((plain.code, plain.stdout, plain.stderr), with);
}
