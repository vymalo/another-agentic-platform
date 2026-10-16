//! What editing a shared object does to the agents that name it (`dependents_after`), and what an object that
//! exists and cannot be read is told to be (not missing).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use aap_forms::{
    AgentKind, ImagePolicy, SharedChange, ToolServerForm, dependents_after, to_resources,
    validate_agent, validate_tool_server,
};
use aap_ports::{Agent, Kind};
use serde_json::json;
use support::*;

/// `websearch` as it is before: an `https://` address.
fn secure() -> ToolServerForm {
    let mut form = tool_form("websearch");
    form.url = "https://search.example.invalid/mcp".to_owned();
    form
}

fn referents() -> aap_forms::Referents {
    aap_forms::externals(
        &[stored(
            Kind::ModelEndpoint,
            "gateway",
            model_form("gateway").draft().spec,
            true,
        )],
        &[stored(
            Kind::ToolProvider,
            "websearch",
            secure().draft().spec,
            true,
        )],
        None,
    )
}

fn stored_agent(form: &aap_forms::AgentForm) -> Agent {
    let d = defaults();
    agent_of(
        &to_resources(form, &d.deployment_for(form.kind(), &form.name), NS),
        true,
    )
}

/// `websearch` with a plain `http://` address on another host: the probe accepts it, because the rule is the agent's.
fn plain_http() -> ToolServerForm {
    let mut form = tool_form("websearch");
    form.url = "http://search.example.invalid:8080/mcp".to_owned();
    form
}

#[test]
fn a_tool_server_made_plain_http_breaks_the_agents_that_do_not_allow_it_and_says_which() {
    let mut allows = coder_form("allows");
    allows.allow_insecure_http = true;
    let mut refuses = coder_form("refuses");
    refuses.allow_insecure_http = false;
    let chat = folder_form("chat");
    let agents = [
        stored_agent(&allows),
        stored_agent(&refuses),
        stored_agent(&chat),
    ];

    let changed = validate_tool_server(&plain_http(), NS, &offered())
        .expect("the probe agent allows plain http, so the object passes by itself");
    let errors = dependents_after(SharedChange::ToolServer(&changed), &agents, &referents());
    assert_eq!(errors.len(), 1, "{errors:?}");
    let e = &errors[0];
    assert_eq!(e.field, "", "the form as a whole");
    assert!(e.message.contains("the agent refuses"), "{e}");
    assert!(e.message.contains("allowInsecureHttp"), "{e}");
    assert!(
        !e.message.contains("allows") && !e.message.contains("chat"),
        "only the agent it breaks: {e}"
    );

    // The same object, unchanged, breaks nothing.
    let unchanged = secure().to_resource(NS);
    assert!(
        dependents_after(SharedChange::ToolServer(&unchanged), &agents, &referents()).is_empty()
    );
}

#[test]
fn an_agent_that_was_already_refused_is_not_blamed_on_the_change() {
    let mut broken = coder_form("broken");
    broken.allow_insecure_http = false;
    broken.model.endpoint = "no-such-model".to_owned();
    // The tool server is plain http already: the agent was invalid before the change as it is after.
    let plain = stored(
        Kind::ToolProvider,
        "websearch",
        plain_http().draft().spec,
        true,
    );
    let before = aap_forms::externals(
        &[stored(
            Kind::ModelEndpoint,
            "gateway",
            model_form("gateway").draft().spec,
            true,
        )],
        &[plain],
        None,
    );
    let changed = plain_http().to_resource(NS);
    let errors = dependents_after(
        SharedChange::ToolServer(&changed),
        &[stored_agent(&broken)],
        &before,
    );
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_model_endpoint_the_operator_would_refuse_is_found_in_its_agents() {
    // `validate_model` would never give this object; the function does not rely on that.
    let mut changed = model_form("gateway").to_resource(NS);
    changed.spec.base_url.value = Some("ftp://x.invalid/v1".to_owned());
    changed.spec.base_url.secret_ref = None;
    let agents = [
        stored_agent(&coder_form("coder-me")),
        stored_agent(&folder_form("chat")),
    ];
    let errors = dependents_after(SharedChange::Model(&changed), &agents, &referents());
    assert_eq!(errors.len(), 2, "both name gateway: {errors:?}");
    assert!(
        errors
            .iter()
            .any(|e| e.message.contains("the agent coder-me"))
    );
    assert!(errors.iter().any(|e| e.message.contains("the agent chat")));
}

#[test]
fn an_agent_that_cannot_be_read_is_reported_and_one_that_names_nothing_is_not_asked() {
    let mut unreadable = stored_agent(&coder_form("odd"));
    unreadable.service.spec = json!({ "configRef": { "name": "odd" }, "interfaces": 3 });
    let mut unrelated = coder_form("unrelated");
    unrelated.tool_servers.clear();
    let changed = plain_http().to_resource(NS);
    let errors = dependents_after(
        SharedChange::ToolServer(&changed),
        &[unreadable, stored_agent(&unrelated)],
        &referents(),
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].message.contains("odd") && errors[0].message.contains("cannot be read"));
}

#[test]
fn an_object_that_exists_and_cannot_be_read_is_not_said_to_be_missing() {
    let referents = aap_forms::externals(
        &[stored(
            Kind::ModelEndpoint,
            "gateway",
            json!({ "title": 3 }),
            false,
        )],
        &[stored(
            Kind::ToolProvider,
            "websearch",
            json!({ "mcp": 1 }),
            false,
        )],
        None,
    );
    let form = coder_form("coder-me");
    let errors = validate_agent(
        &form,
        &defaults().deployment_for(AgentKind::Coder, "coder-me"),
        NS,
        &referents,
        &offered(),
        &ImagePolicy::any_registry(),
    )
    .unwrap_err();
    for field in ["model.endpoint", "toolServers[websearch]"] {
        let found: Vec<_> = errors.iter().filter(|e| e.field == field).collect();
        assert_eq!(found.len(), 1, "{field} once: {errors:?}");
        assert!(found[0].message.contains("exists"), "{}", found[0]);
        assert!(
            !found[0].message.contains("is missing") && found[0].message.contains("View YAML"),
            "{}",
            found[0]
        );
    }
}
