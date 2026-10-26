//! The places where this crate and `aap-domain` say the same thing in two words, held equal.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use aap_domain::references;
use aap_forms::{AgentKind, to_resources};
use aap_ports::{Kind, config_references};
use proptest::prelude::*;
use support::*;

proptest! {
    #![proptest_config(config(256))]

    /// `AgentObjects` refuses to delete what an agent names, reading names from JSON; the operator reads them
    /// from the types. They must name the same objects, or a delete would break an agent the operator can see.
    #[test]
    fn the_store_and_the_operator_read_the_same_references((form, deployment) in arb_agent()) {
        let r = to_resources(&form, &deployment, NS);
        let from_json = config_references(&j(&r.config.spec));
        let from_types = references(&r.config);
        prop_assert_eq!(from_json.model_endpoint, from_types.model_endpoint);
        prop_assert_eq!(from_json.tool_providers, from_types.tool_providers);
    }
}

#[test]
fn the_references_of_the_example_are_read_the_same_way() {
    let agent = example_agent("coder-me.yaml", true);
    let spec = &agent.config.as_ref().unwrap().spec;
    let typed =
        aap_api::AgentConfig::new("coder-me", serde_json::from_value(spec.clone()).unwrap());
    let a = config_references(spec);
    let b = references(&typed);
    assert_eq!(a.model_endpoint.as_deref(), Some("gateway"));
    assert_eq!(a.model_endpoint, b.model_endpoint);
    assert_eq!(a.tool_providers, b.tool_providers);
    assert_eq!(a.tool_providers.iter().collect::<Vec<_>>(), ["websearch"]);
}

#[test]
fn the_label_and_the_permission_strings_are_the_ones_of_the_text() {
    // §60a, "Applying": the label is `app.kubernetes.io/managed-by: dashboard.agents.vymalo.com`.
    assert_eq!(aap_ports::MANAGED_BY_LABEL, "app.kubernetes.io/managed-by");
    assert_eq!(
        aap_ports::MANAGED_BY_DASHBOARD,
        "dashboard.agents.vymalo.com"
    );
    assert_eq!(aap_ports::OFFER_LABEL, "agents.vymalo.com/offer");
    assert_eq!(
        aap_ports::ARGO_TRACKING_ANNOTATION,
        "argocd.argoproj.io/tracking-id"
    );
    // The group of the objects is the one the label's value is a subdomain of.
    assert!(aap_ports::MANAGED_BY_DASHBOARD.ends_with(aap_api::GROUP));
    // The kinds the store holds are the CRDs `aap-api` defines.
    let crds: Vec<String> = aap_api::crds()
        .iter()
        .filter_map(|c| c.spec.names.kind.clone().into())
        .collect();
    for kind in [
        Kind::AgentService,
        Kind::AgentConfig,
        Kind::ModelEndpoint,
        Kind::ToolProvider,
    ] {
        assert!(crds.contains(&kind.as_str().to_owned()), "{kind}");
    }
}

#[test]
fn a_stored_object_that_cannot_be_read_is_missing_for_the_operators_rules() {
    // `externals` leaves out what is not the shape of its kind; the agent that names it is told it is missing.
    let good = stored(
        Kind::ModelEndpoint,
        "gateway",
        model_form("gateway").draft().spec,
        true,
    );
    let bad = stored(
        Kind::ModelEndpoint,
        "broken",
        serde_json::json!({ "title": 3 }),
        true,
    );
    let referents =
        aap_forms::externals(&[good, bad], &[], Some("registry.invalid/agent:tag"), &[]);
    assert!(referents.externals.model_endpoints.contains_key("gateway"));
    assert!(!referents.externals.model_endpoints.contains_key("broken"));
    assert_eq!(
        referents.unreadable,
        std::collections::BTreeSet::from([(Kind::ModelEndpoint, "broken".to_owned())]),
        "an object that cannot be read is not dropped silently"
    );
    assert_eq!(
        referents.externals.default_image.as_deref(),
        Some("registry.invalid/agent:tag")
    );
    let standin = aap_forms::externals(&[], &[], None, &[]);
    assert_eq!(
        standin.externals.default_image.as_deref(),
        Some(aap_forms::DEFAULT_IMAGE_STANDIN)
    );
}

#[test]
fn the_names_of_a_form_fit_what_is_derived_from_them() {
    // §60a: at most 40 characters "so every derived name fits 63": the use permission, the StatefulSet's
    // pod label (`<name>` + 11), `<name>-agent-<hash>`, `<name>-front`, `<name>-db-app`.
    let name = "a".repeat(40);
    let form = coder_form(&name);
    let r = to_resources(
        &form,
        &defaults().deployment_for(AgentKind::Coder, &name),
        NS,
    );
    assert!(r.service.metadata.name.as_deref().unwrap().len() + "-agent-12345678".len() <= 63);
    assert!(aap_api::permission::use_permission(&name).len() <= 50);
}
