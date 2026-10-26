//! The provider's requests for run pods against a fake API server (slice D15a): what it applies and in which order,
//! what it takes away when a service stops asking for them or goes, and that it never touches an object that is
//! not its own. What a real API server says of the RBAC and the admission policy is `tests/cluster.rs`' and the
//! kind end-to-end's.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
#[allow(dead_code)] // the fake serves several test files; this one uses part of it
mod support;

use aap_ports::{IssueReason, RuntimeProvider};
use aap_runtime_kubernetes::KubernetesRuntime;
use common::{resolved, resolved_run_pods};
use serde_json::{Value, json};
use support::{Call, Fake, K};

const NS: &str = "another-agentic-system";

/// A provider that looks for run pods to clear: the operator's, when it has settings for them.
fn setup() -> (Fake, KubernetesRuntime) {
    let fake = Fake::new();
    let runtime = KubernetesRuntime::new(fake.client()).clearing_run_pods(true);
    (fake, runtime)
}

/// The kinds that are not in an operator's `Role` unless the chart has run pods on.
fn is_a_run_pods_kind(k: K) -> bool {
    matches!(k, K::ServiceAccount | K::Quota | K::RoleBinding)
}

fn writes(calls: &[Call], method: &str) -> Vec<(K, String)> {
    calls
        .iter()
        .filter(|c| c.method == method)
        .map(|c| (c.kind(), c.name().to_owned()))
        .collect()
}

/// A run pod as the coder makes it: managed by `adam-coder`, of the release, with the run label.
fn run_pod(instance: &str) -> Value {
    json!({"metadata": {"labels": {
        "app.kubernetes.io/managed-by": "adam-coder",
        "app.kubernetes.io/instance": instance,
        "adam.vymalo.com/run": "0123456789ab",
    }}})
}

#[tokio::test]
async fn the_account_and_the_guards_parameters_come_before_the_right_to_make_pods() {
    let (fake, runtime) = setup();
    let r = resolved_run_pods(|_, _| {});
    runtime.ensure(&r.id, &r.runtime).await.unwrap();
    let patches = writes(&fake.calls(), "PATCH");
    assert_eq!(
        patches,
        [
            (K::ConfigMap, "coder-mcp".to_owned()),
            (K::ConfigMap, "coder-run-pod".to_owned()),
            (K::ConfigMap, "coder-run-pods-policy".to_owned()),
            (K::ServiceAccount, "coder".to_owned()),
            (K::Quota, "coder-run-pods".to_owned()),
            (K::NetworkPolicy, "coder-run-pods".to_owned()),
            (K::RoleBinding, "coder-run-pods".to_owned()),
            (K::Service, "coder".to_owned()),
            (K::StatefulSet, "coder".to_owned()),
            (K::NetworkPolicy, "coder".to_owned()),
        ]
    );
    // Every object is there, ours, owned by the service.
    for (kind, name) in [
        (K::ServiceAccount, "coder"),
        (K::Quota, "coder-run-pods"),
        (K::RoleBinding, "coder-run-pods"),
        (K::NetworkPolicy, "coder-run-pods"),
        (K::ConfigMap, "coder-run-pods-policy"),
    ] {
        let o = fake
            .get(kind, NS, name)
            .unwrap_or_else(|| panic!("no {name}"));
        assert_eq!(
            o["metadata"]["labels"]["app.kubernetes.io/managed-by"],
            "aap-operator"
        );
        assert_eq!(o["metadata"]["ownerReferences"][0]["kind"], "AgentService");
    }
    // A second pass changes nothing it did not have to, and deletes nothing.
    fake.clear_calls();
    runtime.ensure(&r.id, &r.runtime).await.unwrap();
    let calls = fake.calls();
    assert!(calls.iter().all(|c| c.method != "DELETE"), "{calls:#?}");
    assert_eq!(writes(&calls, "PATCH").len(), 10);
}

#[tokio::test]
async fn the_policy_of_the_agent_and_the_policy_of_its_run_pods_do_not_delete_each_other() {
    // With no `allowFrom` the agent has no policy of its own: the run pods' is not "the policy we no longer want".
    let (fake, runtime) = setup();
    let r = resolved_run_pods(|s, _| {
        s["spec"]["access"]["allowFrom"] = json!([]);
    });
    runtime.ensure(&r.id, &r.runtime).await.unwrap();
    runtime.ensure(&r.id, &r.runtime).await.unwrap();
    assert!(fake.get(K::NetworkPolicy, NS, "coder-run-pods").is_some());
    assert!(fake.get(K::NetworkPolicy, NS, "coder").is_none());
    assert!(fake.calls().iter().all(|c| c.method != "DELETE"));

    // And the other way: allowFrom is withdrawn, the run pods' policy stays.
    let (fake, runtime) = setup();
    let with = resolved_run_pods(|_, _| {});
    runtime.ensure(&with.id, &with.runtime).await.unwrap();
    runtime.ensure(&r.id, &r.runtime).await.unwrap();
    assert!(
        fake.get(K::NetworkPolicy, NS, "coder").is_none(),
        "the agent's own policy goes"
    );
    assert!(fake.get(K::NetworkPolicy, NS, "coder-run-pods").is_some());
}

#[tokio::test]
async fn a_service_that_stops_asking_for_run_pods_loses_the_right_first_then_its_pods_then_the_rest()
 {
    let (fake, runtime) = setup();
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    // Two run pods of this service, one of another, and one that is not a run pod of anybody's.
    fake.put(K::Pod, NS, "adam-run-aaa", run_pod("coder"));
    fake.put(K::Pod, NS, "adam-run-bbb", run_pod("coder"));
    fake.put(K::Pod, NS, "adam-run-other", run_pod("another-coder"));
    fake.put(
        K::Pod,
        NS,
        "unrelated",
        json!({"metadata": {"labels": {"app.kubernetes.io/instance": "coder"}}}),
    );

    let off = resolved("coder", |_, _| {});
    fake.clear_calls();
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    let deleted = writes(&fake.calls(), "DELETE");
    let position = |kind: K, name: &str| {
        deleted
            .iter()
            .position(|(k, n)| *k == kind && n == name)
            .unwrap_or_else(|| panic!("{name} was not deleted: {deleted:?}"))
    };
    let binding = position(K::RoleBinding, "coder-run-pods");
    for pod in ["adam-run-aaa", "adam-run-bbb"] {
        assert!(
            binding < position(K::Pod, pod),
            "the right goes before the pods"
        );
    }
    for (kind, name) in [
        (K::Quota, "coder-run-pods"),
        (K::NetworkPolicy, "coder-run-pods"),
        (K::ConfigMap, "coder-run-pods-policy"),
        (K::ServiceAccount, "coder"),
    ] {
        assert!(
            position(K::Pod, "adam-run-bbb") < position(kind, name),
            "{name} goes after the pods"
        );
    }
    assert!(fake.get(K::Pod, NS, "adam-run-aaa").is_none());
    assert!(fake.get(K::Pod, NS, "adam-run-bbb").is_none());
    assert!(
        fake.get(K::Pod, NS, "adam-run-other").is_some(),
        "another service's pod is not ours to delete"
    );
    assert!(fake.get(K::Pod, NS, "unrelated").is_some());
    for (kind, name) in [
        (K::ServiceAccount, "coder"),
        (K::Quota, "coder-run-pods"),
        (K::RoleBinding, "coder-run-pods"),
        (K::NetworkPolicy, "coder-run-pods"),
        (K::ConfigMap, "coder-run-pods-policy"),
        (K::ConfigMap, "coder-run-pod"),
    ] {
        // The template is a file set: it goes with the others once the rollout is done (the phase is not Ready here).
        if name == "coder-run-pod" {
            continue;
        }
        assert!(fake.get(kind, NS, name).is_none(), "{name} is still there");
    }
    // The workload no longer runs as the account and holds no token.
    let sts = fake.get(K::StatefulSet, NS, "coder").unwrap();
    assert!(
        sts["spec"]["template"]["spec"]
            .get("serviceAccountName")
            .is_none()
    );
    assert_eq!(
        sts["spec"]["template"]["spec"]["automountServiceAccountToken"],
        false
    );
}

#[tokio::test]
async fn a_service_that_never_had_run_pods_pays_three_reads_and_nothing_is_deleted() {
    let (fake, runtime) = setup();
    let off = resolved("coder", |_, _| {});
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    let calls = fake.calls();
    assert!(calls.iter().all(|c| c.method != "DELETE"));
    let lists = |kind: K| {
        calls
            .iter()
            .filter(|c| {
                c.method == "GET"
                    && c.kind() == kind
                    && c.name() == kind.collection(NS).rsplit('/').next().unwrap()
            })
            .count()
    };
    for kind in [K::ServiceAccount, K::Quota, K::RoleBinding] {
        assert_eq!(lists(kind), 2, "one list of {kind:?} per pass");
    }
    // No write of a run-pod kind, ever.
    assert!(
        calls
            .iter()
            .filter(|c| c.writes())
            .all(|c| !matches!(c.kind(), K::ServiceAccount | K::Quota | K::RoleBinding))
    );
}

#[tokio::test]
async fn deleting_the_service_clears_its_run_pods_too() {
    let (fake, runtime) = setup();
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    fake.put(K::Pod, NS, "adam-run-aaa", run_pod("coder"));
    fake.put(K::Pod, NS, "adam-run-other", run_pod("another-coder"));
    fake.clear_calls();
    let outcome = runtime.delete(&on.id).await.unwrap();
    assert!(outcome.existed);
    let deleted = writes(&fake.calls(), "DELETE");
    let at = |kind: K, name: &str| {
        deleted
            .iter()
            .position(|(k, n)| *k == kind && n == name)
            .unwrap()
    };
    assert!(at(K::RoleBinding, "coder-run-pods") < at(K::Pod, "adam-run-aaa"));
    assert!(
        at(K::Pod, "adam-run-aaa") < at(K::StatefulSet, "coder"),
        "the coder goes after its pods"
    );
    assert!(fake.get(K::Pod, NS, "adam-run-aaa").is_none());
    assert!(fake.get(K::Pod, NS, "adam-run-other").is_some());
    for (kind, name) in [
        (K::ServiceAccount, "coder"),
        (K::Quota, "coder-run-pods"),
        (K::RoleBinding, "coder-run-pods"),
        (K::NetworkPolicy, "coder-run-pods"),
        (K::ConfigMap, "coder-run-pods-policy"),
        (K::ConfigMap, "coder-run-pod"),
    ] {
        assert!(fake.get(kind, NS, name).is_none(), "{name} is still there");
    }
}

#[tokio::test]
async fn without_the_guards_parameters_the_provider_cannot_find_the_pods_and_deletes_none() {
    let (fake, runtime) = setup();
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    fake.put(K::Pod, NS, "adam-run-aaa", run_pod("coder"));
    // Someone deleted the parameters: the labels to find the pods by are gone with them.
    let path = format!("{}/coder-run-pods-policy", K::ConfigMap.collection(NS));
    let _ = path;
    let params = fake.get(K::ConfigMap, NS, "coder-run-pods-policy").unwrap();
    assert!(params["data"]["managedBy"].is_string());
    fake.fail_with(None);
    // (The fake has no delete helper: take the parameters away by deleting them through the provider's own client.)
    let client = fake.client();
    let api: kube::Api<k8s_openapi::api::core::v1::ConfigMap> = kube::Api::namespaced(client, NS);
    api.delete("coder-run-pods-policy", &Default::default())
        .await
        .unwrap();
    let off = resolved("coder", |_, _| {});
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    assert!(
        fake.get(K::Pod, NS, "adam-run-aaa").is_some(),
        "no label to select by, no pod deleted"
    );
    assert!(
        fake.get(K::RoleBinding, NS, "coder-run-pods").is_none(),
        "the right is gone all the same"
    );
}

#[tokio::test]
async fn an_object_of_a_name_run_pods_need_that_is_not_ours_blocks_and_nothing_is_written() {
    for (kind, name) in [
        (K::ServiceAccount, "coder"),
        (K::Quota, "coder-run-pods"),
        (K::RoleBinding, "coder-run-pods"),
        (K::NetworkPolicy, "coder-run-pods"),
        (K::ConfigMap, "coder-run-pods-policy"),
    ] {
        let (fake, runtime) = setup();
        fake.put(kind, NS, name, json!({"metadata": {"labels": {"app.kubernetes.io/managed-by": "Helm", "app.kubernetes.io/instance": "coder"}}}));
        let r = resolved_run_pods(|_, _| {});
        let status = runtime.ensure(&r.id, &r.runtime).await.unwrap();
        assert!(
            status
                .issues
                .iter()
                .any(|i| i.reason == IssueReason::NameConflict && i.message.contains(name)),
            "{name}: {:?}",
            status.issues
        );
        assert!(
            fake.writes().is_empty(),
            "{name}: something was written: {:?}",
            fake.writes()
        );
        // The foreign object is as it was.
        let o = fake.get(kind, NS, name).unwrap();
        assert_eq!(
            o["metadata"]["labels"]["app.kubernetes.io/managed-by"],
            "Helm"
        );
    }
}

#[tokio::test]
async fn the_run_pods_of_a_service_are_not_part_of_its_status() {
    // A run pod that is failing is the coder's to report; the service's status reads the pods of its workloads.
    let (fake, runtime) = setup();
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    let before = runtime.status(&on.id).await.unwrap();
    let mut crashing = run_pod("coder");
    crashing["status"] = json!({"phase": "Failed", "containerStatuses": [{"name": "run", "restartCount": 9,
        "state": {"waiting": {"reason": "CrashLoopBackOff"}}}]});
    fake.put(K::Pod, NS, "adam-run-aaa", crashing);
    let after = runtime.status(&on.id).await.unwrap();
    assert_eq!(before, after);
}

#[tokio::test]
async fn a_provider_that_is_not_looking_for_run_pods_reads_and_writes_none_of_their_kinds() {
    // The operator of a chart with run pods off has no right on serviceaccounts, resourcequotas and rolebindings, and
    // none to delete a pod: a provider that looked for run pods anyway would fail every ensure and every delete of every
    // agent with Forbidden. The fake API server enforces no RBAC, so this is where it is held: not one request names them.
    let fake = Fake::new();
    let runtime = KubernetesRuntime::new(fake.client());
    let off = resolved("coder", |_, _| {});
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    runtime.status(&off.id).await.unwrap();
    runtime.delete(&off.id).await.unwrap();
    let calls = fake.calls();
    assert!(!calls.is_empty());
    for c in &calls {
        assert!(
            !is_a_run_pods_kind(c.kind()),
            "a request for a kind the default Role has no right on: {} {}",
            c.method,
            c.path
        );
        assert!(
            !(c.method == "DELETE" && matches!(c.kind(), K::Pod)),
            "a pod deleted by a provider that does not look: {}",
            c.path
        );
    }
}

#[tokio::test]
async fn a_provider_that_is_not_looking_applies_run_pods_when_the_spec_has_them_and_leaves_them_when_it_does_not()
 {
    // A spec that has run pods is applied whatever the provider was told (only an operator with settings can resolve one);
    // a later spec without them is not a reason to look for what the operator may not read: the objects stay, with the
    // owner reference that takes them away with the service.
    let fake = Fake::new();
    let runtime = KubernetesRuntime::new(fake.client());
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    assert!(fake.get(K::RoleBinding, NS, "coder-run-pods").is_some());
    let off = resolved("coder", |_, _| {});
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    assert!(
        fake.get(K::RoleBinding, NS, "coder-run-pods").is_some(),
        "left in place: clearing is the operator's choice, made with the rights to do it"
    );
}
