//! What the code of the crate assumes of the CRDs, held where the CRDs are read, so that a change of either is
//! seen here before it is wrong somewhere else.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::Value;
use sha2::{Digest, Sha256};

fn walk(v: &Value, at: &str, f: &mut impl FnMut(&str, &str)) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                f(at, k);
                walk(child, &format!("{at}.{k}"), f);
            }
        }
        Value::Array(items) => items
            .iter()
            .enumerate()
            .for_each(|(i, c)| walk(c, &format!("{at}[{i}]"), f)),
        _ => {}
    }
}

/// `Stored::spec` is read through the fields a manager owns (`src/stored.rs`). That reading is right for CRDs whose
/// lists are all atomic and whose maps are all granular, which is what a schema without these markers means.
#[test]
fn the_platform_crds_have_no_list_type_and_no_map_type() {
    let crds = serde_json::to_value(aap_api::crds()).unwrap();
    let mut found = Vec::new();
    walk(&crds, "crds", &mut |at, key| {
        if [
            "x-kubernetes-list-type",
            "x-kubernetes-list-map-keys",
            "x-kubernetes-map-type",
        ]
        .contains(&key)
        {
            found.push(format!("{at}.{key}"));
        }
    });
    assert!(
        found.is_empty(),
        "a CRD gained a keyed list, a set or an atomic map: src/stored.rs must read its ownership ({found:?})"
    );
}

#[test]
fn the_checked_in_crds_are_what_crdgen_prints_so_the_cluster_test_installs_what_the_operator_ships()
{
    let file: Vec<Value> = serde_yaml::Deserializer::from_str(include_str!(
        "../../../deploy/crds/agents.vymalo.com.yaml"
    ))
    .map(|d| serde::Deserialize::deserialize(d).unwrap())
    .collect();
    let types = serde_json::to_value(aap_api::crds()).unwrap();
    assert_eq!(Value::Array(file), types);
}

#[test]
fn the_external_secret_crd_is_the_pinned_copy_and_serves_the_version_the_crate_reads() {
    let text = include_str!("crds/external-secrets.io_externalsecrets.yaml");
    let sum = format!("{:x}", Sha256::digest(text.as_bytes()));
    let readme = include_str!("crds/README.md");
    assert!(
        readme.contains(&sum),
        "tests/crds/README.md does not hold the sha256 of the file: {sum}"
    );

    let crd: Value = serde_yaml::from_str(text).unwrap();
    assert_eq!(crd["spec"]["group"], "external-secrets.io");
    let (group, version) = aap_objects_kubernetes::EXTERNAL_SECRET_API_VERSION
        .split_once('/')
        .unwrap();
    assert_eq!(group, crd["spec"]["group"]);
    let served = crd["spec"]["versions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == version)
        .unwrap_or_else(|| panic!("the CRD has no version {version}"));
    assert_eq!(served["served"], true);
    assert_eq!(served["storage"], true);
}
