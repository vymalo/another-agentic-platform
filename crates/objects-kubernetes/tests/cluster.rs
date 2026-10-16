//! The crate against a real API server: the conformance suite of `aap-ports`, and what only a server shows (the
//! behaviour of a `resourceVersion` in an apply, the `Status` bodies of its refusals, the defaults of the CRDs,
//! the rights of a Role).
//!
//! **Opt in.** The tests create objects in the namespace `aap-test` and install the platform CRDs and the
//! ExternalSecret CRD (a pinned copy in `tests/crds`: no network is needed); they run only when
//! `AAP_TEST_KUBECONFIG` names a kubeconfig file (never the default context: a developer's cluster is not a
//! fixture by accident). Without it they skip, and with `AAP_TEST_REQUIRE_CLUSTER=1` (CI) skipping is a failure.
//! A throwaway cluster is the point: nothing is cleaned up but what a case deletes itself.
//!
//! * `AAP_TEST_KUBECONFIG`: an administrator. The harness makes GitOps's objects, the operator's status, the
//!   ExternalSecrets and the Secrets with it.
//! * `AAP_TEST_API_KUBECONFIG`: **the identity of the crate under test**, a kubeconfig whose user holds exactly the
//!   `Role` of `tests/rbac.yaml` in `aap-test` and nothing else. Unset, the crate runs as the administrator and the
//!   cases about rights skip; under `AAP_TEST_REQUIRE_CLUSTER=1` it must be set.
//!
//! ```sh
//! kind create cluster --name aap
//! AAP_TEST_KUBECONFIG=$HOME/.kube/config cargo test -p aap-objects-kubernetes --test cluster
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::OnceLock;
use std::time::Duration;

use aap_objects_kubernetes::{FIELD_MANAGER, KubernetesObjects, NEVER};
use aap_ports::testkit::{ObjectsUnderTest, sample_agent, sample_shared, unique};
use aap_ports::{AgentObjects, Kind, ObjectsError, Precondition, SharedKind};
use k8s_openapi::api::authorization::v1::SelfSubjectAccessReview;
use k8s_openapi::api::core::v1::{Namespace, Secret};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use k8s_openapi::serde::Deserialize;
use kube::api::{
    Api, DeleteParams, DynamicObject, ListParams, Patch, PatchParams, PostParams, Preconditions,
};
use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use serde_json::{Value, json};
use support::facts;
use support::harness::{GITOPS, Harness, resource};
use support::intercept::interpose;

const KUBECONFIG_VAR: &str = "AAP_TEST_KUBECONFIG";
const API_KUBECONFIG_VAR: &str = "AAP_TEST_API_KUBECONFIG";
const REQUIRE_VAR: &str = "AAP_TEST_REQUIRE_CLUSTER";

/// The namespace the cases live in.
const NAMESPACE: &str = "aap-test";

/// The platform's CRDs as the checked-in file has them, and the ExternalSecret CRD of the External Secrets
/// Operator v2.11.0 (`tests/crds/README.md` says where it is from and holds its checksum).
const PLATFORM_CRDS: &str = include_str!("../../../deploy/crds/agents.vymalo.com.yaml");
const EXTERNAL_SECRET_CRD: &str = include_str!("crds/external-secrets.io_externalsecrets.yaml");

fn required() -> bool {
    matches!(std::env::var(REQUIRE_VAR).as_deref(), Ok("1" | "true"))
}

async fn client_of(var: &str) -> Option<Client> {
    let path = std::env::var(var).ok().filter(|p| !p.is_empty())?;
    let kubeconfig = Kubeconfig::read_from(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let config = Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default())
        .await
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    Some(Client::try_from(config).unwrap_or_else(|e| panic!("{path}: {e}")))
}

/// The CRDs and the namespace, once per process. It runs on a thread and a runtime of its own: every test has a
/// runtime of its own, and a connection made in one cannot be used from another.
fn install_once() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let path = std::env::var(KUBECONFIG_VAR).unwrap_or_default();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async move {
                    let client = {
                        let kubeconfig = Kubeconfig::read_from(&path).unwrap();
                        let config = Config::from_custom_kubeconfig(
                            kubeconfig,
                            &KubeConfigOptions::default(),
                        )
                        .await
                        .unwrap();
                        Client::try_from(config).unwrap()
                    };
                    install(&client).await;
                });
        })
        .join()
        .expect("installing the CRDs");
    });
}

async fn install(client: &Client) {
    let params = PatchParams::apply("aap-test-harness").force();
    let ns: Namespace = serde_json::from_value(json!({
        "apiVersion": "v1", "kind": "Namespace", "metadata": { "name": NAMESPACE }
    }))
    .unwrap();
    Api::<Namespace>::all(client.clone())
        .patch(NAMESPACE, &params, &Patch::Apply(&ns))
        .await
        .unwrap_or_else(|e| panic!("namespace {NAMESPACE}: {e}"));

    let crds: Api<CustomResourceDefinition> = Api::all(client.clone());
    let mut names = Vec::new();
    for text in [PLATFORM_CRDS, EXTERNAL_SECRET_CRD] {
        for document in serde_yaml::Deserializer::from_str(text) {
            let crd = CustomResourceDefinition::deserialize(document).unwrap();
            let name = crd.metadata.name.clone().unwrap();
            crds.patch(&name, &params, &Patch::Apply(&crd))
                .await
                .unwrap_or_else(|e| panic!("CRD {name}: {e}"));
            names.push(name);
        }
    }
    for name in names {
        let mut waited = 0;
        loop {
            let crd = crds.get(&name).await.unwrap();
            let established = crd.status.and_then(|s| s.conditions).is_some_and(|c| {
                c.iter()
                    .any(|c| c.type_ == "Established" && c.status == "True")
            });
            if established {
                break;
            }
            waited += 1;
            assert!(waited < 120, "CRD {name} is not established");
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

/// A client for a server that is not there.
fn closed_port() -> Client {
    let config = Config::new("http://127.0.0.1:1".parse().unwrap());
    Client::try_from(config).unwrap()
}

/// The administrator's client and the crate's, on the cluster of `AAP_TEST_KUBECONFIG`, or `None` to skip.
async fn identities() -> Option<(Client, Client)> {
    let Some(admin) = client_of(KUBECONFIG_VAR).await else {
        assert!(
            !required(),
            "{KUBECONFIG_VAR} is not set, but {REQUIRE_VAR}=1 forbids skipping"
        );
        eprintln!("skipped: {KUBECONFIG_VAR} is not set");
        return None;
    };
    // A variable that is set names a cluster that must answer.
    admin
        .apiserver_version()
        .await
        .unwrap_or_else(|e| panic!("the cluster of {KUBECONFIG_VAR} does not answer: {e}"));
    install_once();
    let dashboard = match client_of(API_KUBECONFIG_VAR).await {
        Some(client) => client,
        None => {
            assert!(
                !required(),
                "{API_KUBECONFIG_VAR} is not set, but {REQUIRE_VAR}=1 forbids running as an administrator"
            );
            admin.clone()
        }
    };
    Some((admin, dashboard))
}

/// The harness on the cluster, or `None` to skip.
async fn make() -> Option<Harness> {
    let (admin, dashboard) = identities().await?;
    Some(Harness::new(dashboard, admin, NAMESPACE, closed_port()))
}

aap_ports::agent_objects_conformance!(make);

// ------------------------------------------------------------------ what only a server shows

/// The cluster, or a skip.
macro_rules! cluster {
    () => {
        match make().await {
            Some(h) => h,
            None => return,
        }
    };
}

// What the fake API server of `tests/api.rs` copies, asserted here against the real one by the same code.

#[tokio::test]
async fn a_delete_of_an_object_with_a_finalizer_leaves_it_terminating() {
    let h = cluster!();
    facts::a_delete_of_an_object_with_a_finalizer_leaves_it_terminating(&h).await;
}

#[tokio::test]
async fn a_stale_version_is_a_conflict_before_the_rules_of_the_crd() {
    let h = cluster!();
    facts::a_stale_version_is_a_conflict_before_the_rules_of_the_crd(&h).await;
}

#[tokio::test]
async fn a_create_stores_the_defaults_of_the_schema() {
    let h = cluster!();
    facts::a_create_stores_the_defaults_of_the_schema(&h).await;
}

#[tokio::test]
async fn the_dashboard_sees_what_it_wrote_and_the_server_stores_more() {
    let h = cluster!();
    let name = unique("ag");
    let draft = sample_agent(&name);
    let made = h
        .put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    assert_eq!(made.service.spec, draft.service, "what was written");
    let stored = h.raw(Kind::AgentService).get(&name).await.unwrap();
    let spec = &stored.data["spec"];
    assert_ne!(spec, &draft.service, "the server stores its defaults too");
    assert_eq!(spec["suspend"], json!(false));
    assert_eq!(spec["deletionPolicy"], json!("Retain"));
    let managers: Vec<&str> = stored
        .metadata
        .managed_fields
        .as_deref()
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m.manager.as_deref())
        .collect();
    assert_eq!(
        managers,
        [FIELD_MANAGER],
        "one manager wrote it, and it applied"
    );
}

#[tokio::test]
async fn an_apply_that_is_not_the_version_that_was_read_is_refused_even_when_it_changes_nothing() {
    let h = cluster!();
    let name = unique("ag");
    let draft = sample_agent(&name);
    h.put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    let api = h.raw(Kind::AgentService);
    let live = api.get(&name).await.unwrap();
    let apply = |version: &str, dry_run: bool| {
        let api = api.clone();
        let (name, spec, version) = (name.clone(), draft.service.clone(), version.to_owned());
        async move {
            let mut params = PatchParams::apply(FIELD_MANAGER);
            if dry_run {
                params = params.dry_run();
            }
            api.patch(
                &name,
                &params,
                &Patch::Apply(&json!({
                    "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "AgentService",
                    "metadata": { "name": name, "resourceVersion": version,
                                  "labels": { "app.kubernetes.io/managed-by": "dashboard.agents.vymalo.com" } },
                    "spec": spec,
                })),
            )
            .await
        }
    };
    let current = live.metadata.resource_version.clone().unwrap();
    // The same spec: nothing would change. The version is checked all the same, also as a dry run.
    for dry_run in [false, true] {
        let err = apply(NEVER, dry_run).await.unwrap_err();
        assert!(
            matches!(&err, kube::Error::Api(s) if s.code == 409 && s.reason == "Conflict"),
            "{err}"
        );
    }
    let ok = apply(&current, false).await.unwrap();
    assert_eq!(
        ok.metadata.resource_version, live.metadata.resource_version,
        "an apply of an unchanged object keeps its version"
    );
}

#[tokio::test]
async fn a_create_names_a_version_no_object_has_and_never_merges_into_one() {
    let h = cluster!();
    let name = unique("ag");
    let api = h.raw(Kind::ModelEndpoint);
    let body = |title: &str| {
        json!({
            "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "ModelEndpoint",
            "metadata": { "name": name, "resourceVersion": NEVER },
            "spec": sample_shared(SharedKind::Model, &name).spec.as_object().map(|s| {
                let mut s = s.clone(); s.insert("title".into(), json!(title)); Value::Object(s) }),
        })
    };
    // The object is not there: the version is ignored, and it is made.
    api.patch(
        &name,
        &PatchParams::apply(GITOPS),
        &Patch::Apply(&body("first")),
    )
    .await
    .unwrap();
    // The object is there now, and made by somebody else: the same create is a conflict and merges nothing.
    let err = api
        .patch(
            &name,
            &PatchParams::apply(FIELD_MANAGER),
            &Patch::Apply(&body("second")),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, kube::Error::Api(s) if s.code == 409),
        "{err}"
    );
    let seen = api.get(&name).await.unwrap();
    assert_eq!(seen.data["spec"]["title"], json!("first"));
    // And through the crate: AlreadyExists, and the object of the other is as it was.
    let err = h
        .put_shared(
            SharedKind::Model,
            &sample_shared(SharedKind::Model, &name),
            &Precondition::DoesNotExist,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            ObjectsError::AlreadyExists {
                kind: Kind::ModelEndpoint,
                ..
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_uid_ties_a_write_to_the_object_that_was_read() {
    let h = cluster!();
    let name = unique("sh");
    let made = h
        .put_shared(
            SharedKind::Model,
            &sample_shared(SharedKind::Model, &name),
            &Precondition::DoesNotExist,
        )
        .await
        .unwrap();
    let api = h.raw(Kind::ModelEndpoint);
    let uid = api.get(&name).await.unwrap().metadata.uid.unwrap();
    let apply = |uid: &str, name: &str| {
        let api = api.clone();
        let body = json!({
            "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "ModelEndpoint",
            "metadata": { "name": name, "uid": uid }, "spec": made.spec,
        });
        let name = name.to_owned();
        async move {
            api.patch(
                &name,
                &PatchParams::apply(FIELD_MANAGER).dry_run(),
                &Patch::Apply(&body),
            )
            .await
        }
    };
    // The object was deleted and made again (another uid): refused, as an immutable field.
    let err = apply("00000000-0000-0000-0000-000000000000", &name)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, kube::Error::Api(s) if s.code == 422 && s.message.contains("immutable")),
        "{err}"
    );
    // The object is not there at all: a conflict, and never a create.
    let err = apply(&uid, &unique("gone")).await.unwrap_err();
    assert!(
        matches!(&err, kube::Error::Api(s) if s.code == 409 && s.message.contains("no existing object")),
        "{err}"
    );
    apply(&uid, &name).await.unwrap();
}

#[tokio::test]
async fn a_delete_dry_run_checks_its_preconditions_and_deletes_nothing() {
    let h = cluster!();
    let name = unique("sh");
    h.put_shared(
        SharedKind::Model,
        &sample_shared(SharedKind::Model, &name),
        &Precondition::DoesNotExist,
    )
    .await
    .unwrap();
    let api = h.raw(Kind::ModelEndpoint);
    let live = api.get(&name).await.unwrap();
    let delete = |version: &str, dry_run: bool| {
        let api = api.clone();
        let params = DeleteParams {
            dry_run,
            preconditions: Some(Preconditions {
                resource_version: Some(version.to_owned()),
                uid: live.metadata.uid.clone(),
            }),
            ..DeleteParams::default()
        };
        let name = name.clone();
        async move { api.delete(&name, &params).await }
    };
    let err = delete(NEVER, true).await.unwrap_err();
    assert!(
        matches!(&err, kube::Error::Api(s) if s.code == 409),
        "{err}"
    );
    delete(live.metadata.resource_version.as_deref().unwrap(), true)
        .await
        .unwrap();
    assert!(api.get(&name).await.is_ok(), "a dry run deletes nothing");
}

#[tokio::test]
async fn a_field_another_manager_owns_is_a_conflict_that_changes_neither_object() {
    let h = cluster!();
    let name = unique("ag");
    let draft = sample_agent(&name);
    let made = h
        .put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    // Somebody edits the title by hand (an update: it takes the field).
    let services = h.raw(Kind::AgentService);
    services
        .patch(
            &name,
            &PatchParams {
                field_manager: Some("kubectl-edit".to_owned()),
                ..PatchParams::default()
            },
            &Patch::Merge(json!({ "spec": { "registry": { "title": "By hand" } } })),
        )
        .await
        .unwrap();
    let after_the_hand = h.get_agent(&name).await.unwrap().unwrap();

    // The dashboard changes the title and the config. The service is refused, and the config is not written.
    let mut changed = draft.clone();
    changed.service["registry"]["title"] = json!("By the dashboard");
    changed.config["model"]["model"] = json!("another-model");
    let err = h
        .put_agent(&changed, &Precondition::Matches(after_the_hand.etag()))
        .await
        .unwrap_err();
    let ObjectsError::Conflict(message) = &err else {
        panic!("{err:?}")
    };
    assert!(
        message.contains("kubectl-edit") && message.contains("registry.title"),
        "{message}"
    );
    let still = h.get_agent(&name).await.unwrap().unwrap();
    assert_eq!(still, after_the_hand, "neither object was written");
    assert_ne!(still.etag(), made.etag());
}

#[tokio::test]
async fn the_status_of_the_operator_makes_a_token_stale_and_a_save_that_changes_nothing_keeps_it() {
    let h = cluster!();
    let name = unique("ag");
    let draft = sample_agent(&name);
    let first = h
        .put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    // A save of the same bodies, with the token: nothing changes, so the token is the same.
    let again = h
        .put_agent(&draft, &Precondition::Matches(first.etag()))
        .await
        .unwrap();
    assert_eq!(
        again.etag(),
        first.etag(),
        "an apply of an unchanged agent keeps both versions"
    );

    // The operator writes a status: the token the client holds is stale, as the router will say with a 412.
    h.set_status(
        Kind::AgentService,
        &name,
        json!({ "state": "Ready", "observedGeneration": 1 }),
    )
    .await;
    let err = h
        .put_agent(&draft, &Precondition::Matches(first.etag()))
        .await
        .unwrap_err();
    assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
    let current = h.get_agent(&name).await.unwrap().unwrap();
    assert_ne!(current.etag(), first.etag());
    h.put_agent(&draft, &Precondition::Matches(current.etag()))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_cel_rules_of_the_crds_come_back_with_the_field_they_name() {
    let h = cluster!();
    for (kind, spec, path) in [
        (
            Kind::AgentService,
            h.refused_spec(Kind::AgentService).unwrap().0,
            "spec.access.audience",
        ),
        (
            Kind::AgentConfig,
            h.refused_spec(Kind::AgentConfig).unwrap().0,
            "spec.model",
        ),
    ] {
        let name = unique("ag");
        let mut draft = sample_agent(&name);
        match kind {
            Kind::AgentService => {
                draft.service = spec;
                draft.service["configRef"]["name"] = json!(name);
            }
            _ => draft.config = spec,
        }
        let err = h
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap_err();
        let ObjectsError::Invalid(fields) = &err else {
            panic!("{err:?}")
        };
        assert_eq!(fields.len(), 1, "{fields:?}");
        assert_eq!(
            (fields[0].kind, fields[0].path.as_str()),
            (kind, path),
            "{fields:?}"
        );
        assert!(!fields[0].message.is_empty());
        assert_eq!(h.get_agent(&name).await.unwrap(), None, "nothing was made");
    }
}

#[tokio::test]
async fn a_body_the_schema_refuses_is_invalid_with_its_fields_and_not_a_server_error() {
    let h = cluster!();
    let name = unique("ag");
    let mut draft = sample_agent(&name);
    draft.service["suspend"] = json!("yes");
    draft.service["scaling"] = json!({ "workers": "many" });
    draft.config["bogus"] = json!(1);
    let err = h
        .put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap_err();
    let ObjectsError::Invalid(fields) = &err else {
        panic!("{err:?}")
    };
    let seen: Vec<(Kind, &str)> = fields.iter().map(|f| (f.kind, f.path.as_str())).collect();
    assert!(
        seen.contains(&(Kind::AgentService, "spec.suspend")),
        "{seen:?}"
    );
    assert!(
        seen.contains(&(Kind::AgentService, "spec.scaling.workers")),
        "{seen:?}"
    );
    assert!(
        seen.contains(&(Kind::AgentConfig, "spec.bogus")),
        "{seen:?}"
    );
    assert_eq!(h.get_agent(&name).await.unwrap(), None);
    assert!(h.get_agent(&name).await.unwrap().is_none());
}

#[tokio::test]
async fn an_external_secret_with_the_label_is_offered_and_one_without_is_not() {
    let h = cluster!();
    let (secret, other) = (unique("sec"), unique("sec"));
    h.offer(&secret, "API_KEY", &unique("es")).await;
    h.decline(&other, "API_KEY", &unique("es")).await;
    let offered = h.offered_secrets().await.unwrap();
    assert!(
        offered
            .iter()
            .any(|o| o.secret == secret && o.key.as_deref() == Some("API_KEY"))
    );
    assert!(!offered.iter().any(|o| o.secret == other));
}

#[tokio::test]
async fn an_unreachable_server_is_unavailable_and_a_dead_namespace_is_not_a_missing_object() {
    let h = cluster!();
    let off = h.unreachable().await.unwrap();
    let err = off.get_agent("x").await.unwrap_err();
    assert!(matches!(err, ObjectsError::Unavailable(_)), "{err:?}");
    // A namespace that does not exist lists nothing: there is no object there.
    let elsewhere = KubernetesObjects::new(
        client_of(KUBECONFIG_VAR).await.unwrap(),
        "no-such-namespace",
    );
    assert!(elsewhere.list_agents().await.unwrap().is_empty());
    assert!(elsewhere.offered_secrets().await.unwrap().is_empty());
}

// ------------------------------------------------------------------ rights (the identity of AAP_TEST_API_KUBECONFIG)

async fn can(client: &Client, verb: &str, group: &str, resource: &str) -> bool {
    let review: SelfSubjectAccessReview = serde_json::from_value(json!({
        "apiVersion": "authorization.k8s.io/v1", "kind": "SelfSubjectAccessReview",
        "spec": { "resourceAttributes": { "namespace": NAMESPACE, "verb": verb, "group": group, "resource": resource } },
    }))
    .unwrap();
    Api::<SelfSubjectAccessReview>::all(client.clone())
        .create(&PostParams::default(), &review)
        .await
        .unwrap()
        .status
        .is_some_and(|s| s.allowed)
}

#[tokio::test]
async fn the_identity_of_the_crate_has_the_role_and_no_right_on_secrets() {
    let Some(dashboard) = client_of(API_KUBECONFIG_VAR).await else {
        assert!(
            !required(),
            "{API_KUBECONFIG_VAR} is not set, but {REQUIRE_VAR}=1 forbids skipping"
        );
        eprintln!("skipped: {API_KUBECONFIG_VAR} is not set");
        return;
    };
    if identities().await.is_none() {
        return;
    }
    // The four kinds: what the crate does, and nothing more.
    for resource in [
        "agentservices",
        "agentconfigs",
        "modelendpoints",
        "toolproviders",
    ] {
        for (verb, allowed) in [
            ("get", true),
            ("list", true),
            ("create", true),
            ("patch", true),
            ("delete", true),
            ("watch", false),
            ("update", false),
            ("deletecollection", false),
        ] {
            assert_eq!(
                can(&dashboard, verb, "agents.vymalo.com", resource).await,
                allowed,
                "{verb} {resource}"
            );
        }
    }
    // The ExternalSecrets: a list, for the offer.
    for (verb, allowed) in [
        ("list", true),
        ("get", false),
        ("watch", false),
        ("create", false),
        ("patch", false),
        ("update", false),
        ("delete", false),
    ] {
        assert_eq!(
            can(&dashboard, verb, "external-secrets.io", "externalsecrets").await,
            allowed,
            "{verb} externalsecrets"
        );
    }
    // The Secrets: nothing, not a list (which would return their values).
    for verb in [
        "get", "list", "watch", "create", "patch", "update", "delete",
    ] {
        assert!(
            !can(&dashboard, verb, "", "secrets").await,
            "{verb} secrets"
        );
    }
    // And it is the API server that says so, to the client.
    let err = Api::<Secret>::namespaced(dashboard, NAMESPACE)
        .list(&ListParams::default())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, kube::Error::Api(s) if s.code == 403),
        "{err}"
    );
}

// ------------------------------------------------------------------ somebody gets in first

/// The service of the dashboard's agent `name`, as another administrator saves it through the dashboard.
fn service_body(name: &str, spec: &Value) -> Value {
    json!({
        "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "AgentService",
        "metadata": { "name": name, "labels": { "app.kubernetes.io/managed-by": "dashboard.agents.vymalo.com" } },
        "spec": spec,
    })
}

#[tokio::test]
async fn when_the_service_changes_between_the_two_applies_the_config_stays_written_and_a_retry_succeeds()
 {
    let Some((admin, dashboard)) = identities().await else {
        return;
    };
    let (client, hooks) = interpose(&dashboard, NAMESPACE);
    let objects = KubernetesObjects::new(client, NAMESPACE);
    let name = unique("ag");
    let draft = sample_agent(&name);
    let made = objects
        .put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();

    // Another administrator saves the service, through the dashboard, after the dry runs and before the apply.
    let mut theirs = draft.service.clone();
    theirs["description"] = json!("theirs");
    let api = Api::<DynamicObject>::namespaced_with(
        admin.clone(),
        NAMESPACE,
        &resource(Kind::AgentService),
    );
    let service = name.clone();
    hooks.before_the_next_apply("agentservices", &name, move || async move {
        api.patch(
            &service,
            &PatchParams::apply(FIELD_MANAGER),
            &Patch::Apply(&service_body(&service, &theirs)),
        )
        .await
        .unwrap();
    });
    let mut changed = draft.clone();
    changed.service["registry"]["title"] = json!("Changed");
    changed.config["model"]["model"] = json!("another-model");
    let err = objects
        .put_agent(&changed, &Precondition::Matches(made.etag()))
        .await
        .unwrap_err();
    assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");

    // The one place a refusal changes something: the config was written first, and the service is theirs.
    let config = Api::<DynamicObject>::namespaced_with(
        admin.clone(),
        NAMESPACE,
        &resource(Kind::AgentConfig),
    )
    .get(&name)
    .await
    .unwrap();
    assert_eq!(
        config.data["spec"]["model"]["model"],
        json!("another-model")
    );
    let service =
        Api::<DynamicObject>::namespaced_with(admin, NAMESPACE, &resource(Kind::AgentService))
            .get(&name)
            .await
            .unwrap();
    assert_eq!(service.data["spec"]["description"], json!("theirs"));
    assert_eq!(service.data["spec"]["registry"]["title"], json!("Sample"));

    // A retry reads again and succeeds.
    let seen = objects.get_agent(&name).await.unwrap().unwrap();
    let saved = objects
        .put_agent(&changed, &Precondition::Matches(seen.etag()))
        .await
        .unwrap();
    assert_eq!(saved.service.spec["registry"]["title"], json!("Changed"));
}

#[tokio::test]
async fn a_create_never_merges_into_a_service_made_between_the_read_and_the_apply() {
    let Some((admin, dashboard)) = identities().await else {
        return;
    };
    for (their_description, outcome) in [
        (None, "AlreadyExists"),
        (Some("made by somebody else"), "Conflict"),
    ] {
        let (client, hooks) = interpose(&dashboard, NAMESPACE);
        let objects = KubernetesObjects::new(client, NAMESPACE);
        let name = unique("ag");
        let draft = sample_agent(&name);
        let mut theirs = draft.service.clone();
        if let Some(d) = their_description {
            theirs["description"] = json!(d);
        }
        let api = Api::<DynamicObject>::namespaced_with(
            admin.clone(),
            NAMESPACE,
            &resource(Kind::AgentService),
        );
        let service = name.clone();
        hooks.before_the_next_apply("agentservices", &name, move || async move {
            // A GitOps tool: no label of the dashboard.
            let mut body = service_body(&service, &theirs);
            body["metadata"].as_object_mut().unwrap().remove("labels");
            api.patch(&service, &PatchParams::apply(GITOPS), &Patch::Apply(&body))
                .await
                .unwrap();
        });
        let err = objects
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap_err();
        match (outcome, &err) {
            // The same fields: only the version the create names stops it.
            (
                "AlreadyExists",
                ObjectsError::AlreadyExists {
                    kind: Kind::AgentService,
                    name: n,
                },
            ) => {
                assert_eq!(*n, name);
            }
            // A field they own with another value: the server's conflict, and still nothing merged.
            ("Conflict", ObjectsError::Conflict(m)) => assert!(m.contains(GITOPS), "{m}"),
            _ => panic!("{outcome}: {err:?}"),
        }
        let theirs = Api::<DynamicObject>::namespaced_with(
            admin.clone(),
            NAMESPACE,
            &resource(Kind::AgentService),
        )
        .get(&name)
        .await
        .unwrap();
        assert!(
            theirs.metadata.labels.is_none(),
            "{outcome}: the label of the dashboard was merged into it"
        );
        assert_eq!(
            theirs.data["spec"]["description"],
            json!(their_description.unwrap_or("A sample agent.")),
            "{outcome}"
        );
    }
}

#[tokio::test]
async fn a_replace_never_brings_back_a_service_deleted_between_the_read_and_the_apply() {
    let Some((admin, dashboard)) = identities().await else {
        return;
    };
    let (client, hooks) = interpose(&dashboard, NAMESPACE);
    let objects = KubernetesObjects::new(client, NAMESPACE);
    let name = unique("ag");
    let draft = sample_agent(&name);
    let made = objects
        .put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    let api = Api::<DynamicObject>::namespaced_with(
        admin.clone(),
        NAMESPACE,
        &resource(Kind::AgentService),
    );
    let (gone, service) = (api.clone(), name.clone());
    hooks.before_the_next_apply("agentservices", &name, move || async move {
        gone.delete(&service, &DeleteParams::default())
            .await
            .unwrap();
    });
    let mut changed = draft.clone();
    changed.service["registry"]["title"] = json!("Late");
    let err = objects
        .put_agent(&changed, &Precondition::Matches(made.etag()))
        .await
        .unwrap_err();
    assert!(
        matches!(
            &err,
            ObjectsError::NotFound {
                kind: Kind::AgentService,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(
        api.get_opt(&name).await.unwrap().is_none(),
        "it stays deleted"
    );
}

#[tokio::test]
async fn a_config_the_operator_wrote_to_after_the_dry_run_goes_with_its_service() {
    let Some((admin, dashboard)) = identities().await else {
        return;
    };
    let (client, hooks) = interpose(&dashboard, NAMESPACE);
    let objects = KubernetesObjects::new(client, NAMESPACE);
    let name = unique("ag");
    let made = objects
        .put_agent(&sample_agent(&name), &Precondition::DoesNotExist)
        .await
        .unwrap();
    // The operator writes the status of the config after the dry runs, before the config is deleted.
    let configs = Api::<DynamicObject>::namespaced_with(
        admin.clone(),
        NAMESPACE,
        &resource(Kind::AgentConfig),
    );
    let (api, config) = (configs.clone(), name.clone());
    hooks.before_the_next_delete("agentconfigs", &name, move || async move {
        let body = json!({
            "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "AgentConfig",
            "metadata": { "name": config }, "status": { "observedGeneration": 2 },
        });
        api.patch_status(
            &config,
            &PatchParams::apply("aap-operator").force(),
            &Patch::Apply(&body),
        )
        .await
        .unwrap();
    });
    objects.delete_agent(&name, &made.etag()).await.unwrap();
    assert!(
        configs.get_opt(&name).await.unwrap().is_none(),
        "bound to its uid and not to its version"
    );
    assert!(
        Api::<DynamicObject>::namespaced_with(admin, NAMESPACE, &resource(Kind::AgentService))
            .get_opt(&name)
            .await
            .unwrap()
            .is_none()
    );
}
