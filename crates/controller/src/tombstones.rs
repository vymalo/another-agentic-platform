//! The meter's tombstones in a Kubernetes `ConfigMap` (ANR-1198, AD-069 amended): one `ConfigMap`,
//! [`CONFIGMAP`], **per watched namespace**, in the namespace of the pods it is about, with one key per vanished pod
//! (its uid) holding the pod's facts as JSON.
//!
//! * **No new verb.** The operator's `Role` already holds `get`, `list`, `create`, `patch` and `delete` on
//!   `configmaps` in every watched namespace (the provider's own); this adapter uses `get`, `list` (when a scope is
//!   every namespace), `create` and `patch`, and never `update`, `delete` or anything on a Secret.
//! * **One write per namespace per pass at most** (the meter batches), and none for a pod that is still listed.
//! * **Optimistic concurrency**: a write carries the `resourceVersion` read last, as a merge patch does, and a
//!   mismatch is [`TombstoneError::Conflict`]: an overlapping writer cannot lose what the other wrote.
//! * **Bounded**: the values add up to at most [`MAX_BYTES`] (the limit of a `ConfigMap` is 1 MiB, etcd's request
//!   about 1.5 MiB), else [`TombstoneError::TooLarge`] and nothing is written. The meter also holds at most
//!   `aap_usage::MAX_TOMBSTONES` per namespace, so the byte bound is a second fence.
//! * An account id is in the `ConfigMap` (an opaque id, never an e-mail: the same text a line carries); no token, no
//!   secret and no pod content is.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

use aap_usage::{PodKind, Tombstone, TombstoneError, TombstoneStore};
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{ListParams, Patch, PatchParams, PostParams};
use kube::{Api, Client};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The name of the `ConfigMap`, in each watched namespace.
pub const CONFIGMAP: &str = "aap-meter-tombstones";

/// The label that finds the `ConfigMap`s of every namespace in one list.
pub const COMPONENT: &str = "meter-tombstones";

/// The most bytes of values one `ConfigMap` of tombstones may hold.
pub const MAX_BYTES: usize = 900_000;

const COMPONENT_LABEL: &str = "app.kubernetes.io/component";
const NAME_LABEL: &str = "app.kubernetes.io/name";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    uid: String,
    service: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run: Option<String>,
    pod_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subject: Option<String>,
    weight_quarters: u32,
    node_type: String,
    started_at: i64,
    ended_at: i64,
    first_minute: u64,
    written_at: i64,
}

impl Record {
    fn of(t: &Tombstone) -> Self {
        Self {
            uid: t.uid.clone(),
            service: t.service.clone(),
            run: t.run.clone(),
            pod_kind: t.pod_kind.as_str().to_owned(),
            subject: t.subject.clone(),
            weight_quarters: t.weight_quarters,
            node_type: t.node_type.clone(),
            started_at: t.started_at,
            ended_at: t.ended_at,
            first_minute: t.first_minute,
            written_at: t.written_at,
        }
    }

    fn into_tombstone(self, namespace: &str) -> Option<Tombstone> {
        let pod_kind = match self.pod_kind.as_str() {
            "run" => PodKind::Run,
            "agent" => PodKind::Agent,
            _ => return None,
        };
        Some(Tombstone {
            namespace: namespace.to_owned(),
            uid: self.uid,
            service: self.service,
            run: self.run,
            pod_kind,
            subject: self.subject,
            weight_quarters: self.weight_quarters,
            node_type: self.node_type,
            started_at: self.started_at,
            ended_at: self.ended_at,
            first_minute: self.first_minute,
            written_at: self.written_at,
        })
    }
}

/// What was last read or written of a namespace's `ConfigMap`: the version to patch against, and the keys it holds
/// (a merge patch deletes a key only by naming it).
#[derive(Clone, Default)]
struct Known {
    resource_version: String,
    keys: BTreeSet<String>,
}

/// The [`TombstoneStore`] on a `ConfigMap` per namespace.
pub struct KubeTombstones {
    client: Client,
    known: Mutex<BTreeMap<String, Known>>,
}

impl KubeTombstones {
    /// A store that reads and writes through `client`.
    pub fn new(client: Client) -> Self {
        Self {
            client,
            known: Mutex::new(BTreeMap::new()),
        }
    }

    fn remember(&self, namespace: &str, known: Option<Known>) {
        let mut map = self.known.lock().unwrap_or_else(PoisonError::into_inner);
        match known {
            Some(k) => {
                map.insert(namespace.to_owned(), k);
            }
            None => {
                map.remove(namespace);
            }
        }
    }

    fn known(&self, namespace: &str) -> Option<Known> {
        self.known
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(namespace)
            .cloned()
    }

    /// Read one `ConfigMap`: its tombstones (an unreadable record is left out and will not be written back), and
    /// what to remember of it.
    fn read(cm: &ConfigMap) -> (String, Vec<Tombstone>, Known) {
        let namespace = cm.metadata.namespace.clone().unwrap_or_default();
        let mut keys = BTreeSet::new();
        let mut out = Vec::new();
        for (key, value) in cm.data.iter().flatten() {
            keys.insert(key.clone());
            if let Some(t) = serde_json::from_str::<Record>(value)
                .ok()
                .filter(|r| &r.uid == key)
                .and_then(|r| r.into_tombstone(&namespace))
            {
                out.push(t);
            }
        }
        let known = Known {
            resource_version: cm.metadata.resource_version.clone().unwrap_or_default(),
            keys,
        };
        (namespace, out, known)
    }
}

fn map_err(e: &kube::Error) -> TombstoneError {
    match e {
        // The version no longer matches, the object is gone, or it was made by someone else meanwhile.
        kube::Error::Api(status) if matches!(status.code, 404 | 409) => TombstoneError::Conflict,
        _ => TombstoneError::Unavailable,
    }
}

impl TombstoneStore for KubeTombstones {
    async fn load(&self, scope: &str) -> Result<Vec<Tombstone>, TombstoneError> {
        let mut out = Vec::new();
        if scope.is_empty() {
            let api: Api<ConfigMap> = Api::all(self.client.clone());
            let lp = ListParams::default().labels(&format!("{COMPONENT_LABEL}={COMPONENT}"));
            let list = api.list(&lp).await.map_err(|e| map_err(&e))?;
            for cm in list
                .items
                .iter()
                .filter(|cm| cm.metadata.name.as_deref() == Some(CONFIGMAP))
            {
                let (ns, mut set, known) = Self::read(cm);
                self.remember(&ns, Some(known));
                out.append(&mut set);
            }
        } else {
            let api: Api<ConfigMap> = Api::namespaced(self.client.clone(), scope);
            match api.get_opt(CONFIGMAP).await.map_err(|e| map_err(&e))? {
                Some(cm) => {
                    let (_, mut set, known) = Self::read(&cm);
                    self.remember(scope, Some(known));
                    out.append(&mut set);
                }
                None => self.remember(scope, None),
            }
        }
        Ok(out)
    }

    async fn save(&self, namespace: &str, set: &[Tombstone]) -> Result<(), TombstoneError> {
        let mut data: BTreeMap<String, String> = BTreeMap::new();
        let mut bytes = 0usize;
        for t in set {
            let value =
                serde_json::to_string(&Record::of(t)).map_err(|_| TombstoneError::TooLarge)?;
            bytes += t.uid.len() + value.len();
            data.insert(t.uid.clone(), value);
        }
        if bytes > MAX_BYTES {
            return Err(TombstoneError::TooLarge);
        }
        let api: Api<ConfigMap> = Api::namespaced(self.client.clone(), namespace);
        let keys: BTreeSet<String> = data.keys().cloned().collect();
        let written = match self.known(namespace) {
            // Never read, or absent when read: make it. Another writer's `ConfigMap` is a conflict (409).
            None => {
                let cm = ConfigMap {
                    metadata: ObjectMeta {
                        name: Some(CONFIGMAP.to_owned()),
                        namespace: Some(namespace.to_owned()),
                        labels: Some(
                            [
                                (COMPONENT_LABEL.to_owned(), COMPONENT.to_owned()),
                                (NAME_LABEL.to_owned(), "aap-meter".to_owned()),
                            ]
                            .into(),
                        ),
                        ..ObjectMeta::default()
                    },
                    data: Some(data),
                    ..ConfigMap::default()
                };
                api.create(&PostParams::default(), &cm)
                    .await
                    .map_err(|e| map_err(&e))?
            }
            Some(known) => {
                // A merge patch: the keys of the set, and `null` for each key the last read found and the set
                // leaves out. The version in the body is the optimistic lock.
                let mut changes: serde_json::Map<String, serde_json::Value> = known
                    .keys
                    .iter()
                    .filter(|k| !keys.contains(*k))
                    .map(|k| (k.clone(), serde_json::Value::Null))
                    .collect();
                for (k, v) in data {
                    changes.insert(k, serde_json::Value::String(v));
                }
                let patch = json!({
                    "metadata": { "resourceVersion": known.resource_version },
                    "data": changes,
                });
                api.patch(CONFIGMAP, &PatchParams::default(), &Patch::Merge(&patch))
                    .await
                    .map_err(|e| map_err(&e))?
            }
        };
        self.remember(
            namespace,
            Some(Known {
                resource_version: written.metadata.resource_version.unwrap_or_default(),
                keys,
            }),
        );
        Ok(())
    }
}
