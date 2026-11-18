//! `RuntimeProvider::watch`: the ids of runtimes whose objects changed, from watches on the kinds
//! that tell the controller something. A signal is never the truth: it makes the controller look
//! sooner, and the controller still reads `status` and reconciles on a timer.

use aap_ports::RuntimeId;
use futures::stream::{BoxStream, StreamExt, select_all};
use k8s_openapi::api::apps::v1::{Deployment, StatefulSet};
use k8s_openapi::api::core::v1::Pod;
use kube::runtime::WatchStreamExt;
use kube::runtime::watcher::{self, Event};
use kube::{Api, Client, Resource};
use serde::de::DeserializeOwned;
use std::fmt::Debug;

use crate::names::{INSTANCE_LABEL, MANAGED_BY_LABEL, MANAGED_BY_VALUE};

/// The runtime an object belongs to: its namespace and the `instance` label.
fn runtime_of<K: Resource>(object: &K) -> Option<RuntimeId> {
    let meta = object.meta();
    let namespace = meta.namespace.as_deref()?;
    let instance = meta.labels.as_ref()?.get(INSTANCE_LABEL)?;
    Some(RuntimeId::new(namespace, instance))
}

/// The ids of one kind's changes. The initial listing is reported too: a stream that starts after a
/// change, as a restarted controller's does, still hears about what exists.
fn of_kind<K>(api: Api<K>) -> BoxStream<'static, RuntimeId>
where
    K: Resource + Clone + DeserializeOwned + Debug + Send + 'static,
    K::DynamicType: Default,
{
    let config =
        watcher::Config::default().labels(&format!("{MANAGED_BY_LABEL}={MANAGED_BY_VALUE}"));
    watcher::watcher(api, config)
        .default_backoff()
        .filter_map(|event| async move {
            match event {
                Ok(Event::Apply(o) | Event::Delete(o) | Event::InitApply(o)) => runtime_of(&o),
                // A failed watch is retried with a back-off by the stream itself.
                Ok(Event::Init | Event::InitDone) | Err(_) => None,
            }
        })
        .boxed()
}

fn api<K>(client: &Client, namespace: Option<&str>) -> Api<K>
where
    K: Resource<Scope = k8s_openapi::NamespaceResourceScope> + Clone + DeserializeOwned + Debug,
    K::DynamicType: Default,
{
    match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    }
}

/// The three kinds of one namespace.
fn of_namespace(client: &Client, namespace: Option<&str>) -> [BoxStream<'static, RuntimeId>; 3] {
    [
        of_kind(api::<StatefulSet>(client, namespace)),
        of_kind(api::<Deployment>(client, namespace)),
        of_kind(api::<Pod>(client, namespace)),
    ]
}

/// Changes of the workloads and of their pods (a crash loop changes a pod and not its set), in every
/// namespace (`namespaces` empty) or in exactly those named (K6b: one namespaced watch per kind per
/// namespace, and never a cluster-wide one). A change of a namespace that is not watched is not
/// reported.
pub fn changes(client: Client, namespaces: Vec<String>) -> BoxStream<'static, RuntimeId> {
    let streams: Vec<BoxStream<'static, RuntimeId>> = if namespaces.is_empty() {
        of_namespace(&client, None).into()
    } else {
        namespaces
            .iter()
            .flat_map(|ns| of_namespace(&client, Some(ns.as_str())))
            .collect()
    };
    select_all(streams).boxed()
}
