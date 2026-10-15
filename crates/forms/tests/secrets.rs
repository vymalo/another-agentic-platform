//! AD-030: a form names a secret by reference only. No secret value is in a form, in the objects made of
//! it, or in the YAML of them, whatever the input.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::collections::BTreeSet;

use aap_forms::{
    AgentForm, AgentKind, ModelEndpointForm, ToolServerForm, export_agent, export_object,
    to_resources,
};
use aap_ports::{Agent, Kind};
use proptest::prelude::*;
use serde_json::{Map, Value, json};
use support::*;

/// Members a body might use to send a value where a reference is asked.
const VALUE_MEMBERS: &[&str] = &[
    "value",
    "password",
    "token",
    "secretValue",
    "apiKeyValue",
    "data",
    "stringData",
    "privateKeyPem",
    "credential",
];

/// The JSON paths (as lists of keys) of every object in `doc`, except the ones that are maps of any key
/// by design: the files of a folder, and the headers of a tool server (a header may be named `value`).
fn closed_objects(doc: &Value, at: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match doc {
        Value::Object(m) => {
            let open = at.last().is_some_and(|k| k == "files" || k == "headers");
            if !open {
                out.push(at.clone());
            }
            for (k, v) in m {
                at.push(k.clone());
                closed_objects(v, at, out);
                at.pop();
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                at.push(i.to_string());
                closed_objects(v, at, out);
                at.pop();
            }
        }
        _ => {}
    }
}

fn at_mut<'a>(doc: &'a mut Value, path: &[String]) -> &'a mut Value {
    let mut cur = doc;
    for key in path {
        cur = match cur {
            Value::Array(a) => &mut a[key.parse::<usize>().unwrap()],
            other => &mut other[key.as_str()],
        };
    }
    cur
}

/// Every form, maximal: every optional member present.
fn maximal_forms() -> Vec<(&'static str, Value)> {
    let mut coder = coder_form("coder-me");
    coder.image = Some("registry.invalid/coder:tag".into());
    let mut with_value = model_form("gateway");
    with_value.base_url = aap_forms::BaseUrlForm::Value("https://x.invalid/v1".into());
    vec![
        ("coder form", j(&coder)),
        ("folder form", j(&folder_form("chat"))),
        (
            "model form with a Secret address",
            j(&model_form("gateway")),
        ),
        ("model form with an address", j(&with_value)),
        ("tool server form", j(&tool_form("websearch"))),
    ]
}

fn parses(what: &str, doc: &Value) -> bool {
    match what {
        w if w.contains("coder") || w.contains("folder") => {
            serde_json::from_value::<AgentForm>(doc.clone()).is_ok()
        }
        w if w.contains("model") => {
            serde_json::from_value::<ModelEndpointForm>(doc.clone()).is_ok()
        }
        _ => serde_json::from_value::<ToolServerForm>(doc.clone()).is_ok(),
    }
}

#[test]
fn the_unchanged_forms_parse() {
    for (what, doc) in maximal_forms() {
        assert!(parses(what, &doc), "{what}");
    }
}

#[test]
fn no_object_of_a_form_takes_a_member_that_could_carry_a_value() {
    // At every object of every form (the two open maps excepted), a member the form does not have is a body
    // that is not a form: the value would be refused, not ignored.
    let mut tried = 0;
    for (what, doc) in maximal_forms() {
        let mut places = Vec::new();
        closed_objects(&doc, &mut Vec::new(), &mut places);
        assert!(places.len() >= 3, "{what}");
        for place in places {
            for member in VALUE_MEMBERS {
                let mut changed = doc.clone();
                let target = at_mut(&mut changed, &place);
                if target.get(*member).is_some() {
                    continue; // a member the form has
                }
                target[*member] = json!(SENTINEL);
                assert!(
                    !parses(what, &changed),
                    "{what}: {member} at /{} was accepted",
                    place.join("/")
                );
                tried += 1;
            }
        }
    }
    assert!(tried > 150, "{tried}");
}

#[test]
fn a_secret_slot_takes_a_reference_and_nothing_else() {
    let slots: &[(&str, &[&str])] = &[
        ("coder form", &["agent", "github", "privateKey"]),
        ("model form with a Secret address", &["apiKey"]),
        ("model form with a Secret address", &["baseUrl", "secret"]),
        ("tool server form", &["headers", "Authorization", "secret"]),
    ];
    let forms = maximal_forms();
    for (what, slot) in slots {
        let doc = &forms.iter().find(|(w, _)| w == what).unwrap().1;
        let path: Vec<String> = slot.iter().map(|s| (*s).to_owned()).collect();
        let wrong: Vec<Value> = vec![
            json!(SENTINEL),
            json!(42),
            json!(true),
            Value::Null,
            json!([]),
            json!([SENTINEL]),
            json!({}),
            json!({ "value": SENTINEL }),
            json!({ "secret": "s" }),
            json!({ "key": "k" }),
            json!({ "secret": "s", "key": "k", "value": SENTINEL }),
            json!({ "secret": 1, "key": "k" }),
            json!({ "secret": "s", "key": ["k"] }),
            json!({ "secretRef": { "name": "s", "key": "k" } }),
        ];
        let mut ok = doc.clone();
        *at_mut(&mut ok, &path) = json!({ "secret": "s", "key": "k" });
        assert!(
            parses(what, &ok),
            "{what} {slot:?}: a reference is accepted"
        );
        for bad in wrong {
            let mut changed = doc.clone();
            *at_mut(&mut changed, &path) = bad.clone();
            assert!(
                !parses(what, &changed),
                "{what} {slot:?}: {bad} was accepted"
            );
        }
    }
}

#[test]
fn the_picks_of_a_form_are_the_references_of_its_objects() {
    let d = defaults();
    let form = coder_form("coder-me");
    let r = to_resources(&form, &d.deployment_for(AgentKind::Coder, "coder-me"), NS);
    let app =
        &j(&r.config.spec)["harness"]["adam"]["coder"]["github"]["app"]["privateKeySecretRef"];
    let picks = form.secret_picks();
    assert_eq!(picks.len(), 1);
    assert_eq!(
        *app,
        json!({ "name": picks[0].1.secret, "key": picks[0].1.key })
    );

    let m = model_form("gateway");
    let spec = j(&m.to_resource(NS).spec);
    assert_eq!(
        spec["apiKeySecretRef"],
        json!({ "name": "agent-models", "key": "GATEWAY_API_KEY" })
    );
    assert_eq!(
        spec["baseUrl"],
        json!({ "secretRef": { "name": "agent-models", "key": "GATEWAY_BASE_URL" } })
    );
    let t = j(&tool_form("websearch").to_resource(NS).spec);
    assert_eq!(
        t["mcp"]["headers"]["Authorization"],
        json!({ "prefix": "Bearer ", "secretRef": { "name": "agent-tools", "key": "SEARCH_MCP_TOKEN" } })
    );
}

// ------------------------------------------------- what the objects may contain

/// The strings the converters write that no input gave them: constants of the schema.
const CONSTANTS: &[&str] = &[
    "adam-rs",
    "adam-coder",
    "adam-agent",
    "Retain",
    "combined",
    "agents.vymalo.com/v1alpha1",
    "AgentService",
    "AgentConfig",
    "ModelEndpoint",
    "ToolProvider",
    "dashboard.agents.vymalo.com",
];

fn leaves(value: &Value) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    string_leaves(value, &mut out);
    out
}

/// Member names that name a secret, or could carry its value.
fn is_secretish(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    [
        "secret",
        "password",
        "token",
        "credential",
        "apikey",
        "privatekey",
    ]
    .iter()
    .any(|w| n.contains(w))
}

/// In `doc`, a member named like a secret holds a reference: `{ name, key }` and nothing else.
fn secretish_members_are_references(doc: &Value, at: &str, bad: &mut Vec<String>) {
    match doc {
        Value::Object(m) => {
            for (k, v) in m {
                let here = format!("{at}.{k}");
                // `installationId` and the like are not secretish; `bearerTokensSecretRef` is a reference.
                if is_secretish(k) {
                    let reference = match v {
                        Value::Object(r) => {
                            r.len() == 2
                                && r.get("name").is_some_and(Value::is_string)
                                && r.get("key").is_some_and(Value::is_string)
                        }
                        _ => false,
                    };
                    // `secretRef` inside a header or a base URL is a reference too (the object holds it).
                    if !reference {
                        bad.push(here.clone());
                    }
                }
                secretish_members_are_references(v, &here, bad);
            }
        }
        Value::Array(a) => a
            .iter()
            .enumerate()
            .for_each(|(i, v)| secretish_members_are_references(v, &format!("{at}[{i}]"), bad)),
        _ => {}
    }
}

fn agent_values(agent: &Agent) -> Vec<Value> {
    let mut out = vec![agent.service.spec.clone()];
    out.extend(agent.config.iter().map(|c| c.spec.clone()));
    out
}

proptest! {
    #![proptest_config(config(256))]

    /// Under arbitrary forms and deployments: every string in the objects, and in their YAML, was given by the form, the
    /// deployment or the namespace, or is a constant of the schema. The converters have no secret to add, and add none.
    #[test]
    fn the_objects_hold_only_what_they_were_given((form, deployment) in arb_agent()) {
        let given: BTreeSet<String> = leaves(&j(&form))
            .into_iter()
            .chain(leaves(&j(&deployment)))
            .chain([NS.to_owned()])
            .chain(CONSTANTS.iter().map(|c| (*c).to_owned()))
            .collect();
        let r = to_resources(&form, &deployment, NS);
        let agent = agent_of(&r, true);
        let mut docs = agent_values(&agent);
        docs.push(j(&r.service.metadata));
        docs.push(j(&r.config.metadata));
        for doc in &docs {
            for s in leaves(doc) {
                prop_assert!(given.contains(&s), "{s:?} came from nowhere");
            }
        }
        // The same for the YAML a person exports.
        let yaml = export_agent(&agent).unwrap();
        for document in serde_yaml::Deserializer::from_str(&yaml) {
            let value: Value = serde::Deserialize::deserialize(document).unwrap();
            for s in leaves(&value) {
                prop_assert!(given.contains(&s), "{s:?} is in the YAML and came from nowhere");
            }
        }
        // And every member named like a secret holds a reference.
        let mut bad = Vec::new();
        docs.iter().for_each(|d| secretish_members_are_references(d, "", &mut bad));
        prop_assert!(bad.is_empty(), "{bad:?}");
        let mut bad = Vec::new();
        secretish_members_are_references(&serde_yaml::from_str::<Value>(yaml.split("---\n").next().unwrap()).unwrap(), "", &mut bad);
        prop_assert!(bad.is_empty(), "{bad:?}");
    }

    /// The same for a form whose secrets are named after a value somebody might have tried to send: a pick is
    /// a name and a key and ends up as that name and that key, wherever the sentinel is in them.
    #[test]
    fn a_pick_is_a_reference_whatever_its_name((form, deployment) in arb_agent(), name in "[a-z]{1,5}-[a-zA-Z0-9]{1,6}") {
        let mut form = form;
        if let aap_forms::AgentKindForm::Coder(c) = &mut form.agent {
            c.github.private_key = aap_forms::SecretPick::new(format!("{SENTINEL}-{name}"), format!("{name}.{SENTINEL}"));
        }
        let r = to_resources(&form, &deployment, NS);
        let agent = agent_of(&r, true);
        // The sentinel is in the objects exactly as the reference's name and key, and nowhere else.
        let docs = agent_values(&agent);
        let mut found = 0;
        fn count(doc: &Value, found: &mut usize) {
            match doc {
                Value::String(s) if s.contains(SENTINEL) => *found += 1,
                Value::Array(a) => a.iter().for_each(|v| count(v, found)),
                Value::Object(m) => m.values().for_each(|v| count(v, found)),
                _ => {}
            }
        }
        docs.iter().for_each(|d| count(d, &mut found));
        let expected = if form.names_a_secret() { 2 } else { 0 };
        prop_assert_eq!(found, expected);
        if form.names_a_secret() {
            let key = &docs[1]["harness"]["adam"]["coder"]["github"]["app"]["privateKeySecretRef"];
            let mut want = Map::new();
            want.insert("name".into(), json!(format!("{SENTINEL}-{name}")));
            want.insert("key".into(), json!(format!("{name}.{SENTINEL}")));
            prop_assert_eq!(key, &Value::Object(want));
        }
    }

    #[test]
    fn the_shared_objects_hold_only_what_they_were_given(model in arb_model_form(), tool in arb_tool_form()) {
        let given_m: BTreeSet<String> = leaves(&j(&model)).into_iter().chain(CONSTANTS.iter().map(|c| (*c).to_owned())).chain([NS.to_owned()]).collect();
        let object = model.to_resource(NS);
        for doc in [j(&object.spec), j(&object.metadata)] {
            for s in leaves(&doc) {
                prop_assert!(given_m.contains(&s), "{s:?}");
            }
            let mut bad = Vec::new();
            secretish_members_are_references(&doc, "", &mut bad);
            prop_assert!(bad.is_empty(), "{bad:?}");
        }
        let given_t: BTreeSet<String> = leaves(&j(&tool)).into_iter().chain(CONSTANTS.iter().map(|c| (*c).to_owned())).chain([NS.to_owned()]).collect();
        let object = tool.to_resource(NS);
        for doc in [j(&object.spec), j(&object.metadata)] {
            for s in leaves(&doc) {
                prop_assert!(given_t.contains(&s), "{s:?}");
            }
            let mut bad = Vec::new();
            secretish_members_are_references(&doc, "", &mut bad);
            prop_assert!(bad.is_empty(), "{bad:?}");
        }
        // And the YAML of a stored copy.
        let stored_model = stored(Kind::ModelEndpoint, &model.name, model.draft().spec, true);
        let yaml = export_object(&stored_model).unwrap();
        let value: Value = serde_yaml::from_str(&yaml).unwrap();
        for s in leaves(&value) {
            prop_assert!(given_m.contains(&s), "{s:?} in the YAML");
        }
    }
}
