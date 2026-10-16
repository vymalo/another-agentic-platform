//! What the conformance suite of `aap-ports` needs of a backend beyond the port (`ObjectsUnderTest`), written with
//! plain calls of the API that the fake and a real API server both answer, so that **one harness drives both**.
//!
//! The crate under test has the identity of the dashboard (`objects`); the harness has an administrator's (`admin`):
//! it makes the objects of GitOps, the status of the operator, the ExternalSecrets and the Secrets, which the
//! dashboard must never be able to.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use aap_api::{AgentConfig, AgentService, ModelEndpoint, ToolProvider};
use aap_objects_kubernetes::{FIELD_MANAGER, KubernetesObjects};
use aap_ports::testkit::{Foreign, ObjectsUnderTest, sample_agent};
use aap_ports::{
    ARGO_TRACKING_ANNOTATION, Agent, AgentDraft, AgentObjects, ETag, Kind, MANAGED_BY_DASHBOARD,
    MANAGED_BY_LABEL, OFFER_LABEL, ObjectsError, OfferedSecret, Precondition, SharedDraft,
    SharedKind, Stored,
};
use k8s_openapi::api::core::v1::Secret;
use kube::Client;
use kube::api::{Api, ApiResource, DynamicObject, Patch, PatchParams};
use serde_json::{Value, json};

use super::Fake;

/// The manager that stands for Argo CD, which applies server-side.
pub const ARGO: &str = "argocd-controller";
/// The manager that stands for a GitOps author or `kubectl`.
pub const GITOPS: &str = "gitops-author";
/// The manager of the operator, which applies the status.
pub const OPERATOR: &str = "aap-operator";
/// The manager of this harness: ExternalSecrets and Secrets.
pub const HARNESS: &str = "aap-test-harness";

pub fn resource(kind: Kind) -> ApiResource {
    match kind {
        Kind::AgentService => ApiResource::erase::<AgentService>(&()),
        Kind::AgentConfig => ApiResource::erase::<AgentConfig>(&()),
        Kind::ModelEndpoint => ApiResource::erase::<ModelEndpoint>(&()),
        Kind::ToolProvider => ApiResource::erase::<ToolProvider>(&()),
    }
}

fn external_secrets() -> ApiResource {
    ApiResource::from_gvk_with_plural(
        &kube::api::GroupVersionKind::gvk("external-secrets.io", "v1", "ExternalSecret"),
        "externalsecrets",
    )
}

/// One ExternalSecret the harness has made: what it asks for.
#[derive(Default)]
struct Es {
    secret: String,
    keys: BTreeSet<String>,
    any: bool,
    labelled: bool,
}

/// The objects under test, and the hands that set them up.
#[derive(Clone)]
pub struct Harness {
    pub objects: KubernetesObjects,
    admin: Client,
    namespace: String,
    /// The same objects, on a transport that fails.
    off: Option<KubernetesObjects>,
    offers: Arc<Mutex<BTreeMap<String, Es>>>,
    secrets: Arc<Mutex<BTreeMap<String, BTreeMap<String, String>>>>,
}

fn held<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Harness {
    /// `dashboard` is the identity of the crate under test, `admin` the harness's; `unreachable` a client whose
    /// transport fails.
    pub fn new(dashboard: Client, admin: Client, namespace: &str, unreachable: Client) -> Self {
        Self {
            objects: KubernetesObjects::new(dashboard, namespace),
            admin,
            namespace: namespace.to_owned(),
            off: Some(KubernetesObjects::new(unreachable, namespace)),
            offers: Arc::default(),
            secrets: Arc::default(),
        }
    }

    /// The harness on a fake API server, which serves ExternalSecrets.
    pub fn on_fake(fake: &Fake) -> Self {
        fake.serve("externalsecrets", true);
        Self::new(
            fake.client_as("dashboard"),
            fake.client_as("admin"),
            "aap-test",
            Fake::unreachable_client(),
        )
    }

    fn dynamic(&self, resource: &ApiResource) -> Api<DynamicObject> {
        Api::namespaced_with(self.admin.clone(), &self.namespace, resource)
    }

    /// The raw objects of a kind, as an administrator reads them.
    pub fn raw(&self, kind: Kind) -> Api<DynamicObject> {
        self.dynamic(&resource(kind))
    }

    async fn apply_as(
        &self,
        manager: &str,
        force: bool,
        resource: &ApiResource,
        name: &str,
        body: Value,
    ) {
        let mut params = PatchParams::apply(manager);
        if force {
            params = params.force();
        }
        self.dynamic(resource)
            .patch(name, &params, &Patch::Apply(&body))
            .await
            .unwrap_or_else(|e| {
                panic!("the harness could not apply {} {name}: {e}", resource.kind)
            });
    }

    async fn make_external_secret(&self, es: &str) {
        let body = {
            let offers = held(&self.offers);
            let state = &offers[es];
            let mut metadata = json!({ "name": es });
            if state.labelled {
                metadata["labels"] = json!({ OFFER_LABEL: "true" });
            }
            let mut spec = json!({
                "refreshInterval": "1h",
                "secretStoreRef": { "kind": "ClusterSecretStore", "name": "aap-test" },
                "target": { "name": state.secret },
            });
            if !state.keys.is_empty() {
                spec["data"] = state
                    .keys
                    .iter()
                    .map(|k| json!({ "secretKey": k, "remoteRef": { "key": "aap/test", "property": k } }))
                    .collect();
            }
            if state.any {
                spec["dataFrom"] = json!([{ "extract": { "key": "aap/test" } }]);
            }
            json!({ "apiVersion": "external-secrets.io/v1", "kind": "ExternalSecret", "metadata": metadata, "spec": spec })
        };
        self.apply_as(HARNESS, true, &external_secrets(), es, body)
            .await;
    }

    /// The ExternalSecret that holds what `secret` is asked to have: the one named `es`, unless that one makes
    /// another Secret (the cases name an ExternalSecret and a Secret independently).
    fn external_secret_for(&self, es: &str, secret: &str) -> String {
        let mut offers = held(&self.offers);
        let mut name = es.to_owned();
        while offers.get(&name).is_some_and(|e| e.secret != secret) {
            name = format!("{name}-{secret}");
        }
        offers.entry(name.clone()).or_insert_with(|| Es {
            secret: secret.to_owned(),
            labelled: true,
            ..Es::default()
        });
        name
    }
}

impl AgentObjects for Harness {
    async fn list_agents(&self) -> Result<Vec<Agent>, ObjectsError> {
        self.objects.list_agents().await
    }

    async fn get_agent(&self, name: &str) -> Result<Option<Agent>, ObjectsError> {
        self.objects.get_agent(name).await
    }

    async fn put_agent(
        &self,
        draft: &AgentDraft,
        when: &Precondition,
    ) -> Result<Agent, ObjectsError> {
        self.objects.put_agent(draft, when).await
    }

    async fn delete_agent(&self, name: &str, when: &ETag) -> Result<(), ObjectsError> {
        self.objects.delete_agent(name, when).await
    }

    async fn list_shared(&self, kind: SharedKind) -> Result<Vec<Stored>, ObjectsError> {
        self.objects.list_shared(kind).await
    }

    async fn get_shared(
        &self,
        kind: SharedKind,
        name: &str,
    ) -> Result<Option<Stored>, ObjectsError> {
        self.objects.get_shared(kind, name).await
    }

    async fn put_shared(
        &self,
        kind: SharedKind,
        draft: &SharedDraft,
        when: &Precondition,
    ) -> Result<Stored, ObjectsError> {
        self.objects.put_shared(kind, draft, when).await
    }

    async fn delete_shared(
        &self,
        kind: SharedKind,
        name: &str,
        when: &ETag,
    ) -> Result<(), ObjectsError> {
        self.objects.delete_shared(kind, name, when).await
    }

    async fn offered_secrets(&self) -> Result<Vec<OfferedSecret>, ObjectsError> {
        self.objects.offered_secrets().await
    }
}

impl ObjectsUnderTest for Harness {
    async fn seed_foreign(&self, kind: Kind, name: &str, spec: Value, how: Foreign) {
        let resource = resource(kind);
        let mut metadata = json!({ "name": name });
        let manager = match how {
            Foreign::Unlabelled => GITOPS,
            Foreign::ArgoTracked => {
                // Argo CD tracks by its annotation and leaves the label the dashboard wrote.
                metadata["labels"] = json!({ MANAGED_BY_LABEL: MANAGED_BY_DASHBOARD });
                metadata["annotations"] = json!({
                    ARGO_TRACKING_ANNOTATION: format!("agentic:agents.vymalo.com/{kind}:{}/{name}", self.namespace)
                });
                ARGO
            }
        };
        let body = json!({
            "apiVersion": resource.api_version, "kind": resource.kind, "metadata": metadata, "spec": spec,
        });
        // It takes the object over: another manager's fields are forced.
        self.apply_as(manager, true, &resource, name, body).await;
        if how == Foreign::Unlabelled {
            // An apply does not remove a field that another manager owns: an update does.
            self.dynamic(&resource)
                .patch(
                    name,
                    &PatchParams {
                        field_manager: Some(manager.to_owned()),
                        ..PatchParams::default()
                    },
                    &Patch::Merge(json!({ "metadata": { "labels": { MANAGED_BY_LABEL: null } } })),
                )
                .await
                .unwrap_or_else(|e| {
                    panic!("the harness could not take the label off {kind} {name}: {e}")
                });
        }
    }

    async fn seed_orphan_config(&self, name: &str, spec: Value) {
        let resource = resource(Kind::AgentConfig);
        let body = json!({
            "apiVersion": resource.api_version, "kind": resource.kind,
            "metadata": { "name": name, "labels": { MANAGED_BY_LABEL: MANAGED_BY_DASHBOARD } },
            "spec": spec,
        });
        self.apply_as(FIELD_MANAGER, false, &resource, name, body)
            .await;
    }

    async fn set_status(&self, kind: Kind, name: &str, status: Value) {
        let resource = resource(kind);
        let body = json!({
            "apiVersion": resource.api_version, "kind": resource.kind,
            "metadata": { "name": name }, "status": status,
        });
        self.dynamic(&resource)
            .patch_status(
                name,
                &PatchParams::apply(OPERATOR).force(),
                &Patch::Apply(&body),
            )
            .await
            .unwrap_or_else(|e| {
                panic!("the harness could not write the status of {kind} {name}: {e}")
            });
    }

    async fn offer(&self, secret: &str, key: &str, external_secret: &str) {
        let es = self.external_secret_for(external_secret, secret);
        held(&self.offers)
            .get_mut(&es)
            .expect("just made")
            .keys
            .insert(key.to_owned());
        self.make_external_secret(&es).await;
    }

    async fn offer_any_key(&self, secret: &str, external_secret: &str) {
        let es = self.external_secret_for(external_secret, secret);
        held(&self.offers).get_mut(&es).expect("just made").any = true;
        self.make_external_secret(&es).await;
    }

    async fn decline(&self, secret: &str, key: &str, external_secret: &str) {
        let es = self.external_secret_for(external_secret, secret);
        {
            let mut offers = held(&self.offers);
            let state = offers.get_mut(&es).expect("just made");
            state.labelled = false;
            state.keys.insert(key.to_owned());
        }
        self.make_external_secret(&es).await;
    }

    async fn plant_secret_value(&self, secret: &str, key: &str, value: &str) {
        let string_data = {
            let mut secrets = held(&self.secrets);
            let keys = secrets.entry(secret.to_owned()).or_default();
            keys.insert(key.to_owned(), value.to_owned());
            keys.clone()
        };
        let body = json!({
            "apiVersion": "v1", "kind": "Secret", "metadata": { "name": secret },
            "type": "Opaque", "stringData": string_data,
        });
        self.apply_as(
            HARNESS,
            true,
            &ApiResource::erase::<Secret>(&()),
            secret,
            body,
        )
        .await;
    }

    async fn unreachable(&self) -> Option<Self> {
        let mut off = self.clone();
        off.objects = self.off.clone()?;
        Some(off)
    }

    /// Bodies that break a rule of a CRD (a CEL rule): the `422` of the API server names the field.
    fn refused_spec(&self, kind: Kind) -> Option<(Value, String)> {
        let sample = sample_agent("refused");
        match kind {
            Kind::AgentService => {
                let mut spec = sample.service;
                spec["access"]["audience"] = json!(["*", "agent.use:refused"]);
                Some((spec, "spec.access.audience".to_owned()))
            }
            Kind::AgentConfig => {
                let mut spec = sample.config;
                spec["model"] = json!({
                    "model": "chat-model",
                    "endpointRef": { "name": "gateway" },
                    "baseUrl": { "value": "https://gateway.example.invalid/v1" },
                    "apiKeySecretRef": { "name": "a-secret", "key": "KEY" },
                });
                Some((spec, "spec.model".to_owned()))
            }
            _ => Some((
                Value::String("a spec is an object".to_owned()),
                "spec".to_owned(),
            )),
        }
    }
}
