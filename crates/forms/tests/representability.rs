//! An object is a form, or it is YAML only and says why: a form never loses anything.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use aap_forms::{
    AgentKind, BaseUrlForm, ModelEndpointForm, Representation, ToolServerForm, Unrepresentable,
    load_agent, to_resources,
};
use aap_ports::{ARGO_TRACKING_ANNOTATION, Agent, Kind, Owner};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::*;

fn reasons_of(agent: &Agent) -> Vec<Unrepresentable> {
    let loaded = load_agent(agent);
    match loaded.representation {
        Representation::YamlOnly(r) => {
            assert!(!r.is_empty(), "a YAML-only object has its reasons");
            r
        }
        Representation::Form(_) => Vec::new(),
    }
}

fn paths(reasons: &[Unrepresentable]) -> Vec<String> {
    reasons
        .iter()
        .map(|r| format!("{} {}", r.object, r.path))
        .collect()
}

fn coder_agent() -> Agent {
    let d = defaults();
    agent_of(
        &to_resources(
            &coder_form("coder-me"),
            &d.deployment_for(AgentKind::Coder, "coder-me"),
            NS,
        ),
        true,
    )
}

fn folder_agent() -> Agent {
    let d = defaults();
    agent_of(
        &to_resources(
            &folder_form("chat"),
            &d.deployment_for(AgentKind::Folder, "chat"),
            NS,
        ),
        true,
    )
}

// ----------------------------------------------------------------- the examples

#[test]
fn the_coder_me_example_is_a_form_whoever_owns_it() {
    // The example of §60a, as objects: a form can say all of it.
    for (dashboard, owner) in [(true, Owner::Dashboard), (false, Owner::GitOps)] {
        let loaded = load_agent(&example_agent("coder-me.yaml", dashboard));
        assert_eq!(loaded.managed_by, owner);
        let view = loaded
            .form()
            .unwrap_or_else(|| panic!("{:?}", loaded.reasons()));
        assert_eq!(view.kind(), AgentKind::Coder);
        let f = &view.form;
        assert_eq!(f.name, "coder-me");
        assert_eq!(f.title, "Coder (me)");
        assert_eq!(f.model.endpoint, "gateway");
        assert_eq!(f.tool_servers, ["websearch".to_owned()].into());
        assert_eq!(
            f.audience.as_deref(),
            Some(&["agent.use:coder-me".to_owned()][..])
        );
        // The store, the bearer and the peers are the deployment's part, as the object holds them.
        assert!(view.deployment.store.postgres.cnpg.is_some());
        assert_eq!(
            view.deployment.a2a_bearer.as_ref().unwrap().name,
            "coder-me-secrets"
        );
        assert_eq!(view.deployment.allow_from.len(), 1);
    }
}

#[test]
fn the_netcup_coder_and_chat_are_yaml_only_and_say_what_to_change() {
    // examples/coder.yaml and chat.yaml are the GitOps objects the dashboard takes over (AD-031): they
    // set the model and the tool servers inline, and a form names ModelEndpoints and ToolProviders.
    let coder = load_agent(&example_agent("coder.yaml", false));
    assert_eq!(coder.managed_by, Owner::GitOps);
    let paths_of_coder = paths(coder.reasons());
    for expected in [
        "AgentConfig spec.model.baseUrl",
        "AgentConfig spec.model.apiKeySecretRef",
        "AgentConfig spec.tools.mcpServers.websearch.url",
        "AgentConfig spec.tools.mcpServers.context7.url",
        "AgentConfig spec.harness.adam.coder.maxCheckCycles",
        "AgentService spec.interfaces.a2a.publicUrl",
    ] {
        assert!(
            paths_of_coder.iter().any(|p| p == expected),
            "{expected} is not among {paths_of_coder:#?}"
        );
    }
    let chat = load_agent(&example_agent("chat.yaml", false));
    assert!(chat.form().is_none());
    let chat_paths = paths(chat.reasons());
    assert!(
        chat_paths.contains(&"AgentConfig spec.model.baseUrl".to_owned()),
        "{chat_paths:?}"
    );
    assert!(
        chat_paths
            .iter()
            .all(|p| p.starts_with("AgentConfig spec.model")),
        "the model is the only thing of chat a form cannot say: {chat_paths:?}"
    );
}

// ------------------------------------------ what the deployment's part may hold

#[test]
fn the_deployments_part_is_read_whatever_it_holds() {
    let d = defaults();
    // Change every field the deployment owns, in the objects, as GitOps or an older default would have.
    let mut agent = coder_agent();
    set(
        &mut agent.service.spec,
        "/store",
        json!({ "postgres": { "secretRef": { "name": "elsewhere", "key": "uri" } } }),
    );
    set(
        &mut agent.service.spec,
        "/interfaces/a2a/bearerTokensSecretRef",
        json!({ "name": "other", "key": "TOKENS" }),
    );
    set(&mut agent.service.spec, "/access/allowFrom", json!([]));
    {
        let config = agent.config.as_mut().unwrap();
        set(&mut config.spec, "/security", json!({ "runAsUser": 2000 }));
        set(
            &mut config.spec,
            "/environment/resources",
            json!({ "limits": { "memory": "9Gi" } }),
        );
        set(&mut config.spec, "/environment/volumes", json!([]));
        set(
            &mut config.spec,
            "/tools/githubMcp",
            json!({ "sidecar": false, "port": 9000 }),
        );
        set(
            &mut config.spec,
            "/harness/adam/coder/gitAuthor",
            json!({ "name": "n", "email": "e@x.invalid" }),
        );
        set(
            &mut config.spec,
            "/harness/adam/coder/allowedRepoHosts",
            json!(["git.example.invalid"]),
        );
        set(
            &mut config.spec,
            "/harness/adam/coder/githubApiUrl",
            json!("https://ghe.example.invalid/api"),
        );
    }

    let loaded = load_agent(&agent);
    let view = loaded
        .form()
        .unwrap_or_else(|| panic!("{:?}", loaded.reasons()));
    let held = &view.deployment;
    assert_eq!(
        held.store.postgres.secret_ref.as_ref().unwrap().name,
        "elsewhere"
    );
    assert_eq!(held.a2a_bearer.as_ref().unwrap().name, "other");
    assert!(held.allow_from.is_empty());
    assert_eq!(held.security.run_as_user, Some(2000));
    assert!(held.volumes.is_empty() && held.resources.is_some());
    assert_eq!(held.github_mcp.as_ref().unwrap().port, 9000);
    assert_eq!(held.coder.allowed_repo_hosts, ["git.example.invalid"]);
    // It differs from today's defaults, and the form is the same one: that is the point.
    assert_ne!(*held, d.deployment_for(AgentKind::Coder, "coder-me"));
    assert_eq!(view.form, coder_form("coder-me"));
    // A save with what it holds writes the objects back as they were.
    let again = to_resources(&view.form, held, NS).draft();
    assert_eq!(again.service, agent.service.spec_typed_json());
    assert_eq!(
        again.config,
        agent.config.as_ref().unwrap().spec_typed_json()
    );
}

/// The spec as the types write it (a spec read and written again), to compare with a draft.
trait TypedJson {
    fn spec_typed_json(&self) -> Value;
}
impl TypedJson for aap_ports::Stored {
    fn spec_typed_json(&self) -> Value {
        match self.kind {
            Kind::AgentService => j(&serde_json::from_value::<aap_api::AgentServiceSpec>(
                self.spec.clone(),
            )
            .unwrap()),
            Kind::AgentConfig => {
                j(&serde_json::from_value::<aap_api::AgentConfigSpec>(self.spec.clone()).unwrap())
            }
            other => panic!("{other}"),
        }
    }
}

// --------------------------------------------- what no form and no deployment says

/// A change of one field, and the field a reason must name.
struct Change {
    object: Kind,
    pointer: &'static str,
    value: Value,
    reason_path: &'static str,
}

fn change(object: Kind, pointer: &'static str, value: Value, reason_path: &'static str) -> Change {
    Change {
        object,
        pointer,
        value,
        reason_path,
    }
}

fn coder_changes() -> Vec<Change> {
    let s = Kind::AgentService;
    let c = Kind::AgentConfig;
    vec![
        change(
            s,
            "/interfaces/a2a/publicUrl",
            json!("http://elsewhere.invalid/"),
            "spec.interfaces.a2a.publicUrl",
        ),
        change(
            s,
            "/interfaces/a2a/enabled",
            json!(false),
            "spec.interfaces.a2a.enabled",
        ),
        change(
            s,
            "/interfaces/mcp/enabled",
            json!(true),
            "spec.interfaces.mcp.enabled",
        ),
        change(
            s,
            "/interfaces/responses/enabled",
            json!(true),
            "spec.interfaces.responses.enabled",
        ),
        change(
            s,
            "/scaling/topology",
            json!("split"),
            "spec.scaling.topology",
        ),
        change(s, "/scaling/workers", json!(3), "spec.scaling.workers"),
        change(
            s,
            "/scaling/front",
            json!({ "replicas": 2 }),
            "spec.scaling.front",
        ),
        change(s, "/deletionPolicy", json!("Delete"), "spec.deletionPolicy"),
        change(
            c,
            "/harness/adam/coder/maxCheckCycles",
            json!(3),
            "spec.harness.adam.coder.maxCheckCycles",
        ),
        change(
            c,
            "/harness/adam/coder/checkTimeoutSecs",
            json!(900),
            "spec.harness.adam.coder.checkTimeoutSecs",
        ),
        change(
            c,
            "/harness/adam/coder/workspaceSweepSecs",
            json!(300),
            "spec.harness.adam.coder.workspaceSweepSecs",
        ),
        change(
            c,
            "/harness/adam/coder/workspacePlacement",
            json!("shared"),
            "spec.harness.adam.coder.workspacePlacement",
        ),
        change(
            c,
            "/harness/adam/coder/github",
            json!({ "token": { "secretRef": { "name": "s", "key": "k" } } }),
            "spec.harness.adam.coder.github.token",
        ),
        change(
            c,
            "/harness/adam/coder/github/app/installationId",
            json!(12345),
            "spec.harness.adam.coder.github.app.installationId",
        ),
        change(
            c,
            "/extraEnv",
            json!({ "FEATURE": "on" }),
            "spec.extraEnv.FEATURE",
        ),
        change(
            c,
            "/environment/terminationGracePeriodSeconds",
            json!(120),
            "spec.environment.terminationGracePeriodSeconds",
        ),
        change(
            c,
            "/model",
            json!({ "model": "m", "baseUrl": { "value": "https://x.invalid/v1" }, "apiKeySecretRef": { "name": "s", "key": "k" } }),
            "spec.model.baseUrl",
        ),
        change(
            c,
            "/tools/mcpServers/websearch",
            json!({ "url": "https://x.invalid/mcp" }),
            "spec.tools.mcpServers.websearch.url",
        ),
        change(
            c,
            "/tools/mcpServers/websearch/optional",
            json!(true),
            "spec.tools.mcpServers.websearch.optional",
        ),
        change(
            c,
            "/tools/mcpServers/other",
            json!({ "url": "https://y.invalid/mcp" }),
            "spec.tools.mcpServers.other.url",
        ),
    ]
}

fn apply(agent: &mut Agent, change: &Change) {
    let object = match change.object {
        Kind::AgentService => &mut agent.service,
        _ => agent.config.as_mut().unwrap(),
    };
    set(&mut object.spec, change.pointer, change.value.clone());
}

#[test]
fn a_field_that_no_form_and_no_deployment_says_makes_the_object_yaml_only_and_is_named() {
    for ch in coder_changes() {
        let mut agent = coder_agent();
        assert!(reasons_of(&agent).is_empty(), "the base is a form");
        apply(&mut agent, &ch);
        let reasons = reasons_of(&agent);
        let wanted = format!("{} {}", ch.object, ch.reason_path);
        assert!(
            paths(&reasons).contains(&wanted),
            "{}: {wanted} is not named by {:#?}",
            ch.pointer,
            paths(&reasons)
        );
    }
}

#[test]
fn a_folder_that_is_a_config_map_is_yaml_only() {
    let mut agent = folder_agent();
    set(
        &mut agent.config.as_mut().unwrap().spec,
        "/harness/adam/agent/folder",
        json!({ "configMapRef": { "name": "chat-agent" } }),
    );
    let reasons = paths(&reasons_of(&agent));
    assert!(
        reasons.contains(&"AgentConfig spec.harness.adam.agent.folder.configMapRef".to_owned()),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&"AgentConfig spec.harness.adam.agent.folder.files".to_owned()),
        "{reasons:?}"
    );
}

#[test]
fn a_service_whose_config_is_missing_is_yaml_only() {
    let mut agent = coder_agent();
    agent.config = None;
    let reasons = reasons_of(&agent);
    assert_eq!(paths(&reasons), ["AgentService spec.configRef.name"]);
}

#[test]
fn a_member_this_version_does_not_know_is_never_dropped_silently() {
    // An API server prunes what the CRD does not know, but a dashboard that is older than the CRD must not be the
    // one to lose it: serde would.
    for (kind, pointer, path) in [
        (Kind::AgentService, "/futureField", "spec.futureField"),
        (
            Kind::AgentService,
            "/registry/futureField",
            "spec.registry.futureField",
        ),
        (
            Kind::AgentConfig,
            "/harness/adam/coder/github/app/futureField",
            "spec.harness.adam.coder.github.app.futureField",
        ),
        (
            Kind::AgentConfig,
            "/model/futureField",
            "spec.model.futureField",
        ),
    ] {
        let mut agent = coder_agent();
        let object = if kind == Kind::AgentService {
            &mut agent.service
        } else {
            agent.config.as_mut().unwrap()
        };
        set(&mut object.spec, pointer, json!("x"));
        let reasons = reasons_of(&agent);
        assert!(!reasons.is_empty(), "{path}");
        let r = reasons
            .iter()
            .find(|r| r.path == path)
            .unwrap_or_else(|| panic!("{path}: {reasons:?}"));
        assert!(r.reason.contains("does not know"), "{}", r.reason);
    }
    // An explicit null is absence.
    let mut agent = coder_agent();
    set(&mut agent.service.spec, "/description", Value::Null);
    set(&mut agent.service.spec, "/publicNothing", Value::Null);
    let loaded = load_agent(&agent);
    assert!(loaded.form().is_some(), "{:?}", loaded.reasons());
}

#[test]
fn a_spec_that_is_not_the_shape_of_its_kind_is_yaml_only() {
    let mut agent = coder_agent();
    agent.service.spec = json!({ "configRef": 3 });
    let reasons = reasons_of(&agent);
    assert_eq!(reasons[0].object, Kind::AgentService);
    let mut agent = coder_agent();
    agent.config.as_mut().unwrap().spec = json!("nope");
    assert_eq!(reasons_of(&agent)[0].object, Kind::AgentConfig);
}

#[test]
fn a_reason_names_a_field_and_never_quotes_a_value() {
    let mut agent = coder_agent();
    set(
        &mut agent.config.as_mut().unwrap().spec,
        "/extraEnv",
        json!({ "TOKEN": SENTINEL }),
    );
    set(
        &mut agent.config.as_mut().unwrap().spec,
        "/model",
        json!({ "model": "m", "baseUrl": { "value": format!("https://user:{SENTINEL}@x.invalid/v1") }, "apiKeySecretRef": { "name": "s", "key": "k" } }),
    );
    set(
        &mut agent.service.spec,
        "/interfaces/a2a/publicUrl",
        json!(format!("http://x.invalid/?k={SENTINEL}")),
    );
    // An unknown member too, and a shape error.
    set(&mut agent.service.spec, "/registry/oops", json!(SENTINEL));
    let reasons = reasons_of(&agent);
    assert!(reasons.len() >= 4);
    for r in &reasons {
        assert!(
            !format!("{r} {r:?}").contains(SENTINEL),
            "a value is in {r:?}"
        );
    }
    let mut broken = coder_agent();
    broken.service.spec = json!({ "configRef": SENTINEL });
    for r in reasons_of(&broken) {
        assert!(
            !format!("{r} {r:?}").contains(SENTINEL),
            "a value is in {r:?}"
        );
    }
}

#[test]
fn a_gitops_object_a_form_can_say_is_a_form_for_a_gitops_owner() {
    // The object carries the dashboard's label and Argo CD's annotation: Argo CD owns it.
    let mut agent = coder_agent();
    agent
        .service
        .annotations
        .insert(ARGO_TRACKING_ANNOTATION.to_owned(), "app:x".to_owned());
    let loaded = load_agent(&agent);
    assert_eq!(loaded.managed_by, Owner::GitOps);
    assert!(
        loaded.form().is_some(),
        "it is shown as a form, and the client does not let it be saved"
    );
}

proptest! {
    #![proptest_config(config(256))]

    /// The second law: an object is a form only if the form writes it back unchanged. For any form and any one
    /// change of a field a form does not say, the object is YAML only and names the field.
    #[test]
    fn an_object_is_a_form_only_if_the_form_writes_it_back(
        (form, deployment) in arb_agent(),
        pick in 0usize..64,
    ) {
        // A change that needs a part the agent does not have is not a change of this agent.
        let changes: Vec<Change> = coder_changes()
            .into_iter()
            .filter(|c| form.kind() == AgentKind::Coder || !c.pointer.starts_with("/harness/adam/coder"))
            .filter(|c| c.pointer != "/tools/mcpServers/websearch/optional" || form.tool_servers.contains("websearch"))
            .collect();
        let ch = &changes[pick % changes.len()];
        let mut agent = agent_of(&to_resources(&form, &deployment, NS), true);
        apply(&mut agent, ch);
        let loaded = load_agent(&agent);
        match &loaded.representation {
            Representation::Form(view) => {
                // Then the form writes the object back exactly: nothing was lost.
                let again = to_resources(&view.form, &view.deployment, NS).draft();
                prop_assert_eq!(again.service, agent.service.spec_typed_json());
                prop_assert_eq!(again.config, agent.config.as_ref().unwrap().spec_typed_json());
            }
            Representation::YamlOnly(reasons) => {
                prop_assert!(!reasons.is_empty());
                let named = reasons.iter().any(|r| r.object == ch.object && r.path.starts_with(ch.reason_path));
                prop_assert!(named, "{} is not named by {:?}", ch.pointer, paths(reasons));
            }
        }
    }
}

// ---------------------------------------------------------- the shared kinds

#[test]
fn a_model_endpoint_and_a_tool_server_are_forms_unless_they_hold_what_a_form_cannot_say() {
    let model = model_form("gateway");
    let mut stored_model = stored(Kind::ModelEndpoint, "gateway", model.draft().spec, true);
    let loaded = ModelEndpointForm::load(&stored_model);
    assert_eq!(loaded.form(), Some(&model));
    // Both an address and a Secret key: the CRD refuses it, and a form cannot say it.
    set(
        &mut stored_model.spec,
        "/baseUrl/value",
        json!("https://x.invalid/v1"),
    );
    let loaded = ModelEndpointForm::load(&stored_model);
    assert!(loaded.form().is_none());
    assert!(
        loaded
            .reasons()
            .iter()
            .any(|r| r.path.starts_with("spec.baseUrl")),
        "{:?}",
        loaded.reasons()
    );
    // An unknown member.
    let mut with_unknown = stored(Kind::ModelEndpoint, "gateway", model.draft().spec, false);
    set(&mut with_unknown.spec, "/futureField", json!(1));
    let loaded = ModelEndpointForm::load(&with_unknown);
    assert_eq!(loaded.managed_by, Owner::GitOps);
    assert_eq!(loaded.reasons()[0].path, "spec.futureField");
    assert!(matches!(model.base_url, BaseUrlForm::Secret(_)));

    let tool = tool_form("websearch");
    let mut stored_tool = stored(Kind::ToolProvider, "websearch", tool.draft().spec, true);
    assert_eq!(ToolServerForm::load(&stored_tool).form(), Some(&tool));
    set(&mut stored_tool.spec, "/mcp/futureField", json!(true));
    let loaded = ToolServerForm::load(&stored_tool);
    assert_eq!(loaded.reasons()[0].path, "spec.mcp.futureField");
    stored_tool.spec = json!({ "title": 3 });
    assert!(ToolServerForm::load(&stored_tool).form().is_none());
}
