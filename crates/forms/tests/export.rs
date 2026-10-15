//! The YAML export: both objects, without `status`, `managedFields` and what a server sets.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeSet;

use aap_forms::{AgentKind, export_agent, export_object, to_resources};
use aap_ports::{ARGO_TRACKING_ANNOTATION, Agent, Kind, Stored};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::*;

const NOISE: &[&str] = &[
    "uid",
    "resourceVersion",
    "creationTimestamp",
    "managedFields",
    "ownerReferences",
    "finalizers",
    "selfLink",
];

/// An object as a live cluster holds it: a status, tool annotations that copy the object, and the sentinel in
/// every place the export must not read.
fn noisy(mut object: Stored) -> Stored {
    object.annotations.insert(
        "kubectl.kubernetes.io/last-applied-configuration".into(),
        format!("{{\"data\":\"{SENTINEL}\"}}"),
    );
    object
        .annotations
        .insert(ARGO_TRACKING_ANNOTATION.into(), format!("app:{SENTINEL}"));
    object
        .annotations
        .insert("note.example.invalid/owner".into(), "platform team".into());
    object.labels.insert("team".into(), "agents".into());
    object.status = Some(
        json!({ "state": "Ready", "conditions": [{ "message": SENTINEL }], "secret": SENTINEL }),
    );
    object.generation = 9;
    object
}

fn parsed(yaml: &str) -> Vec<Value> {
    serde_yaml::Deserializer::from_str(yaml)
        .map(|d| serde::Deserialize::deserialize(d).unwrap())
        .collect()
}

fn coder_agent() -> Agent {
    let d = defaults();
    let r = to_resources(
        &coder_form("coder-me"),
        &d.deployment_for(AgentKind::Coder, "coder-me"),
        NS,
    );
    let mut agent = agent_of(&r, true);
    agent.service = noisy(agent.service);
    agent.config = agent.config.map(noisy);
    agent
}

#[test]
fn an_agent_is_two_documents_the_config_first() {
    let yaml = export_agent(&coder_agent()).unwrap();
    let docs = parsed(&yaml);
    assert_eq!(docs.len(), 2, "{yaml}");
    assert_eq!(docs[0]["kind"], "AgentConfig");
    assert_eq!(docs[1]["kind"], "AgentService");
    for doc in &docs {
        assert_eq!(doc["apiVersion"], "agents.vymalo.com/v1alpha1");
        let top: BTreeSet<&str> = doc
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            top,
            BTreeSet::from(["apiVersion", "kind", "metadata", "spec"]),
            "no status"
        );
        let meta: BTreeSet<&str> = doc["metadata"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert!(
            meta.is_subset(&BTreeSet::from([
                "name",
                "namespace",
                "labels",
                "annotations"
            ])),
            "{meta:?}"
        );
        assert_eq!(doc["metadata"]["name"], "coder-me");
        assert_eq!(doc["metadata"]["namespace"], NS);
    }
    assert!(yaml.starts_with("apiVersion:"), "{yaml}");
}

#[test]
fn what_a_server_sets_and_what_a_tool_writes_is_not_exported() {
    let yaml = export_agent(&coder_agent()).unwrap();
    assert!(!yaml.contains(SENTINEL), "{yaml}");
    for word in NOISE {
        assert!(!yaml.contains(word), "{word} in\n{yaml}");
    }
    for word in [
        "status",
        "generation",
        "last-applied",
        "tracking-id",
        "conditions",
    ] {
        assert!(!yaml.contains(word), "{word} in\n{yaml}");
    }
    // What a person put stays: their annotation, their label and the dashboard's.
    let docs = parsed(&yaml);
    assert_eq!(
        docs[1]["metadata"]["annotations"]["note.example.invalid/owner"],
        "platform team"
    );
    assert_eq!(docs[1]["metadata"]["labels"]["team"], "agents");
    assert_eq!(
        docs[1]["metadata"]["labels"][aap_ports::MANAGED_BY_LABEL],
        aap_ports::MANAGED_BY_DASHBOARD
    );
    assert_eq!(
        docs[1]["metadata"]["annotations"]
            .as_object()
            .unwrap()
            .len(),
        1,
        "only the person's"
    );
}

#[test]
fn the_spec_is_exported_whole_and_reads_back_as_the_spec() {
    let agent = coder_agent();
    let docs = parsed(&export_agent(&agent).unwrap());
    assert_eq!(docs[1]["spec"], agent.service.spec);
    assert_eq!(docs[0]["spec"], agent.config.as_ref().unwrap().spec);
    // It is what the types read: the YAML is an object a cluster accepts.
    let service: aap_api::AgentServiceSpec =
        serde_json::from_value(docs[1]["spec"].clone()).unwrap();
    assert_eq!(j(&service)["configRef"]["name"], "coder-me");
}

#[test]
fn an_object_with_no_annotation_has_no_empty_map_and_a_missing_config_is_left_out() {
    let mut agent = coder_agent();
    agent.service.annotations.clear();
    agent.service.labels.clear();
    agent.config = None;
    let yaml = export_agent(&agent).unwrap();
    let docs = parsed(&yaml);
    assert_eq!(docs.len(), 1);
    assert!(
        docs[0]["metadata"].get("annotations").is_none()
            && docs[0]["metadata"].get("labels").is_none(),
        "{yaml}"
    );
}

#[test]
fn the_export_of_the_example_is_the_golden() {
    let d = defaults();
    let r = to_resources(
        &coder_form("coder-me"),
        &d.deployment_for(AgentKind::Coder, "coder-me"),
        NS,
    );
    let yaml = export_agent(&agent_of(&r, true).tap_noise()).unwrap();
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/coder-me.export.yaml");
    if std::env::var("AAP_UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::write(&path, &yaml).unwrap();
    }
    let golden = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e} (AAP_UPDATE_GOLDEN=1 writes it)", path.display()));
    assert_eq!(
        yaml, golden,
        "the export changed: AAP_UPDATE_GOLDEN=1 rewrites the golden"
    );
}

trait TapNoise {
    fn tap_noise(self) -> Self;
}
impl TapNoise for Agent {
    fn tap_noise(mut self) -> Self {
        self.service = noisy(self.service);
        self.config = self.config.map(noisy);
        self
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// For any agent and any noise, the export is the four members, a few of `metadata`, and nothing of the noise.
    #[test]
    fn the_export_is_a_whitelist((form, deployment) in arb_agent(), extra in prop::collection::btree_map("[a-z]{1,8}", "[a-z]{1,8}", 0..4)) {
        let mut agent = agent_of(&to_resources(&form, &deployment, NS), true).tap_noise();
        for stored in std::iter::once(&mut agent.service).chain(agent.config.as_mut()) {
            for (k, v) in &extra {
                stored.annotations.insert(format!("example.invalid/{k}"), v.clone());
            }
            // Members of `metadata` a future Kubernetes adds are not on the whitelist: they are not in `Stored` at
            // all, and the export builds the object from named members.
            stored.status = Some(json!({ "anything": SENTINEL, "nested": { "managedFields": [SENTINEL] } }));
        }
        let yaml = export_agent(&agent).unwrap();
        prop_assert!(!yaml.contains(SENTINEL));
        for doc in parsed(&yaml) {
            let top: BTreeSet<&str> = doc.as_object().unwrap().keys().map(String::as_str).collect();
            prop_assert_eq!(top, BTreeSet::from(["apiVersion", "kind", "metadata", "spec"]));
            let meta: BTreeSet<&str> = doc["metadata"].as_object().unwrap().keys().map(String::as_str).collect();
            prop_assert!(meta.is_subset(&BTreeSet::from(["name", "namespace", "labels", "annotations"])));
            let annotations = doc["metadata"]["annotations"].as_object().map(|m| m.len()).unwrap_or(0);
            prop_assert_eq!(annotations, extra.len() + 1, "the person's annotations, and no tool's");
        }
    }

    #[test]
    fn the_export_of_a_shared_object_reads_back(model in arb_model_form(), tool in arb_tool_form()) {
        for (kind, name, spec) in [
            (Kind::ModelEndpoint, model.name.clone(), model.draft().spec),
            (Kind::ToolProvider, tool.name.clone(), tool.draft().spec),
        ] {
            let object = noisy(stored(kind, &name, spec.clone(), true));
            let yaml = export_object(&object).unwrap();
            prop_assert!(!yaml.contains(SENTINEL));
            let doc = &parsed(&yaml)[0];
            prop_assert_eq!(&doc["spec"], &spec);
            prop_assert_eq!(doc["kind"].as_str(), Some(kind.as_str()));
        }
    }
}
