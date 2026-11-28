//! The objects of the examples: golden YAML (what the cluster is asked to make), and the rules those
//! goldens could hide behind a bulk update stated one by one.
//!
//! Regenerate the goldens with `AAP_UPDATE_GOLDENS=1 cargo test -p aap-runtime-kubernetes --test render`
//! and read the diff: it is the change of what the operator makes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use aap_ports::{
    DeletionPolicy, OwnerHandle, RuntimeId, RuntimeSpec, Selector, Sharing, VolumeSource,
};
use aap_runtime_kubernetes::names::{
    DELETION_POLICY_ANNOTATION, DIGEST_ANNOTATION, MANAGED_BY_LABEL, MANAGED_BY_VALUE,
};
use aap_runtime_kubernetes::{Rendered, WorkloadObject, names, render};
use common::{example, resolved, resolved_activity, set};
use serde_json::{Value, json};

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

fn coder() -> (RuntimeId, RuntimeSpec) {
    let r = resolved("coder", |_, _| {});
    (r.id, r.runtime)
}

fn split() -> (RuntimeId, RuntimeSpec) {
    let r = resolved("coder", |s, c| {
        set(s, "/spec/scaling/topology", json!("split"));
        set(s, "/spec/scaling/workers", json!(2));
        set(s, "/spec/scaling/front", json!({"replicas": 2}));
        set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!("isolated"),
        );
    });
    (r.id, r.runtime)
}

fn chat() -> (RuntimeId, RuntimeSpec) {
    let r = resolved("chat", |_, _| {});
    (r.id, r.runtime)
}

fn a2a() -> (RuntimeId, RuntimeSpec) {
    let r = resolved("a2a", |_, _| {});
    (r.id, r.runtime)
}

fn rendered((id, spec): &(RuntimeId, RuntimeSpec)) -> Rendered {
    render(id, spec).unwrap()
}

// ---------------------------------------------------------------- goldens

#[test]
fn the_coder_combined() {
    assert_golden("coder", &rendered(&coder()));
}

#[test]
fn the_coder_split() {
    assert_golden("coder-split", &rendered(&split()));
}

/// A coder that sleeps, with the deployment's activity listener (AD-058, ANR-1030a).
fn coder_activity() -> (RuntimeId, RuntimeSpec) {
    let r = resolved_activity("coder", 8081, |s, _| {
        set(s, "/spec/scaling/idleTimeout", json!("15m"));
    });
    (r.id, r.runtime)
}

#[test]
fn the_coder_with_an_activity_listener() {
    assert_golden("coder-activity", &rendered(&coder_activity()));
}

#[test]
fn the_chat_folder_agent() {
    assert_golden("chat", &rendered(&chat()));
}

#[test]
fn the_a2a_image() {
    assert_golden("a2a", &rendered(&a2a()));
}

#[test]
fn an_a2a_image_runs_its_own_entrypoint_with_only_its_variables() {
    let docs = rendered(&a2a()).documents();
    let deployment = find(&docs, "Deployment", "a2a");
    let container = &deployment["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(container["image"], "aap-stub:ci");
    assert!(
        container.get("command").is_none() && container.get("args").is_none(),
        "the image's entrypoint: {container}"
    );
    assert_eq!(container["ports"][0]["containerPort"], 8080);
    assert_eq!(container["ports"][0]["name"], "http");
    let env: Vec<(&str, &Value)> = container["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["name"].as_str().unwrap(), e))
        .collect();
    // The two the config lists, then the service's own A2A token: a listed agent accepts the
    // orchestrator's token, and the operator passes it to either harness.
    assert_eq!(env.len(), 3, "{env:?}");
    assert_eq!(env[0].0, "LOG_LEVEL");
    assert_eq!(env[0].1["value"], "info");
    assert_eq!(env[1].0, "API_TOKEN");
    assert_eq!(
        env[1].1["valueFrom"]["secretKeyRef"],
        json!({"key": "API_TOKEN", "name": "a2a-secrets"})
    );
    assert_eq!(env[2].0, "A2A_BEARER_TOKENS");
    assert_eq!(
        env[2].1["valueFrom"]["secretKeyRef"],
        json!({"key": "A2A_BEARER_TOKENS", "name": "a2a-secrets"})
    );
    assert!(
        container.get("volumeMounts").is_none(),
        "no volume: {container}"
    );
    assert_eq!(
        deployment["spec"]["template"]["spec"]["containers"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "no sidecar"
    );
    assert_eq!(container["startupProbe"]["httpGet"]["path"], "/healthz");
    assert_eq!(container["readinessProbe"]["httpGet"]["path"], "/healthz");
    assert!(container.get("livenessProbe").is_none(), "{container}");
    assert!(
        deployment["spec"]["template"]["spec"]
            .get("volumes")
            .is_none(),
        "no volume"
    );
    assert!(
        deployment["spec"]["template"]["spec"]
            .get("serviceAccountName")
            .is_none(),
        "the namespace's default account"
    );
    // The Service: the harness's port, and the card path the provider answers `endpoint()` with.
    let service = find(&docs, "Service", "a2a");
    assert_eq!(service["spec"]["ports"][0]["port"], 8080);
    assert_eq!(
        service["metadata"]["annotations"]["agents.vymalo.com/card-path"],
        "/.well-known/agent-card.json"
    );
}

// ---------------------------------------------------------------- the rules, one by one

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

#[test]
fn the_coder_is_a_stateful_set_with_a_claim_per_replica() {
    let spec = coder();
    let docs = rendered(&spec).documents();
    let names = kinds(&docs);
    assert_eq!(
        names,
        [
            "\"ConfigMap\"/\"coder-mcp\"",
            "\"Service\"/\"coder\"",
            "\"StatefulSet\"/\"coder\"",
            "\"NetworkPolicy\"/\"coder\"",
        ]
    );
    let sts = find(&docs, "StatefulSet", "coder");
    let claim = &sts["spec"]["volumeClaimTemplates"][0];
    assert_eq!(claim["metadata"]["name"], "work");
    assert_eq!(claim["spec"]["accessModes"], json!(["ReadWriteOnce"]));
    assert_eq!(claim["spec"]["resources"]["requests"]["storage"], "20Gi");
    assert_eq!(claim["spec"]["storageClassName"], "longhorn");
    // A claim is data: no owner, and nothing in its metadata that moves (a template is immutable).
    assert!(claim["metadata"].get("ownerReferences").is_none());
    assert!(claim["metadata"].get("annotations").is_none());
    assert_eq!(
        sts["spec"]["persistentVolumeClaimRetentionPolicy"]["whenDeleted"],
        "Retain"
    );
    assert_eq!(
        sts["spec"]["persistentVolumeClaimRetentionPolicy"]["whenScaled"],
        "Retain"
    );
    assert_eq!(sts["spec"]["podManagementPolicy"], "OrderedReady");
    assert_eq!(sts["spec"]["updateStrategy"]["type"], "RollingUpdate");
    assert_eq!(sts["spec"]["replicas"], 1);
}

#[test]
fn the_package_cache_is_the_users_claim_mounted_as_it_is() {
    let r = resolved("coder", |_, c| {
        set(
            c,
            "/spec/environment/cache",
            json!({ "existingClaim": "package-cache" }),
        );
    });
    let docs = rendered(&(r.id, r.runtime)).documents();
    // K1: no object of ours is made for it: the claim is the person's, whatever the deletion policy says.
    assert!(
        !docs
            .iter()
            .any(|d| d["kind"] == "PersistentVolumeClaim" && d["metadata"]["name"] == "coder-cache"),
        "{:?}",
        kinds(&docs)
    );
    let sts = find(&docs, "StatefulSet", "coder");
    let pod = &sts["spec"]["template"]["spec"];
    let volume = pod["volumes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "cache")
        .unwrap();
    assert_eq!(
        volume["persistentVolumeClaim"]["claimName"],
        "package-cache"
    );
    assert!(volume["persistentVolumeClaim"].get("readOnly").is_none());
    let mount = pod["containers"][0]["volumeMounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "cache")
        .unwrap();
    assert_eq!(mount["mountPath"], "/cache");
    assert!(
        sts["spec"]["volumeClaimTemplates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["metadata"]["name"] != "cache")
    );
}

#[test]
fn every_object_carries_the_labels_the_digest_and_the_owner() {
    let (id, spec) = coder();
    let docs = rendered(&(id, spec.clone())).documents();
    for d in &docs {
        let what = format!("{}/{}", d["kind"], d["metadata"]["name"]);
        let labels = &d["metadata"]["labels"];
        assert_eq!(labels[MANAGED_BY_LABEL], MANAGED_BY_VALUE, "{what}");
        assert_eq!(labels["app.kubernetes.io/instance"], "coder", "{what}");
        assert!(labels["app.kubernetes.io/name"].is_string(), "{what}");
        assert_eq!(
            d["metadata"]["annotations"][DIGEST_ANNOTATION], spec.digest,
            "{what}"
        );
        assert_eq!(
            d["metadata"]["namespace"], "another-agentic-system",
            "{what}"
        );
        let owner = &d["metadata"]["ownerReferences"][0];
        assert_eq!(owner["kind"], "AgentService", "{what}");
        assert_eq!(owner["name"], "coder", "{what}");
        assert_eq!(
            owner["uid"], "0b1f3c5e-1111-4222-8333-444455556666",
            "{what}"
        );
        assert_eq!(owner["controller"], true, "{what}");
    }
    let sts = find(&docs, "StatefulSet", "coder");
    let pod = &sts["spec"]["template"];
    assert_eq!(
        pod["metadata"]["annotations"][DIGEST_ANNOTATION],
        spec.digest
    );
    assert_eq!(
        sts["metadata"]["annotations"][DELETION_POLICY_ANNOTATION],
        "Retain"
    );
    // The selector is the name and the service, and the Service selects the same.
    assert_eq!(
        sts["spec"]["selector"]["matchLabels"],
        json!({"app.kubernetes.io/instance": "coder", "app.kubernetes.io/name": "coder"})
    );
    let service = find(&docs, "Service", "coder");
    assert_eq!(
        service["spec"]["selector"],
        sts["spec"]["selector"]["matchLabels"]
    );
    assert_eq!(service["spec"]["ports"][0]["port"], 8080);
    assert_eq!(service["spec"]["ports"][0]["targetPort"], "http");
}

#[test]
fn an_owner_the_provider_does_not_understand_is_no_owner() {
    let (id, mut spec) = coder();
    for token in [
        "",
        "sample-owner",
        "{}",
        r#"{"apiVersion":"v1","kind":"X","name":"x","uid":""}"#,
    ] {
        spec.owner = OwnerHandle::new(token);
        for d in rendered(&(id.clone(), spec.clone())).documents() {
            assert!(
                d["metadata"].get("ownerReferences").is_none(),
                "{token}: {d}"
            );
        }
    }
}

#[test]
fn the_pod_template_is_the_charts() {
    let docs = rendered(&coder()).documents();
    let pod = &find(&docs, "StatefulSet", "coder")["spec"]["template"]["spec"];
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!(pod["terminationGracePeriodSeconds"], 120);
    let sc = &pod["securityContext"];
    assert_eq!(sc["runAsNonRoot"], true);
    assert_eq!(sc["runAsUser"], 10001);
    assert_eq!(sc["runAsGroup"], 10001);
    assert_eq!(sc["fsGroup"], 10001);
    assert_eq!(sc["fsGroupChangePolicy"], "OnRootMismatch");
    assert_eq!(sc["seccompProfile"]["type"], "RuntimeDefault");

    let agent = &pod["containers"][0];
    assert_eq!(agent["name"], "agent");
    assert_eq!(agent["securityContext"]["allowPrivilegeEscalation"], false);
    assert_eq!(
        agent["securityContext"]["capabilities"]["drop"],
        json!(["ALL"])
    );
    assert_eq!(
        agent["ports"][0],
        json!({"name": "http", "containerPort": 8080, "protocol": "TCP"})
    );
    for (probe, period) in [
        ("startupProbe", 3),
        ("livenessProbe", 15),
        ("readinessProbe", 5),
    ] {
        assert_eq!(
            agent[probe]["httpGet"],
            json!({"path": "/healthz", "port": "http"}),
            "{probe}"
        );
        assert_eq!(agent[probe]["periodSeconds"], period, "{probe}");
    }

    // The native sidecar: an init container that restarts, probed by exec on loopback.
    let sidecar = &pod["initContainers"][0];
    assert_eq!(sidecar["name"], "github-mcp");
    assert_eq!(sidecar["restartPolicy"], "Always");
    assert_eq!(
        sidecar["securityContext"]["capabilities"]["drop"],
        json!(["ALL"])
    );
    assert_eq!(sidecar["startupProbe"]["exec"]["command"][0], "bash");
    assert!(sidecar.get("ports").is_none());
    assert_eq!(pod["containers"].as_array().unwrap().len(), 1);
}

#[test]
fn a_secret_is_a_reference_and_a_pod_name_is_the_downward_api() {
    let docs = rendered(&coder()).documents();
    let pod = &find(&docs, "StatefulSet", "coder")["spec"]["template"]["spec"];
    let env = pod["containers"][0]["env"].as_array().unwrap();
    let var = |n: &str| {
        env.iter()
            .find(|e| e["name"] == n)
            .unwrap_or_else(|| panic!("{n}"))
    };
    assert_eq!(
        var("MODEL_API_KEY")["valueFrom"]["secretKeyRef"],
        json!({"name": "coder-secrets", "key": "MODEL_API_KEY"})
    );
    assert!(var("MODEL_API_KEY").get("value").is_none());
    assert_eq!(var("LISTEN_ADDR")["value"], "0.0.0.0:8080");
    assert!(var("LISTEN_ADDR").get("valueFrom").is_none());
    // The GitHub App key is a file of a Secret volume, never a variable.
    let key = pod["volumes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "github-app")
        .unwrap();
    assert_eq!(key["secret"]["secretName"], "coder-github-app");
    assert_eq!(key["secret"]["defaultMode"], 0o440);
    assert_eq!(
        key["secret"]["items"],
        json!([{"key": "private-key.pem", "path": "private-key.pem"}])
    );
}

#[test]
fn the_network_policy_admits_the_namespace_to_the_port_and_has_no_egress() {
    let docs = rendered(&coder()).documents();
    let np = find(&docs, "NetworkPolicy", "coder");
    assert_eq!(np["spec"]["policyTypes"], json!(["Ingress"]));
    assert!(np["spec"].get("egress").is_none());
    assert_eq!(
        np["spec"]["ingress"][0]["from"],
        json!([{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "another-agentic-system"}}}])
    );
    assert_eq!(
        np["spec"]["ingress"][0]["ports"],
        json!([{"protocol": "TCP", "port": 8080}])
    );
    assert_eq!(
        np["spec"]["podSelector"]["matchLabels"],
        json!({"app.kubernetes.io/instance": "coder", "app.kubernetes.io/managed-by": "aap-operator"})
    );
}

#[test]
fn no_allow_from_is_no_policy() {
    let (id, mut spec) = coder();
    spec.network.allow_from.clear();
    let r = render(&id, &spec).unwrap();
    assert!(r.network_policy.is_none());
    assert!(
        !kinds(&r.documents())
            .iter()
            .any(|k| k.contains("NetworkPolicy"))
    );
}

// ---------------------------------------------------------------- the activator switch (AD-058 P3a-2)

fn activator_selector() -> Selector {
    Selector {
        match_labels: BTreeMap::from([(
            "app.kubernetes.io/name".to_owned(),
            "aap-activator".to_owned(),
        )]),
        match_expressions: Vec::new(),
    }
}

#[test]
fn the_service_selects_the_activator_and_direct_selects_the_workload() {
    let (id, mut spec) = chat();
    spec.network.route_to = Some(activator_selector());
    spec.network.direct = true;
    let docs = render(&id, &spec).unwrap().documents();

    let service = find(&docs, "Service", "chat");
    assert_eq!(
        service["spec"]["selector"],
        json!({"app.kubernetes.io/name": "aap-activator"}),
        "while it is set, `<svc>` sends to the activator's pods"
    );
    assert_ne!(
        service["spec"]["selector"],
        json!(names::selector_labels("chat", &id)),
        "and not to the workload"
    );

    let direct = find(&docs, "Service", "chat-direct");
    assert_eq!(
        direct["spec"]["selector"],
        json!(names::selector_labels("chat", &id)),
        "the name the activator forwards to reaches the workload"
    );
    assert_eq!(direct["spec"]["ports"][0]["name"], "http");
    assert_eq!(direct["spec"]["ports"][0]["targetPort"], "http");
    assert_eq!(direct["spec"]["ports"][0]["port"], 8080);
}

#[test]
fn the_network_policy_admits_the_activator() {
    // The agent's own ingress is restricted (`allowFrom`), so a policy is made and the activator's pods must be in it.
    let r = resolved("chat", |s, _| {
        set(
            s,
            "/spec/access/allowFrom",
            json!([{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "another-agentic-system"}}}]),
        );
    });
    let (id, mut spec) = (r.id, r.runtime);
    assert!(!spec.network.allow_from.is_empty());
    // Awake, after the switch-back: `<svc>` is on the workload again, and a forward through `<svc>-direct` must pass.
    spec.network.direct = true;
    spec.network.activator = Some(activator_selector());
    spec.network.route_to = None;
    let docs = render(&id, &spec).unwrap().documents();

    let rule = &find(&docs, "NetworkPolicy", "chat")["spec"]["ingress"][0];
    assert_eq!(
        rule["ports"],
        json!([{"protocol": "TCP", "port": 8080}]),
        "the rule is the agent's port"
    );
    let peers = rule["from"].as_array().unwrap();
    let activator = peers
        .iter()
        .find(|p| p["podSelector"]["matchLabels"]["app.kubernetes.io/name"] == "aap-activator")
        .unwrap_or_else(|| panic!("the activator's pods are not admitted: {peers:?}"));
    assert!(
        activator.get("namespaceSelector").is_none(),
        "the activator is in the same namespace: {activator}"
    );

    // The condition is `activator` (one is configured), not `route_to` or `direct`: without it the peer is not written.
    spec.network.activator = None;
    let docs = render(&id, &spec).unwrap().documents();
    let peers = find(&docs, "NetworkPolicy", "chat")["spec"]["ingress"][0]["from"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        !peers
            .iter()
            .any(|p| p["podSelector"]["matchLabels"]["app.kubernetes.io/name"] == "aap-activator"),
        "no activator is configured: {peers:?}"
    );
}

#[test]
fn the_activator_does_not_close_an_open_port() {
    // No `allowFrom`, but an activity listener, so there is a policy and its A2A rule has no `from`.
    let r = resolved_activity("chat", 8081, |s, _| {
        set(s, "/spec/scaling/idleTimeout", json!("15m"));
        set(s, "/spec/access/allowFrom", json!([]));
    });
    let (id, mut spec) = (r.id, r.runtime);
    assert!(spec.network.allow_from.is_empty());
    assert!(spec.network.activity.is_some());
    spec.network.direct = true;
    spec.network.activator = Some(activator_selector());
    let docs = render(&id, &spec).unwrap().documents();
    let rule = &find(&docs, "NetworkPolicy", "chat")["spec"]["ingress"][0];
    assert!(
        rule.get("from").is_none(),
        "naming the activator alone would shut everyone else out: {rule}"
    );
}

#[test]
fn with_an_activator_the_policy_is_applied_first() {
    let (id, mut spec) = chat();
    assert!(spec.network.activity.is_none());
    spec.network.activator = Some(activator_selector());
    assert!(
        render(&id, &spec).unwrap().policy_first,
        "or a forward through `<svc>-direct` is dropped until the policy lands"
    );
    spec.network.activator = None;
    assert!(!render(&id, &spec).unwrap().policy_first);
}

#[test]
fn a_route_to_with_match_expressions_is_refused() {
    let (id, mut spec) = chat();
    spec.network.route_to = Some(Selector {
        match_labels: BTreeMap::new(),
        match_expressions: vec![aap_ports::Requirement {
            key: "app.kubernetes.io/name".to_owned(),
            operator: aap_ports::SelectorOperator::In,
            values: vec!["aap-activator".to_owned()],
        }],
    });
    let err = render(&id, &spec).unwrap_err();
    assert!(
        matches!(err, aap_runtime_kubernetes::Error::InvalidSpec(ref what) if what.contains("route_to")),
        "{err:?}"
    );
}

#[test]
fn the_digest_does_not_move_when_the_switch_flips() {
    let (_, mut spec) = chat();
    let before = aap_domain::spec_digest(&spec);
    spec.network.route_to = Some(activator_selector());
    spec.network.direct = true;
    spec.network.activator = Some(activator_selector());
    assert_eq!(
        aap_domain::spec_digest(&spec),
        before,
        "where the Service routes is applied, it does not roll the pods"
    );
}

#[test]
fn split_has_a_front_a_budget_and_a_service_that_selects_the_front() {
    let spec = split();
    let docs = rendered(&spec).documents();
    let front = find(&docs, "Deployment", "coder-front");
    assert_eq!(front["spec"]["replicas"], 2);
    assert_eq!(
        front["metadata"]["labels"]["app.kubernetes.io/component"],
        "front"
    );
    let workers = find(&docs, "StatefulSet", "coder");
    assert_eq!(workers["spec"]["replicas"], 2);
    assert_eq!(
        workers["metadata"]["labels"]["app.kubernetes.io/component"],
        "worker"
    );
    let pdb = find(&docs, "PodDisruptionBudget", "coder-front");
    assert_eq!(pdb["spec"]["minAvailable"], 1);
    assert_eq!(
        pdb["spec"]["selector"]["matchLabels"],
        json!({"app.kubernetes.io/instance": "coder", "app.kubernetes.io/name": "coder-front"})
    );
    assert_eq!(
        find(&docs, "Service", "coder")["spec"]["selector"]["app.kubernetes.io/name"],
        "coder-front"
    );
    // One policy covers both workloads' pods.
    let np = find(&docs, "NetworkPolicy", "coder");
    assert!(np["spec"]["podSelector"]["matchLabels"]["app.kubernetes.io/name"].is_null());
}

#[test]
fn a_front_of_one_has_no_budget() {
    let r = rendered(&{
        let r = resolved("coder", |s, _| {
            set(s, "/spec/scaling/topology", json!("split"));
            set(s, "/spec/scaling/front", json!({"replicas": 1}));
        });
        (r.id, r.runtime)
    });
    assert!(r.disruption_budgets.is_empty());
}

#[test]
fn a_folder_agent_is_a_deployment_with_an_immutable_file_set() {
    let spec = chat();
    let r = rendered(&spec);
    let docs = r.documents();
    assert_eq!(r.workloads.len(), 1);
    assert!(matches!(r.workloads[0].1, WorkloadObject::Deployment(_)));
    assert!(r.network_policy.is_none(), "chat has no allowFrom");
    assert!(r.disruption_budgets.is_empty());
    assert!(r.claims.is_empty());

    let cm = &r.config_maps[0];
    let name = cm.metadata.name.as_deref().unwrap();
    assert!(
        name.starts_with("chat-agent-") && name.len() == "chat-agent-".len() + 8,
        "{name}"
    );
    assert_eq!(cm.immutable, Some(true));
    assert!(cm.data.as_ref().unwrap().contains_key("instructions.md"));

    let pod = &find(&docs, "Deployment", "chat")["spec"]["template"]["spec"];
    let volume = pod["volumes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "agent-folder")
        .unwrap();
    assert_eq!(volume["configMap"]["name"], name);
    assert_eq!(volume["configMap"]["defaultMode"], 0o444);
    assert_eq!(
        volume["configMap"]["items"],
        json!([{"key": "instructions.md", "path": "instructions.md"}])
    );
    let agent = &pod["containers"][0];
    assert_eq!(agent["command"], json!(["tini", "--", "adam-agent"]));
    assert!(pod.get("initContainers").is_none(), "no sidecar");
}

#[test]
fn a_file_path_with_a_slash_is_a_key_and_is_mounted_at_its_path() {
    let (id, mut spec) = chat_with_a_nested_file();
    spec.deletion = DeletionPolicy::Delete;
    let r = render(&id, &spec).unwrap();
    let cm = &r.config_maps[0];
    assert!(
        cm.data
            .as_ref()
            .unwrap()
            .contains_key("skills_sreview_sSKILL.md")
    );
    let docs = r.documents();
    let pod = &find(&docs, "Deployment", "chat")["spec"]["template"]["spec"];
    let items = &pod["volumes"][0]["configMap"]["items"];
    assert!(
        items.as_array().unwrap().contains(
            &json!({"key": "skills_sreview_sSKILL.md", "path": "skills/review/SKILL.md"})
        ),
        "{items}"
    );
}

fn chat_with_a_nested_file() -> (RuntimeId, RuntimeSpec) {
    let r = resolved("chat", |_, c| {
        set(
            c,
            "/spec/harness/adam/agent/folder/files/skills~1review~1SKILL.md",
            json!("# Review\n"),
        );
    });
    (r.id, r.runtime)
}

#[test]
fn a_shared_volume_is_one_claim_with_no_owner_that_every_pod_mounts() {
    let r = resolved("coder", |s, c| {
        set(s, "/spec/scaling/workers", json!(2));
        set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!("affinity"),
        );
        set(
            c,
            "/spec/environment/volumes/0/source/persistent/perReplica",
            json!(false),
        );
    });
    let (id, spec) = (r.id, r.runtime);
    let rendered = render(&id, &spec).unwrap();
    assert_eq!(rendered.claims.len(), 1);
    let claim = &rendered.claims[0];
    assert_eq!(claim.metadata.name.as_deref(), Some("coder-work"));
    assert!(
        claim.metadata.owner_references.is_none(),
        "data has no owner"
    );
    let annotations = claim.metadata.annotations.as_ref().unwrap();
    assert_eq!(annotations[DELETION_POLICY_ANNOTATION], "Retain");
    assert_eq!(
        claim.spec.as_ref().unwrap().access_modes.as_deref(),
        Some(&["ReadWriteMany".to_owned()][..])
    );
    let docs = rendered.documents();
    let sts = find(&docs, "StatefulSet", "coder");
    assert!(sts["spec"].get("volumeClaimTemplates").is_none());
    let volumes = sts["spec"]["template"]["spec"]["volumes"]
        .as_array()
        .unwrap();
    let work = volumes.iter().find(|v| v["name"] == "work").unwrap();
    assert_eq!(work["persistentVolumeClaim"]["claimName"], "coder-work");
    // The name of each pod is its worker's identity, so the set keeps its identity.
    let env = sts["spec"]["template"]["spec"]["containers"][0]["env"]
        .as_array()
        .unwrap();
    assert!(
        env.iter().any(|e| e["name"] == "WORKER_ID"
            && e["valueFrom"]["fieldRef"]["fieldPath"] == "metadata.name")
    );
}

#[test]
fn suspend_is_zero_replicas_and_changes_nothing_else() {
    let (id, mut spec) = coder();
    let before = render(&id, &spec).unwrap().documents();
    spec.suspend = true;
    let after = render(&id, &spec).unwrap().documents();
    let sts = find(&after, "StatefulSet", "coder");
    assert_eq!(sts["spec"]["replicas"], 0);
    let mut expected = before;
    for d in &mut expected {
        if d["kind"] == "StatefulSet" {
            d["spec"]["replicas"] = json!(0);
        }
    }
    assert_eq!(after, expected);
}

#[test]
fn what_is_applied_does_not_move_the_pods() {
    // Scale and network are applied, not rolled: the pod template is the same.
    let (id, spec) = coder();
    let template = |spec: &RuntimeSpec| {
        let docs = render(&id, spec).unwrap().documents();
        find(&docs, "StatefulSet", "coder")["spec"]["template"].clone()
    };
    let mut bigger = spec.clone();
    bigger.workloads[0].replicas = 5;
    bigger.network.allow_from.clear();
    bigger.owner = OwnerHandle::none();
    assert_eq!(template(&spec), template(&bigger));
}

#[test]
fn no_secret_name_is_in_plain_text() {
    // Every Secret of the example gets a name with a sentinel; it may be a reference and nothing else.
    const SENTINEL: &str = "aap-sentinel-never-a-value";
    let (mut service, mut config) = example("coder");
    let swap = |v: &mut Value| {
        let text = v
            .to_string()
            .replace("coder-secrets", &format!("{SENTINEL}-secrets"));
        let text = text.replace("coder-github-app", &format!("{SENTINEL}-key"));
        *v = serde_json::from_str(&text).unwrap();
    };
    swap(&mut service);
    swap(&mut config);
    let r = {
        let s = serde_json::from_value(service).unwrap();
        let c = serde_json::from_value(config).unwrap();
        aap_domain::resolve(
            &s,
            &c,
            &aap_domain::Externals::default(),
            OwnerHandle::none(),
        )
        .unwrap()
    };
    let docs = render(&r.id, &r.runtime).unwrap().documents();
    let mut text = Vec::new();
    for mut d in docs {
        strip(&mut d);
        collect(&d, &mut text);
    }
    assert!(!text.is_empty());
    assert!(
        !text.iter().any(|t| t.contains(SENTINEL)),
        "a Secret's name reached plain text: {:?}",
        text.iter()
            .filter(|t| t.contains(SENTINEL))
            .collect::<Vec<_>>()
    );
    // And the references are there, so the strip removed something.
    let all = render(&r.id, &r.runtime).unwrap().documents();
    assert!(all.iter().any(|d| d.to_string().contains(SENTINEL)));
}

fn strip(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.remove("secretKeyRef");
            if m.contains_key("name") {
                m.remove("secret");
            }
            m.values_mut().for_each(strip);
        }
        Value::Array(a) => a.iter_mut().for_each(strip),
        _ => {}
    }
}

fn collect(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => out.push(s.clone()),
        Value::Array(a) => a.iter().for_each(|i| collect(i, out)),
        Value::Object(m) => {
            for (k, c) in m {
                out.push(k.clone());
                collect(c, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------- what is refused

fn refused(change: impl FnOnce(&mut RuntimeSpec), says: &str) {
    let (id, mut spec) = coder();
    change(&mut spec);
    let err = render(&id, &spec).expect_err(says).to_string();
    assert!(err.contains(says), "{err:?} should say {says:?}");
}

#[test]
fn a_per_replica_volume_needs_a_stable_identity() {
    refused(
        |s| s.workloads[0].stable_identity = false,
        "stable identity",
    );
}

#[test]
fn an_http_probe_needs_a_port() {
    refused(|s| s.workloads[0].container.port = None, "serve a port");
}

#[test]
fn a_mode_that_is_not_a_permission_is_refused() {
    refused(
        |s| {
            for v in &mut s.workloads[0].volumes {
                if let VolumeSource::SecretFile { mode, .. } = &mut v.source {
                    *mode = 0o1777;
                }
            }
        },
        "not a permission",
    );
}

#[test]
fn a_claim_defined_twice_differently_is_refused() {
    let r = resolved("coder", |s, c| {
        set(s, "/spec/scaling/topology", json!("split"));
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
    let (id, mut spec) = (r.id, r.runtime);
    // The front gets the worker's volume too, with another size.
    let mut front = spec.workloads[0].clone();
    front.name = "coder-front".to_owned();
    for v in &mut front.volumes {
        if let VolumeSource::Persistent(p) = &mut v.source {
            p.size = "1Gi".to_owned();
            assert_eq!(p.sharing, Sharing::Shared);
        }
    }
    spec.workloads.push(front);
    let err = render(&id, &spec).unwrap_err().to_string();
    assert!(err.contains("defined twice"), "{err}");
}

#[test]
fn an_id_without_a_namespace_is_refused() {
    let (_, spec) = coder();
    assert!(render(&RuntimeId::new("", "coder"), &spec).is_err());
}

#[test]
fn the_agents_runtime_class_is_on_every_workload_and_nowhere_when_the_spec_names_none() {
    // K14a (AD-052): the deployment's class is every workload's pod template, for both harnesses, and the goldens above
    // are the proof that nothing moves while it is not named.
    for (name, (id, spec)) in [("coder", coder()), ("chat", chat()), ("a2a", a2a())] {
        let docs = rendered(&(id.clone(), spec.clone())).documents();
        let mut seen = 0;
        for d in &docs {
            let pod = &d["spec"]["template"]["spec"];
            if pod.is_object() {
                assert!(
                    pod.get("runtimeClassName").is_none(),
                    "{name}: {} has a class no one named",
                    d["kind"]
                );
                seen += 1;
            }
        }
        assert_eq!(
            seen,
            spec.workloads.len(),
            "{name}: every workload, and only it"
        );

        let mut named = spec;
        for w in &mut named.workloads {
            w.runtime_class_name = Some("gvisor".to_owned());
        }
        let docs = rendered(&(id, named.clone())).documents();
        let mut seen = 0;
        for d in &docs {
            let pod = &d["spec"]["template"]["spec"];
            if pod.is_object() {
                assert_eq!(pod["runtimeClassName"], "gvisor", "{name}: {}", d["kind"]);
                seen += 1;
            }
        }
        assert_eq!(seen, named.workloads.len(), "{name}: every workload");
    }
}

// ------------------------------------------------------- the activity listener (AD-058, ANR-1030a)

#[test]
fn an_agent_without_the_listener_has_no_activity_port_anywhere() {
    for spec in [coder(), chat(), a2a(), split()] {
        let text = yaml(&rendered(&spec));
        assert!(!text.contains("activity"), "{text}");
    }
}

#[test]
fn the_listener_is_a_container_port_and_a_service_port_and_nothing_else_moves() {
    let docs = rendered(&coder_activity()).documents();
    let svc = find(&docs, "Service", "coder");
    assert_eq!(
        svc["spec"]["ports"],
        json!([
            {"name": "http", "port": 8080, "protocol": "TCP", "targetPort": "http"},
            {"name": "activity", "port": 8081, "protocol": "TCP", "targetPort": "activity"},
        ])
    );
    let ss = find(&docs, "StatefulSet", "coder");
    let container = &ss["spec"]["template"]["spec"]["containers"][0];
    assert_eq!(
        container["ports"],
        json!([
            {"name": "http", "containerPort": 8080, "protocol": "TCP"},
            {"name": "activity", "containerPort": 8081, "protocol": "TCP"},
        ])
    );
    let env = container["env"].as_array().unwrap();
    let tokens = env
        .iter()
        .find(|e| e["name"] == "ACTIVITY_BEARER_TOKENS")
        .unwrap();
    assert_eq!(
        tokens["valueFrom"]["secretKeyRef"],
        json!({"name": "aap-activity", "key": "tokens"})
    );
    assert!(tokens.get("value").is_none(), "never a literal");
    let addr = env
        .iter()
        .find(|e| e["name"] == "ACTIVITY_LISTEN_ADDR")
        .unwrap();
    assert_eq!(addr["value"], "0.0.0.0:8081");
}

#[test]
fn the_policy_keeps_the_a2a_rule_and_adds_one_for_the_operator_alone() {
    let plain = rendered(&coder()).documents();
    let with = rendered(&coder_activity()).documents();
    let before = find(&plain, "NetworkPolicy", "coder");
    let after = find(&with, "NetworkPolicy", "coder");
    let (b, a) = (
        before["spec"]["ingress"].as_array().unwrap(),
        after["spec"]["ingress"].as_array().unwrap(),
    );
    assert_eq!(b.len(), 1);
    assert_eq!(a.len(), 2, "exactly one rule more");
    assert_eq!(a[0], b[0], "rule 1 is byte for byte today's");
    // One `from` entry, with both selectors: two entries would be an OR.
    assert_eq!(
        a[1],
        json!({
            "from": [{
                "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "aap-system"}},
                "podSelector": {"matchLabels": {"app.kubernetes.io/name": "aap-operator"}},
            }],
            "ports": [{"protocol": "TCP", "port": 8081}],
        })
    );
    // Everything else of the policy is as it was, the digest the pods carry aside (the listener is in it).
    let (mut b2, mut a2) = (before.clone(), after.clone());
    for d in [&mut b2, &mut a2] {
        d["spec"]["ingress"] = Value::Null;
        d["metadata"]["annotations"] = Value::Null;
    }
    assert_eq!(a2, b2);
}

#[test]
fn with_no_allow_from_the_listener_still_gets_a_policy_and_the_a2a_rule_stays_open() {
    let (id, mut spec) = coder_activity();
    spec.network.allow_from.clear();
    let docs = render(&id, &spec).unwrap().documents();
    let np = find(&docs, "NetworkPolicy", "coder");
    let ingress = np["spec"]["ingress"].as_array().unwrap();
    assert_eq!(ingress.len(), 2);
    assert_eq!(
        ingress[0],
        json!({"ports": [{"protocol": "TCP", "port": 8080}]}),
        "no `from`: open, as with no policy"
    );
    assert_eq!(
        ingress[1]["ports"],
        json!([{"protocol": "TCP", "port": 8081}])
    );
    assert_eq!(ingress[1]["from"].as_array().unwrap().len(), 1);
    assert_eq!(np["spec"]["policyTypes"], json!(["Ingress"]));
}
