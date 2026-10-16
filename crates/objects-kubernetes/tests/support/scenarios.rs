//! What a delete does while the operator is finalizing the service, as scenarios that **run against the fake API
//! server (`tests/api.rs`) and against a real one (`tests/cluster.rs`) by the same code**. The operator is played
//! by the harness: it puts its finalizer on the service, reads the config when it finalizes (as `cleanup` of
//! `aap-controller` does to honour `deletionPolicy`), and removes the finalizer.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

use aap_ports::testkit::{Foreign, ObjectsUnderTest, sample_agent, unique};
use aap_ports::{AgentObjects, Kind, ObjectsError, Precondition};
use serde_json::{Value, json};

use super::harness::{DELETE_WAIT, FINALIZER, Harness};

/// An `AgentService` spec that names the config `config`.
fn service_naming(name: &str, config: &str) -> Value {
    let mut service = sample_agent(name).service;
    service["configRef"]["name"] = json!(config);
    service
}

/// Whether the config of the agent `name` is there, read as an administrator.
async fn config_is_there(h: &Harness, name: &str) -> bool {
    h.raw(Kind::AgentConfig)
        .get_opt(name)
        .await
        .unwrap()
        .is_some()
}

/// Whether the service of the agent `name` is there, read as an administrator.
async fn service_is_there(h: &Harness, name: &str) -> bool {
    h.raw(Kind::AgentService)
        .get_opt(name)
        .await
        .unwrap()
        .is_some()
}

/// Wait until the service `name` is being deleted (it has a `deletionTimestamp`): the moment the operator would start.
/// What the operator does next is done after this, never at a time chosen in advance: the requests of a delete take as
/// long as the API server takes, which grows with the objects an earlier run left.
async fn wait_until_terminating(h: &Harness, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let found = h.raw(Kind::AgentService).get_opt(name).await.unwrap();
        if found.is_some_and(|s| s.metadata.deletion_timestamp.is_some()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the service {name} never began to be deleted"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// **The data-loss case.** A person saves `deletionPolicy: Retain` over `Delete` and deletes the agent in one
/// breath, the operator not having reconciled in between. The operator's finalizer honours the policy it can read:
/// it needs the config (a config it cannot find leaves the remembered `Delete` standing, and the volumes and the
/// database go). So the config is there for as long as the service is, and goes after it.
pub async fn the_config_stays_until_the_service_is_gone(h: &Harness) {
    let name = unique("ag");
    let mut draft = sample_agent(&name);
    draft.service["deletionPolicy"] = json!("Delete");
    h.put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    // The operator has made the compute under `Delete`, and put its finalizer on.
    h.set_finalizers(&name, json!([FINALIZER])).await;
    let seen = h.get_agent(&name).await.unwrap().unwrap();
    draft.service["deletionPolicy"] = json!("Retain");
    let saved = h
        .put_agent(&draft, &Precondition::Matches(seen.etag()))
        .await
        .unwrap();

    let started = Instant::now();
    h.delete_agent(&name, &saved.etag())
        .await
        .expect("the delete is accepted while the operator is finalizing");
    assert!(
        started.elapsed() >= DELETE_WAIT,
        "the delete waited for the operator, for at most the bound: {:?}",
        started.elapsed()
    );

    // The service is being deleted, and the config is there for the operator to read.
    let terminating = h.raw(Kind::AgentService).get(&name).await.unwrap();
    assert!(terminating.metadata.deletion_timestamp.is_some());
    assert_eq!(
        terminating.data["spec"]["deletionPolicy"],
        json!("Retain"),
        "the policy the person saved"
    );
    let agent = h.get_agent(&name).await.unwrap().unwrap();
    assert!(agent.deleting());
    assert_eq!(
        agent.config.as_ref().map(|c| &c.spec),
        Some(&draft.config),
        "the config the operator reads when it finalizes is there"
    );
    assert!(
        matches!(
            h.put_agent(&draft, &Precondition::Matches(agent.etag()))
                .await,
            Err(ObjectsError::Deleting {
                kind: Kind::AgentService,
                ..
            })
        ),
        "nothing is written to it meanwhile"
    );

    // The operator finalizes: it reads the config, and the service goes.
    assert!(
        config_is_there(h, &name).await,
        "the operator finds its config"
    );
    h.run_finalizer(&name).await;
    assert!(!service_is_there(h, &name).await);
    assert!(config_is_there(h, &name).await, "the config is left");

    // The config is an orphan, and a later delete of the name removes it.
    let orphan = h
        .list_orphans()
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.name == name)
        .expect("the config is listed as an orphan");
    h.delete_agent(&name, &orphan.etag()).await.unwrap();
    assert!(!config_is_there(h, &name).await);
    assert_eq!(h.get_agent(&name).await.unwrap(), None);
}

/// A delete that the operator finishes while the delete waits answers with the agent gone: the config goes too, and
/// the delete does not wait for the bound.
pub async fn a_delete_the_operator_finishes_in_time_removes_the_config(h: &Harness) {
    let name = unique("ag");
    let draft = sample_agent(&name);
    h.put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    h.set_finalizers(&name, json!([FINALIZER])).await;
    let agent = h.get_agent(&name).await.unwrap().unwrap();

    let objects = h.objects.clone().with_delete_wait(Duration::from_secs(20));
    // The operator reacts to the deletion: it finalizes once the service is being deleted, a moment later.
    let operator = {
        let (h, name) = (h.clone(), name.clone());
        tokio::spawn(async move {
            wait_until_terminating(&h, &name).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
            h.run_finalizer(&name).await;
        })
    };
    let started = Instant::now();
    objects.delete_agent(&name, &agent.etag()).await.unwrap();
    operator.await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the delete returned when the service had gone, not at the bound: {:?}",
        started.elapsed()
    );
    assert!(!service_is_there(h, &name).await);
    assert!(!config_is_there(h, &name).await, "the config went with it");
    assert_eq!(h.get_agent(&name).await.unwrap(), None);
    assert!(
        !h.list_orphans()
            .await
            .unwrap()
            .iter()
            .any(|o| o.name == name)
    );
}

/// The services are listed again right before the config is deleted: a service made while the delete waited that
/// names the config keeps it.
pub async fn a_service_made_meanwhile_keeps_the_config(h: &Harness) {
    let name = unique("ag");
    let draft = sample_agent(&name);
    h.put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    h.set_finalizers(&name, json!([FINALIZER])).await;
    let agent = h.get_agent(&name).await.unwrap().unwrap();

    // While the delete waits for the operator, somebody makes another service that names the config, and then the
    // operator is done.
    let other = unique("reader");
    let objects = h.objects.clone().with_delete_wait(Duration::from_secs(20));
    let meanwhile = {
        let (h, name, other) = (h.clone(), name.clone(), other.clone());
        tokio::spawn(async move {
            wait_until_terminating(&h, &name).await;
            h.seed_foreign(
                Kind::AgentService,
                &other,
                service_naming(&other, &name),
                Foreign::Unlabelled,
            )
            .await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            h.run_finalizer(&name).await;
        })
    };
    objects.delete_agent(&name, &agent.etag()).await.unwrap();
    meanwhile.await.unwrap();
    assert!(!service_is_there(h, &name).await, "the service went");
    assert!(
        config_is_there(h, &name).await,
        "the config is named by another service now: it stays"
    );
    assert_eq!(
        h.get_agent(&other)
            .await
            .unwrap()
            .unwrap()
            .config
            .map(|c| c.name),
        Some(name),
        "and that service has its config"
    );
}
