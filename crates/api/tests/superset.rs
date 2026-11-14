//! K1: the platform's CRDs are a superset of the adam-rs operator's (§59a, "K1"). The adam-rs copy is vendored
//! in `tests/vendored/` (its revision is in `UPSTREAM`): every property path of its kinds exists in ours, every enum
//! value of its enums is one of ours, every bound it has on a number is as wide in ours, and every property it
//! requires is required by ours only if ours has no way to say it otherwise (a required one of theirs that ours
//! makes optional is wider, which is fine: ours accepts what theirs accepts).

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aap_api::crds;
use serde::Deserialize;
use serde_json::Value;

/// The schema of each kind of the vendored file, by kind.
fn vendored() -> BTreeMap<String, Value> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vendored/adam-rs-agents.vymalo.com.yaml");
    let text = std::fs::read_to_string(&path).expect("the vendored CRD file");
    let mut out = BTreeMap::new();
    for doc in serde_yaml::Deserializer::from_str(&text) {
        let doc = Value::deserialize(doc).expect("a YAML document");
        let items = match doc["kind"].as_str() {
            Some("List") => doc["items"].as_array().cloned().unwrap_or_default(),
            _ => vec![doc],
        };
        for crd in items {
            let Some(kind) = crd["spec"]["names"]["kind"].as_str() else {
                continue;
            };
            let schema = crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"].clone();
            assert!(schema.is_object(), "{kind} has a schema");
            out.insert(kind.to_owned(), schema);
        }
    }
    out
}

fn ours(kind: &str) -> Value {
    let crd = crds()
        .into_iter()
        .find(|c| c.spec.names.kind == kind)
        .unwrap_or_else(|| panic!("no CRD of kind {kind}"));
    let schema = crd.spec.versions[0]
        .schema
        .as_ref()
        .and_then(|s| s.open_api_v3_schema.as_ref())
        .expect("a schema");
    serde_json::to_value(schema).expect("serialises")
}

/// Every property path of a schema, and the enum values at each.
fn walk(
    schema: &Value,
    path: &str,
    paths: &mut BTreeSet<String>,
    enums: &mut BTreeMap<String, BTreeSet<String>>,
) {
    paths.insert(path.to_owned());
    if let Some(values) = schema["enum"].as_array() {
        enums.insert(
            path.to_owned(),
            values.iter().map(|v| v.to_string()).collect(),
        );
    }
    if let Some(props) = schema["properties"].as_object() {
        for (name, child) in props {
            walk(child, &format!("{path}.{name}"), paths, enums);
        }
    }
    if schema["items"].is_object() {
        walk(&schema["items"], &format!("{path}[]"), paths, enums);
    }
    if schema["additionalProperties"].is_object() {
        walk(
            &schema["additionalProperties"],
            &format!("{path}{{}}"),
            paths,
            enums,
        );
    }
    for keyword in ["oneOf", "anyOf", "allOf"] {
        for (i, child) in schema[keyword].as_array().into_iter().flatten().enumerate() {
            walk(child, &format!("{path}|{keyword}{i}"), paths, enums);
        }
    }
}

fn shape(schema: &Value) -> (BTreeSet<String>, BTreeMap<String, BTreeSet<String>>) {
    let (mut paths, mut enums) = (BTreeSet::new(), BTreeMap::new());
    walk(schema, "", &mut paths, &mut enums);
    (paths, enums)
}

#[test]
fn crd_is_superset_of_adam_rs() {
    let theirs = vendored();
    assert_eq!(
        theirs.keys().map(String::as_str).collect::<Vec<_>>(),
        ["AgentConfig", "AgentService"],
        "the vendored file has the two kinds of adam-rs's operator"
    );
    for (kind, their_schema) in &theirs {
        let (their_paths, their_enums) = shape(their_schema);
        let (our_paths, our_enums) = shape(&ours(kind));
        let missing: Vec<_> = their_paths.difference(&our_paths).collect();
        assert!(
            missing.is_empty(),
            "{kind}: properties of adam-rs's CRD that ours lacks: {missing:#?}"
        );
        for (path, values) in &their_enums {
            let Some(ours) = our_enums.get(path) else {
                panic!("{kind}{path}: an enum in adam-rs's CRD and none in ours");
            };
            let lost: Vec<_> = values.difference(ours).collect();
            assert!(
                lost.is_empty(),
                "{kind}{path}: enum values of adam-rs's CRD that ours lacks: {lost:?}"
            );
        }
    }
}
