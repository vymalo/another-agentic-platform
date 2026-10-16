//! `MemoryObjects`: the dashboard's custom resources kept in memory.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::lock;
use crate::testkit::{Foreign, ObjectsUnderTest};
use crate::{
    ARGO_TRACKING_ANNOTATION, Agent, AgentDraft, AgentObjects, ETag, FieldError, Kind,
    MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, ObjectsError, OfferedSecret, Precondition,
    ResourceVersion, SharedDraft, SharedKind, Stored, config_name, config_references,
};

#[derive(Default)]
struct State {
    objects: BTreeMap<(Kind, String), Stored>,
    next_version: u64,
    offered: BTreeSet<OfferedSecret>,
    declined: BTreeSet<OfferedSecret>,
    /// Values a Secret of the backend holds. Nothing reads them: they are there so that a case can show
    /// that no answer of the port carries one.
    values: BTreeMap<(String, String), String>,
    unavailable: bool,
}

impl State {
    fn up(&self) -> Result<(), ObjectsError> {
        if self.unavailable {
            return Err(ObjectsError::Unavailable(
                "the in-memory object store is switched off".into(),
            ));
        }
        Ok(())
    }

    fn fresh_version(&mut self) -> ResourceVersion {
        self.next_version += 1;
        ResourceVersion::new(self.next_version.to_string())
    }

    fn get(&self, kind: Kind, name: &str) -> Option<&Stored> {
        self.objects.get(&(kind, name.to_owned()))
    }

    /// The agent whose service is `name`: its config is the one the service names.
    fn agent(&self, name: &str) -> Option<Agent> {
        let service = self.get(Kind::AgentService, name)?.clone();
        let config = config_name(&service.spec)
            .and_then(|c| self.get(Kind::AgentConfig, c))
            .cloned();
        Some(Agent { service, config })
    }

    /// Write `spec` as the dashboard: the object is created, or changed when the spec is not what it
    /// holds. The labels it already has stay, and the label of the dashboard is there afterwards. The
    /// status is never touched.
    fn write(&mut self, namespace: &str, kind: Kind, name: &str, spec: &Value) -> Stored {
        let version = self.fresh_version();
        let key = (kind, name.to_owned());
        match self.objects.get_mut(&key) {
            Some(o) => {
                let relabel = o.labels.get(MANAGED_BY_LABEL).map(String::as_str)
                    != Some(MANAGED_BY_DASHBOARD);
                if &o.spec != spec || relabel {
                    if &o.spec != spec {
                        o.generation += 1;
                    }
                    o.spec = spec.clone();
                    o.labels
                        .insert(MANAGED_BY_LABEL.to_owned(), MANAGED_BY_DASHBOARD.to_owned());
                    o.version = version;
                }
                o.clone()
            }
            None => {
                let stored = Stored {
                    kind,
                    namespace: namespace.to_owned(),
                    name: name.to_owned(),
                    version,
                    generation: 1,
                    labels: BTreeMap::from([(
                        MANAGED_BY_LABEL.to_owned(),
                        MANAGED_BY_DASHBOARD.to_owned(),
                    )]),
                    annotations: BTreeMap::new(),
                    spec: spec.clone(),
                    status: None,
                };
                self.objects.insert(key, stored.clone());
                stored
            }
        }
    }

    /// The agents that name the shared object `name` of `kind`, in byte order.
    fn referenced_by(&self, kind: SharedKind, name: &str) -> Vec<String> {
        let mut by = BTreeSet::new();
        for (k, service) in self.objects.keys() {
            if *k != Kind::AgentService {
                continue;
            }
            if let Some(agent) = self.agent(service)
                && let Some(config) = &agent.config
                && config_references(&config.spec).names(kind, name)
            {
                by.insert(service.clone());
            }
        }
        by.into_iter().collect()
    }
}

fn not_an_object(kind: Kind) -> FieldError {
    FieldError::new(kind, "spec", "a spec is an object")
}

/// [`AgentObjects`] over a map. Cheap to clone: every clone is a handle on the same state.
///
/// It behaves as the contract says and as an API server behaves, up to the validation of the CRDs: the
/// only body it refuses is one that is not an object (and a `configRef` that is not the agent's name).
/// A write that changes nothing keeps the version (a server-side apply of an unchanged object is believed to: *unverified*).
#[derive(Clone)]
pub struct MemoryObjects {
    namespace: String,
    state: Arc<Mutex<State>>,
}

impl Default for MemoryObjects {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryObjects {
    /// An empty store, in the namespace `aap-test`.
    pub fn new() -> Self {
        Self::in_namespace("aap-test")
    }

    /// An empty store in `namespace`.
    pub fn in_namespace(namespace: impl Into<String>) -> Self {
        Self {
            namespace: namespace.into(),
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    /// Every call fails with `Unavailable` while this is on.
    pub fn set_unavailable(&self, on: bool) {
        lock(&self.state).unavailable = on;
    }

    /// An object that is not the dashboard's, as Argo CD or `kubectl` would have made it.
    pub fn seed_foreign(&self, kind: Kind, name: &str, spec: Value, how: Foreign) {
        let mut s = lock(&self.state);
        let version = s.fresh_version();
        let mut labels = BTreeMap::new();
        let mut annotations = BTreeMap::new();
        match how {
            Foreign::Unlabelled => {}
            Foreign::ArgoTracked => {
                // The label is the dashboard's; the annotation makes the object GitOps's all the same.
                labels.insert(MANAGED_BY_LABEL.to_owned(), MANAGED_BY_DASHBOARD.to_owned());
                annotations.insert(
                    ARGO_TRACKING_ANNOTATION.to_owned(),
                    format!("app:agents.vymalo.com/{kind}:{}/{name}", self.namespace),
                );
            }
        }
        s.objects.insert(
            (kind, name.to_owned()),
            Stored {
                kind,
                namespace: self.namespace.clone(),
                name: name.to_owned(),
                version,
                generation: 1,
                labels,
                annotations,
                spec,
                status: None,
            },
        );
    }

    /// The status the operator would have written.
    pub fn set_status(&self, kind: Kind, name: &str, status: Value) {
        let mut s = lock(&self.state);
        let version = s.fresh_version();
        if let Some(o) = s.objects.get_mut(&(kind, name.to_owned())) {
            o.status = Some(status);
            o.version = version;
        }
    }

    /// An ExternalSecret that carries the offer label.
    pub fn offer(&self, secret: &str, key: &str, external_secret: &str) {
        lock(&self.state).offered.insert(OfferedSecret {
            secret: secret.to_owned(),
            key: key.to_owned(),
            external_secret: external_secret.to_owned(),
        });
    }

    /// An ExternalSecret without the offer label: it exists and is not offered.
    pub fn decline(&self, secret: &str, key: &str, external_secret: &str) {
        lock(&self.state).declined.insert(OfferedSecret {
            secret: secret.to_owned(),
            key: key.to_owned(),
            external_secret: external_secret.to_owned(),
        });
    }

    /// A Secret that holds `value` under `key`. Nothing in the store reads it.
    pub fn plant_secret_value(&self, secret: &str, key: &str, value: &str) {
        lock(&self.state)
            .values
            .insert((secret.to_owned(), key.to_owned()), value.to_owned());
    }
}

impl AgentObjects for MemoryObjects {
    async fn list_agents(&self) -> Result<Vec<Agent>, ObjectsError> {
        let s = lock(&self.state);
        s.up()?;
        let names: Vec<String> = s
            .objects
            .keys()
            .filter(|(k, _)| *k == Kind::AgentService)
            .map(|(_, n)| n.clone())
            .collect();
        Ok(names.iter().filter_map(|n| s.agent(n)).collect())
    }

    async fn get_agent(&self, name: &str) -> Result<Option<Agent>, ObjectsError> {
        let s = lock(&self.state);
        s.up()?;
        Ok(s.agent(name))
    }

    async fn put_agent(
        &self,
        draft: &AgentDraft,
        when: &Precondition,
    ) -> Result<Agent, ObjectsError> {
        let mut s = lock(&self.state);
        s.up()?;
        let name = draft.name.as_str();

        let mut invalid = Vec::new();
        if !draft.service.is_object() {
            invalid.push(not_an_object(Kind::AgentService));
        }
        if !draft.config.is_object() {
            invalid.push(not_an_object(Kind::AgentConfig));
        }
        if draft.service.is_object() && config_name(&draft.service) != Some(name) {
            invalid.push(FieldError::new(
                Kind::AgentService,
                "spec.configRef.name",
                format!("must be {name:?}: the config of an agent has the agent's name"),
            ));
        }
        if !invalid.is_empty() {
            return Err(ObjectsError::Invalid(invalid));
        }

        let service_in_place = s.get(Kind::AgentService, name).cloned();
        let config_in_place = s.get(Kind::AgentConfig, name).cloned();
        match when {
            Precondition::DoesNotExist => {
                if service_in_place.is_some() {
                    return Err(ObjectsError::AlreadyExists {
                        kind: Kind::AgentService,
                        name: name.to_owned(),
                    });
                }
            }
            Precondition::Matches(_) if service_in_place.is_none() => {
                return Err(ObjectsError::NotFound {
                    kind: Kind::AgentService,
                    name: name.to_owned(),
                });
            }
            Precondition::Matches(_) => {}
        }
        for object in service_in_place.iter().chain(&config_in_place) {
            if !object.owner().writable() {
                return Err(ObjectsError::NotOwned {
                    kind: object.kind,
                    name: object.name.clone(),
                });
            }
        }
        if let Precondition::Matches(etag) = when
            && s.agent(name).map(|a| a.etag()).as_ref() != Some(etag)
        {
            return Err(ObjectsError::PreconditionFailed);
        }

        s.write(&self.namespace, Kind::AgentConfig, name, &draft.config);
        s.write(&self.namespace, Kind::AgentService, name, &draft.service);
        s.agent(name)
            .ok_or_else(|| ObjectsError::Internal("an agent just written is not there".into()))
    }

    async fn delete_agent(&self, name: &str, when: Option<&ETag>) -> Result<(), ObjectsError> {
        let mut s = lock(&self.state);
        s.up()?;
        let agent = s.agent(name).ok_or_else(|| ObjectsError::NotFound {
            kind: Kind::AgentService,
            name: name.to_owned(),
        })?;
        for object in std::iter::once(&agent.service).chain(&agent.config) {
            if !object.owner().writable() {
                return Err(ObjectsError::NotOwned {
                    kind: object.kind,
                    name: object.name.clone(),
                });
            }
        }
        if when.is_some_and(|etag| *etag != agent.etag()) {
            return Err(ObjectsError::PreconditionFailed);
        }
        s.objects.remove(&(Kind::AgentService, name.to_owned()));
        if let Some(config) = &agent.config {
            s.objects.remove(&(Kind::AgentConfig, config.name.clone()));
        }
        Ok(())
    }

    async fn list_shared(&self, kind: SharedKind) -> Result<Vec<Stored>, ObjectsError> {
        let s = lock(&self.state);
        s.up()?;
        Ok(s.objects
            .iter()
            .filter(|((k, _), _)| *k == kind.kind())
            .map(|(_, o)| o.clone())
            .collect())
    }

    async fn get_shared(
        &self,
        kind: SharedKind,
        name: &str,
    ) -> Result<Option<Stored>, ObjectsError> {
        let s = lock(&self.state);
        s.up()?;
        Ok(s.get(kind.kind(), name).cloned())
    }

    async fn put_shared(
        &self,
        kind: SharedKind,
        draft: &SharedDraft,
        when: &Precondition,
    ) -> Result<Stored, ObjectsError> {
        let mut s = lock(&self.state);
        s.up()?;
        let kind = kind.kind();
        if !draft.spec.is_object() {
            return Err(ObjectsError::Invalid(vec![not_an_object(kind)]));
        }
        let in_place = s.get(kind, &draft.name).cloned();
        match (when, &in_place) {
            (Precondition::DoesNotExist, Some(_)) => {
                return Err(ObjectsError::AlreadyExists {
                    kind,
                    name: draft.name.clone(),
                });
            }
            (Precondition::Matches(_), None) => {
                return Err(ObjectsError::NotFound {
                    kind,
                    name: draft.name.clone(),
                });
            }
            _ => {}
        }
        if let Some(object) = &in_place {
            if !object.owner().writable() {
                return Err(ObjectsError::NotOwned {
                    kind,
                    name: draft.name.clone(),
                });
            }
            if let Precondition::Matches(etag) = when
                && *etag != object.etag()
            {
                return Err(ObjectsError::PreconditionFailed);
            }
        }
        Ok(s.write(&self.namespace, kind, &draft.name, &draft.spec))
    }

    async fn delete_shared(
        &self,
        kind: SharedKind,
        name: &str,
        when: Option<&ETag>,
    ) -> Result<(), ObjectsError> {
        let mut s = lock(&self.state);
        s.up()?;
        let object = s
            .get(kind.kind(), name)
            .cloned()
            .ok_or_else(|| ObjectsError::NotFound {
                kind: kind.kind(),
                name: name.to_owned(),
            })?;
        if !object.owner().writable() {
            return Err(ObjectsError::NotOwned {
                kind: kind.kind(),
                name: name.to_owned(),
            });
        }
        if when.is_some_and(|etag| *etag != object.etag()) {
            return Err(ObjectsError::PreconditionFailed);
        }
        let by = s.referenced_by(kind, name);
        if !by.is_empty() {
            return Err(ObjectsError::Referenced {
                kind: kind.kind(),
                name: name.to_owned(),
                by,
            });
        }
        s.objects.remove(&(kind.kind(), name.to_owned()));
        Ok(())
    }

    async fn offered_secrets(&self) -> Result<Vec<OfferedSecret>, ObjectsError> {
        let s = lock(&self.state);
        s.up()?;
        Ok(s.offered.iter().cloned().collect())
    }
}

impl ObjectsUnderTest for MemoryObjects {
    async fn seed_foreign(&self, kind: Kind, name: &str, spec: Value, how: Foreign) {
        Self::seed_foreign(self, kind, name, spec, how);
    }

    async fn set_status(&self, kind: Kind, name: &str, status: Value) {
        Self::set_status(self, kind, name, status);
    }

    async fn offer(&self, secret: &str, key: &str, external_secret: &str) {
        Self::offer(self, secret, key, external_secret);
    }

    async fn decline(&self, secret: &str, key: &str, external_secret: &str) {
        Self::decline(self, secret, key, external_secret);
    }

    async fn plant_secret_value(&self, secret: &str, key: &str, value: &str) {
        Self::plant_secret_value(self, secret, key, value);
    }

    async fn unreachable(&self) -> Option<Self> {
        let off = Self::in_namespace(self.namespace.clone());
        off.set_unavailable(true);
        Some(off)
    }
}
