//! K1: the platform's CRDs read everything adam-rs's former operator (removed, its ADR 0046) accepted, except the listed differences (§59a, "K1"). The adam-rs
//! copy is vendored in `tests/vendored/` (its revision is in `UPSTREAM`, and is the revision of the parity goldens: a test holds
//! them equal). For every property path of its two kinds, ours has the path; and for each keyword that bounds what a value may be,
//! ours is **never stricter**: the same `type`; a `minimum`, `minLength` or `minItems` no higher, a `maximum`, `maxLength`,
//! `maxItems` or `maxProperties` no lower, a set of `required` properties that is a subset, and the enum values a superset. A
//! bound that ours has and theirs lacks is stricter, and is in [`INTENDED`] with its reason, or the test fails.

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use aap_api::crds;
use serde::Deserialize;
use serde_json::Value;

/// The differences that are meant, as `(kind, path, keyword, why)`. A path is as `walk` writes it. Each is ours being
/// stricter than theirs, or theirs having a default that ours does not.
const INTENDED: &[(&str, &str, &str, &str)] = &[
    (
        "AgentConfig",
        ".spec.tools.mcpServers",
        "maxProperties",
        "kube-apiserver prices a rule that walks a map by its bound: an unbounded map of servers made the CRD too costly (D1, verified 2026-10-15); adam-rs's CRD has no bound and a config with more than 32 servers is refused here (decision kept, §59a K1)",
    ),
    (
        "AgentConfig",
        ".spec.model.extraBody",
        "maxProperties",
        "one environment string of the pod is limited: extraBody has at most 64 members here and 16 KiB of JSON (`validate`), and none there (K1 review)",
    ),
    (
        "AgentService",
        ".spec.interfaces.a2a.enabled",
        "default",
        "adam-rs defaults `enabled` to true; here it stays false, as before K1: a manifest that writes `a2a` with only a token must not turn a disabled agent on at the next apply (§59a K1)",
    ),
];

/// What a path of a schema says that bounds a value.
#[derive(Debug, Default, Clone, PartialEq)]
struct Facts {
    ty: Option<String>,
    minimum: Option<f64>,
    maximum: Option<f64>,
    min_length: Option<u64>,
    max_length: Option<u64>,
    min_items: Option<u64>,
    max_items: Option<u64>,
    max_properties: Option<u64>,
    required: BTreeSet<String>,
    values: Option<BTreeSet<String>>,
    default: Option<String>,
}

fn facts(schema: &Value) -> Facts {
    let u = |k: &str| schema[k].as_u64();
    Facts {
        ty: schema["type"].as_str().map(str::to_owned),
        minimum: schema["minimum"].as_f64(),
        maximum: schema["maximum"].as_f64(),
        min_length: u("minLength"),
        max_length: u("maxLength"),
        min_items: u("minItems"),
        max_items: u("maxItems"),
        max_properties: u("maxProperties"),
        required: schema["required"]
            .as_array()
            .map(|r| {
                r.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default(),
        values: schema["enum"]
            .as_array()
            .map(|v| v.iter().map(ToString::to_string).collect()),
        default: schema.get("default").map(ToString::to_string),
    }
}

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

/// Every property path of a schema with what it says.
fn walk(schema: &Value, path: &str, out: &mut BTreeMap<String, Facts>) {
    out.insert(path.to_owned(), facts(schema));
    if let Some(props) = schema["properties"].as_object() {
        for (name, child) in props {
            walk(child, &format!("{path}.{name}"), out);
        }
    }
    if schema["items"].is_object() {
        walk(&schema["items"], &format!("{path}[]"), out);
    }
    if schema["additionalProperties"].is_object() {
        walk(&schema["additionalProperties"], &format!("{path}{{}}"), out);
    }
    for keyword in ["oneOf", "anyOf", "allOf"] {
        for (i, child) in schema[keyword].as_array().into_iter().flatten().enumerate() {
            walk(child, &format!("{path}|{keyword}{i}"), out);
        }
    }
}

fn shape(schema: &Value) -> BTreeMap<String, Facts> {
    let mut out = BTreeMap::new();
    walk(schema, "", &mut out);
    out
}

/// The keywords on which `ours` is stricter than `theirs`.
fn stricter(ours: &Facts, theirs: &Facts) -> Vec<&'static str> {
    let mut out = Vec::new();
    // `type` and `required` and the enum are checked beside this.
    let lower = |o: Option<f64>, t: Option<f64>| o.is_some_and(|o| t.is_none_or(|t| o > t));
    let upper = |o: Option<f64>, t: Option<f64>| o.is_some_and(|o| t.is_none_or(|t| o < t));
    #[allow(clippy::cast_precision_loss)]
    let f = |v: Option<u64>| v.map(|v| v as f64);
    if lower(ours.minimum, theirs.minimum) {
        out.push("minimum");
    }
    if upper(ours.maximum, theirs.maximum) {
        out.push("maximum");
    }
    if lower(f(ours.min_length), f(theirs.min_length)) {
        out.push("minLength");
    }
    if upper(f(ours.max_length), f(theirs.max_length)) {
        out.push("maxLength");
    }
    if lower(f(ours.min_items), f(theirs.min_items)) {
        out.push("minItems");
    }
    if upper(f(ours.max_items), f(theirs.max_items)) {
        out.push("maxItems");
    }
    if upper(f(ours.max_properties), f(theirs.max_properties)) {
        out.push("maxProperties");
    }
    if !ours.required.is_subset(&theirs.required) {
        out.push("required");
    }
    if let (Some(o), Some(t)) = (&ours.values, &theirs.values)
        && !o.is_superset(t)
    {
        out.push("enum");
    }
    if ours.values.is_none() && theirs.values.is_some() {
        out.push("enum");
    }
    if ours.ty != theirs.ty && theirs.ty.is_some() {
        out.push("type");
    }
    // A default that differs is a different meaning of an object that leaves the field out.
    if theirs.default.is_some() && ours.default != theirs.default {
        out.push("default");
    }
    out
}

#[test]
fn crd_reads_everything_adam_rs_accepts_except_the_listed_differences() {
    let theirs = vendored();
    assert_eq!(
        theirs.keys().map(String::as_str).collect::<Vec<_>>(),
        ["AgentConfig", "AgentService"],
        "the vendored file has the two kinds of adam-rs's former operator (removed, its ADR 0046)"
    );
    let mut seen = BTreeSet::new();
    let mut problems = Vec::new();
    for (kind, their_schema) in &theirs {
        let their_facts = shape(their_schema);
        let our_facts = shape(&ours(kind));
        for (path, t) in &their_facts {
            let Some(o) = our_facts.get(path) else {
                problems.push(format!(
                    "{kind}{path}: a property of adam-rs's CRD that ours lacks"
                ));
                continue;
            };
            for keyword in stricter(o, t) {
                match INTENDED
                    .iter()
                    .find(|(k, p, w, _)| *k == kind && *p == path && *w == keyword)
                {
                    Some(entry) => {
                        seen.insert(*entry);
                    }
                    None => problems.push(format!(
                        "{kind}{path}: `{keyword}` is stricter here than in adam-rs's CRD (ours {o:?}, theirs {t:?}); fix it or list it in INTENDED with the reason"
                    )),
                }
            }
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
    let unused: Vec<_> = INTENDED.iter().filter(|e| !seen.contains(*e)).collect();
    assert!(
        unused.is_empty(),
        "INTENDED lists a difference that is not there any more: {unused:#?}"
    );
}

#[test]
fn the_vendored_crd_is_at_the_revision_of_the_parity_goldens() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let upstream = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vendored/UPSTREAM"),
    )
    .expect("UPSTREAM");
    let vendored_rev = upstream
        .lines()
        .find_map(|l| l.strip_prefix("revision:"))
        .and_then(|l| l.split_whitespace().next())
        .expect("a revision line");
    let regen = std::fs::read_to_string(root.join("tools/adam-parity/regen.sh")).expect("regen.sh");
    let golden_rev = regen
        .lines()
        .find_map(|l| l.strip_prefix("ADAM_RS_REV=\""))
        .and_then(|l| l.strip_suffix('"'))
        .expect("ADAM_RS_REV");
    assert_eq!(
        vendored_rev, golden_rev,
        "one pin of adam-rs: the vendored CRD and the parity goldens are read at the same revision"
    );
}

#[test]
fn extra_body_schema_is_an_object() {
    let schema = ours("AgentConfig");
    let extra = &schema["properties"]["spec"]["properties"]["model"]["properties"]["extraBody"];
    assert_eq!(extra["type"], "object", "{extra}");
    assert_eq!(extra["maxProperties"], 64);
    assert_eq!(extra["nullable"], true, "as its sibling optional fields");
    assert_eq!(extra["x-kubernetes-preserve-unknown-fields"], true);
}
