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
    // What a tool writes: no prefix of ours, so none of it is exported (an allow-list, not a list of tools).
    for tool_label in [
        "app.kubernetes.io/instance",
        "argocd.argoproj.io/instance",
        "kustomize.toolkit.fluxcd.io/name",
        "helm.sh/chart",
        "tool.example.invalid/anything-next-year",
    ] {
        object
            .labels
            .insert(tool_label.into(), format!("{SENTINEL}-label"));
    }
    for tool_annotation in [
        "meta.helm.sh/release-name",
        "reloader.example.invalid/auto",
        "kubernetes.io/change-cause",
        "unprefixed-note",
    ] {
        object
            .annotations
            .insert(tool_annotation.into(), format!("{SENTINEL}-annotation"));
    }
    // What the platform puts on its own objects, and what a person keeps: a label with no prefix.
    object
        .annotations
        .insert("agents.vymalo.com/note".into(), "kept".into());
    object
        .labels
        .insert("agents.vymalo.com/team".into(), "kept".into());
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
    // What stays: the person's label (no prefix), the platform's label and annotation, and the dashboard's mark.
    let docs = parsed(&yaml);
    for doc in &docs[1..] {
        assert_eq!(doc["metadata"]["labels"]["team"], "agents");
        assert_eq!(doc["metadata"]["labels"]["agents.vymalo.com/team"], "kept");
        assert_eq!(
            doc["metadata"]["labels"][aap_ports::MANAGED_BY_LABEL],
            aap_ports::MANAGED_BY_DASHBOARD
        );
        assert_eq!(
            doc["metadata"]["annotations"]["agents.vymalo.com/note"],
            "kept"
        );
        let labels: BTreeSet<&str> = doc["metadata"]["labels"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            labels,
            BTreeSet::from([
                "agents.vymalo.com/team",
                "app.kubernetes.io/managed-by",
                "team"
            ]),
            "the allow-list: no tool's label"
        );
        // The annotations: the platform's, and nothing else, a person's note included.
        assert_eq!(
            doc["metadata"]["annotations"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["agents.vymalo.com/note"]
        );
    }
    for gone in [
        "note.example.invalid/owner",
        "app.kubernetes.io/instance",
        "helm.sh",
        "fluxcd",
        "unprefixed-note",
        "reloader",
    ] {
        assert!(!yaml.contains(gone), "{gone} in\n{yaml}");
    }
}

#[test]
fn the_managed_by_label_is_kept_when_it_says_the_dashboard_and_dropped_when_it_says_another_tool() {
    let mut agent = coder_agent();
    agent
        .service
        .labels
        .insert(aap_ports::MANAGED_BY_LABEL.into(), "Helm".into());
    let docs = parsed(&export_agent(&agent).unwrap());
    assert!(
        docs[1]["metadata"]["labels"]
            .get(aap_ports::MANAGED_BY_LABEL)
            .is_none(),
        "an object Helm manages is not exported as the dashboard's"
    );
    assert_eq!(
        docs[0]["metadata"]["labels"][aap_ports::MANAGED_BY_LABEL],
        aap_ports::MANAGED_BY_DASHBOARD
    );
}

#[test]
fn a_string_that_yaml_1_1_reads_as_something_else_is_quoted_and_comes_back_a_string() {
    let strings = [
        "yes",
        "no",
        "on",
        "off",
        "y",
        "n",
        "Y",
        "N",
        "12:30",
        "1_000",
        "2026-10-16",
    ];
    let mut object = stored(
        Kind::ToolProvider,
        "websearch",
        json!({
            "title": "Web search",
            // As values, as members of a list and as keys of a map.
            "values": strings,
            "keys": strings.iter().map(|s| ((*s).to_owned(), json!(*s))).collect::<serde_json::Map<_, _>>(),
        }),
        true,
    );
    object.labels.insert("answer".into(), "yes".into());
    object
        .annotations
        .insert("agents.vymalo.com/since".into(), "2026-10-16".into());
    let yaml = export_object(&object).unwrap();

    // Quoted, line by line: what a YAML 1.1 reader would otherwise read as a boolean, an integer or a date.
    for s in strings {
        assert!(
            yaml.contains(&format!("- \"{s}\"\n")),
            "the value {s}:\n{yaml}"
        );
        assert!(
            yaml.contains(&format!("\"{s}\": \"{s}\"\n")),
            "the key {s}:\n{yaml}"
        );
    }
    assert!(yaml.contains("    answer: \"yes\"\n"), "{yaml}");
    assert!(
        yaml.contains("    agents.vymalo.com/since: \"2026-10-16\"\n"),
        "{yaml}"
    );
    // And nothing of the value is a bare word that a YAML 1.1 reader would not read as a string.
    for line in yaml.lines() {
        let bare = line
            .rsplit(": ")
            .next()
            .unwrap_or(line)
            .trim_start_matches("- ");
        assert!(
            ![
                "yes",
                "no",
                "on",
                "off",
                "y",
                "n",
                "Y",
                "N",
                "12:30",
                "1_000",
                "2026-10-16"
            ]
            .contains(&bare),
            "a bare {bare:?} in\n{yaml}"
        );
    }
    // It reads back as the strings it was.
    let doc = &parsed(&yaml)[0];
    assert_eq!(doc["spec"], object.spec);
    assert_eq!(doc["metadata"]["labels"]["answer"], "yes");
    assert_eq!(
        doc["metadata"]["annotations"]["agents.vymalo.com/since"],
        "2026-10-16"
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
    #![proptest_config(config(256))]

    /// For any agent and any noise, the export is the four members, a few of `metadata`, and nothing of the noise.
    #[test]
    fn the_export_is_a_whitelist((form, deployment) in arb_agent(), extra in prop::collection::btree_map("[a-z]{1,8}", "[a-z]{1,8}", 0..4)) {
        let mut agent = agent_of(&to_resources(&form, &deployment, NS), true).tap_noise();
        for stored in std::iter::once(&mut agent.service).chain(agent.config.as_mut()) {
            for (k, v) in &extra {
                // A person's or a tool's (no prefix of ours): not exported. The platform's: exported.
                stored.annotations.insert(format!("example.invalid/{k}"), v.clone());
                stored.annotations.insert(format!("agents.vymalo.com/{k}"), v.clone());
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
            prop_assert_eq!(annotations, extra.len() + 1, "the platform's annotations, and nobody else's");
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
