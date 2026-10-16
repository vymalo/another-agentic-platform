//! What the fake API server copies of a real one, as assertions: **the same code runs against the fake
//! (`tests/api.rs`) and against a real API server (`tests/cluster.rs`)**, so that the fake cannot drift from what it
//! claims to be without a test of the real thing failing. They observe the API server with the administrator's
//! client of the harness, not the crate.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use aap_ports::testkit::{sample_agent, unique};
use aap_ports::{AgentObjects, Kind, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, Precondition};
use kube::api::{DeleteParams, Patch, PatchParams};
use serde_json::{Value, json};

use super::harness::{Harness, OPERATOR};

/// The finalizer the operator puts on a service before it makes anything (`aap_controller`).
pub const FINALIZER: &str = "agents.vymalo.com/runtime";

fn version(object: &kube::api::DynamicObject) -> String {
    object.metadata.resource_version.clone().unwrap()
}

/// A `DELETE` of an object that has a finalizer answers **the object**, with `deletionTimestamp`, and leaves it; the
/// version and the generation move on the first delete and not on the next; an apply of a terminating object is
/// accepted; and the object goes with its last finalizer. (*Observed* on a kube-apiserver v1.35.8, 2026-10-16.)
pub async fn a_delete_of_an_object_with_a_finalizer_leaves_it_terminating(h: &Harness) {
    let name = unique("ag");
    let draft = sample_agent(&name);
    h.put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    let services = h.raw(Kind::AgentService);
    let operator = PatchParams {
        field_manager: Some(OPERATOR.to_owned()),
        ..PatchParams::default()
    };
    services
        .patch(
            &name,
            &operator,
            &Patch::Merge(json!({ "metadata": { "finalizers": [FINALIZER] } })),
        )
        .await
        .unwrap();
    let before = services.get(&name).await.unwrap();
    assert!(before.metadata.deletion_timestamp.is_none());

    // A dry run answers what it would do, and does it not.
    let dry = services
        .delete(
            &name,
            &DeleteParams {
                dry_run: true,
                ..DeleteParams::default()
            },
        )
        .await
        .unwrap();
    assert!(
        dry.is_left(),
        "a delete of an object with a finalizer answers the object"
    );
    assert!(
        services
            .get(&name)
            .await
            .unwrap()
            .metadata
            .deletion_timestamp
            .is_none()
    );

    // The first delete.
    let first = services
        .delete(&name, &DeleteParams::default())
        .await
        .unwrap()
        .left()
        .expect("the object, not a Status");
    assert!(first.metadata.deletion_timestamp.is_some());
    let terminating = services.get(&name).await.unwrap();
    assert!(terminating.metadata.deletion_timestamp.is_some());
    assert_eq!(
        terminating.metadata.finalizers,
        Some(vec![FINALIZER.into()])
    );
    assert_ne!(version(&terminating), version(&before), "the version moved");
    assert_eq!(
        terminating.metadata.generation,
        before.metadata.generation.map(|g| g + 1),
        "and the generation"
    );

    // The next one moves nothing.
    let again = services
        .delete(&name, &DeleteParams::default())
        .await
        .unwrap()
        .left()
        .expect("the object again");
    assert_eq!(version(&again), version(&terminating));
    assert_eq!(
        services.get(&name).await.unwrap().metadata.generation,
        terminating.metadata.generation
    );

    // A terminating object can still be applied, and the config is untouched.
    let body = json!({
        "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "AgentService",
        "metadata": { "name": name, "resourceVersion": version(&terminating),
                      "labels": { MANAGED_BY_LABEL: MANAGED_BY_DASHBOARD } },
        "spec": draft.service,
    });
    services
        .patch(
            &name,
            &PatchParams::apply("agents.vymalo.com/dashboard"),
            &Patch::Apply(&body),
        )
        .await
        .expect("an apply of a terminating object is accepted");
    assert!(
        h.raw(Kind::AgentConfig)
            .get_opt(&name)
            .await
            .unwrap()
            .is_some()
    );

    // The finalizer runs: the object goes.
    services
        .patch(
            &name,
            &operator,
            &Patch::Merge(json!({ "metadata": { "finalizers": null } })),
        )
        .await
        .unwrap();
    assert!(services.get_opt(&name).await.unwrap().is_none());
    assert!(
        h.raw(Kind::AgentConfig)
            .get_opt(&name)
            .await
            .unwrap()
            .is_some()
    );
}

/// A stale `resourceVersion` is a `409`, **also when the body breaks a rule of the CRD** (a CEL rule is a `422` for a
/// current version): the version is checked first. A body the schema refuses is a `500` whatever the version is.
pub async fn a_stale_version_is_a_conflict_before_the_rules_of_the_crd(h: &Harness) {
    let name = unique("ag");
    let draft = sample_agent(&name);
    h.put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    let services = h.raw(Kind::AgentService);
    let current = version(&services.get(&name).await.unwrap());
    let apply = |spec: Value, version: &str| {
        let body = json!({
            "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "AgentService",
            "metadata": { "name": name, "resourceVersion": version,
                          "labels": { MANAGED_BY_LABEL: MANAGED_BY_DASHBOARD } },
            "spec": spec,
        });
        let services = services.clone();
        let name = name.clone();
        async move {
            services
                .patch(
                    &name,
                    &PatchParams::apply("agents.vymalo.com/dashboard").dry_run(),
                    &Patch::Apply(&body),
                )
                .await
        }
    };
    let code = |error: kube::Error| match error {
        kube::Error::Api(status) => status.code,
        other => panic!("not an answer of the API server: {other}"),
    };

    let mut cel = draft.service.clone();
    cel["access"]["audience"] = json!(["*", "agent.use:x"]);
    assert_eq!(
        code(apply(cel.clone(), "1").await.unwrap_err()),
        409,
        "a stale version and a CEL rule"
    );
    assert_eq!(
        code(apply(cel, &current).await.unwrap_err()),
        422,
        "a current version and a CEL rule"
    );
    let mut typed = draft.service.clone();
    typed["suspend"] = json!("yes");
    assert_eq!(
        code(apply(typed, "1").await.unwrap_err()),
        500,
        "a stale version and a body the schema refuses"
    );
}

/// A `POST` stores the defaults of the schema, as an apply does.
pub async fn a_create_stores_the_defaults_of_the_schema(h: &Harness) {
    let name = unique("ag");
    let draft = sample_agent(&name);
    let body: kube::api::DynamicObject = serde_json::from_value(json!({
        "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "AgentService",
        "metadata": { "name": name }, "spec": draft.service,
    }))
    .unwrap();
    let made = h
        .raw(Kind::AgentService)
        .create(&kube::api::PostParams::default(), &body)
        .await
        .unwrap();
    assert_eq!(made.data["spec"]["suspend"], json!(false));
    assert_eq!(made.data["spec"]["deletionPolicy"], json!("Retain"));
    let stored = h.raw(Kind::AgentService).get(&name).await.unwrap();
    assert_eq!(stored.data["spec"]["suspend"], json!(false));
    assert_eq!(
        stored.data["spec"]["scaling"],
        json!({ "topology": "combined", "workers": 1 })
    );
    // The defaults an update caused are its own: the dashboard reads them as written by that manager.
    let seen = h.get_agent(&name).await.unwrap().unwrap();
    assert_eq!(seen.service.spec["suspend"], json!(false));
    h.raw(Kind::AgentService)
        .delete(&name, &DeleteParams::default())
        .await
        .unwrap();
}
