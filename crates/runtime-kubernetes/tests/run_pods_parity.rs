//! What the coder's chart renders for run pods (adam-rs `deploy/coder` at the revision of `tools/adam-parity/regen.sh`, checked in
//! as `crates/domain/tests/golden/coder-run-pods.json`) against what the operator and its chart make for the same agent: the
//! names and the guarantees. The template the run pods are made from is `crates/domain/tests/parity.rs`'s; this file is the
//! rest: the quota, the network policy, the account and its binding, the Role, the PriorityClass, and the admission policy,
//! which the operator's chart writes once with the agent's values in a parameter object and the coder's chart writes for the
//! agent with the values inline. The policy is compared **with the parameters substituted**: if a rule moves in one chart and
//! not in the other, or an expression is reworded, or a parameter is not the value the rule reads, a test here fails.
//!
//! What differs, on purpose, and is asserted as differing: the names of the cluster-scoped objects (the operator's chart
//! names them after the release and the namespace; the coder's after the agent), and the policy's match condition, which the
//! operator's reads from the parameter (one policy, many agents).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;

use aap_runtime_kubernetes::render;
use common::{repo_root, resolved_run_pods};
use serde::Deserialize;
use serde_json::{Value, json};

/// The coder chart's objects for run pods, as `tools/adam-parity/extract.py` projects them.
fn adam() -> Value {
    let path = repo_root().join("crates/domain/tests/golden/coder-run-pods.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    golden["runPods"].clone()
}

/// The documents of the operator chart's golden render with run pods on (`render-check.sh` holds it equal to the render).
fn chart() -> Vec<Value> {
    let path = repo_root().join("deploy/operator/tests/golden/runpods.yaml");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_yaml::Deserializer::from_str(&text)
        .map(|d| Value::deserialize(d).unwrap())
        .filter(|d| !d.is_null())
        .collect()
}

fn of_kind<'a>(docs: &'a [Value], kind: &str) -> &'a Value {
    let found: Vec<_> = docs.iter().filter(|d| d["kind"] == kind).collect();
    assert_eq!(found.len(), 1, "one {kind} in the chart's render");
    found[0]
}

/// What the operator makes for the coder example with the chart's default settings.
fn operator() -> Vec<Value> {
    let r = resolved_run_pods(|_, _| {});
    render(&r.id, &r.runtime).unwrap().documents()
}

fn find<'a>(docs: &'a [Value], kind: &str, name: &str) -> &'a Value {
    docs.iter()
        .find(|d| d["kind"] == kind && d["metadata"]["name"] == name)
        .unwrap_or_else(|| panic!("no {kind} {name}"))
}

#[test]
fn the_quota_the_network_policy_the_account_and_its_binding_are_the_coder_charts() {
    let adam = adam();
    let docs = operator();

    let quota = find(
        &docs,
        "ResourceQuota",
        adam["quota"]["name"].as_str().unwrap(),
    );
    assert_eq!(
        quota["spec"]["hard"], adam["quota"]["hard"],
        "what the run pods may use in all"
    );
    assert_eq!(
        quota["spec"]["scopeSelector"], adam["quota"]["scopeSelector"],
        "and which pods the quota counts: those of the priority class"
    );

    let policy = find(
        &docs,
        "NetworkPolicy",
        adam["networkPolicy"]["name"].as_str().unwrap(),
    );
    for key in ["podSelector", "policyTypes", "ingress", "egress"] {
        assert_eq!(
            policy["spec"][key], adam["networkPolicy"][key],
            "the network policy of run pods: {key}"
        );
    }

    let account = find(
        &docs,
        "ServiceAccount",
        adam["serviceAccount"]["name"].as_str().unwrap(),
    );
    assert_eq!(
        account["automountServiceAccountToken"],
        adam["serviceAccount"]["automountServiceAccountToken"],
        "the account's token is mounted by the pods that ask for it"
    );

    let binding = find(
        &docs,
        "RoleBinding",
        adam["roleBinding"]["name"].as_str().unwrap(),
    );
    assert_eq!(binding["roleRef"], adam["roleBinding"]["roleRef"]);
    assert_eq!(binding["subjects"], adam["roleBinding"]["subjects"]);
}

#[test]
fn the_role_and_the_priority_class_of_the_charts_are_the_coder_charts() {
    let adam = adam();
    let chart = chart();
    let role = chart
        .iter()
        .find(|d| d["kind"] == "Role" && d["metadata"]["name"] == "aap-operator-run-pods")
        .expect("the Role of run pods");
    assert_eq!(
        role["rules"], adam["role"]["rules"],
        "what an account bound to the Role may do"
    );
    let class = of_kind(&chart, "PriorityClass");
    for key in ["value", "preemptionPolicy", "globalDefault"] {
        assert_eq!(
            class[key], adam["priorityClass"][key],
            "the priority class: {key}"
        );
    }
}

/// `params.data.<name>` of an expression, replaced by the parameter's value as a CEL string (or a list of strings for the images).
fn substitute(expression: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = expression.to_owned();
    let images: Vec<String> = params["images"]
        .split('\n')
        .map(|i| format!("'{i}'"))
        .collect();
    out = out.replace(
        "params.data.images.split('\\n')",
        &format!("[{}]", images.join(", ")),
    );
    for (key, value) in params {
        out = out.replace(&format!("params.data.{key}"), &format!("'{value}'"));
    }
    assert!(!out.contains("params."), "a parameter nobody gives: {out}");
    out
}

/// The coder's image carries `@sha256:...` after its tag in the operator's example, and the coder chart's case names the tag alone:
/// the one difference of the images (the same as `crates/domain/tests/parity.rs`' `strip_digest`), taken off before comparing.
fn without_the_coders_digest(text: &str) -> String {
    const CODER: &str = "another-adam-rs/coder:";
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find(CODER) {
        let after = at + CODER.len();
        out.push_str(&rest[..after]);
        rest = &rest[after..];
        let tag_end = rest.find(['@', '\'', ' ', ']']).unwrap_or(rest.len());
        out.push_str(&rest[..tag_end]);
        rest = &rest[tag_end..];
        if let Some(digest) = rest.strip_prefix("@sha256:")
            && digest.len() >= 64
            && digest.as_bytes()[..64].iter().all(u8::is_ascii_hexdigit)
        {
            rest = &digest[64..];
        }
    }
    out.push_str(rest);
    out
}

/// A `messageExpression` that only joins strings, evaluated: `'a' + 'b'` is `'ab'`, and the quotes come off.
fn message(rule: &Value, params: &BTreeMap<String, String>) -> String {
    if let Some(m) = rule["message"].as_str() {
        return m.to_owned();
    }
    let joined = substitute(
        rule["messageExpression"].as_str().expect("a message"),
        params,
    )
    .replace("' + '", "");
    joined
        .strip_prefix('\'')
        .and_then(|m| m.strip_suffix('\''))
        .unwrap_or_else(|| panic!("a message expression that only joins strings: {joined}"))
        .to_owned()
}

#[test]
fn the_admission_policy_of_the_operators_chart_with_the_agents_parameters_is_the_coder_charts_policy()
 {
    let adam = adam();
    let chart = chart();
    let policy = of_kind(&chart, "ValidatingAdmissionPolicy");
    let binding = of_kind(&chart, "ValidatingAdmissionPolicyBinding");

    // The parameters the operator makes for this agent, as the chart's policy reads them.
    let docs = operator();
    let params_object = find(&docs, "ConfigMap", "coder-run-pods-policy");
    let params: BTreeMap<String, String> =
        serde_json::from_value(params_object["data"].clone()).unwrap();
    assert_eq!(
        params_object["metadata"]["labels"]["agents.vymalo.com/run-pods-policy"], "true",
        "the label the chart's binding selects the parameters by"
    );

    // The same policy as a whole.
    assert_eq!(
        policy["spec"]["failurePolicy"],
        adam["admissionPolicy"]["failurePolicy"]
    );
    assert_eq!(
        policy["spec"]["matchConstraints"],
        adam["admissionPolicy"]["matchConstraints"]
    );
    assert_eq!(
        policy["spec"]["variables"],
        adam["admissionPolicy"]["variables"]
    );
    let conditions: Vec<Value> = policy["spec"]["matchConditions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| json!({"name": c["name"], "expression": substitute(c["expression"].as_str().unwrap(), &params)}))
        .collect();
    assert_eq!(
        Value::Array(conditions),
        adam["admissionPolicy"]["matchConditions"],
        "the policy judges the pods created by the agent's account, named by the parameter"
    );

    // Rule by rule: the same expression with the values substituted, the same words said when it refuses.
    let ours = policy["spec"]["validations"].as_array().unwrap();
    let theirs = adam["admissionPolicy"]["validations"].as_array().unwrap();
    assert_eq!(ours.len(), theirs.len(), "the same number of rules");
    for (mine, chart_rule) in ours.iter().zip(theirs) {
        assert_eq!(
            without_the_coders_digest(&substitute(mine["expression"].as_str().unwrap(), &params)),
            chart_rule["expression"].as_str().unwrap(),
            "the rule: {}",
            chart_rule["message"]
        );
        assert_eq!(
            message(mine, &params),
            chart_rule["message"].as_str().unwrap()
        );
    }

    // The binding: it denies, and covers one namespace by its name label.
    assert_eq!(
        binding["spec"]["validationActions"],
        adam["admissionBinding"]["validationActions"]
    );
    assert_eq!(
        binding["spec"]["matchResources"]["namespaceSelector"]["matchLabels"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        adam["admissionBinding"]["matchResources"]["namespaceSelector"]["matchLabels"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>()
    );
}

#[test]
fn what_differs_from_the_coder_chart_is_what_is_meant_to() {
    let adam = adam();
    let chart = chart();
    // The names of the cluster-scoped objects are the release's and the namespace's, the coder chart's are the agent's.
    assert_ne!(
        of_kind(&chart, "ValidatingAdmissionPolicy")["metadata"]["name"],
        adam["admissionPolicy"]["name"]
    );
    // One policy for every agent: the account it judges is read from the parameter, not written in the policy.
    let policy = of_kind(&chart, "ValidatingAdmissionPolicy");
    assert!(
        policy["spec"]["matchConditions"][0]["expression"]
            .as_str()
            .unwrap()
            .contains("params.data.serviceAccount")
    );
    // A namespace in which no agent asks for run pods has no parameter; the binding must not deny every pod there.
    let binding = of_kind(&chart, "ValidatingAdmissionPolicyBinding");
    assert_eq!(
        binding["spec"]["paramRef"]["parameterNotFoundAction"],
        "Allow"
    );
}
