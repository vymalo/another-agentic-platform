//! `RuntimeProvider::watch` against a fake API server: the namespaces it watches (K6b). What it does with the events is
//! `tests/api.rs`'s; what a real API server and a real kubelet make of it is `tests/cluster.rs`'s.

#![allow(clippy::unwrap_used, clippy::expect_used)]
// The fake is the provider's, shared with `tests/api.rs`: this test drives only its calls and its client.
#![allow(dead_code)]

mod support;

use std::time::Duration;

use aap_ports::RuntimeProvider;
use aap_runtime_kubernetes::KubernetesRuntime;
use futures::StreamExt;
use support::Fake;

/// The API group of a kind.
fn group(kind: &str) -> &'static str {
    match kind {
        "statefulsets" | "deployments" => "/apis/apps/v1",
        _ => "/api/v1",
    }
}

/// The collection a kind is listed at, in one namespace.
fn collection(kind: &str, ns: &str) -> String {
    format!("{}/namespaces/{ns}/{kind}", group(kind))
}

/// The collection a kind is listed at, in every namespace.
fn every_namespace(kind: &str) -> String {
    format!("{}/{kind}", group(kind))
}

/// Every request of the fake that lists a collection (the one before a watch), as its path: a watch request carries
/// `watch=true`, and a list does not.
fn lists(fake: &Fake) -> Vec<String> {
    fake.calls()
        .into_iter()
        .filter(|call| call.method == "GET" && !call.query.contains("watch=true"))
        .map(|call| call.path)
        .collect()
}

/// Wait until `check` holds of the fake, for at most five seconds: the watches start as tasks, and nothing says when.
async fn eventually(fake: &Fake, what: &str, mut check: impl FnMut(&Fake) -> bool) {
    for _ in 0..500 {
        if check(fake) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "timed out waiting for {what}; the lists were {:?}",
        lists(fake)
    );
}

/// K6b: an operator with a namespaced `Role` in each namespace it watches may not list or watch the whole cluster, so
/// `watch` asks the API server for each kind in each of the namespaces it was given, and never for a kind in every
/// namespace (`Api::all` would be the 403 an operator with those Roles can never recover from).
#[tokio::test]
async fn the_watch_is_one_namespaced_watch_per_kind_and_namespace_and_never_the_whole_cluster() {
    let fake = Fake::new();
    let runtime = KubernetesRuntime::new(fake.client())
        .watching_namespaces(["team".to_owned(), "other".to_owned()]);
    let watches = tokio::spawn(async move {
        let mut watch = runtime.watch();
        while watch.next().await.is_some() {}
    });
    eventually(&fake, "the six lists", |fake| {
        let lists = lists(fake);
        ["team", "other"].iter().all(|ns| {
            ["statefulsets", "deployments", "pods"]
                .iter()
                .all(|kind| lists.contains(&collection(kind, ns)))
        })
    })
    .await;

    for path in lists(&fake) {
        assert!(
            path.contains("/namespaces/team/") || path.contains("/namespaces/other/"),
            "a list of the whole cluster, which a namespaced Role cannot allow: {path}"
        );
    }
    watches.abort();
}

/// And an empty list is every namespace, the shape of an operator an administrator runs with a `ClusterRole`: one
/// list per kind, with no namespace in its path.
#[tokio::test]
async fn no_namespace_named_is_one_watch_of_every_namespace() {
    let fake = Fake::new();
    let runtime = KubernetesRuntime::new(fake.client());
    let watches = tokio::spawn(async move {
        let mut watch = runtime.watch();
        while watch.next().await.is_some() {}
    });
    eventually(&fake, "the three cluster-wide lists", |fake| {
        let lists = lists(fake);
        ["statefulsets", "deployments", "pods"]
            .iter()
            .all(|kind| lists.contains(&every_namespace(kind)))
    })
    .await;
    watches.abort();
}
