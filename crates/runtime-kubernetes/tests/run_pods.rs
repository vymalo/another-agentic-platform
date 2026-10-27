//! The objects of run pods (slice D15a): what the provider makes of `RuntimeSpec::run_pods`, rule by rule and as a
//! golden, and that they agree with the workloads they belong to. What the coder's chart renders for the same
//! (names, selectors, quota, rules) is `tests/run_pods_parity.rs`; the requests against an API server are
//! `tests/api.rs`'s.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::path::PathBuf;

use aap_ports::RuntimeId;
use aap_runtime_kubernetes::names::{DIGEST_ANNOTATION, MANAGED_BY_LABEL, MANAGED_BY_VALUE};
use aap_runtime_kubernetes::{Rendered, render};
use common::{resolved, resolved_run_pods, set};
use serde_json::{Value, json};

fn rendered(r: &aap_domain::ResolvedAgent) -> Rendered {
    render(&r.id, &r.runtime).unwrap()
}

fn find<'a>(docs: &'a [Value], kind: &str, name: &str) -> &'a Value {
    docs.iter()
        .find(|d| d["kind"] == kind && d["metadata"]["name"] == name)
        .unwrap_or_else(|| panic!("no {kind} {name} in {:?}", kinds(docs)))
}

fn kinds(docs: &[Value]) -> Vec<String> {
    docs.iter()
        .map(|d| format!("{}/{}", d["kind"], d["metadata"]["name"]))
        .collect()
}

fn golden_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}.yaml"))
}

fn yaml(r: &Rendered) -> String {
    r.documents()
        .iter()
        .map(|d| serde_yaml::to_string(d).unwrap())
        .collect::<Vec<_>>()
        .join("---\n")
}

/// Regenerate with `AAP_UPDATE_GOLDENS=1 cargo test -p aap-runtime-kubernetes --test run_pods` and read the diff: it is the
/// change of what the operator makes for run pods.
fn assert_golden(name: &str, r: &Rendered) {
    let got = yaml(r);
    let path = golden_path(name);
    if std::env::var("AAP_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::write(&path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with AAP_UPDATE_GOLDENS=1", path.display()));
    assert!(
        got == want,
        "{name} differs from {}; run with AAP_UPDATE_GOLDENS=1 and read the diff.\n--- got ---\n{got}",
        path.display()
    );
}

// ---------------------------------------------------------------- the golden and the names

#[test]
fn the_coder_with_run_pods() {
    assert_golden("coder-run-pods", &rendered(&resolved_run_pods(|_, _| {})));
}

#[test]
fn the_template_is_the_file_set_the_provider_removes_by_name() {
    // The provider clears the template when a service stops asking for run pods, and finds it by this name: the file set
    // the domain makes, `<service>-run-pod`.
    let r = resolved_run_pods(|_, _| {});
    let name = aap_runtime_kubernetes::names::run_pod_template(&r.id);
    let docs = rendered(&r).documents();
    let template = find(&docs, "ConfigMap", &name);
    assert!(template["data"]["pod.yaml"].is_string());
}

#[test]
fn the_objects_and_their_order() {
    let docs = rendered(&resolved_run_pods(|_, _| {})).documents();
    assert_eq!(
        kinds(&docs),
        [
            // The template and the file of the extra MCP servers are file sets, in the order of the spec.
            "\"ConfigMap\"/\"coder-mcp\"",
            "\"ConfigMap\"/\"coder-run-pod\"",
            // The guard's parameters and the account exist before the right to make a pod does.
            "\"ConfigMap\"/\"coder-run-pods-policy\"",
            "\"ServiceAccount\"/\"coder\"",
            "\"ResourceQuota\"/\"coder-run-pods\"",
            "\"NetworkPolicy\"/\"coder-run-pods\"",
            "\"RoleBinding\"/\"coder-run-pods\"",
            "\"Service\"/\"coder\"",
            "\"StatefulSet\"/\"coder\"",
            "\"NetworkPolicy\"/\"coder\"",
        ]
    );
}

#[test]
fn without_run_pods_none_of_it_is_made() {
    let docs = rendered(&resolved("coder", |_, _| {})).documents();
    for d in &docs {
        assert!(
            !["ServiceAccount", "ResourceQuota", "RoleBinding"]
                .contains(&d["kind"].as_str().unwrap()),
            "{}",
            d["metadata"]["name"]
        );
    }
    assert!(
        docs.iter()
            .all(|d| d["metadata"]["name"] != "coder-run-pods")
    );
    let sts = find(&docs, "StatefulSet", "coder");
    let pod = &sts["spec"]["template"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert!(pod.get("serviceAccountName").is_none());
}

#[test]
fn every_object_carries_the_labels_the_digest_and_the_owner_and_the_parameters_the_policys_label() {
    let r = resolved_run_pods(|_, _| {});
    let docs = rendered(&r).documents();
    for name in ["coder-run-pods-policy", "coder-run-pods", "coder"] {
        for d in docs.iter().filter(|d| d["metadata"]["name"] == name) {
            let what = format!("{}/{name}", d["kind"]);
            let labels = &d["metadata"]["labels"];
            assert_eq!(labels[MANAGED_BY_LABEL], MANAGED_BY_VALUE, "{what}");
            assert_eq!(labels["app.kubernetes.io/instance"], "coder", "{what}");
            assert_eq!(
                d["metadata"]["annotations"][DIGEST_ANNOTATION], r.digest,
                "{what}"
            );
            assert_eq!(
                d["metadata"]["namespace"], "another-agentic-system",
                "{what}"
            );
            assert_eq!(
                d["metadata"]["ownerReferences"][0]["kind"], "AgentService",
                "{what}"
            );
        }
    }
    // The label the chart's binding selects the parameters by is on them and on nothing else.
    let labelled: Vec<_> = docs
        .iter()
        .filter(|d| d["metadata"]["labels"]["agents.vymalo.com/run-pods-policy"] == "true")
        .map(|d| d["metadata"]["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(labelled, ["coder-run-pods-policy"]);
}

// ---------------------------------------------------------------- the objects, one by one

#[test]
fn the_account_holds_its_token_in_the_workers_pod_alone() {
    let docs = rendered(&resolved_run_pods(|_, _| {})).documents();
    let account = find(&docs, "ServiceAccount", "coder");
    assert_eq!(
        account["automountServiceAccountToken"], false,
        "the account's own default"
    );
    let pod = &find(&docs, "StatefulSet", "coder")["spec"]["template"]["spec"];
    assert_eq!(pod["serviceAccountName"], "coder");
    assert_eq!(pod["automountServiceAccountToken"], true);
    let env = pod["containers"][0]["env"].as_array().unwrap();
    let namespace = env
        .iter()
        .find(|e| e["name"] == "RUN_POD_NAMESPACE")
        .unwrap();
    assert_eq!(
        namespace["valueFrom"]["fieldRef"]["fieldPath"],
        "metadata.namespace"
    );

    // A split front has the namespace's default account and no token.
    let split = resolved_run_pods(|s, c| {
        set(s, "/spec/scaling/topology", json!("split"));
        set(s, "/spec/scaling/workers", json!(2));
        set(s, "/spec/scaling/front", json!({"replicas": 1}));
        set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!("shared"),
        );
        set(
            c,
            "/spec/environment/volumes/0/source/persistent/perReplica",
            json!(false),
        );
    });
    let docs = rendered(&split).documents();
    let front = &find(&docs, "Deployment", "coder-front")["spec"]["template"]["spec"];
    assert_eq!(front["automountServiceAccountToken"], false);
    assert!(front.get("serviceAccountName").is_none());
    let workers = &find(&docs, "StatefulSet", "coder")["spec"]["template"]["spec"];
    assert_eq!(workers["serviceAccountName"], "coder");
}

#[test]
fn the_account_is_bound_to_the_deployments_role_and_to_no_other() {
    let docs = rendered(&resolved_run_pods(|_, _| {})).documents();
    let binding = find(&docs, "RoleBinding", "coder-run-pods");
    assert_eq!(
        binding["roleRef"],
        json!({"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "coder-run-pods"})
    );
    assert_eq!(
        binding["subjects"],
        json!([{"kind": "ServiceAccount", "name": "coder", "namespace": "another-agentic-system"}])
    );
    // The provider makes no Role: it binds the one the deployment made.
    assert!(
        docs.iter()
            .all(|d| d["kind"] != "Role" && d["kind"] != "ClusterRole")
    );
}

#[test]
fn the_quota_is_scoped_to_the_priority_class() {
    let docs = rendered(&resolved_run_pods(|_, _| {})).documents();
    let quota = &find(&docs, "ResourceQuota", "coder-run-pods")["spec"];
    assert_eq!(quota["hard"], json!({"limits.memory": "8Gi", "pods": "4"}));
    assert_eq!(
        quota["scopeSelector"]["matchExpressions"],
        json!([{"scopeName": "PriorityClass", "operator": "In", "values": ["coder-run"]}])
    );
}

#[test]
fn the_network_policy_selects_the_coders_run_pods_and_opens_dns_and_the_internet_but_not_the_private_ranges()
 {
    let docs = rendered(&resolved_run_pods(|_, _| {})).documents();
    let np = &find(&docs, "NetworkPolicy", "coder-run-pods")["spec"];
    assert_eq!(
        np["podSelector"],
        json!({
            "matchLabels": {"app.kubernetes.io/managed-by": "adam-coder", "app.kubernetes.io/instance": "coder"},
            "matchExpressions": [{"key": "adam.vymalo.com/run", "operator": "Exists"}],
        })
    );
    assert_eq!(np["policyTypes"], json!(["Ingress", "Egress"]));
    assert_eq!(
        np["ingress"],
        json!([]),
        "nothing connects to a run pod, and the policy says it"
    );
    let dns = &np["egress"][0];
    assert_eq!(
        dns["to"][0],
        json!({
            "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}},
            "podSelector": {"matchLabels": {"k8s-app": "kube-dns"}},
        })
    );
    assert_eq!(
        dns["ports"],
        json!([{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}])
    );
    let internet = &np["egress"][1]["to"][0]["ipBlock"];
    assert_eq!(internet["cidr"], "0.0.0.0/0");
    assert_eq!(
        internet["except"],
        json!([
            "10.0.0.0/8",
            "172.16.0.0/12",
            "192.168.0.0/16",
            "169.254.0.0/16",
            "100.64.0.0/10"
        ])
    );
    assert_eq!(np["egress"].as_array().unwrap().len(), 2);
    // The agent's own ingress policy (`coder`) does not select a run pod: they are managed by adam-coder, not by us.
    let own = &find(&docs, "NetworkPolicy", "coder")["spec"]["podSelector"]["matchLabels"];
    assert_eq!(own[MANAGED_BY_LABEL], MANAGED_BY_VALUE);
}

#[test]
fn the_policys_parameters_are_what_the_guard_checks() {
    let r = resolved_run_pods(|_, _| {});
    let docs = rendered(&r).documents();
    let params = &find(&docs, "ConfigMap", "coder-run-pods-policy")["data"];
    let rp = r.runtime.run_pods.as_ref().unwrap();
    assert_eq!(
        params["serviceAccount"],
        "system:serviceaccount:another-agentic-system:coder"
    );
    assert_eq!(params["priorityClass"], "coder-run");
    assert_eq!(params["managedBy"], "adam-coder");
    assert_eq!(params["runLabel"], "adam.vymalo.com/run");
    assert_eq!(params["claim"], "work-coder-0");
    assert_eq!(params["secretName"], "coder-secrets");
    assert_eq!(params["secretKey"], "MODEL_API_KEY");
    assert_eq!(params["images"], rp.guard.images.join("\n"));
    assert_eq!(params.as_object().unwrap().len(), 8, "{params}");
}

// ---------------------------------------------------------------- they agree with the workloads

#[test]
fn the_template_is_mounted_where_the_coder_reads_it() {
    let docs = rendered(&resolved_run_pods(|_, _| {})).documents();
    let pod = &find(&docs, "StatefulSet", "coder")["spec"]["template"]["spec"];
    let volume = pod["volumes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "run-pod")
        .unwrap();
    assert_eq!(volume["configMap"]["name"], "coder-run-pod");
    assert_eq!(volume["configMap"]["defaultMode"], 0o444);
    let agent = &pod["containers"][0];
    let mount = agent["volumeMounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "run-pod")
        .unwrap();
    assert_eq!(mount["mountPath"], "/etc/adam/run-pod");
    assert_eq!(mount["readOnly"], true);
    let file = agent["env"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "RUN_POD_TEMPLATE_FILE")
        .unwrap();
    // The file the variable names is an item of the ConfigMap in the directory the volume is mounted at.
    let items = volume["configMap"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(
        file["value"],
        format!("/etc/adam/run-pod/{}", items[0]["path"].as_str().unwrap())
    );
    let template = find(&docs, "ConfigMap", "coder-run-pod");
    assert!(
        template["data"]
            .get(items[0]["key"].as_str().unwrap())
            .is_some()
    );
}

/// The labels the pods of a workload carry, which the template's pod affinity selects.
#[test]
fn the_run_pod_goes_beside_the_coder_it_selects() {
    let r = resolved_run_pods(|_, _| {});
    let docs = rendered(&r).documents();
    let pod_labels = &find(&docs, "StatefulSet", "coder")["spec"]["template"]["metadata"]["labels"];
    let template: Value = serde_json::from_str(
        find(&docs, "ConfigMap", "coder-run-pod")["data"]["pod.yaml"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let wanted = &template["spec"]["affinity"]["podAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]
        [0]["labelSelector"]["matchLabels"];
    assert!(!wanted.as_object().unwrap().is_empty());
    for (k, v) in wanted.as_object().unwrap() {
        assert_eq!(&pod_labels[k], v, "the coder's pod has no label {k}={v}");
    }
    // …and no other workload's pod would match it: the front of a split shares the instance, not the name.
    let name = wanted["app.kubernetes.io/name"].as_str().unwrap();
    assert_eq!(name, "coder");
    // The run pods themselves are not selected by anything of the service: their name label is not a workload's.
    let run_name = template["metadata"]["labels"]["app.kubernetes.io/name"]
        .as_str()
        .unwrap();
    assert_eq!(run_name, "coder-run");
    assert!(
        docs.iter()
            .filter(|d| d["kind"] == "StatefulSet" || d["kind"] == "Deployment")
            .all(|d| d["metadata"]["name"] != run_name)
    );
}

#[test]
fn the_claim_the_pod_and_the_guard_name_is_a_claim_the_service_has() {
    for (per_replica, change) in [
        (
            true,
            (|_: &mut Value, _: &mut Value| {}) as fn(&mut Value, &mut Value),
        ),
        (false, |s, c| {
            set(s, "/spec/scaling/workers", json!(2));
            set(
                c,
                "/spec/harness/adam/coder/workspacePlacement",
                json!("shared"),
            );
            set(
                c,
                "/spec/environment/volumes/0/source/persistent/perReplica",
                json!(false),
            );
        }),
    ] {
        let r = resolved_run_pods(change);
        let docs = rendered(&r).documents();
        let guard_claim = r.runtime.run_pods.as_ref().unwrap().guard.claim.clone();
        let template: Value = serde_json::from_str(
            find(&docs, "ConfigMap", "coder-run-pod")["data"]["pod.yaml"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            template["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"], guard_claim,
            "the template and the guard name one claim"
        );
        if per_replica {
            // The StatefulSet makes `<template name>-<pod name>`: the claim of the first worker.
            let sts = find(&docs, "StatefulSet", "coder");
            let t = sts["spec"]["volumeClaimTemplates"][0]["metadata"]["name"]
                .as_str()
                .unwrap();
            assert_eq!(guard_claim, format!("{t}-coder-0"));
        } else {
            assert!(
                docs.iter().any(|d| d["kind"] == "PersistentVolumeClaim"
                    && d["metadata"]["name"] == guard_claim.as_str()),
                "no claim {guard_claim} in {:?}",
                kinds(&docs)
            );
            assert!(template["spec"].get("affinity").is_none());
        }
    }
}

#[test]
fn a_spec_the_provider_cannot_render_is_refused_by_name() {
    let r = resolved_run_pods(|_, _| {});
    let mut spec = r.runtime.clone();
    spec.run_pods.as_mut().unwrap().service_account = String::new();
    let e = render(&RuntimeId::new("ns", "coder"), &spec);
    // `RuntimeSpec::check` refuses it before `render` would; render makes no object of a name it was not given.
    assert!(spec.check().is_err(), "{e:?}");
}
