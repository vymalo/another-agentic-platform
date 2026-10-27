//! What the coder's chart renders for run pods (adam-rs `deploy/coder` at the revision of `tools/adam-parity/regen.sh`, checked in
//! as `crates/domain/tests/golden/coder-run-pods.json`) against what the operator and its chart make for the same agent: the
//! names and the guarantees. The template the run pods are made from is `crates/domain/tests/parity.rs`'s; this file is the
//! rest: the quota, the network policy, the account and its binding, the Role, the PriorityClass, and the admission policy,
//! which the operator's chart writes once with the agent's values in a parameter object and the coder's chart writes for the
//! agent with the values inline. The policy is compared **with the parameters substituted**: if a rule moves in one chart and
//! not in the other, or an expression is reworded, or a parameter is not the value the rule reads, a test here fails.
//!
//! The egress proxy (slice D15c, adam-rs ADR 0042) is compared object by object in every run pod case of the goldens: the
//! Deployment, Service, ServiceAccount, allow-list, PodDisruptionBudget and NetworkPolicy, and the run pods' own policy that
//! names it.
//!
//! What differs, on purpose, and is asserted as differing: the names of the cluster-scoped objects (the operator's chart
//! names them after the release and the namespace; the coder's after the agent), the policy's match condition, which the
//! operator's reads from the parameter (one policy, many agents), the labels the chart puts on its objects that are the
//! release's (`version`, `managed-by: Helm`, `helm.sh/chart`; the operator's objects carry `managed-by: aap-operator`), the
//! value of the proxy's allow-list checksum (the chart hashes its rendered ConfigMap, the operator the file) and the digest of
//! the coder's image.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;

use aap_runtime_kubernetes::render;
use common::{repo_root, resolved_run_pods};
use serde::Deserialize;
use serde_json::{Value, json};

/// The coder chart's objects for run pods, as `tools/adam-parity/extract.py` projects them.
fn adam() -> Value {
    adam_case("coder-run-pods")
}

fn adam_case(case: &str) -> Value {
    let path = repo_root().join(format!("crates/domain/tests/golden/{case}.json"));
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

/// The labels of the chart's objects that are the operator's too: the selector labels (the release's own are `version`,
/// `managed-by: Helm` and `helm.sh/chart`).
fn selector_labels(labels: &Value) -> Value {
    json!({
        "app.kubernetes.io/name": labels["app.kubernetes.io/name"],
        "app.kubernetes.io/instance": labels["app.kubernetes.io/instance"],
        "app.kubernetes.io/component": labels["app.kubernetes.io/component"],
    })
}

/// What the operator makes for a case: the objects of its run pods with the case's settings (the case's values, in the
/// operator chart's terms: the keys of `egressProxy` and `networkPolicy` are the same).
fn operator_case(case: &str) -> Vec<Value> {
    let (settings, class) = common::run_pod_settings_of(case);
    let r = common::resolved_with("coder", &settings, |s, c| {
        common::set(
            c,
            "/spec/harness/adam/coder/runPods",
            json!({ "sizeClass": class }),
        );
        // The cases are about the objects of run pods, not about the shape of the coder: the example as it is.
        let _ = s;
    });
    render(&r.id, &r.runtime).unwrap().documents()
}

#[test]
fn the_egress_proxy_is_the_coder_charts_in_every_case() {
    for case in [
        "coder-run-pods",
        "coder-run-pods-shared",
        "coder-run-pods-split",
        "coder-run-pods-tuned",
    ] {
        let adam = adam_case(case);
        let docs = if case == "coder-run-pods" {
            operator()
        } else {
            operator_case(case)
        };
        let proxy = &adam["egressProxy"];
        let at = |what: &str| format!("{case}: {what}");

        // The names, and the selector labels the objects share.
        let deployment = &proxy["deployment"];
        let name = deployment["name"].as_str().unwrap();
        assert_eq!(name, proxy["service"]["name"].as_str().unwrap());
        let ours = find(&docs, "Deployment", name);
        assert_eq!(
            ours["metadata"]["labels"]["app.kubernetes.io/managed-by"],
            "aap-operator",
            "{}",
            at("ours, not Helm's")
        );
        assert_eq!(
            selector_labels(&ours["metadata"]["labels"]),
            deployment["labels"],
            "{}",
            at("the labels of the proxy's objects")
        );
        for (kind, key) in [
            ("Service", "service"),
            ("ServiceAccount", "serviceAccount"),
            ("NetworkPolicy", "networkPolicy"),
        ] {
            let o = find(&docs, kind, proxy[key]["name"].as_str().unwrap());
            assert_eq!(
                selector_labels(&o["metadata"]["labels"]),
                proxy[key]["labels"],
                "{}",
                at(kind)
            );
        }

        // The Deployment: replicas, the roll, the selector, the pod and its container, field by field.
        let spec = &ours["spec"];
        assert_eq!(
            spec["replicas"],
            deployment["replicas"],
            "{}",
            at("replicas")
        );
        assert_eq!(
            spec["strategy"],
            deployment["strategy"],
            "{}",
            at("strategy")
        );
        assert_eq!(
            spec["selector"],
            deployment["selector"],
            "{}",
            at("selector")
        );
        let pod_template = &spec["template"];
        assert_eq!(
            pod_template["metadata"]["labels"],
            deployment["podLabels"],
            "{}",
            at("pod labels")
        );
        let annotations: Vec<&str> = pod_template["metadata"]["annotations"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            annotations,
            deployment["podAnnotationKeys"]
                .as_array()
                .unwrap()
                .iter()
                .map(|k| k.as_str().unwrap())
                .collect::<Vec<_>>(),
            "{}",
            at("the annotation that rolls the pods with the allow-list")
        );
        let pod = &pod_template["spec"];
        for (key, theirs) in [
            ("serviceAccountName", "serviceAccountName"),
            (
                "automountServiceAccountToken",
                "automountServiceAccountToken",
            ),
            ("enableServiceLinks", "enableServiceLinks"),
            (
                "terminationGracePeriodSeconds",
                "terminationGracePeriodSeconds",
            ),
            ("securityContext", "securityContext"),
            ("volumes", "volumes"),
        ] {
            assert_eq!(pod[key], deployment[theirs], "{}", at(key));
        }
        assert_eq!(pod["containers"].as_array().unwrap().len(), 1);
        let (mine, theirs) = (&pod["containers"][0], &deployment["containers"][0]);
        for key in [
            "name",
            "command",
            "imagePullPolicy",
            "securityContext",
            "lifecycle",
            "readinessProbe",
            "livenessProbe",
            "volumeMounts",
        ] {
            assert_eq!(
                mine[key],
                theirs[key],
                "{}",
                at(&format!("the proxy's container: {key}"))
            );
        }
        // The ports: the same; the resources: the same; the arguments: none.
        assert_eq!(mine["ports"], theirs["ports"], "{}", at("ports"));
        assert_eq!(
            mine["resources"],
            theirs["resources"],
            "{}",
            at("resources")
        );
        assert!(theirs["args"].as_array().unwrap().is_empty());
        assert!(mine.get("args").is_none());
        // The variables: the same names in the same order with the same literal values (the projection's `literal`).
        let env: Vec<Value> = mine["env"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| json!({"name": e["name"], "literal": e["value"]}))
            .collect();
        assert_eq!(Value::Array(env), theirs["env"], "{}", at("env"));
        // The image: the coder's own. The operator's carries the digest the chart cannot express.
        assert_eq!(
            without_the_coders_digest(mine["image"].as_str().unwrap()),
            theirs["image"].as_str().unwrap(),
            "{}",
            at("the image")
        );

        // The Service.
        let service = find(&docs, "Service", name);
        for (key, theirs) in [
            ("type", "type"),
            ("selector", "selector"),
            ("ports", "ports"),
        ] {
            assert_eq!(
                service["spec"][key],
                proxy["service"][theirs],
                "{}",
                at(&format!("the proxy's service: {key}"))
            );
        }
        // The account: no token.
        let account = find(&docs, "ServiceAccount", name);
        assert_eq!(
            account["automountServiceAccountToken"],
            proxy["serviceAccount"]["automountServiceAccountToken"]
        );
        // The allow-list: the same file, to the letter.
        let allow = find(
            &docs,
            "ConfigMap",
            proxy["allowList"]["name"].as_str().unwrap(),
        );
        assert_eq!(
            allow["data"],
            proxy["allowList"]["data"],
            "{}",
            at("the allow-list")
        );
        assert_eq!(
            selector_labels(&allow["metadata"]["labels"]),
            proxy["allowList"]["labels"]
        );
        // The budget: only for more than one replica, as the chart's.
        match proxy.get("podDisruptionBudget") {
            Some(budget) => {
                let ours = find(&docs, "PodDisruptionBudget", name);
                assert_eq!(
                    ours["spec"]["minAvailable"],
                    budget["minAvailable"],
                    "{}",
                    at("minAvailable")
                );
                assert_eq!(
                    ours["spec"]["selector"],
                    budget["selector"],
                    "{}",
                    at("the budget's selector")
                );
            }
            None => assert!(
                docs.iter()
                    .all(|d| d["kind"] != "PodDisruptionBudget" || d["metadata"]["name"] != name),
                "{}",
                at("no budget for one replica")
            ),
        }
        // The proxy's policy, and the run pods' policy that names it.
        let policy = find(&docs, "NetworkPolicy", name);
        for key in ["podSelector", "policyTypes", "ingress", "egress"] {
            assert_eq!(
                policy["spec"][key],
                proxy["networkPolicy"][key],
                "{}",
                at(&format!("the proxy's network policy: {key}"))
            );
        }
        let run_pods = find(
            &docs,
            "NetworkPolicy",
            adam["networkPolicy"]["name"].as_str().unwrap(),
        );
        for key in ["podSelector", "policyTypes", "ingress", "egress"] {
            assert_eq!(
                run_pods["spec"][key],
                adam["networkPolicy"][key],
                "{}",
                at(&format!("the run pods' network policy: {key}"))
            );
        }
    }
}

// ---------------------------------------------------------------- the coder's pod: the git sandbox (slice D15d)

/// The projection of `tools/adam-parity/extract.py` for the workloads of a case: what the coder chart renders for its pods.
fn adam_workloads(case: &str) -> Value {
    let path = repo_root().join(format!("crates/domain/tests/golden/{case}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let golden: Value = serde_json::from_str(&text).unwrap();
    golden["workloads"].clone()
}

/// A probe of the pod spec, in the golden's shape.
fn probe_of(p: &Value) -> Value {
    if p.is_null() {
        return Value::Null;
    }
    json!({
        "exec": p["exec"]["command"],
        "failureThreshold": p["failureThreshold"],
        "period": p["periodSeconds"],
        "timeout": p["timeoutSeconds"],
    })
}

/// A container of the pod spec, in the golden's shape (`extract.py`'s `container`): the fields the sandbox has.
fn container_of(c: &Value) -> Value {
    let image = c["image"].as_str().unwrap();
    let mut out = json!({
        "image": image.split_once('@').map_or(image, |(tag, _)| tag),
        "command": c["command"],
        "args": c.get("args").cloned().unwrap_or_else(|| json!([])),
        "env": c["env"].as_array().unwrap().iter().map(|e| {
            json!({"name": e["name"], "literal": e["value"]})
        }).collect::<Vec<_>>(),
        "mounts": c["volumeMounts"].as_array().unwrap().iter().map(|m| {
            json!({"name": m["name"], "path": m["mountPath"], "readOnly": m.get("readOnly").and_then(Value::as_bool).unwrap_or(false)})
        }).collect::<Vec<_>>(),
        "probes": {
            "startup": probe_of(&c["startupProbe"]),
            "liveness": probe_of(&c["livenessProbe"]),
            "readiness": probe_of(&c["readinessProbe"]),
        },
        "resources": {
            "requests": c["resources"].get("requests").cloned().unwrap_or_else(|| json!({})),
            "limits": c["resources"].get("limits").cloned().unwrap_or_else(|| json!({})),
        },
        "port": Value::Null,
    });
    if c["securityContext"]["readOnlyRootFilesystem"] == true {
        out["readOnlyRootFilesystem"] = json!(true);
    }
    out
}

/// A volume of the pod spec of one of the kinds the sandbox adds, in the golden's shape.
fn volume_of(v: &Value) -> Option<Value> {
    if let Some(e) = v.get("emptyDir") {
        return Some(json!({
            "name": v["name"], "kind": "emptyDir",
            "medium": e.get("medium").cloned().unwrap_or(Value::Null),
            "sizeLimit": e.get("sizeLimit").cloned().unwrap_or(Value::Null),
        }));
    }
    v.get("projected").map(|p| {
        json!({
            "name": v["name"], "kind": "projected",
            "mode": p["defaultMode"], "sources": p["sources"],
        })
    })
}

#[test]
fn the_coders_pod_has_the_git_sandbox_the_coder_chart_renders_in_every_case() {
    // The cases are about the settings and the objects of run pods, as the case above: the worker's pod, with the case's settings.
    for case in [
        "coder-run-pods",
        "coder-run-pods-shared",
        "coder-run-pods-split",
        "coder-run-pods-tuned",
    ] {
        let adam = adam_workloads(case);
        let docs = if case == "coder-run-pods" {
            operator()
        } else {
            operator_case(case)
        };
        let at = |what: &str| format!("{case}: {what}");
        let pod = &find(&docs, "StatefulSet", "coder")["spec"]["template"]["spec"];
        let theirs = &adam["coder"];

        // The pod: no automounted token, the account named.
        assert_eq!(
            pod["automountServiceAccountToken"],
            json!(theirs["automountServiceAccountToken"]),
            "{}",
            at("automountServiceAccountToken")
        );
        assert_eq!(pod["serviceAccountName"], theirs["serviceAccount"]);

        // The sandbox: command, variables, mounts, probes, resources and the read-only root, field by field, as the chart renders them.
        let sandbox = pod["initContainers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "git-sandbox")
            .unwrap_or_else(|| panic!("{}", at("no git-sandbox")));
        assert_eq!(
            container_of(sandbox),
            theirs["sidecars"]["git-sandbox"],
            "{}",
            at("the git-sandbox container")
        );
        assert_eq!(
            sandbox["restartPolicy"],
            "Always",
            "{}",
            at("a native sidecar")
        );
        // The chart says the seccomp profile once more at the container; the operator says it once for the pod.
        assert_eq!(
            pod["securityContext"]["seccompProfile"],
            json!({"type": "RuntimeDefault"})
        );

        // The volumes of those kinds: the account's credentials and the two scratch directories, as the chart renders them.
        let mut ours: Vec<Value> = pod["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(volume_of)
            .collect();
        let mut chart: Vec<Value> = theirs["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["kind"] == "emptyDir" || v["kind"] == "projected")
            .cloned()
            .collect();
        ours.sort_by_key(|v| v["name"].as_str().unwrap().to_owned());
        chart.sort_by_key(|v| v["name"].as_str().unwrap().to_owned());
        assert_eq!(
            ours.len(),
            3,
            "{}",
            at("the credentials and two scratch volumes")
        );
        assert_eq!(ours, chart, "{}", at("the volumes"));

        // The coder container: the credentials, read-only, where an in-cluster client reads them, and the socket's directory.
        let coder = container_of(&pod["containers"][0]);
        for name in ["kube-api-access", "git-sandbox"] {
            let mount = |c: &Value| {
                c["mounts"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["name"] == name)
                    .cloned()
            };
            assert!(mount(&coder).is_some(), "{}", at(name));
            assert_eq!(mount(&coder), mount(&theirs["agent"]), "{}", at(name));
        }
        let socket = |c: &Value| {
            c["env"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e["name"] == "GIT_SANDBOX_SOCKET")
                .cloned()
        };
        assert!(socket(&coder).is_some());
        assert_eq!(
            socket(&coder),
            socket(&theirs["agent"]),
            "{}",
            at("GIT_SANDBOX_SOCKET")
        );

        // Who holds the credentials, in the render itself: the coder container and no other.
        let holders: Vec<&str> = pod["containers"]
            .as_array()
            .unwrap()
            .iter()
            .chain(pod["initContainers"].as_array().unwrap())
            .filter(|c| {
                c["volumeMounts"]
                    .as_array()
                    .is_some_and(|m| m.iter().any(|m| m["name"] == "kube-api-access"))
            })
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(holders, ["agent"], "{}", at("who mounts the credentials"));
    }
}

#[test]
fn a_split_front_has_none_of_it_in_the_chart_and_in_the_operator() {
    let adam = adam_workloads("coder-run-pods-split");
    let front = &adam["coder-front"];
    assert!(front["sidecars"].as_object().unwrap().is_empty());
    assert!(front["serviceAccount"].is_null());
    assert_eq!(front["automountServiceAccountToken"], false);
    assert!(
        front["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["kind"] != "projected" && v["kind"] != "emptyDir")
    );
    assert!(
        front["agent"]["env"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["name"] != "GIT_SANDBOX_SOCKET")
    );
    let split = common::resolved_run_pods(|s, c| {
        common::set(s, "/spec/scaling/topology", json!("split"));
        common::set(s, "/spec/scaling/workers", json!(2));
        common::set(s, "/spec/scaling/front", json!({"replicas": 2}));
        common::set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!("shared"),
        );
        common::set(
            c,
            "/spec/environment/volumes/0/source/persistent/perReplica",
            json!(false),
        );
    });
    let docs = render(&split.id, &split.runtime).unwrap().documents();
    let ours = &find(&docs, "Deployment", "coder-front")["spec"]["template"]["spec"];
    assert!(
        ours.get("initContainers").is_none(),
        "a front with a sidecar"
    );
    assert!(ours.get("serviceAccountName").is_none());
    assert_eq!(ours["automountServiceAccountToken"], false);
    assert!(
        ours["volumes"].is_null()
            || ours["volumes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|v| v.get("projected").is_none() && v.get("emptyDir").is_none())
    );
    let workers = &find(&docs, "StatefulSet", "coder")["spec"]["template"]["spec"];
    assert!(
        workers["initContainers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "git-sandbox")
    );
}

#[test]
fn the_chart_ships_the_coder_charts_defaults_for_the_git_sandbox() {
    // The operator chart's defaults are the coder chart's: resources, size of /tmp and number of commands, read from the golden.
    let adam = adam_workloads("coder-run-pods");
    let sandbox = &adam["coder"]["sidecars"]["git-sandbox"];
    let values = common::operator_chart_values();
    let ours = &values["runPods"]["gitSandbox"];
    assert_eq!(
        ours["resources"]["requests"],
        sandbox["resources"]["requests"]
    );
    assert_eq!(ours["resources"]["limits"], sandbox["resources"]["limits"]);
    let tmp = adam["coder"]["volumes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "git-sandbox-tmp")
        .unwrap();
    assert_eq!(ours["tmpSize"], tmp["sizeLimit"]);
    let max = sandbox["env"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "GIT_SANDBOX_MAX_COMMANDS")
        .unwrap();
    assert_eq!(
        ours["maxCommands"].to_string(),
        max["literal"].as_str().unwrap()
    );
    // And no switch in the operator chart's values, as in the coder chart's.
    assert!(ours.get("enabled").is_none());
}

#[test]
fn the_chart_ships_the_coder_charts_default_allow_list_and_nothing_is_open_by_default() {
    // The operator chart's default list is the coder chart's, host for host, in order: a host added to one and not the other
    // fails here.
    let adam = adam();
    let text = adam["egressProxy"]["allowList"]["data"]["allow.txt"]
        .as_str()
        .unwrap();
    let hosts: Vec<&str> = text.lines().filter(|l| !l.starts_with('#')).collect();
    let values = common::operator_chart_values();
    let ours: Vec<&str> = values["runPods"]["egressProxy"]["allow"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h.as_str().unwrap())
        .collect();
    assert_eq!(ours, hosts);
    assert_eq!(values["runPods"]["egressProxy"]["extraAllow"], json!([]));
    assert_eq!(values["runPods"]["egressProxy"]["extraNoProxy"], json!([]));
    // Defaults of the rest, as the coder chart's.
    let deployment = &adam["egressProxy"]["deployment"];
    assert_eq!(
        values["runPods"]["egressProxy"]["replicas"],
        deployment["replicas"]
    );
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
    // The labels of the proxy's objects: the chart's are the release's besides the selector labels, ours say who made them.
    let docs = operator();
    let labels = &find(&docs, "Deployment", "coder-egress")["metadata"]["labels"];
    assert_eq!(labels["app.kubernetes.io/managed-by"], "aap-operator");
    assert!(labels.get("helm.sh/chart").is_none());
    // The checksum that rolls the proxy is a hash of the file here and of the rendered ConfigMap there: same key, other value.
    let ours = find(&docs, "Deployment", "coder-egress")["spec"]["template"]["metadata"]["annotations"]
        ["checksum/egress-allow"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(ours.len(), 64);
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
