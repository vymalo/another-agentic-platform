//! A form to the objects and back: nothing is lost, for every form.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use aap_forms::{
    AgentForm, AgentKind, AgentKindForm, AgentView, BaseUrlForm, ModelEndpointForm, Representation,
    ToolServerForm, load_agent, to_resources,
};
use aap_ports::{Kind, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, Owner};
use proptest::prelude::*;
use support::*;

fn view_of(form: &AgentForm, deployment: &aap_forms::Deployment) -> Option<AgentView> {
    let agent = agent_of(&to_resources(form, deployment, NS), true);
    let loaded = load_agent(&agent);
    assert_eq!(loaded.managed_by, Owner::Dashboard);
    match loaded.representation {
        Representation::Form(view) => Some(view),
        Representation::YamlOnly(reasons) => panic!("a form must be a form: {reasons:?}"),
    }
}

#[test]
fn the_form_of_a_coder_round_trips_with_the_defaults_filled_in() {
    let defaults = defaults();
    let form = coder_form("coder-me");
    let deployment = defaults.deployment_for(AgentKind::Coder, "coder-me");
    let view = view_of(&form, &deployment).unwrap();
    assert_eq!(view.form, form);
    assert_eq!(
        view.deployment, deployment,
        "the defaults are the deployment's part, not the form's"
    );
    assert_eq!(view.kind(), AgentKind::Coder);
}

#[test]
fn a_form_with_every_optional_field_set_round_trips() {
    // Every `Option` is `Some`, every list has an element: no field can be dropped unseen.
    let mut form = coder_form("coder-max");
    form.description = Some("A description.".to_owned());
    form.tags = vec!["one".to_owned(), "two-words".to_owned()];
    form.suspend = true;
    form.tool_servers = ["context7".to_owned(), "websearch".to_owned()].into();
    form.image = Some("registry.invalid/coder:sha-1234567".to_owned());
    form.audience = Some(vec![
        "agent.use:coder-max".to_owned(),
        "agent.use:other".to_owned(),
    ]);
    if let AgentKindForm::Coder(c) = &mut form.agent {
        c.github.owners = vec!["vymalo".to_owned(), "stephane-segning".to_owned()];
        c.create_repo_owners = vec!["stephane-segning".to_owned()];
        c.workers = Some(16);
        c.pr_draft = Some(false);
        c.opencode_model = Some("another-model".to_owned());
        c.run_pod_size_class = Some("large".to_owned());
    }
    let deployment = defaults().deployment_for(AgentKind::Coder, "coder-max");
    let view = view_of(&form, &deployment).unwrap();
    assert_eq!(view.form, form);
    assert_eq!(view.deployment, deployment);
    // And a folder agent with the same.
    let mut folder = folder_form("chat-max");
    folder.description = Some("d".to_owned());
    folder.suspend = true;
    folder.tool_servers = ["websearch".to_owned()].into();
    folder.allow_insecure_http = true;
    folder.image = Some("registry.invalid/agent:tag".to_owned());
    folder.audience = Some(Vec::new());
    let deployment = defaults().deployment_for(AgentKind::Folder, "chat-max");
    let view = view_of(&folder, &deployment).unwrap();
    assert_eq!(view.form, folder);
}

#[test]
fn the_form_of_a_folder_agent_round_trips() {
    let form = folder_form("chat");
    let deployment = defaults().deployment_for(AgentKind::Folder, "chat");
    let view = view_of(&form, &deployment).unwrap();
    assert_eq!(view.form, form);
    assert_eq!(view.deployment, deployment);
}

#[test]
fn the_objects_of_a_coder_are_the_ones_of_the_text() {
    // §60a, "What the custom resources gain": the coder-me example, field by field.
    let defaults = defaults();
    let r = to_resources(
        &coder_form("coder-me"),
        &defaults.deployment_for(AgentKind::Coder, "coder-me"),
        NS,
    );
    let service = j(&r.service.spec);
    assert_eq!(service["configRef"]["name"], "coder-me");
    assert_eq!(
        service["registry"],
        json_str(r#"{"title":"Coder (me)","tags":["coding","git"]}"#)
    );
    assert_eq!(
        service["access"]["audience"],
        json_str(r#"["agent.use:coder-me"]"#)
    );
    assert_eq!(service["interfaces"]["a2a"]["enabled"], true);
    assert_eq!(
        service["interfaces"]["a2a"]["bearerTokensSecretRef"],
        json_str(r#"{"name":"agent-a2a","key":"AGENT_REGISTRY_AGENT_TOKEN"}"#),
        "the bearer is the deployment's"
    );
    assert_eq!(service["store"]["postgres"]["cnpg"]["instances"], 1);
    let config = j(&r.config.spec);
    assert_eq!(config["harness"]["adam"]["binary"], "adam-coder");
    assert_eq!(
        config["harness"]["adam"]["agent"],
        json_str(r#"{"embedded":{}}"#)
    );
    let app = &config["harness"]["adam"]["coder"]["github"]["app"];
    assert_eq!(app["id"], "Iv1.example");
    assert_eq!(app["owners"], json_str(r#"["stephane-segning"]"#));
    assert_eq!(
        app["privateKeySecretRef"],
        json_str(r#"{"name":"github-app","key":"private-key.pem"}"#)
    );
    assert_eq!(
        config["model"],
        json_str(r#"{"model":"coding-model","endpointRef":{"name":"gateway"}}"#)
    );
    let servers = config["tools"]["mcpServers"].as_object().unwrap();
    assert_eq!(servers.len(), 1);
    assert_eq!(
        servers["websearch"]["providerRef"],
        json_str(r#"{"name":"websearch"}"#),
        "a server is a reference to the ToolProvider of its name, with no url and no header"
    );
    assert!(servers["websearch"].get("url").is_none());
    assert_eq!(servers["websearch"]["optional"], false);
    assert_eq!(config["tools"]["allowInsecureHttp"], true);
    assert!(
        config["environment"].get("image").is_none(),
        "no image: the operator's default (AD-029)"
    );
    assert_eq!(
        config["harness"]["adam"]["coder"]["gitAuthor"]["name"],
        "bored-giant-panda[bot]"
    );
}

fn json_str(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap()
}

#[test]
fn every_object_the_forms_make_carries_the_label_of_the_dashboard() {
    let defaults = defaults();
    let r = to_resources(
        &coder_form("coder-me"),
        &defaults.deployment_for(AgentKind::Coder, "coder-me"),
        NS,
    );
    for meta in [&r.service.metadata, &r.config.metadata] {
        assert_eq!(meta.namespace.as_deref(), Some(NS));
        assert_eq!(meta.name.as_deref(), Some("coder-me"));
        assert_eq!(
            meta.labels
                .as_ref()
                .unwrap()
                .get(MANAGED_BY_LABEL)
                .map(String::as_str),
            Some(MANAGED_BY_DASHBOARD)
        );
    }
    let model = model_form("gateway").to_resource(NS);
    let tool = tool_form("websearch").to_resource(NS);
    for meta in [&model.metadata, &tool.metadata] {
        assert_eq!(meta.namespace.as_deref(), Some(NS));
        assert_eq!(
            meta.labels
                .as_ref()
                .unwrap()
                .get(MANAGED_BY_LABEL)
                .map(String::as_str),
            Some(MANAGED_BY_DASHBOARD)
        );
    }
}

#[test]
fn the_draft_is_the_two_specs_and_the_name() {
    let r = to_resources(
        &folder_form("chat"),
        &defaults().deployment_for(AgentKind::Folder, "chat"),
        NS,
    );
    let draft = r.draft();
    assert_eq!(draft.name, "chat");
    assert_eq!(draft.service, j(&r.service.spec));
    assert_eq!(draft.config, j(&r.config.spec));
    assert_eq!(
        aap_ports::config_name(&draft.service),
        Some("chat"),
        "the store requires it"
    );
}

proptest! {
    #![proptest_config(config(256))]

    /// The first law: a form, with any deployment part, gives objects that read back as that form and that part.
    #[test]
    fn every_form_round_trips((form, deployment) in arb_agent()) {
        let view = view_of(&form, &deployment).unwrap();
        prop_assert_eq!(&view.form, &form);
        prop_assert_eq!(&view.deployment, &deployment);
    }

    /// The same through JSON, which is how a form reaches the API.
    #[test]
    fn every_form_survives_its_own_json((form, _d) in arb_agent()) {
        let text = serde_json::to_string(&form).unwrap();
        let back: AgentForm = serde_json::from_str(&text).unwrap();
        prop_assert_eq!(back, form);
    }

    #[test]
    fn every_model_endpoint_form_round_trips(form in arb_model_form()) {
        let draft = form.draft();
        let stored = stored(Kind::ModelEndpoint, &form.name, draft.spec, true);
        let loaded = ModelEndpointForm::load(&stored);
        prop_assert_eq!(loaded.managed_by, Owner::Dashboard);
        prop_assert_eq!(loaded.form(), Some(&form));
        // And the object says what the draft says.
        prop_assert_eq!(j(&form.to_resource(NS).spec), stored.spec);
    }

    #[test]
    fn every_tool_server_form_round_trips(form in arb_tool_form()) {
        let draft = form.draft();
        let stored = stored(Kind::ToolProvider, &form.name, draft.spec, true);
        let loaded = ToolServerForm::load(&stored);
        prop_assert_eq!(loaded.managed_by, Owner::Dashboard);
        prop_assert_eq!(loaded.form(), Some(&form));
        prop_assert_eq!(j(&form.to_resource(NS).spec), stored.spec);
    }
}

#[test]
fn the_wire_form_of_a_coder_is_the_one_the_clients_are_told() {
    let form: AgentForm = serde_json::from_value(example_json_of_a_coder_form()).unwrap();
    assert_eq!(
        form,
        coder_form("coder-me").tap(|f| {
            if let aap_forms::AgentKindForm::Coder(c) = &mut f.agent {
                c.opencode_model = Some("coding-model".to_owned());
            }
        })
    );
    assert_eq!(j(&form), example_json_of_a_coder_form(), "and back");
}

#[test]
fn a_coder_with_run_pods_has_the_size_class_on_the_wire_and_one_without_has_no_key() {
    let mut wire = example_json_of_a_coder_form();
    wire["agent"]["runPodSizeClass"] = "large".into();
    let form: AgentForm = serde_json::from_value(wire.clone()).unwrap();
    let AgentKindForm::Coder(c) = &form.agent else {
        panic!("a coder")
    };
    assert_eq!(c.run_pod_size_class.as_deref(), Some("large"));
    assert_eq!(j(&form), wire, "and back");
    assert!(
        example_json_of_a_coder_form()["agent"]
            .get("runPodSizeClass")
            .is_none(),
        "the wire form of a coder with no run pods is the one it was"
    );
}

trait Tap: Sized {
    fn tap(mut self, f: impl FnOnce(&mut Self)) -> Self {
        f(&mut self);
        self
    }
}
impl<T> Tap for T {}

#[test]
fn the_wire_form_of_a_model_endpoint_and_a_tool_server() {
    let model: ModelEndpointForm = serde_json::from_value(json_str(
        r#"{"name":"gateway","title":"Gateway",
            "baseUrl":{"secret":{"secret":"agent-models","key":"GATEWAY_BASE_URL"}},
            "apiKey":{"secret":"agent-models","key":"GATEWAY_API_KEY"},
            "models":["coding-model","chat-model"]}"#,
    ))
    .unwrap();
    assert_eq!(model, model_form("gateway"));
    let by_value: ModelEndpointForm = serde_json::from_value(json_str(
        r#"{"name":"g","title":"G","baseUrl":{"value":"https://x.invalid/v1"},"apiKey":{"secret":"s","key":"k"}}"#,
    ))
    .unwrap();
    assert_eq!(
        by_value.base_url,
        BaseUrlForm::Value("https://x.invalid/v1".to_owned())
    );
    assert!(by_value.models.is_empty(), "the aliases are optional");

    let tool: ToolServerForm = serde_json::from_value(json_str(
        r#"{"name":"websearch","title":"Web search",
            "url":"http://another-agentic-websearch.another-agentic-system.svc:8080/mcp",
            "headers":{"Authorization":{"prefix":"Bearer ","secret":{"secret":"agent-tools","key":"SEARCH_MCP_TOKEN"}}},
            "tools":[],"optional":true}"#,
    ))
    .unwrap();
    assert_eq!(tool, tool_form("websearch"));
}

#[test]
fn a_folder_form_cannot_carry_what_only_a_coder_has_and_the_reverse() {
    let mut wire = example_json_of_a_coder_form();
    wire["agent"]["kind"] = "folder".into();
    assert!(
        serde_json::from_value::<AgentForm>(wire).is_err(),
        "a folder with a GitHub App is not a form"
    );
    let mut wire = serde_json::to_value(folder_form("chat")).unwrap();
    wire["agent"]["kind"] = "coder".into();
    assert!(
        serde_json::from_value::<AgentForm>(wire).is_err(),
        "a coder with files is not a form"
    );
    let mut wire = example_json_of_a_coder_form();
    wire["agent"]["kind"] = "mystery".into();
    assert!(serde_json::from_value::<AgentForm>(wire).is_err());
}

#[test]
fn a_new_agent_gets_the_defaults_and_an_old_one_keeps_what_it_holds() {
    let a = defaults();
    let form = coder_form("coder-me");
    let original = a.deployment_for(AgentKind::Coder, "coder-me");
    let objects = to_resources(&form, &original, NS);

    // The deployment's defaults change: another store, another host list, another bearer.
    let mut b = a.clone();
    b.store = aap_forms::StoreDefault::SecretRef(aap_forms::SecretPattern {
        name: "{name}-db-app".to_owned(),
        key: "uri".to_owned(),
    });
    b.coder.allowed_repo_hosts = vec!["git.example.invalid".to_owned()];
    b.a2a_bearer.name = "another-secret".to_owned();
    let for_a_new_one = b.deployment_for(AgentKind::Coder, "another");
    assert_ne!(for_a_new_one, original);
    assert_eq!(
        for_a_new_one
            .store
            .postgres
            .secret_ref
            .as_ref()
            .unwrap()
            .name,
        "another-db-app",
        "{{name}} is the agent's name"
    );

    // An edit of the existing agent: read it, change a field, write it back with what it holds.
    let loaded = load_agent(&agent_of(&objects, true));
    let mut view = loaded.form().unwrap().clone();
    view.form.title = "A new title".to_owned();
    let saved = to_resources(&view.form, &view.deployment, NS);
    let (mut before, mut after) = (j(&objects.service.spec), j(&saved.service.spec));
    assert_eq!(after["registry"]["title"], "A new title");
    for doc in [&mut before, &mut after] {
        doc["registry"]["title"] = "-".into();
    }
    assert_eq!(
        before, after,
        "an edit of the title moves nothing else of the service"
    );
    assert_eq!(
        j(&objects.config.spec),
        j(&saved.config.spec),
        "nor of the config: the store, the hosts and the bearer stay"
    );
    assert_eq!(saved.service.spec.store, original.store);
    assert_eq!(
        saved
            .config
            .spec
            .harness
            .adam
            .coder
            .as_ref()
            .unwrap()
            .allowed_repo_hosts,
        ["github.com"]
    );

    // Applying the new defaults is a choice of the caller: it gives the form the deployment of `deployment_for`.
    let converged = to_resources(&view.form, &for_a_new_one, NS);
    assert_ne!(j(&converged.service.spec), j(&saved.service.spec));
}

#[test]
fn the_defaults_are_in_no_form() {
    // A form says nothing of the store, the bearer, the peers, the hosts, the volume, the resources or the security.
    let d = defaults();
    let form_json = serde_json::to_string(&coder_form("coder-me")).unwrap();
    for deployment_only in [
        "agent-a2a",
        "AGENT_REGISTRY_AGENT_TOKEN",
        "github.com",
        "api.github.com",
        "longhorn",
        "bored-giant-panda",
        "10001",
        "cnpg",
    ] {
        assert!(
            !form_json.contains(deployment_only),
            "{deployment_only} is in the form"
        );
    }
    let _ = d;
}
