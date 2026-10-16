//! The crate against a fake API server: the conformance suite of `aap-ports`, and what the suite cannot see of
//! the requests the crate makes. The same suite runs against a real API server in `tests/cluster.rs`.
//!
//! The fake behaves as a kube-apiserver v1.35.8 does where the crate depends on it (its header says where), and
//! its refusals are the `Status` bodies captured from one (`tests/fixtures`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use aap_objects_kubernetes::{FIELD_MANAGER, KubernetesObjects, NEVER};
use aap_ports::testkit::{ObjectsUnderTest, sample_agent, sample_shared, unique};
use aap_ports::{
    AgentDraft, AgentObjects, Kind, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, ObjectsError,
    Precondition, SharedKind,
};
use serde_json::{Value, json};
use support::facts;
use support::harness::Harness;
use support::scenarios;
use support::{Call, Fake};

async fn make() -> Option<Harness> {
    Some(Harness::on_fake(&Fake::new()))
}

aap_ports::agent_objects_conformance!(make);

// ------------------------------------------------------------------ helpers

fn setup() -> (Fake, Harness) {
    let fake = Fake::new();
    let h = Harness::on_fake(&fake);
    (fake, h)
}

fn retitled(draft: &AgentDraft, title: &str) -> AgentDraft {
    let mut d = draft.clone();
    d.service["registry"]["title"] = json!(title);
    d
}

fn reconfigured(draft: &AgentDraft, model: &str) -> AgentDraft {
    let mut d = draft.clone();
    d.config["model"]["model"] = json!(model);
    d
}

/// What the crate asked of the API server.
fn asked(fake: &Fake) -> Vec<Call> {
    fake.calls()
        .into_iter()
        .filter(|c| c.who == "dashboard")
        .collect()
}

/// The calls that changed something, for real.
fn changes(fake: &Fake) -> Vec<Call> {
    asked(fake).into_iter().filter(Call::changes).collect()
}

async fn made(h: &Harness) -> (String, AgentDraft, aap_ports::Agent) {
    let name = unique("ag");
    let draft = sample_agent(&name);
    let agent = h
        .put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    (name, draft, agent)
}

// ------------------------------------------------------------------ the fake is what it claims to be

// The same assertions run against a real API server in `tests/cluster.rs`.

#[tokio::test]
async fn the_fake_leaves_a_deleted_object_with_a_finalizer_terminating_as_a_real_server_does() {
    let (_fake, h) = setup();
    facts::a_delete_of_an_object_with_a_finalizer_leaves_it_terminating(&h).await;
}

#[tokio::test]
async fn the_fake_checks_the_version_before_the_rules_of_the_crd_as_a_real_server_does() {
    let (_fake, h) = setup();
    facts::a_stale_version_is_a_conflict_before_the_rules_of_the_crd(&h).await;
}

#[tokio::test]
async fn the_fake_stores_the_defaults_of_a_post_as_a_real_server_does() {
    let (_fake, h) = setup();
    facts::a_create_stores_the_defaults_of_the_schema(&h).await;
}

// ------------------------------------------------------------------ atomicity

#[tokio::test]
async fn a_replace_is_two_dry_runs_and_then_two_applies_the_config_first() {
    let (fake, h) = setup();
    let (name, draft, agent) = made(&h).await;
    fake.clear_calls();
    let changed = reconfigured(&retitled(&draft, "Changed"), "another-model");
    h.put_agent(&changed, &Precondition::Matches(agent.etag()))
        .await
        .unwrap();

    let patches: Vec<Call> = asked(&fake)
        .into_iter()
        .filter(|c| c.method == "PATCH")
        .collect();
    let shape: Vec<(&str, bool)> = patches.iter().map(|c| (c.plural(), c.dry_run())).collect();
    assert_eq!(
        shape,
        [
            ("agentconfigs", true),
            ("agentservices", true),
            ("agentconfigs", false),
            ("agentservices", false)
        ],
        "dry runs of both, then the applies, the config before the service"
    );
    for patch in &patches {
        assert_eq!(patch.name(), Some(name.as_str()));
        assert_eq!(patch.query["fieldManager"], FIELD_MANAGER);
        assert!(
            !patch.query.contains_key("force"),
            "an apply is never forced: {:?}",
            patch.query
        );
        assert!(patch.is_apply());
    }
}

#[tokio::test]
async fn every_apply_names_the_version_and_the_uid_that_were_read_and_a_create_names_a_version_nobody_has()
 {
    let (fake, h) = setup();
    let name = unique("ag");
    let draft = sample_agent(&name);
    h.put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap();
    let creates: Vec<Call> = asked(&fake)
        .into_iter()
        .filter(|c| c.method == "PATCH")
        .collect();
    assert_eq!(creates.len(), 4);
    for c in &creates {
        let meta = &c.body.as_ref().unwrap()["metadata"];
        assert_eq!(meta["resourceVersion"], json!(NEVER), "a create: {meta}");
        assert!(meta.get("uid").is_none(), "{meta}");
    }

    let current = h.get_agent(&name).await.unwrap().unwrap();
    fake.clear_calls();
    h.put_agent(
        &retitled(&draft, "Again"),
        &Precondition::Matches(current.etag()),
    )
    .await
    .unwrap();
    for c in asked(&fake).into_iter().filter(|c| c.method == "PATCH") {
        let meta = &c.body.as_ref().unwrap()["metadata"];
        let stored = fake.stored(c.plural(), &name).unwrap();
        // The service changed; the version read is the one the apply names, for the dry run and the real one.
        if c.plural() == "agentconfigs" {
            assert_eq!(
                meta["resourceVersion"], stored["metadata"]["resourceVersion"],
                "{meta}"
            );
        }
        assert_eq!(meta["uid"], stored["metadata"]["uid"], "{meta}");
        assert_ne!(meta["resourceVersion"], json!(NEVER));
    }
}

#[tokio::test]
async fn what_is_written_is_the_label_and_the_spec_and_nothing_else() {
    let (fake, h) = setup();
    // The whole suite, on one server: every apply the crate made.
    run_the_suite(&h).await;
    let applies: Vec<Call> = asked(&fake).into_iter().filter(|c| c.is_apply()).collect();
    assert!(applies.len() > 50, "{}", applies.len());
    for call in applies {
        assert_eq!(call.query["fieldManager"], FIELD_MANAGER);
        assert!(!call.query.contains_key("force"), "{call:?}");
        assert!(
            !call.is_subresource(),
            "the status is the operator's: {call:?}"
        );
        let body = call.body.as_ref().unwrap();
        let mut top: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        top.sort_unstable();
        assert_eq!(
            top,
            ["apiVersion", "kind", "metadata", "spec"],
            "no status, no more"
        );
        let meta = body["metadata"].as_object().unwrap();
        assert!(
            meta.keys()
                .all(|k| ["name", "labels", "resourceVersion", "uid"].contains(&k.as_str())),
            "{meta:?}"
        );
        assert_eq!(
            meta["labels"],
            json!({ MANAGED_BY_LABEL: MANAGED_BY_DASHBOARD })
        );
    }
}

/// Every case of the suite, one after the other, on one harness.
async fn run_the_suite(h: &Harness) {
    use aap_ports::testkit::objects::cases;
    cases::an_agent_is_created_and_read_back(h.clone()).await;
    cases::a_second_create_is_refused_and_changes_nothing(h.clone()).await;
    cases::a_replace_with_the_current_token_changes_the_agent(h.clone()).await;
    cases::a_stale_token_is_refused_and_nothing_changes(h.clone()).await;
    cases::a_refused_write_changes_neither_object(h.clone()).await;
    cases::a_change_of_either_object_makes_the_token_stale(h.clone()).await;
    cases::a_token_is_for_the_object_it_was_read_from(h.clone()).await;
    cases::a_replace_of_an_agent_that_is_not_there_is_not_found(h.clone()).await;
    cases::a_body_that_is_not_acceptable_is_invalid_with_fields(h.clone()).await;
    cases::an_agent_is_deleted_with_its_config(h.clone()).await;
    cases::a_delete_holds_the_token_it_is_given(h.clone()).await;
    cases::a_gitops_agent_is_read_and_never_written(h.clone()).await;
    cases::a_gitops_config_blocks_the_create_of_its_name(h.clone()).await;
    cases::a_pair_with_one_gitops_object_is_gitops(h.clone()).await;
    cases::a_create_after_a_half_failed_create_takes_the_config_over(h.clone()).await;
    cases::agents_are_listed_by_name_and_are_independent(h.clone()).await;
    cases::an_agent_whose_config_is_missing_is_listed_and_read(h.clone()).await;
    cases::an_agent_whose_config_another_service_names_is_left_alone(h.clone()).await;
    cases::shared_objects_have_the_same_life(h.clone()).await;
    cases::shared_objects_are_listed_by_name(h.clone()).await;
    cases::a_gitops_shared_object_is_read_and_never_written(h.clone()).await;
    cases::a_shared_object_that_agents_name_is_not_deleted(h.clone()).await;
    cases::a_shared_object_nobody_names_is_deleted(h.clone()).await;
    cases::a_write_does_not_check_what_the_agent_names(h.clone()).await;
    cases::the_status_is_the_operators_and_is_read_back(h.clone()).await;
    cases::offered_secrets_are_references_and_never_values(h.clone()).await;
    cases::no_read_carries_a_secret_value(h.clone()).await;
    cases::an_unreachable_store_is_unavailable(h.clone()).await;
}

/// The two objects of an agent as stored, to show that a refusal did not touch them.
fn stored_pair(fake: &Fake, name: &str) -> (Option<Value>, Option<Value>) {
    (
        fake.stored("agentservices", name),
        fake.stored("agentconfigs", name),
    )
}

/// Nothing was changed for real since the calls were cleared, and the agent is as it was.
fn untouched(fake: &Fake, name: &str, before: &(Option<Value>, Option<Value>), what: &str) {
    let writes = changes(fake);
    assert!(writes.is_empty(), "{what}: {writes:?}");
    assert_eq!(
        &stored_pair(fake, name),
        before,
        "{what}: an object was touched"
    );
    fake.clear_calls();
}

#[tokio::test]
async fn a_refusal_makes_no_write_of_any_kind() {
    let (fake, h) = setup();
    let (name, draft, agent) = made(&h).await;
    // A change of both objects, which each refusal below must leave undone.
    let changed = reconfigured(&retitled(&draft, "Changed"), "another-model");
    let before = stored_pair(&fake, &name);
    fake.clear_calls();

    // A rule of the service, with a change of the config that comes first in the order of writes.
    let mut broken = changed.clone();
    broken.service["access"]["audience"] = json!(["*", "agent.use:x"]);
    let err = h
        .put_agent(&broken, &Precondition::Matches(agent.etag()))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ObjectsError::Invalid(f) if f.iter().any(|f| f.kind == Kind::AgentService && f.path == "spec.access.audience")),
        "{err:?}"
    );
    untouched(&fake, &name, &before, "a CEL rule of the service");

    // A rule of the config, with a change of the service.
    let mut broken = changed.clone();
    broken.config["model"]["endpointRef"] = json!({ "name": "gateway" });
    let err = h
        .put_agent(&broken, &Precondition::Matches(agent.etag()))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ObjectsError::Invalid(f) if f.iter().any(|f| f.kind == Kind::AgentConfig)),
        "{err:?}"
    );
    untouched(&fake, &name, &before, "a CEL rule of the config");

    // A body the schema refuses, in the service.
    let mut broken = changed.clone();
    broken.service["suspend"] = json!("yes");
    let err = h
        .put_agent(&broken, &Precondition::Matches(agent.etag()))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ObjectsError::Invalid(f) if f.iter().any(|f| f.path == "spec.suspend")),
        "{err:?}"
    );
    untouched(&fake, &name, &before, "a typed patch the schema refuses");

    // A token that is stale on the config only, and on the service only.
    for kind in [Kind::AgentConfig, Kind::AgentService] {
        let (name, draft, agent) = made(&h).await;
        h.set_status(
            kind,
            &name,
            json!({ "state": "Ready", "observedGeneration": 1 }),
        )
        .await;
        let before = stored_pair(&fake, &name);
        fake.clear_calls();
        let err = h
            .put_agent(
                &reconfigured(&retitled(&draft, "Late"), "late-model"),
                &Precondition::Matches(agent.etag()),
            )
            .await
            .unwrap_err();
        assert!(
            matches!(err, ObjectsError::PreconditionFailed),
            "{kind}: {err:?}"
        );
        untouched(
            &fake,
            &name,
            &before,
            &format!("a token stale on the {kind}"),
        );
    }

    // A field another manager owns, in the service: found by the dry run, before the config is written.
    let (name, draft, _) = made(&h).await;
    fake.edit(|s| {
        s.merge_as(
            "kubectl-edit",
            "agentservices",
            &name,
            json!({ "spec": { "registry": { "title": "By hand" } } }),
        );
    });
    let seen = h.get_agent(&name).await.unwrap().unwrap();
    let before = stored_pair(&fake, &name);
    fake.clear_calls();
    let err = h
        .put_agent(
            &reconfigured(&retitled(&draft, "By the dashboard"), "other"),
            &Precondition::Matches(seen.etag()),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ObjectsError::Conflict(m) if m.contains("kubectl-edit")),
        "{err:?}"
    );
    untouched(&fake, &name, &before, "a field another manager owns");

    // A delete that is refused, of each kind.
    let (name, _, agent) = made(&h).await;
    h.set_status(Kind::AgentConfig, &name, json!({ "observedGeneration": 1 }))
        .await;
    let before = stored_pair(&fake, &name);
    fake.clear_calls();
    let err = h.delete_agent(&name, &agent.etag()).await.unwrap_err();
    assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
    untouched(&fake, &name, &before, "a delete with a stale token");
}

// ------------------------------------------------------------------ the windows that remain

#[tokio::test]
async fn when_the_service_changes_between_the_two_applies_the_config_stays_written_and_a_retry_succeeds()
 {
    let (fake, h) = setup();
    let (name, draft, agent) = made(&h).await;
    let changed = reconfigured(&retitled(&draft, "Changed"), "another-model");
    // Another administrator saves the service through the dashboard after the dry runs, before this one applies it.
    let theirs = {
        let mut d = draft.clone();
        d.service["description"] = json!("theirs");
        d.service
    };
    let service = name.clone();
    fake.before_the_next_real_apply("agentservices", &name, move |s| {
        let body = json!({
            "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "AgentService",
            "metadata": { "name": service, "labels": { MANAGED_BY_LABEL: MANAGED_BY_DASHBOARD } }, "spec": theirs,
        });
        s.apply_as(FIELD_MANAGER, false, "agentservices", &service, body);
    });
    let err = h
        .put_agent(&changed, &Precondition::Matches(agent.etag()))
        .await
        .unwrap_err();
    assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
    // The one place a refusal changes something: the config was written first, and the service is theirs.
    let config = fake.stored("agentconfigs", &name).unwrap();
    assert_eq!(config["spec"]["model"]["model"], json!("another-model"));
    let service = fake.stored("agentservices", &name).unwrap();
    assert_eq!(service["spec"]["description"], json!("theirs"));
    assert_ne!(service["spec"]["registry"]["title"], json!("Changed"));

    // A retry reads again and succeeds.
    let seen = h.get_agent(&name).await.unwrap().unwrap();
    let saved = h
        .put_agent(&changed, &Precondition::Matches(seen.etag()))
        .await
        .unwrap();
    assert_eq!(saved.service.spec["registry"]["title"], json!("Changed"));
}

#[tokio::test]
async fn a_create_never_merges_into_a_service_made_between_the_read_and_the_apply() {
    for (their_description, outcome) in [
        (None, "AlreadyExists"),
        (Some("made by somebody else"), "Conflict"),
    ] {
        let (fake, h) = setup();
        let name = unique("ag");
        let draft = sample_agent(&name);
        let mut theirs = draft.service.clone();
        if let Some(d) = their_description {
            theirs["description"] = json!(d);
        }
        let service = name.clone();
        fake.before_the_next_real_apply("agentservices", &name, move |s| {
            let body = json!({
                "apiVersion": "agents.vymalo.com/v1alpha1", "kind": "AgentService",
                "metadata": { "name": service }, "spec": theirs,
            });
            s.apply_as("gitops-author", false, "agentservices", &service, body);
        });
        let err = h
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap_err();
        match (outcome, &err) {
            // The same fields: only the version named by the create stops it.
            (
                "AlreadyExists",
                ObjectsError::AlreadyExists {
                    kind: Kind::AgentService,
                    name: n,
                },
            ) => assert_eq!(*n, name),
            // A field they own, with another value: the server's conflict, and still nothing merged.
            ("Conflict", ObjectsError::Conflict(m)) => assert!(m.contains("gitops-author"), "{m}"),
            _ => panic!("{outcome}: {err:?}"),
        }
        // Theirs, as they made it: no label of the dashboard was merged into it.
        let service = fake.stored("agentservices", &name).unwrap();
        assert!(
            service["metadata"].get("labels").is_none(),
            "{outcome}: {service}"
        );
        assert_eq!(
            service["spec"]["description"],
            json!(their_description.unwrap_or("A sample agent."))
        );
    }
}

#[tokio::test]
async fn a_replace_never_brings_back_a_service_deleted_between_the_read_and_the_apply() {
    let (fake, h) = setup();
    let (name, draft, agent) = made(&h).await;
    let service = name.clone();
    fake.before_the_next_real_apply("agentservices", &name, move |s| {
        s.remove("agentservices", &service)
    });
    let err = h
        .put_agent(
            &retitled(&draft, "Late"),
            &Precondition::Matches(agent.etag()),
        )
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
        fake.stored("agentservices", &name).is_none(),
        "it stays deleted"
    );
}

// ------------------------------------------------------------------ deletes

#[tokio::test]
async fn a_delete_is_two_dry_runs_then_the_service_and_then_the_config_bound_to_what_was_read() {
    let (fake, h) = setup();
    let (name, _, agent) = made(&h).await;
    let service = fake.stored("agentservices", &name).unwrap();
    let config = fake.stored("agentconfigs", &name).unwrap();
    fake.clear_calls();
    h.delete_agent(&name, &agent.etag()).await.unwrap();

    let deletes: Vec<Call> = asked(&fake)
        .into_iter()
        .filter(|c| c.method == "DELETE")
        .collect();
    let shape: Vec<(&str, bool)> = deletes.iter().map(|c| (c.plural(), c.dry_run())).collect();
    assert_eq!(
        shape,
        [
            ("agentconfigs", true),
            ("agentservices", true),
            ("agentservices", false),
            ("agentconfigs", false)
        ]
    );
    let pre = |c: &Call| c.body.as_ref().unwrap()["preconditions"].clone();
    assert_eq!(
        pre(&deletes[1])["resourceVersion"],
        service["metadata"]["resourceVersion"]
    );
    assert_eq!(
        pre(&deletes[2])["resourceVersion"],
        service["metadata"]["resourceVersion"]
    );
    assert_eq!(pre(&deletes[2])["uid"], service["metadata"]["uid"]);
    // The config goes with its service whatever the operator has written to it since: its uid, not its version.
    assert_eq!(pre(&deletes[3])["uid"], config["metadata"]["uid"]);
    assert!(
        pre(&deletes[3]).get("resourceVersion").is_none(),
        "{}",
        pre(&deletes[3])
    );
}

#[tokio::test]
async fn a_config_the_operator_wrote_to_after_the_dry_run_goes_with_its_service() {
    let (fake, h) = setup();
    let (name, _, agent) = made(&h).await;
    // The operator writes the status of the config after the dry runs, before the config is deleted.
    let config = name.clone();
    fake.before_the_next_delete_of("agentservices", &name, move |s| {
        s.apply_status_as(
            "aap-operator",
            "agentconfigs",
            &config,
            json!({ "observedGeneration": 2 }),
        );
    });
    h.delete_agent(&name, &agent.etag()).await.unwrap();
    assert!(fake.stored("agentservices", &name).is_none());
    assert!(
        fake.stored("agentconfigs", &name).is_none(),
        "the config is bound to its uid and not to its version"
    );
}

// ------------------------------------------------------------------ a service the operator is finalizing

// The same scenarios run against a real API server in `tests/cluster.rs`.

#[tokio::test]
async fn the_config_stays_until_the_service_is_gone() {
    let (_fake, h) = setup();
    scenarios::the_config_stays_until_the_service_is_gone(&h).await;
}

#[tokio::test]
async fn a_delete_the_operator_finishes_in_time_removes_the_config() {
    let (_fake, h) = setup();
    scenarios::a_delete_the_operator_finishes_in_time_removes_the_config(&h).await;
}

#[tokio::test]
async fn a_service_made_meanwhile_keeps_the_config() {
    let (_fake, h) = setup();
    scenarios::a_service_made_meanwhile_keeps_the_config(&h).await;
}

// ------------------------------------------------------------------ answers of the server

#[tokio::test]
async fn an_outage_is_unavailable_a_missing_kind_too_and_a_bug_is_internal() {
    let (fake, h) = setup();
    for code in [401, 403, 429, 500, 503] {
        fake.fail_with(Some(code));
        let err = h.list_agents().await.unwrap_err();
        assert!(
            matches!(err, ObjectsError::Unavailable(_)),
            "{code}: {err:?}"
        );
        let err = h.offered_secrets().await.unwrap_err();
        assert!(
            matches!(err, ObjectsError::Unavailable(_)),
            "{code}: {err:?}"
        );
    }
    fake.fail_with(Some(418));
    let err = h.get_agent("x").await.unwrap_err();
    assert!(matches!(err, ObjectsError::Internal(_)), "{err:?}");
    fake.fail_with(None);

    // The CRDs are not installed: the server answers the plain text `404 page not found`.
    fake.serve("agentservices", false);
    let err = h.get_agent("x").await.unwrap_err();
    assert!(matches!(err, ObjectsError::Unavailable(_)), "{err:?}");
    assert!(err.to_string().contains("unavailable"), "{err}");
    let err = h
        .put_agent(&sample_agent("x"), &Precondition::DoesNotExist)
        .await
        .unwrap_err();
    assert!(matches!(err, ObjectsError::Unavailable(_)), "{err:?}");
}

#[tokio::test]
async fn a_body_the_schema_refuses_is_invalid_with_the_fields_and_every_mistake_is_reported() {
    let (_fake, h) = setup();
    let name = unique("ag");
    let mut draft = sample_agent(&name);
    draft.service["suspend"] = json!("yes");
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
        (Kind::AgentService, "spec.suspend")
    );
    assert_eq!(
        fields[0].message, "expected boolean",
        "the value is not repeated"
    );

    // Our mistake and the server's, in one answer.
    let mut draft = sample_agent(&name);
    draft.service["configRef"]["name"] = json!("somebody-else");
    draft.service["access"]["audience"] = json!(["*", "x"]);
    draft.config["model"]["endpointRef"] = json!({ "name": "gateway" });
    let err = h
        .put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap_err();
    let ObjectsError::Invalid(fields) = &err else {
        panic!("{err:?}")
    };
    let seen: Vec<(Kind, &str)> = fields.iter().map(|f| (f.kind, f.path.as_str())).collect();
    assert_eq!(
        seen,
        [
            (Kind::AgentService, "spec.configRef.name"),
            (Kind::AgentConfig, "spec.model"),
            (Kind::AgentService, "spec.access.audience"),
        ]
    );
}

#[tokio::test]
async fn a_spec_that_is_not_an_object_is_refused_without_a_request() {
    let (fake, h) = setup();
    fake.clear_calls();
    let mut draft = sample_agent("x");
    draft.config = json!("a spec is an object");
    let err = h
        .put_agent(&draft, &Precondition::DoesNotExist)
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ObjectsError::Invalid(f) if f.len() == 1 && f[0].kind == Kind::AgentConfig && f[0].path == "spec"),
        "{err:?}"
    );
    let mut shared = sample_shared(SharedKind::Model, "x");
    shared.spec = json!([1]);
    let err = h
        .put_shared(SharedKind::Model, &shared, &Precondition::DoesNotExist)
        .await
        .unwrap_err();
    assert!(matches!(err, ObjectsError::Invalid(_)), "{err:?}");
    assert!(asked(&fake).is_empty(), "{:?}", asked(&fake));
}

// ------------------------------------------------------------------ secrets

#[tokio::test]
async fn nothing_reads_a_secret_and_the_offer_is_read_with_a_list_of_the_labelled_external_secrets()
{
    let (fake, h) = setup();
    run_the_suite(&h).await;
    let reads: Vec<Call> = fake
        .calls()
        .into_iter()
        .filter(|c| c.plural() == "secrets" && matches!(c.method.as_str(), "GET" | "WATCH"))
        .collect();
    assert!(reads.is_empty(), "a Secret was read: {reads:?}");
    let dashboard: Vec<Call> = asked(&fake)
        .into_iter()
        .filter(|c| c.plural() == "secrets")
        .collect();
    assert!(
        dashboard.is_empty(),
        "the dashboard asked about a Secret: {dashboard:?}"
    );

    let offers: Vec<Call> = asked(&fake)
        .into_iter()
        .filter(|c| c.plural() == "externalsecrets")
        .collect();
    assert!(!offers.is_empty());
    for call in offers {
        assert_eq!(call.method, "GET");
        assert_eq!(call.name(), None, "a list, never a get");
        assert_eq!(call.query["labelSelector"], "agents.vymalo.com/offer=true");
    }
}

#[tokio::test]
async fn the_offer_label_is_the_deployments_and_without_the_operator_nothing_is_offered() {
    let (fake, h) = setup();
    let secret = unique("sec");
    h.offer(&secret, "KEY", "es").await;
    let mine =
        |o: Vec<aap_ports::OfferedSecret>| o.into_iter().filter(|o| o.secret == secret).count();
    assert_eq!(mine(h.offered_secrets().await.unwrap()), 1);

    // Another label: the same ExternalSecrets do not carry it.
    let other = KubernetesObjects::new(fake.client_as("dashboard"), "aap-test")
        .with_offer_label("example.com/offered");
    assert_eq!(mine(other.offered_secrets().await.unwrap()), 0);

    // No External Secrets Operator: nothing is offered, and it is not an error.
    fake.serve("externalsecrets", false);
    assert!(h.offered_secrets().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_name_that_is_not_a_name_never_reaches_a_request_path() {
    let (fake, h) = setup();
    fake.clear_calls();
    let long = "x".repeat(254);
    for bad in [
        "x?watch=true",
        "a/b",
        "UPPER",
        "",
        "-x",
        "x-",
        "a..b",
        ".x",
        "a b",
        long.as_str(),
    ] {
        assert_eq!(h.get_agent(bad).await.unwrap(), None, "{bad:?}");
        assert_eq!(h.get_shared(SharedKind::Model, bad).await.unwrap(), None);
        let err = h
            .delete_agent(bad, &aap_ports::ETag::new("1"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ObjectsError::NotFound { .. }),
            "{bad:?}: {err:?}"
        );
        let err = h
            .delete_shared(SharedKind::ToolServer, bad, &aap_ports::ETag::new("1"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ObjectsError::NotFound { .. }),
            "{bad:?}: {err:?}"
        );
        let mut draft = sample_agent(bad);
        draft.service["configRef"]["name"] = json!(bad);
        let err = h
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ObjectsError::Invalid(f) if f.iter().any(|f| f.path == "metadata.name")),
            "{bad:?}: {err:?}"
        );
        let err = h
            .put_shared(
                SharedKind::Model,
                &sample_shared(SharedKind::Model, bad),
                &Precondition::DoesNotExist,
            )
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ObjectsError::Invalid(f) if f.iter().any(|f| f.path == "metadata.name")),
            "{bad:?}: {err:?}"
        );
    }
    assert!(
        asked(&fake).is_empty(),
        "a request was made: {:?}",
        asked(&fake)
    );

    // The names the dashboard uses are names.
    for good in [
        "chat",
        "coder-me",
        "coder.vymalo",
        "a1",
        "x".repeat(253).as_str(),
    ] {
        assert_eq!(h.get_agent(good).await.unwrap(), None);
    }
    assert_eq!(
        asked(&fake).len(),
        5,
        "one request each: the service, which is not there"
    );
}
