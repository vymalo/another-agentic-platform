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

/// The indexes in `calls` of every request of this method on this object.
fn at(calls: &[Call], method: &str, kind: K, name: &str) -> Vec<usize> {
    calls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.method == method && c.kind() == kind && c.name() == name)
        .map(|(i, _)| i)
        .collect()
}

/// The merge patches (not the applies) among the requests: the only way to take a name out of a pod template.
fn merge_patches(calls: &[Call]) -> Vec<&Call> {
    calls
        .iter()
        .filter(|c| c.method == "PATCH" && c.content_type.contains("merge-patch"))
        .collect()
}

/// What a pod template of a workload names: `(serviceAccountName, serviceAccount)`.
fn account_of(workload: &Value) -> (Option<String>, Option<String>) {
    let pod = &workload["spec"]["template"]["spec"];
    let text = |k: &str| pod.get(k).and_then(Value::as_str).map(str::to_owned);
    (text("serviceAccountName"), text("serviceAccount"))
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
            // The egress proxy, then the policy that makes it the one way out, then the right to make a pod: a run pod
            // is never made into a network that has a policy and no way out, or no policy at all.
            (K::ConfigMap, "coder-egress-allow".to_owned()),
            (K::ServiceAccount, "coder-egress".to_owned()),
            (K::Deployment, "coder-egress".to_owned()),
            (K::Service, "coder-egress".to_owned()),
            (K::Budget, "coder-egress".to_owned()),
            (K::NetworkPolicy, "coder-egress".to_owned()),
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
        (K::ConfigMap, "coder-egress-allow"),
        (K::ServiceAccount, "coder-egress"),
        (K::Deployment, "coder-egress"),
        (K::Service, "coder-egress"),
        (K::Budget, "coder-egress"),
        (K::NetworkPolicy, "coder-egress"),
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
    assert_eq!(
        writes(&calls, "PATCH").len(),
        16,
        "the same sixteen objects: nothing of the proxy is taken for stale"
    );
}

/// The pod a workload's object holds on the fake API server, and the names of the containers that mount a volume.
fn pod_of(workload: &Value) -> &Value {
    &workload["spec"]["template"]["spec"]
}

fn mounting(pod: &Value, volume: &str) -> Vec<String> {
    pod["containers"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(pod["initContainers"].as_array().into_iter().flatten())
        .filter(|c| {
            c["volumeMounts"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m["name"] == volume))
        })
        .map(|c| c["name"].as_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn the_workers_pod_on_the_server_has_the_git_sandbox_and_the_token_in_the_coder_container_alone()
 {
    // D15d: what the provider applies for a coder with run pods, as the API server holds it: the sidecar first, its directories, the
    // account's credentials as a volume of the coder container alone, the pod's own token setting off.
    let (fake, runtime) = setup();
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    let sts = fake.get(K::StatefulSet, NS, "coder").unwrap();
    let pod = pod_of(&sts);
    assert_eq!(pod["automountServiceAccountToken"], false);
    assert_eq!(pod["serviceAccountName"], "coder");
    let inits: Vec<&str> = pod["initContainers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    assert_eq!(inits, ["git-sandbox", "github-mcp"]);
    assert_eq!(pod["initContainers"][0]["restartPolicy"], "Always");
    assert_eq!(
        pod["initContainers"][0]["securityContext"]["readOnlyRootFilesystem"],
        true
    );
    assert_eq!(mounting(pod, "kube-api-access"), ["agent"]);
    assert_eq!(mounting(pod, "git-sandbox"), ["agent", "git-sandbox"]);
    assert_eq!(mounting(pod, "git-sandbox-tmp"), ["git-sandbox"]);
    let volume = |name: &str| {
        pod["volumes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == name)
            .unwrap_or_else(|| panic!("no volume {name}"))
    };
    assert!(volume("kube-api-access").get("projected").is_some());
    assert_eq!(volume("git-sandbox")["emptyDir"], json!({}));
    assert_eq!(
        volume("git-sandbox-tmp")["emptyDir"],
        json!({"medium": "Memory", "sizeLimit": "64Mi"})
    );
    // The front of a split coder has none of it, and holds no token.
    let split = resolved_run_pods(|s, c| {
        common::set(s, "/spec/scaling/topology", json!("split"));
        common::set(s, "/spec/scaling/workers", json!(2));
        common::set(s, "/spec/scaling/front", json!({"replicas": 1}));
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
    let (fake, runtime) = setup();
    runtime.ensure(&split.id, &split.runtime).await.unwrap();
    let front = fake.get(K::Deployment, NS, "coder-front").unwrap();
    assert_eq!(pod_of(&front)["automountServiceAccountToken"], false);
    assert!(pod_of(&front).get("initContainers").is_none());
    assert!(mounting(pod_of(&front), "kube-api-access").is_empty());
}

#[tokio::test]
async fn a_coder_without_run_pods_holds_no_sandbox_no_scratch_and_no_token_on_the_server_and_loses_them_when_it_stops_asking()
 {
    let (fake, runtime) = setup();
    let off = resolved("coder", |_, _| {});
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    let text = |fake: &Fake| {
        let sts = fake.get(K::StatefulSet, NS, "coder").unwrap();
        serde_json::to_string(pod_of(&sts)).unwrap()
    };
    let before = text(&fake);
    for absent in [
        "git-sandbox",
        "GIT_SANDBOX",
        "kube-api-access",
        "emptyDir",
        "readOnlyRootFilesystem",
        "projected",
    ] {
        assert!(
            !before.contains(absent),
            "{absent} is in a coder without run pods"
        );
    }
    // It asks, and has them; it stops asking, and the pod is what it was.
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    assert!(text(&fake).contains("git-sandbox"));
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    assert_eq!(text(&fake), before);
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
        // The egress proxy goes with them (D15c), and after the pods too.
        (K::Deployment, "coder-egress"),
        (K::Service, "coder-egress"),
        (K::Budget, "coder-egress"),
        (K::NetworkPolicy, "coder-egress"),
        (K::ConfigMap, "coder-egress-allow"),
        (K::ServiceAccount, "coder-egress"),
    ] {
        assert!(
            position(K::Pod, "adam-run-bbb") < position(kind, name),
            "{name} goes after the pods"
        );
    }
    // The run pods' own policy, the one that denies, is the last policy to go: while it stands a run pod that was made in the
    // instant before the right went is cut off from the way out and never under no policy at all. The proxy's pods and then
    // its account go before it is gone too.
    assert!(
        position(K::Deployment, "coder-egress") < position(K::NetworkPolicy, "coder-egress"),
        "the proxy's pods go before the policy that confines them"
    );
    assert!(
        position(K::NetworkPolicy, "coder-egress") < position(K::NetworkPolicy, "coder-run-pods"),
        "the run pods' policy goes after the proxy's"
    );
    assert!(
        position(K::Deployment, "coder-egress") < position(K::ServiceAccount, "coder-egress"),
        "no pod of the proxy outlives its account"
    );
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
        (K::ServiceAccount, "coder-egress"),
        (K::Deployment, "coder-egress"),
        (K::Service, "coder-egress"),
        (K::Budget, "coder-egress"),
        (K::NetworkPolicy, "coder-egress"),
        (K::ConfigMap, "coder-egress-allow"),
    ] {
        // The template goes in the same pass although the phase is not Ready: it is the account's, and the workload
        // that mounted it is applied without it before it goes.
        assert!(fake.get(kind, NS, name).is_none(), "{name} is still there");
    }
    // The workload no longer runs as the account and holds no token. Neither of the two names it has for its account
    // is left (a real API server keeps the deprecated one through an apply that leaves the first out: see the fake).
    let sts = fake.get(K::StatefulSet, NS, "coder").unwrap();
    assert_eq!(account_of(&sts), (None, None));
    assert_eq!(
        sts["spec"]["template"]["spec"]["automountServiceAccountToken"],
        false
    );
}

#[tokio::test]
async fn the_account_is_deleted_only_after_the_workload_stopped_naming_it() {
    // Found by run-pods-e2e's first full run (run 40432460348): the account went first, the StatefulSet still named it,
    // and the pod its controller made next was refused ("serviceaccount not found"): 0/1, Degraded, for good. A real
    // API server also keeps the deprecated `serviceAccount` through an apply that leaves `serviceAccountName` out.
    let (fake, runtime) = setup();
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    assert_eq!(
        account_of(&fake.get(K::StatefulSet, NS, "coder").unwrap()),
        (Some("coder".to_owned()), Some("coder".to_owned())),
        "the workload runs as the account, by both of its names"
    );

    let off = resolved("coder", |_, _| {});
    fake.clear_calls();
    runtime.ensure(&off.id, &off.runtime).await.unwrap();

    assert_eq!(
        fake.accounts_deleted_while_named(),
        [],
        "the account was deleted while a workload's pod template still named it"
    );
    let calls = fake.calls();
    let sts = fake.get(K::StatefulSet, NS, "coder").unwrap();
    assert_eq!(account_of(&sts), (None, None));

    // The order: the right and the pods first; then the workload, applied without the account and then freed of the
    // name the server keeps; only then the account and what only the account used.
    let binding = at(&calls, "DELETE", K::RoleBinding, "coder-run-pods");
    let applied = at(&calls, "PATCH", K::StatefulSet, "coder");
    let account = at(&calls, "DELETE", K::ServiceAccount, "coder");
    assert_eq!((binding.len(), account.len()), (1, 1), "{calls:#?}");
    let freed: Vec<usize> = calls
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            c.method == "PATCH" && c.content_type.contains("merge-patch") && c.name() == "coder"
        })
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        applied.len(),
        2,
        "applied, then freed of the name: {calls:#?}"
    );
    assert_eq!(freed.len(), 1, "{calls:#?}");
    assert!(
        binding[0] < applied[0],
        "the right goes before the workload is touched"
    );
    assert!(
        applied[0] < freed[0],
        "the name is taken out of what was applied"
    );
    for (kind, name) in [
        (K::Quota, "coder-run-pods"),
        (K::ConfigMap, "coder-run-pods-policy"),
        (K::ConfigMap, "coder-run-pod"),
        (K::Deployment, "coder-egress"),
        (K::Service, "coder-egress"),
        (K::NetworkPolicy, "coder-egress"),
        (K::ConfigMap, "coder-egress-allow"),
    ] {
        let deleted = at(&calls, "DELETE", kind, name);
        assert_eq!(deleted.len(), 1, "{name}: {calls:#?}");
        assert!(
            freed[0] < deleted[0]
                || kind == K::Deployment
                || kind == K::Service
                || kind == K::NetworkPolicy
                || name.ends_with("-allow"),
            "{name} goes after the workload stopped naming the account"
        );
        assert!(deleted[0] < account[0], "the account goes last");
    }
    assert!(freed[0] < account[0]);
    // The proxy's own account goes with the agent's, last of all, and after the Deployment whose pods run as it.
    let proxy_account = at(&calls, "DELETE", K::ServiceAccount, "coder-egress");
    let proxy_pods = at(&calls, "DELETE", K::Deployment, "coder-egress");
    assert_eq!(
        (proxy_account.len(), proxy_pods.len()),
        (1, 1),
        "{calls:#?}"
    );
    assert!(
        proxy_pods[0] < proxy_account[0],
        "no pod of the proxy outlives its account"
    );
    assert_eq!(
        fake.accounts_deleted_while_named(),
        [],
        "neither account was deleted while a Deployment or a StatefulSet named it"
    );

    // Both names, null, on the one workload: a merge patch of only `serviceAccountName` changes nothing on a real server.
    let patches = merge_patches(&calls);
    assert_eq!(patches.len(), 1);
    assert_eq!(
        patches[0].body,
        Some(
            json!({"spec": {"template": {"spec": {"serviceAccountName": null, "serviceAccount": null}}}})
        )
    );
    assert!(
        patches[0].query.contains("fieldManager=aap-operator"),
        "{}",
        patches[0].query
    );
}

#[tokio::test]
async fn a_template_that_is_not_ours_is_not_deleted_with_the_run_pod_objects() {
    let (fake, runtime) = setup();
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    // Someone else's ConfigMap of the template's name (and of the parameters'): theirs, whatever it is called.
    for name in ["coder-run-pod", "coder-run-pods-policy"] {
        fake.put(
            K::ConfigMap,
            NS,
            name,
            json!({"metadata": {"labels": {"app.kubernetes.io/managed-by": "Helm", "app.kubernetes.io/instance": "coder"}}}),
        );
    }
    let off = resolved("coder", |_, _| {});
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    for name in ["coder-run-pod", "coder-run-pods-policy"] {
        let o = fake
            .get(K::ConfigMap, NS, name)
            .unwrap_or_else(|| panic!("{name} was deleted"));
        assert_eq!(
            o["metadata"]["labels"]["app.kubernetes.io/managed-by"],
            "Helm"
        );
    }
    // The run pod objects that are ours still went.
    assert!(fake.get(K::RoleBinding, NS, "coder-run-pods").is_none());
    assert!(fake.get(K::ServiceAccount, NS, "coder").is_none());
}

#[tokio::test]
async fn a_workload_left_naming_a_deleted_account_is_freed_by_the_next_pass_and_only_once() {
    // What the operator before this fix left behind: no account, no binding, no quota, and a StatefulSet at 0 pods
    // whose template names the account. The next pass, finding nothing of run pods to clear, still frees it.
    let (fake, runtime) = setup();
    let on = resolved_run_pods(|_, _| {});
    runtime.ensure(&on.id, &on.runtime).await.unwrap();
    {
        use k8s_openapi::api::core::v1::{ConfigMap, ResourceQuota, ServiceAccount};
        use k8s_openapi::api::networking::v1::NetworkPolicy;
        use k8s_openapi::api::rbac::v1::RoleBinding;
        let client = fake.client();
        let d = kube::api::DeleteParams::default();
        kube::Api::<RoleBinding>::namespaced(client.clone(), NS)
            .delete("coder-run-pods", &d)
            .await
            .unwrap();
        kube::Api::<ResourceQuota>::namespaced(client.clone(), NS)
            .delete("coder-run-pods", &d)
            .await
            .unwrap();
        kube::Api::<NetworkPolicy>::namespaced(client.clone(), NS)
            .delete("coder-run-pods", &d)
            .await
            .unwrap();
        kube::Api::<ConfigMap>::namespaced(client.clone(), NS)
            .delete("coder-run-pods-policy", &d)
            .await
            .unwrap();
        kube::Api::<ServiceAccount>::namespaced(client.clone(), NS)
            .delete("coder", &d)
            .await
            .unwrap();
        // The proxy of the same operator is gone too: what is left is what the operator before this fix left.
        kube::Api::<ServiceAccount>::namespaced(client.clone(), NS)
            .delete("coder-egress", &d)
            .await
            .unwrap();
        kube::Api::<k8s_openapi::api::apps::v1::Deployment>::namespaced(client.clone(), NS)
            .delete("coder-egress", &d)
            .await
            .unwrap();
        kube::Api::<k8s_openapi::api::core::v1::Service>::namespaced(client.clone(), NS)
            .delete("coder-egress", &d)
            .await
            .unwrap();
        kube::Api::<k8s_openapi::api::policy::v1::PodDisruptionBudget>::namespaced(
            client.clone(),
            NS,
        )
        .delete("coder-egress", &d)
        .await
        .unwrap();
        kube::Api::<NetworkPolicy>::namespaced(client.clone(), NS)
            .delete("coder-egress", &d)
            .await
            .unwrap();
        kube::Api::<ConfigMap>::namespaced(client, NS)
            .delete("coder-egress-allow", &d)
            .await
            .unwrap();
    }
    assert_eq!(
        account_of(&fake.get(K::StatefulSet, NS, "coder").unwrap()),
        (Some("coder".to_owned()), Some("coder".to_owned()))
    );

    let off = resolved("coder", |_, _| {});
    fake.clear_calls();
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    assert_eq!(
        account_of(&fake.get(K::StatefulSet, NS, "coder").unwrap()),
        (None, None)
    );
    assert_eq!(merge_patches(&fake.calls()).len(), 1);

    // And then it is as it should be: no patch, nothing deleted.
    fake.clear_calls();
    runtime.ensure(&off.id, &off.runtime).await.unwrap();
    assert!(
        merge_patches(&fake.calls()).is_empty(),
        "{:#?}",
        fake.calls()
    );
    assert!(fake.calls().iter().all(|c| c.method != "DELETE"));
}

#[tokio::test]
async fn a_deployment_that_names_an_account_the_spec_does_not_want_is_freed_too() {
    let (fake, runtime) = setup();
    let chat = resolved("chat", |_, _| {});
    runtime.ensure(&chat.id, &chat.runtime).await.unwrap();
    // Not something the operator renders today: a Deployment that someone's earlier spec had run as an account.
    let mut d = fake.get(K::Deployment, NS, "chat").unwrap();
    d["spec"]["template"]["spec"]["serviceAccountName"] = json!("chat");
    d["spec"]["template"]["spec"]["serviceAccount"] = json!("chat");
    fake.put(K::Deployment, NS, "chat", d);
    fake.clear_calls();
    runtime.ensure(&chat.id, &chat.runtime).await.unwrap();
    assert_eq!(
        account_of(&fake.get(K::Deployment, NS, "chat").unwrap()),
        (None, None)
    );
    let calls = fake.calls();
    let patches = merge_patches(&calls);
    assert_eq!(patches.len(), 1);
    assert!(
        patches[0].path.contains("/deployments/chat"),
        "{}",
        patches[0].path
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
    // Every render without run pods stays as it was: the workload is applied, and nothing is patched after it.
    assert!(merge_patches(&calls).is_empty(), "{calls:#?}");
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
        (K::ServiceAccount, "coder-egress"),
        (K::Deployment, "coder-egress"),
        (K::Service, "coder-egress"),
        (K::Budget, "coder-egress"),
        (K::NetworkPolicy, "coder-egress"),
        (K::ConfigMap, "coder-egress-allow"),
    ] {
        assert!(fake.get(kind, NS, name).is_none(), "{name} is still there");
    }
    // The proxy's pods are gone before the account they run as (the agent's own StatefulSet is deleted right after its account
    // on this path, which is the service going and no pod to make again).
    assert!(
        fake.accounts_deleted_while_named()
            .iter()
            .all(|(account, _)| account != "coder-egress"),
        "{:?}",
        fake.accounts_deleted_while_named()
    );
    // The proxy goes before the run pods' policy here too, and its pods before its account.
    let deleted = writes(&fake.calls(), "DELETE");
    let at = |kind: K, name: &str| {
        deleted
            .iter()
            .position(|(k, n)| *k == kind && n == name)
            .unwrap()
    };
    assert!(at(K::Pod, "adam-run-aaa") < at(K::Deployment, "coder-egress"));
    assert!(at(K::Deployment, "coder-egress") < at(K::NetworkPolicy, "coder-egress"));
    assert!(at(K::NetworkPolicy, "coder-egress") < at(K::NetworkPolicy, "coder-run-pods"));
    assert!(at(K::Deployment, "coder-egress") < at(K::ServiceAccount, "coder-egress"));
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
        // The egress proxy's names (D15c): a Service, a Deployment, an account, a policy, a budget, a file.
        (K::Service, "coder-egress"),
        (K::Deployment, "coder-egress"),
        (K::ServiceAccount, "coder-egress"),
        (K::NetworkPolicy, "coder-egress"),
        (K::Budget, "coder-egress"),
        (K::ConfigMap, "coder-egress-allow"),
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

    // Nor is the egress proxy a workload of the service (§60a, D15c, decision 4): its Deployment is not rolled out, and the
    // service's phase, its replicas and its issues are what they were.
    let mut proxy = fake.get(K::Deployment, NS, "coder-egress").unwrap();
    proxy["status"] =
        json!({"replicas": 2, "readyReplicas": 0, "updatedReplicas": 2, "observedGeneration": 1});
    fake.put(K::Deployment, NS, "coder-egress", proxy);
    assert_eq!(before, runtime.status(&on.id).await.unwrap());
}

#[tokio::test]
async fn fewer_replicas_of_the_proxy_take_its_budget_away_and_nothing_else() {
    let (fake, runtime) = setup();
    let two = resolved_run_pods(|_, _| {});
    runtime.ensure(&two.id, &two.runtime).await.unwrap();
    assert!(fake.get(K::Budget, NS, "coder-egress").is_some());
    let one = common::resolved_run_pods_with(
        |settings| settings["egressProxy"]["replicas"] = json!(1),
        |_, _| {},
    );
    fake.clear_calls();
    runtime.ensure(&one.id, &one.runtime).await.unwrap();
    assert_eq!(
        writes(&fake.calls(), "DELETE"),
        [(K::Budget, "coder-egress".to_owned())],
        "a budget of 1 of 1 would block every drain; nothing else of the proxy is stale"
    );
    assert!(fake.get(K::Deployment, NS, "coder-egress").is_some());
    assert!(fake.get(K::NetworkPolicy, NS, "coder-egress").is_some());
    assert_eq!(
        fake.get(K::Deployment, NS, "coder-egress").unwrap()["spec"]["replicas"],
        1
    );
}

#[tokio::test]
async fn a_changed_allow_list_rolls_the_proxy_and_strands_nothing() {
    let (fake, runtime) = setup();
    let base = resolved_run_pods(|_, _| {});
    runtime.ensure(&base.id, &base.runtime).await.unwrap();
    let rolled = |fake: &Fake| {
        fake.get(K::Deployment, NS, "coder-egress").unwrap()["spec"]["template"]["metadata"]
            ["annotations"]["checksum/egress-allow"]
            .clone()
    };
    let before = rolled(&fake);
    let more = common::resolved_run_pods_with(
        |settings| settings["egressProxy"]["extraAllow"] = json!(["artifacts.example.com"]),
        |_, _| {},
    );
    runtime.ensure(&more.id, &more.runtime).await.unwrap();
    assert_ne!(
        before,
        rolled(&fake),
        "the pods are rolled by the file they read at start"
    );
    let file = fake.get(K::ConfigMap, NS, "coder-egress-allow").unwrap();
    assert!(
        file["data"]["allow.txt"]
            .as_str()
            .unwrap()
            .contains("artifacts.example.com")
    );
    // Nothing is deleted: the file is edited in place, and the sweep of superseded file sets never takes the proxy's.
    fake.clear_calls();
    runtime.ensure(&more.id, &more.runtime).await.unwrap();
    assert!(fake.get(K::ConfigMap, NS, "coder-egress-allow").is_some());
    assert!(
        fake.calls().iter().all(|c| c.method != "DELETE"),
        "{:#?}",
        fake.calls()
    );
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
    assert!(fake.get(K::ServiceAccount, NS, "coder").is_some());
    // What is not the choice: the workload runs as the account no more, by neither of its names.
    assert_eq!(
        account_of(&fake.get(K::StatefulSet, NS, "coder").unwrap()),
        (None, None)
    );
}
