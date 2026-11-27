//! The meter's tombstones in Kubernetes `ConfigMap`s (ANR-1198, AD-069 amended): **in the operator's own namespace**
//! (its primary one, where its `Role` is), one `ConfigMap` [`name_for`]`(namespace)` per **watched** namespace, with one
//! key per vanished pod (its uid) holding the pod's facts as JSON.
//!
//! * **Why the operator's namespace, not the pod's.** Whoever can write a `ConfigMap` in a tenant's namespace could
//!   forge a record (any account, a day of minutes) or delete one, and read the account ids. In the operator's
//!   namespace only its own account writes. On read a `ConfigMap` is refused (and counted) unless it has the operator's
//!   labels, names the namespace its own name says, and has **no owner reference** (the operator sets none).
//! * **No new verb.** The operator's `Role` already holds `get`, `list`, `create`, `patch` and `delete` on
//!   `configmaps` in each watched namespace, the primary one included; this adapter uses `get`, `list` (scope "every
//!   namespace" and the sweep), `create`, `patch` and `delete`, never `update`, and nothing on a Secret.
//! * **One write per namespace per pass at most** (the meter batches), and none for a pod that is still listed.
//! * **Optimistic concurrency**: a write carries the `resourceVersion` read last (a merge patch, or the precondition
//!   of a delete), and a mismatch is [`TombstoneError::Conflict`]: an overlapping writer cannot lose what the other
//!   wrote.
//! * **Never delete what was not understood**: a record this version cannot read (a newer replica's) is left in the
//!   `ConfigMap` and counted; an empty save deletes the `ConfigMap` only when it holds nothing else.
//! * **Bounded**: the values add up to at most [`MAX_BYTES`] (the limit of a `ConfigMap` is 1 MiB), else
//!   [`TombstoneError::TooLarge`] and nothing is written; a `403` is [`TombstoneError::Forbidden`] and a `422` or `413`
//!   is `TooLarge`, so a namespace that cannot be stored falls back to memory instead of being retried for ever.
//! * An account id is in the `ConfigMap` (an opaque id, never an e-mail: the same text a line carries); no token, no
//!   secret and no pod content is.
//! * **Removal.** The `ConfigMap`s carry the label `app.kubernetes.io/component=meter-tombstones` and no owner:
//!   `kubectl -n <operator namespace> delete configmap -l app.kubernetes.io/component=meter-tombstones` removes them
//!   on uninstall. The meter deletes a set it emptied, and deletes the set of a namespace that is no longer watched
//!   once every record of it is older than 24 h.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, PoisonError};

use aap_usage::{Loaded, PodKind, Tombstone, TombstoneError, TombstoneStore};
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams, PostParams, Preconditions};
use kube::{Api, Client};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// The prefix of the name of a `ConfigMap`: `aap-meter-tombstones-<namespace>`.
pub const CONFIGMAP: &str = "aap-meter-tombstones";

/// The name of the `ConfigMap` that holds the tombstones of the pods of `namespace`.
pub fn name_for(namespace: &str) -> String {
    format!("{CONFIGMAP}-{namespace}")
}

/// The label that finds the `ConfigMap`s of every namespace in one list.
pub const COMPONENT: &str = "meter-tombstones";

/// The most bytes of values one `ConfigMap` of tombstones may hold.
pub const MAX_BYTES: usize = 900_000;

const COMPONENT_LABEL: &str = "app.kubernetes.io/component";
const NAME_LABEL: &str = "app.kubernetes.io/name";
const MANAGED_BY_LABEL: &str = "app.kubernetes.io/managed-by";
const MANAGED_BY: &str = "aap-operator";
/// The namespace whose pods the `ConfigMap` is about.
const NAMESPACE_LABEL: &str = "agents.vymalo.com/meter-namespace";

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

/// What was last read or written of a namespace's `ConfigMap`: the version to patch against, the keys of the records
/// this version understood (a merge patch deletes a key only by naming it) and the keys it did not (never touched).
#[derive(Clone, Default)]
struct Known {
    resource_version: String,
    understood: BTreeSet<String>,
    foreign: BTreeSet<String>,
}

/// The [`TombstoneStore`] on a `ConfigMap` per watched namespace, kept in the operator's own namespace.
pub struct KubeTombstones {
    client: Client,
    home: String,
    known: Mutex<BTreeMap<String, Known>>,
    /// Namespaces whose `ConfigMap` exists and is not the operator's: never written over.
    refused: Mutex<BTreeSet<String>>,
}

impl KubeTombstones {
    /// A store that reads and writes through `client`, in the namespace `home` (the operator's primary one).
    pub fn new(client: Client, home: &str) -> Self {
        Self {
            client,
            home: home.to_owned(),
            known: Mutex::new(BTreeMap::new()),
            refused: Mutex::new(BTreeSet::new()),
        }
    }

    fn api(&self) -> Api<ConfigMap> {
        Api::namespaced(self.client.clone(), &self.home)
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

    /// Whether the operator made this `ConfigMap`, and the namespace it is about: its labels, a name that says the
    /// same namespace, and no owner. Anything else is somebody else's object and is not trusted.
    fn trusted(cm: &ConfigMap) -> Option<String> {
        let labels = cm.metadata.labels.as_ref()?;
        let namespace = labels.get(NAMESPACE_LABEL)?;
        let ok = labels.get(COMPONENT_LABEL).map(String::as_str) == Some(COMPONENT)
            && labels.get(MANAGED_BY_LABEL).map(String::as_str) == Some(MANAGED_BY)
            && cm.metadata.name.as_deref() == Some(name_for(namespace).as_str())
            && cm
                .metadata
                .owner_references
                .as_ref()
                .is_none_or(Vec::is_empty);
        ok.then(|| namespace.clone())
    }

    /// Read one `ConfigMap`: its tombstones, what it set aside, and what to remember of it.
    fn read(cm: &ConfigMap, namespace: &str) -> (Vec<Tombstone>, u64, Known) {
        let mut known = Known {
            resource_version: cm.metadata.resource_version.clone().unwrap_or_default(),
            ..Known::default()
        };
        let mut out = Vec::new();
        let mut unreadable = 0u64;
        for (key, value) in cm.data.iter().flatten() {
            match serde_json::from_str::<Record>(value)
                .ok()
                .filter(|r| &r.uid == key)
                .and_then(|r| r.into_tombstone(namespace))
            {
                Some(t) => {
                    known.understood.insert(key.clone());
                    out.push(t);
                }
                None => {
                    known.foreign.insert(key.clone());
                    unreadable += 1;
                }
            }
        }
        (out, unreadable, known)
    }
}

fn map_err(e: &kube::Error) -> TombstoneError {
    match e {
        // The version no longer matches, the object is gone, or it was made by someone else meanwhile.
        kube::Error::Api(status) if matches!(status.code, 404 | 409) => TombstoneError::Conflict,
        // Too big for the API server: this namespace cannot be stored, whatever is tried again.
        kube::Error::Api(status) if matches!(status.code, 413 | 422) => TombstoneError::TooLarge,
        // No right to it: retrying cannot help.
        kube::Error::Api(status) if status.code == 403 => TombstoneError::Forbidden,
        _ => TombstoneError::Unavailable,
    }
}

impl TombstoneStore for KubeTombstones {
    async fn load(&self, scope: &str) -> Result<Loaded, TombstoneError> {
        let mut out = Loaded::default();
        let found: Vec<(String, ConfigMap)> = if scope.is_empty() {
            let lp = ListParams::default().labels(&format!("{COMPONENT_LABEL}={COMPONENT}"));
            let list = self.api().list(&lp).await.map_err(|e| map_err(&e))?;
            let mut found = Vec::new();
            for cm in list.items {
                match Self::trusted(&cm) {
                    Some(ns) => found.push((ns, cm)),
                    None => out.refused += 1,
                }
            }
            found
        } else {
            match self
                .api()
                .get_opt(&name_for(scope))
                .await
                .map_err(|e| map_err(&e))?
            {
                Some(cm) => match Self::trusted(&cm).filter(|ns| ns == scope) {
                    Some(ns) => vec![(ns, cm)],
                    None => {
                        // Not ours: the set of this namespace is empty to us, and it is never written over.
                        out.refused += 1;
                        self.refused
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(scope.to_owned());
                        self.remember(scope, None);
                        return Ok(out);
                    }
                },
                None => {
                    self.refused
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .remove(scope);
                    self.remember(scope, None);
                    return Ok(out);
                }
            }
        };
        for (ns, cm) in found {
            let (mut set, unreadable, known) = Self::read(&cm, &ns);
            self.remember(&ns, Some(known));
            out.tombstones.append(&mut set);
            out.unreadable += unreadable;
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
        if self
            .refused
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(namespace)
        {
            // Somebody else's `ConfigMap` is in the way: no write of ours goes over it.
            return Err(TombstoneError::Forbidden);
        }
        let api = self.api();
        let name = name_for(namespace);
        let keys: BTreeSet<String> = data.keys().cloned().collect();
        let known = self.known(namespace);
        let foreign = known
            .as_ref()
            .map(|k| k.foreign.clone())
            .unwrap_or_default();
        let written = match known {
            // Never read, or absent when read.
            None if data.is_empty() => return Ok(()),
            None => {
                let cm = ConfigMap {
                    metadata: ObjectMeta {
                        name: Some(name),
                        namespace: Some(self.home.clone()),
                        labels: Some(
                            [
                                (COMPONENT_LABEL.to_owned(), COMPONENT.to_owned()),
                                (NAME_LABEL.to_owned(), "aap-meter".to_owned()),
                                (MANAGED_BY_LABEL.to_owned(), MANAGED_BY.to_owned()),
                                (NAMESPACE_LABEL.to_owned(), namespace.to_owned()),
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
            // Nothing left, and nothing in it that was not understood: the `ConfigMap` goes.
            Some(known) if data.is_empty() && known.foreign.is_empty() => {
                let dp = DeleteParams {
                    preconditions: Some(Preconditions {
                        resource_version: Some(known.resource_version.clone()),
                        uid: None,
                    }),
                    ..DeleteParams::default()
                };
                api.delete(&name, &dp).await.map_err(|e| map_err(&e))?;
                self.remember(namespace, None);
                return Ok(());
            }
            Some(known) => {
                // A merge patch: the keys of the set, and `null` for each key this version understood and the set
                // leaves out. A key it did not understand is never named. The version in the body is the lock.
                let mut changes: serde_json::Map<String, serde_json::Value> = known
                    .understood
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
                api.patch(&name, &PatchParams::default(), &Patch::Merge(&patch))
                    .await
                    .map_err(|e| map_err(&e))?
            }
        };
        self.remember(
            namespace,
            Some(Known {
                resource_version: written.metadata.resource_version.unwrap_or_default(),
                understood: keys,
                foreign,
            }),
        );
        Ok(())
    }
}
