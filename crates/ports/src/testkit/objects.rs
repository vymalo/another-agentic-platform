//! Cases of the `AgentObjects` suite, and the samples they write. The Kubernetes implementation (D5) is
//! held to exactly these.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::*;
use crate::{
    ARGO_TRACKING_ANNOTATION, Agent, AgentDraft, ETag, Kind, MANAGED_BY_DASHBOARD,
    MANAGED_BY_LABEL, ObjectsError, OfferedSecret, Owner, Precondition, SharedDraft, SharedKind,
    Stored,
};

/// A value no read of the port may carry: the cases plant it as the content of Secrets.
pub const SECRET_VALUE: &str = "aap-secret-value-never-read-2d7e90";

/// The name of a Secret in the samples, with the sentinel in it.
fn secret(what: &str) -> String {
    format!("{SENTINEL}-{what}")
}

/// An agent that is only a folder, as the forms write one: it names no model and no tool server, and
/// every secret is a reference. Its audience is its use permission.
pub fn sample_agent(name: &str) -> AgentDraft {
    sample_agent_using(name, None, &[])
}

/// As [`sample_agent`], naming a `ModelEndpoint` and `ToolProvider`s.
pub fn sample_agent_using(name: &str, endpoint: Option<&str>, providers: &[&str]) -> AgentDraft {
    let model = match endpoint {
        Some(endpoint) => json!({ "model": "chat-model", "endpointRef": { "name": endpoint } }),
        None => json!({
            "model": "chat-model",
            "baseUrl": { "value": "https://gateway.example.invalid/v1" },
            "apiKeySecretRef": { "name": secret("models"), "key": "MODEL_API_KEY" },
        }),
    };
    let servers: serde_json::Map<String, Value> = providers
        .iter()
        .map(|p| ((*p).to_owned(), json!({ "providerRef": { "name": p } })))
        .collect();
    AgentDraft {
        name: name.to_owned(),
        service: json!({
            "description": "A sample agent.",
            "configRef": { "name": name },
            "interfaces": { "a2a": {
                "enabled": true,
                "bearerTokensSecretRef": { "name": secret("a2a"), "key": "A2A_BEARER_TOKENS" },
            } },
            "store": { "postgres": { "cnpg": { "instances": 1, "storage": { "size": "5Gi" } } } },
            "access": { "audience": [format!("agent.use:{name}")] },
            "registry": { "title": "Sample", "tags": ["sample"] },
        }),
        config: json!({
            "harness": { "type": "adam-rs", "adam": {
                "binary": "adam-agent",
                "agent": { "folder": { "files": { "instructions.md": "---\nname: sample\n---\nHello.\n" } } },
            } },
            "model": model,
            "tools": { "mcpServers": servers },
            "environment": {},
        }),
    }
}

/// A shared object of `kind`, as the forms write one: its secrets are references.
pub fn sample_shared(kind: SharedKind, name: &str) -> SharedDraft {
    let spec = match kind {
        SharedKind::Model => json!({
            "title": "Gateway",
            "baseUrl": { "secretRef": { "name": secret("models"), "key": "GATEWAY_BASE_URL" } },
            "apiKeySecretRef": { "name": secret("models"), "key": "GATEWAY_API_KEY" },
            "models": ["chat-model", "coding-model"],
        }),
        SharedKind::ToolServer => json!({
            "title": "Web search",
            "mcp": {
                "url": "https://search.example.invalid/mcp",
                "headers": { "Authorization": {
                    "prefix": "Bearer ",
                    "secretRef": { "name": secret("tools"), "key": "SEARCH_MCP_TOKEN" },
                } },
                "tools": [],
                "optional": true,
            },
        }),
    };
    SharedDraft {
        name: name.to_owned(),
        spec,
    }
}

/// An agent whose title is `title`, for a change that is easy to see.
fn retitled(draft: &AgentDraft, title: &str) -> AgentDraft {
    let mut changed = draft.clone();
    changed.service["registry"]["title"] = json!(title);
    changed
}

fn labelled_as_the_dashboards(object: &Stored) {
    assert_eq!(
        object.labels.get(MANAGED_BY_LABEL).map(String::as_str),
        Some(MANAGED_BY_DASHBOARD),
        "{} {} carries the label of the dashboard",
        object.kind,
        object.name
    );
    assert!(
        !object.annotations.contains_key(ARGO_TRACKING_ANNOTATION),
        "{} {} is not tracked by Argo CD",
        object.kind,
        object.name
    );
    assert_eq!(object.owner(), Owner::Dashboard);
}

async fn agent<O: ObjectsUnderTest>(p: &O, name: &str) -> Agent {
    p.get_agent(name).await.unwrap().unwrap()
}

fn config_of(agent: &Agent) -> &Stored {
    agent.config.as_ref().expect("the agent has its config")
}

/// The agent names of `prefix`, as `list_agents` gives them.
async fn listed<O: ObjectsUnderTest>(p: &O, prefix: &str) -> Vec<String> {
    p.list_agents()
        .await
        .unwrap()
        .iter()
        .map(|a| a.name().to_owned())
        .filter(|n| n.starts_with(prefix))
        .collect()
}

/// An agent whose model is `model`, for a change of the config that is easy to see.
fn reconfigured(draft: &AgentDraft, model: &str) -> AgentDraft {
    let mut changed = draft.clone();
    changed.config["model"]["model"] = json!(model);
    changed
}

/// An `AgentService` spec as `sample_agent(name)` writes it, that names the config `config`.
fn service_naming(name: &str, config: &str) -> Value {
    let mut service = sample_agent(name).service;
    service["configRef"]["name"] = json!(config);
    service
}

/// The error of a call that had to be refused. The panic of a call that was accepted names the call, so
/// that a test of the suite can tell which assertion of a case failed.
fn refused<T: std::fmt::Debug>(
    what: impl std::fmt::Display,
    result: Result<T, ObjectsError>,
) -> ObjectsError {
    match result {
        Err(error) => error,
        Ok(value) => {
            let shown: String = format!("{value:?}").chars().take(240).collect();
            panic!("{what}: was accepted, and gave {shown}")
        }
    }
}

/// Delete an agent on the token it has now.
async fn delete_agent_now<O: ObjectsUnderTest>(p: &O, name: &str) {
    let token = agent(p, name).await.etag();
    p.delete_agent(name, &token).await.unwrap();
}

/// Delete a shared object on the token it has now.
async fn delete_shared_now<O: ObjectsUnderTest>(p: &O, kind: SharedKind, name: &str) {
    let token = p.get_shared(kind, name).await.unwrap().unwrap().etag();
    p.delete_shared(kind, name, &token).await.unwrap();
}

/// Whether `error` says that `by` (in this order) name the config `config`.
fn is_referenced_config(error: &ObjectsError, config: &str, by: &[&str]) -> bool {
    matches!(
        error,
        ObjectsError::Referenced { kind: Kind::AgentConfig, name, by: users }
            if name == config && users.iter().map(String::as_str).eq(by.iter().copied())
    )
}

/// Whether `error` says that the object of `kind` is not the dashboard's.
fn is_not_owned(error: &ObjectsError, kind: Kind) -> bool {
    matches!(error, ObjectsError::NotOwned { kind: k, .. } if *k == kind)
}

/// Cases of the objects suite, as plain async functions for harnesses that do not use the macro.
///
/// A case that expects a refusal names the call it refuses in its panic (`refused`), so that the test of
/// the suite for each fault can require the one assertion that fault has to break.
#[allow(missing_docs)] // each case is named for what it asserts
pub mod cases {
    use super::*;

    pub async fn an_agent_is_created_and_read_back<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let draft = sample_agent(&name);
        let made = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
        assert_eq!(made.name(), name);
        assert_eq!(made.service.kind, Kind::AgentService);
        assert_eq!(config_of(&made).kind, Kind::AgentConfig);
        assert_eq!(made.service.name, name);
        assert_eq!(
            config_of(&made).name,
            name,
            "an agent's config has its name"
        );
        assert_eq!(
            made.service.spec, draft.service,
            "the spec is what was written"
        );
        assert_eq!(config_of(&made).spec, draft.config);
        labelled_as_the_dashboards(&made.service);
        labelled_as_the_dashboards(config_of(&made));
        assert_eq!(made.owner(), Owner::Dashboard);
        assert_eq!(
            made.service.status, None,
            "the operator has not written a status"
        );

        let read = agent(&p, &name).await;
        assert_eq!(read, made, "a read gives what the write gave");
        assert_eq!(read.etag(), made.etag());
        let in_the_list = p
            .list_agents()
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.name() == name)
            .expect("the agent is listed");
        assert_eq!(in_the_list, made);
        assert_eq!(p.get_agent(&unique("none")).await.unwrap(), None);
    }

    pub async fn a_second_create_is_refused_and_changes_nothing<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let draft = sample_agent(&name);
        let made = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
        // Both objects would change if the create were accepted.
        let other = reconfigured(&retitled(&draft, "Other"), "another-model");
        let err = refused(
            "a second create",
            p.put_agent(&other, &Precondition::DoesNotExist).await,
        );
        assert!(
            matches!(&err, ObjectsError::AlreadyExists { kind: Kind::AgentService, name: n } if *n == name),
            "{err:?}"
        );
        assert_eq!(agent(&p, &name).await, made, "nothing was written");
    }

    pub async fn a_replace_with_the_current_token_changes_the_agent<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let draft = sample_agent(&name);
        let first = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
        let changed = retitled(&draft, "Renamed");
        let second = p
            .put_agent(&changed, &Precondition::Matches(first.etag()))
            .await
            .unwrap();
        assert_eq!(second.service.spec, changed.service);
        assert_ne!(second.etag(), first.etag(), "a change moves the token");
        assert!(
            second.service.generation > first.service.generation,
            "a change of the spec moves the generation: {} then {}",
            first.service.generation,
            second.service.generation
        );
        assert_eq!(agent(&p, &name).await, second);
        labelled_as_the_dashboards(&second.service);
        labelled_as_the_dashboards(config_of(&second));
    }

    pub async fn a_stale_token_is_refused_and_nothing_changes<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let draft = sample_agent(&name);
        let first = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
        let second = p
            .put_agent(
                &retitled(&draft, "Saved first"),
                &Precondition::Matches(first.etag()),
            )
            .await
            .unwrap();
        // The second administrator still holds the first token.
        let err = refused(
            "a replace with a stale token",
            p.put_agent(
                &retitled(&draft, "Saved second"),
                &Precondition::Matches(first.etag()),
            )
            .await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(
            agent(&p, &name).await,
            second,
            "the stale write changed nothing"
        );
        // A token no object ever had is as stale.
        let err = refused(
            "a replace with a token nothing ever had",
            p.put_agent(&draft, &Precondition::Matches(ETag::new("0.nothing")))
                .await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
    }

    pub async fn a_refused_write_changes_neither_object<O: ObjectsUnderTest>(p: O) {
        // An agent is two objects and a store may write them one after the other: a write that is refused
        // for the one must not have written the other.
        let name = unique("ag");
        let draft = sample_agent(&name);
        let first = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();

        // The token is stale on the service only, and the draft changes the config only.
        let by_another = retitled(&draft, "Saved by another administrator");
        let second = p
            .put_agent(&by_another, &Precondition::Matches(first.etag()))
            .await
            .unwrap();
        let config_edit = reconfigured(&by_another, "the-edit-of-the-config");
        let err = refused(
            "a replace that changes the config, on a token stale on the service",
            p.put_agent(&config_edit, &Precondition::Matches(first.etag()))
                .await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(
            agent(&p, &name).await,
            second,
            "the config was written by a refused replace"
        );

        // The token is stale on the config only, and the draft changes the service only.
        let by_another = reconfigured(&by_another, "the-model-of-another-administrator");
        let third = p
            .put_agent(&by_another, &Precondition::Matches(second.etag()))
            .await
            .unwrap();
        let service_edit = retitled(&by_another, "the-edit-of-the-service");
        let err = refused(
            "a replace that changes the service, on a token stale on the config",
            p.put_agent(&service_edit, &Precondition::Matches(second.etag()))
                .await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(
            agent(&p, &name).await,
            third,
            "the service was written by a refused replace"
        );
    }

    pub async fn a_change_of_either_object_makes_the_token_stale<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let draft = sample_agent(&name);
        let first = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();

        // Only the config changes.
        let config_only = reconfigured(&draft, "another-model");
        let second = p
            .put_agent(&config_only, &Precondition::Matches(first.etag()))
            .await
            .unwrap();
        assert_ne!(second.etag(), first.etag(), "the config moved the token");
        assert_eq!(second.service.spec, first.service.spec);
        // The token from before is refused, by a replace and by a delete, though the service did not change.
        let err = refused(
            "a replace with the token from before a change of the config",
            p.put_agent(&config_only, &Precondition::Matches(first.etag()))
                .await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        let err = refused(
            "a delete with the token from before a change of the config",
            p.delete_agent(&name, &first.etag()).await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(
            agent(&p, &name).await,
            second,
            "the refusals changed nothing"
        );

        // Only the service changes.
        let service_only = retitled(&config_only, "a-title-of-the-service");
        let third = p
            .put_agent(&service_only, &Precondition::Matches(second.etag()))
            .await
            .unwrap();
        assert_ne!(third.etag(), second.etag(), "the service moved the token");
        assert_eq!(config_of(&third).spec, config_of(&second).spec);
        let err = refused(
            "a replace with the token from before a change of the service",
            p.put_agent(&service_only, &Precondition::Matches(second.etag()))
                .await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        let err = refused(
            "a delete with the token from before a change of the service",
            p.delete_agent(&name, &second.etag()).await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(
            agent(&p, &name).await,
            third,
            "the refusals changed nothing"
        );
    }

    pub async fn a_token_is_for_the_object_it_was_read_from<O: ObjectsUnderTest>(p: O) {
        let (a, b) = (unique("ag"), unique("ag"));
        let made_a = p
            .put_agent(&sample_agent(&a), &Precondition::DoesNotExist)
            .await
            .unwrap();
        let made_b = p
            .put_agent(&sample_agent(&b), &Precondition::DoesNotExist)
            .await
            .unwrap();
        assert_ne!(made_a.etag(), made_b.etag());
        let err = refused(
            "a replace of one agent with the token of another",
            p.put_agent(
                &retitled(&sample_agent(&b), "Not mine"),
                &Precondition::Matches(made_a.etag()),
            )
            .await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        let err = refused(
            "a delete of one agent with the token of another",
            p.delete_agent(&b, &made_a.etag()).await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(agent(&p, &b).await, made_b, "nothing was changed");

        // A model and a tool server of one name are two objects, and each token is its object's.
        let name = unique("sh");
        let model = p
            .put_shared(
                SharedKind::Model,
                &sample_shared(SharedKind::Model, &name),
                &Precondition::DoesNotExist,
            )
            .await
            .unwrap();
        let tool = p
            .put_shared(
                SharedKind::ToolServer,
                &sample_shared(SharedKind::ToolServer, &name),
                &Precondition::DoesNotExist,
            )
            .await
            .unwrap();
        assert_ne!(model.etag(), tool.etag());
        let err = refused(
            "a replace of a tool server with the token of a model",
            p.put_shared(
                SharedKind::ToolServer,
                &sample_shared(SharedKind::ToolServer, &name),
                &Precondition::Matches(model.etag()),
            )
            .await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        let err = refused(
            "a delete of a tool server with the token of a model",
            p.delete_shared(SharedKind::ToolServer, &name, &model.etag())
                .await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(
            p.get_shared(SharedKind::ToolServer, &name).await.unwrap(),
            Some(tool),
            "nothing was changed"
        );
    }

    pub async fn a_replace_of_an_agent_that_is_not_there_is_not_found<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let err = refused(
            "a replace of an agent that is not there",
            p.put_agent(
                &sample_agent(&name),
                &Precondition::Matches(ETag::new("1:1.1:1")),
            )
            .await,
        );
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
        assert_eq!(p.get_agent(&name).await.unwrap(), None, "nothing was made");
    }

    pub async fn a_body_that_is_not_acceptable_is_invalid_with_fields<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        // The config of an agent has the agent's name.
        let mut draft = sample_agent(&name);
        draft.service["configRef"]["name"] = json!("somebody-else");
        let err = refused(
            "a create whose configRef is not the agent's name",
            p.put_agent(&draft, &Precondition::DoesNotExist).await,
        );
        let ObjectsError::Invalid(fields) = &err else {
            panic!("{err:?}")
        };
        assert!(
            fields
                .iter()
                .any(|f| f.kind == Kind::AgentService && f.path == "spec.configRef.name"),
            "{fields:?}"
        );
        assert_eq!(p.get_agent(&name).await.unwrap(), None, "nothing was made");

        // What the backend itself refuses.
        for kind in [Kind::AgentService, Kind::AgentConfig] {
            let Some((spec, path)) = p.refused_spec(kind) else {
                continue;
            };
            let mut draft = sample_agent(&name);
            match kind {
                Kind::AgentService => draft.service = spec,
                _ => draft.config = spec,
            }
            let err = refused(
                format!("a create of a {kind} the backend refuses"),
                p.put_agent(&draft, &Precondition::DoesNotExist).await,
            );
            let ObjectsError::Invalid(fields) = &err else {
                panic!("{kind}: {err:?}")
            };
            assert!(
                fields
                    .iter()
                    .any(|f| f.kind == kind && f.path.contains(&path)),
                "{kind}: a field of {kind} has {path} in its path: {fields:?}"
            );
            assert_eq!(p.get_agent(&name).await.unwrap(), None, "nothing was made");
        }
    }

    pub async fn an_agent_is_deleted_with_its_config<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let draft = sample_agent(&name);
        let made = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
        p.delete_agent(&name, &made.etag()).await.unwrap();
        assert_eq!(p.get_agent(&name).await.unwrap(), None);
        assert!(!listed(&p, &name).await.contains(&name));
        let err = refused(
            "a delete of an agent that is not there",
            p.delete_agent(&name, &made.etag()).await,
        );
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

        // The config went with the service: a new agent of the name starts a new config.
        let mut again = draft.clone();
        again.config["model"]["model"] = json!("a-model-the-old-config-did-not-have");
        let new = p
            .put_agent(&again, &Precondition::DoesNotExist)
            .await
            .unwrap();
        assert_eq!(
            config_of(&new).generation,
            1,
            "the old config is not what was replaced"
        );
        assert_ne!(new.etag(), made.etag());
    }

    pub async fn a_delete_holds_the_token_it_is_given<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let draft = sample_agent(&name);
        let first = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
        let second = p
            .put_agent(
                &retitled(&draft, "Moved on"),
                &Precondition::Matches(first.etag()),
            )
            .await
            .unwrap();
        let err = refused(
            "a delete with a stale token",
            p.delete_agent(&name, &first.etag()).await,
        );
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(
            agent(&p, &name).await,
            second,
            "the stale delete changed nothing"
        );
        p.delete_agent(&name, &second.etag()).await.unwrap();
        assert_eq!(p.get_agent(&name).await.unwrap(), None);
    }

    pub async fn a_gitops_agent_is_read_and_never_written<O: ObjectsUnderTest>(p: O) {
        for how in [Foreign::Unlabelled, Foreign::ArgoTracked] {
            let name = unique("gitops");
            let draft = sample_agent(&name);
            p.seed_foreign(Kind::AgentConfig, &name, draft.config.clone(), how)
                .await;
            p.seed_foreign(Kind::AgentService, &name, draft.service.clone(), how)
                .await;

            let seen = agent(&p, &name).await;
            assert_eq!(seen.owner(), Owner::GitOps, "{how:?}");
            assert_eq!(
                seen.service.spec, draft.service,
                "{how:?}: it is read whole"
            );
            assert_eq!(config_of(&seen).spec, draft.config);
            assert!(
                listed(&p, &name).await.contains(&name),
                "{how:?}: it is listed"
            );

            let changed = retitled(&draft, "Dashboard's edit");
            let err = refused(
                format!("{how:?}: a replace of a GitOps agent"),
                p.put_agent(&changed, &Precondition::Matches(seen.etag()))
                    .await,
            );
            assert!(
                matches!(err, ObjectsError::NotOwned { .. }),
                "{how:?}: {err:?}"
            );
            let err = refused(
                format!("{how:?}: a create over a GitOps agent"),
                p.put_agent(&changed, &Precondition::DoesNotExist).await,
            );
            assert!(
                matches!(
                    &err,
                    ObjectsError::AlreadyExists {
                        kind: Kind::AgentService,
                        ..
                    }
                ),
                "{how:?}: a create over a name that exists: {err:?}"
            );
            let err = refused(
                format!("{how:?}: a delete of a GitOps agent"),
                p.delete_agent(&name, &seen.etag()).await,
            );
            assert!(
                matches!(err, ObjectsError::NotOwned { .. }),
                "{how:?}: {err:?}"
            );
            assert_eq!(agent(&p, &name).await, seen, "{how:?}: nothing was changed");
        }
    }

    pub async fn a_gitops_config_blocks_the_create_of_its_name<O: ObjectsUnderTest>(p: O) {
        for how in [Foreign::Unlabelled, Foreign::ArgoTracked] {
            let name = unique("gitops");
            let draft = sample_agent(&name);
            p.seed_foreign(Kind::AgentConfig, &name, draft.config.clone(), how)
                .await;
            let err = refused(
                format!("{how:?}: a create over a GitOps config"),
                p.put_agent(&draft, &Precondition::DoesNotExist).await,
            );
            assert!(is_not_owned(&err, Kind::AgentConfig), "{how:?}: {err:?}");
            assert_eq!(
                p.get_agent(&name).await.unwrap(),
                None,
                "{how:?}: no service was made"
            );
        }
    }

    pub async fn a_pair_with_one_gitops_object_is_gitops<O: ObjectsUnderTest>(p: O) {
        // The dashboard wrote both; then something else took one of them (Argo CD tracks the config only,
        // or a GitOps author applies the service over it).
        for which in [Kind::AgentService, Kind::AgentConfig] {
            for how in [Foreign::Unlabelled, Foreign::ArgoTracked] {
                let name = unique("mixed");
                let draft = sample_agent(&name);
                p.put_agent(&draft, &Precondition::DoesNotExist)
                    .await
                    .unwrap();
                let spec = match which {
                    Kind::AgentService => draft.service.clone(),
                    _ => draft.config.clone(),
                };
                p.seed_foreign(which, &name, spec, how).await;

                let seen = agent(&p, &name).await;
                let (theirs, ours) = match which {
                    Kind::AgentService => (&seen.service, config_of(&seen)),
                    _ => (config_of(&seen), &seen.service),
                };
                assert_eq!(theirs.owner(), Owner::GitOps, "{which} {how:?}");
                assert_eq!(ours.owner(), Owner::Dashboard, "{which} {how:?}");
                assert_eq!(
                    seen.owner(),
                    Owner::GitOps,
                    "{which} {how:?}: one is enough"
                );

                // A draft that would change both objects.
                let changed = reconfigured(&retitled(&draft, "Dashboard's edit"), "edited-model");
                let err = refused(
                    format!("{how:?}: a replace of an agent whose {which} is GitOps's"),
                    p.put_agent(&changed, &Precondition::Matches(seen.etag()))
                        .await,
                );
                assert!(is_not_owned(&err, which), "{which} {how:?}: {err:?}");
                let err = refused(
                    format!("{how:?}: a delete of an agent whose {which} is GitOps's"),
                    p.delete_agent(&name, &seen.etag()).await,
                );
                assert!(is_not_owned(&err, which), "{which} {how:?}: {err:?}");
                assert_eq!(
                    agent(&p, &name).await,
                    seen,
                    "{which} {how:?}: neither object was changed"
                );
            }
        }
    }

    pub async fn a_create_after_a_half_failed_create_takes_the_config_over<O: ObjectsUnderTest>(
        p: O,
    ) {
        // The first write of a create (the config) went through and the second failed: the dashboard's own
        // config is there and no service. A retry of the create finishes the job.
        let name = unique("half");
        let draft = sample_agent(&name);
        p.seed_orphan_config(&name, reconfigured(&draft, "the-failed-create").config)
            .await;
        assert_eq!(
            p.get_agent(&name).await.unwrap(),
            None,
            "a config alone is not an agent"
        );
        assert!(!listed(&p, &name).await.contains(&name));

        let made = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .expect("a create after a create that failed halfway takes the config over");
        assert_eq!(
            config_of(&made).spec,
            draft.config,
            "the config is the new create's"
        );
        labelled_as_the_dashboards(&made.service);
        labelled_as_the_dashboards(config_of(&made));
        assert_eq!(agent(&p, &name).await, made);

        // From then on it is an agent like any other.
        p.put_agent(
            &retitled(&draft, "After the retry"),
            &Precondition::Matches(made.etag()),
        )
        .await
        .unwrap();
    }

    pub async fn agents_are_listed_by_name_and_are_independent<O: ObjectsUnderTest>(p: O) {
        let prefix = unique("ls");
        let names: Vec<String> = ["c", "a", "b"]
            .iter()
            .map(|s| format!("{prefix}-{s}"))
            .collect();
        for name in &names {
            p.put_agent(&sample_agent(name), &Precondition::DoesNotExist)
                .await
                .unwrap();
        }
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(listed(&p, &prefix).await, sorted, "ordered by name");
        delete_agent_now(&p, &names[1]).await;
        let mut left: Vec<String> = names.iter().filter(|n| **n != names[1]).cloned().collect();
        left.sort();
        assert_eq!(
            listed(&p, &prefix).await,
            left,
            "deleting one leaves the others"
        );
        for name in left {
            assert!(agent(&p, &name).await.config.is_some());
        }
    }

    pub async fn an_agent_whose_config_is_missing_is_listed_and_read<O: ObjectsUnderTest>(p: O) {
        // The operator marks such a service `Blocked`, and the dashboard shows it: it is an agent.
        let name = unique("blocked");
        let service = service_naming(&name, &format!("{name}-gone"));
        p.seed_foreign(
            Kind::AgentService,
            &name,
            service.clone(),
            Foreign::Unlabelled,
        )
        .await;
        let read = p
            .get_agent(&name)
            .await
            .unwrap()
            .expect("a service whose config is missing is an agent");
        assert_eq!(read.service.spec, service);
        assert_eq!(read.config, None, "its config is not there");
        let listed = p
            .list_agents()
            .await
            .unwrap()
            .into_iter()
            .find(|a| a.name() == name)
            .expect("a service whose config is missing is listed");
        assert_eq!(listed, read);

        // The config is the one the service names, whatever its name is.
        let config = format!("{name}-config");
        p.seed_foreign(
            Kind::AgentConfig,
            &config,
            sample_agent(&name).config,
            Foreign::Unlabelled,
        )
        .await;
        let other = unique("names-another");
        p.seed_foreign(
            Kind::AgentService,
            &other,
            service_naming(&other, &config),
            Foreign::Unlabelled,
        )
        .await;
        assert_eq!(
            agent(&p, &other).await.config.map(|c| c.name),
            Some(config),
            "the config of a service is the one it names"
        );
    }

    pub async fn an_agent_whose_config_another_service_names_is_left_alone<O: ObjectsUnderTest>(
        p: O,
    ) {
        // The dashboard never shares a config between two services; a GitOps service that names the
        // config of a dashboard agent makes it shared, and the dashboard stops touching it.
        let name = unique("shared-config");
        let prefix = unique("sharer");
        let (a, b) = (format!("{prefix}-a"), format!("{prefix}-b"));
        let draft = sample_agent(&name);
        let made = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
        for sharer in [&b, &a] {
            p.seed_foreign(
                Kind::AgentService,
                sharer,
                service_naming(sharer, &name),
                Foreign::Unlabelled,
            )
            .await;
        }

        let changed = reconfigured(&retitled(&draft, "Dashboard's edit"), "edited-model");
        let err = refused(
            "a replace of an agent whose config another service names",
            p.put_agent(&changed, &Precondition::Matches(made.etag()))
                .await,
        );
        assert!(is_referenced_config(&err, &name, &[&a, &b]), "{err:?}");
        let err = refused(
            "a delete of an agent whose config another service names",
            p.delete_agent(&name, &made.etag()).await,
        );
        assert!(is_referenced_config(&err, &name, &[&a, &b]), "{err:?}");
        assert_eq!(
            agent(&p, &name).await,
            made,
            "neither the service nor the config was changed or deleted"
        );
        for sharer in [&a, &b] {
            assert_eq!(
                agent(&p, sharer).await.config.map(|c| c.spec),
                Some(draft.config.clone()),
                "{sharer} still has its config"
            );
        }

        // A create of a name whose config a service already waits for is refused too: it would give that
        // service a config it did not get from its author.
        let pending = unique("pending");
        let waiting = unique("waiting");
        p.seed_foreign(
            Kind::AgentService,
            &waiting,
            service_naming(&waiting, &pending),
            Foreign::Unlabelled,
        )
        .await;
        let err = refused(
            "a create of an agent whose config another service names",
            p.put_agent(&sample_agent(&pending), &Precondition::DoesNotExist)
                .await,
        );
        assert!(is_referenced_config(&err, &pending, &[&waiting]), "{err:?}");
        assert_eq!(p.get_agent(&pending).await.unwrap(), None);

        // Nor does a create take over the config a half-failed create left, when another service names it.
        let orphan = unique("orphan");
        let namer = unique("namer");
        p.seed_orphan_config(&orphan, sample_agent(&orphan).config)
            .await;
        p.seed_foreign(
            Kind::AgentService,
            &namer,
            service_naming(&namer, &orphan),
            Foreign::Unlabelled,
        )
        .await;
        let err = refused(
            "a create over the config of a half-failed create that another service names",
            p.put_agent(&sample_agent(&orphan), &Precondition::DoesNotExist)
                .await,
        );
        assert!(is_referenced_config(&err, &orphan, &[&namer]), "{err:?}");
        assert_eq!(p.get_agent(&orphan).await.unwrap(), None);

        // It is the naming that stops the dashboard, not a mark on the config: once nobody else names it,
        // the agent is the dashboard's again.
        for sharer in [&a, &b] {
            p.seed_foreign(
                Kind::AgentService,
                sharer,
                service_naming(sharer, sharer),
                Foreign::Unlabelled,
            )
            .await;
        }
        let saved = p
            .put_agent(&changed, &Precondition::Matches(made.etag()))
            .await
            .expect("nobody else names the config any more");
        assert_eq!(config_of(&saved).spec, changed.config);
    }

    pub async fn shared_objects_have_the_same_life<O: ObjectsUnderTest>(p: O) {
        for kind in [SharedKind::Model, SharedKind::ToolServer] {
            let name = unique("sh");
            let draft = sample_shared(kind, &name);
            let made = p
                .put_shared(kind, &draft, &Precondition::DoesNotExist)
                .await
                .unwrap();
            assert_eq!(made.kind, kind.kind());
            assert_eq!(made.name, name);
            assert_eq!(
                made.spec, draft.spec,
                "{kind:?}: the spec is what was written"
            );
            labelled_as_the_dashboards(&made);
            assert_eq!(p.get_shared(kind, &name).await.unwrap(), Some(made.clone()));
            assert!(p.list_shared(kind).await.unwrap().contains(&made));
            assert_eq!(p.get_shared(kind, &unique("none")).await.unwrap(), None);

            let err = refused(
                format!("{kind:?}: a second create of a shared object"),
                p.put_shared(kind, &draft, &Precondition::DoesNotExist)
                    .await,
            );
            assert!(
                matches!(&err, ObjectsError::AlreadyExists { kind: k, .. } if *k == kind.kind()),
                "{kind:?}: {err:?}"
            );

            let mut changed = draft.clone();
            changed.spec["title"] = json!("Renamed");
            let second = p
                .put_shared(kind, &changed, &Precondition::Matches(made.etag()))
                .await
                .unwrap();
            assert_eq!(second.spec["title"], "Renamed");
            assert_ne!(second.etag(), made.etag());
            assert!(second.generation > made.generation);

            // The first administrator is late.
            let err = refused(
                format!("{kind:?}: a replace of a shared object with a stale token"),
                p.put_shared(kind, &draft, &Precondition::Matches(made.etag()))
                    .await,
            );
            assert!(
                matches!(err, ObjectsError::PreconditionFailed),
                "{kind:?}: {err:?}"
            );
            assert_eq!(
                p.get_shared(kind, &name).await.unwrap(),
                Some(second.clone())
            );

            let err = refused(
                format!("{kind:?}: a replace of a shared object that is not there"),
                p.put_shared(
                    kind,
                    &sample_shared(kind, &unique("sh")),
                    &Precondition::Matches(second.etag()),
                )
                .await,
            );
            assert!(
                matches!(err, ObjectsError::NotFound { .. }),
                "{kind:?}: {err:?}"
            );

            let err = refused(
                format!("{kind:?}: a delete of a shared object with a stale token"),
                p.delete_shared(kind, &name, &made.etag()).await,
            );
            assert!(
                matches!(err, ObjectsError::PreconditionFailed),
                "{kind:?}: {err:?}"
            );
            assert_eq!(
                p.get_shared(kind, &name).await.unwrap(),
                Some(second.clone()),
                "{kind:?}: the stale delete changed nothing"
            );
            p.delete_shared(kind, &name, &second.etag()).await.unwrap();
            assert_eq!(p.get_shared(kind, &name).await.unwrap(), None);
            let err = refused(
                format!("{kind:?}: a delete of a shared object that is not there"),
                p.delete_shared(kind, &name, &second.etag()).await,
            );
            assert!(
                matches!(err, ObjectsError::NotFound { .. }),
                "{kind:?}: {err:?}"
            );
        }
    }

    pub async fn shared_objects_are_listed_by_name<O: ObjectsUnderTest>(p: O) {
        for kind in [SharedKind::Model, SharedKind::ToolServer] {
            let prefix = unique("sl");
            let names: Vec<String> = ["z", "m", "a"]
                .iter()
                .map(|s| format!("{prefix}-{s}"))
                .collect();
            for name in &names {
                p.put_shared(
                    kind,
                    &sample_shared(kind, name),
                    &Precondition::DoesNotExist,
                )
                .await
                .unwrap();
            }
            let got: Vec<String> = p
                .list_shared(kind)
                .await
                .unwrap()
                .into_iter()
                .map(|o| o.name)
                .filter(|n| n.starts_with(&prefix))
                .collect();
            let mut sorted = names;
            sorted.sort();
            assert_eq!(got, sorted, "{kind:?}: ordered by name");
        }
    }

    pub async fn a_gitops_shared_object_is_read_and_never_written<O: ObjectsUnderTest>(p: O) {
        for kind in [SharedKind::Model, SharedKind::ToolServer] {
            for how in [Foreign::Unlabelled, Foreign::ArgoTracked] {
                let name = unique("gitops");
                let draft = sample_shared(kind, &name);
                p.seed_foreign(kind.kind(), &name, draft.spec.clone(), how)
                    .await;
                let seen = p.get_shared(kind, &name).await.unwrap().unwrap();
                assert_eq!(seen.owner(), Owner::GitOps, "{kind:?} {how:?}");
                assert_eq!(seen.spec, draft.spec);
                assert!(p.list_shared(kind).await.unwrap().contains(&seen));

                let mut changed = draft.clone();
                changed.spec["title"] = json!("Dashboard's edit");
                let err = refused(
                    format!("{kind:?} {how:?}: a replace of a GitOps shared object"),
                    p.put_shared(kind, &changed, &Precondition::Matches(seen.etag()))
                        .await,
                );
                assert!(
                    matches!(err, ObjectsError::NotOwned { .. }),
                    "{kind:?} {how:?}: {err:?}"
                );
                let err = refused(
                    format!("{kind:?} {how:?}: a create over a GitOps shared object"),
                    p.put_shared(kind, &changed, &Precondition::DoesNotExist)
                        .await,
                );
                assert!(
                    matches!(err, ObjectsError::AlreadyExists { .. }),
                    "{kind:?} {how:?}: {err:?}"
                );
                let err = refused(
                    format!("{kind:?} {how:?}: a delete of a GitOps shared object"),
                    p.delete_shared(kind, &name, &seen.etag()).await,
                );
                assert!(
                    matches!(err, ObjectsError::NotOwned { .. }),
                    "{kind:?} {how:?}: {err:?}"
                );
                assert_eq!(
                    p.get_shared(kind, &name).await.unwrap(),
                    Some(seen),
                    "{kind:?} {how:?}: nothing was changed"
                );
            }
        }
    }

    pub async fn a_shared_object_that_agents_name_is_not_deleted<O: ObjectsUnderTest>(p: O) {
        for kind in [SharedKind::Model, SharedKind::ToolServer] {
            let shared = unique("used");
            let prefix = unique("user");
            let (a, b, bystander, gitops) = (
                format!("{prefix}-a"),
                format!("{prefix}-b"),
                format!("{prefix}-c"),
                format!("{prefix}-d"),
            );
            p.put_shared(
                kind,
                &sample_shared(kind, &shared),
                &Precondition::DoesNotExist,
            )
            .await
            .unwrap();
            let using = |name: &str| match kind {
                SharedKind::Model => sample_agent_using(name, Some(&shared), &[]),
                // The same provider twice, under two names: one agent, once in the answer.
                SharedKind::ToolServer => sample_agent_using(name, None, &[&shared]),
            };
            for name in [&b, &a] {
                p.put_agent(&using(name), &Precondition::DoesNotExist)
                    .await
                    .unwrap();
            }
            p.put_agent(&sample_agent(&bystander), &Precondition::DoesNotExist)
                .await
                .unwrap();
            let theirs = using(&gitops);
            p.seed_foreign(
                Kind::AgentConfig,
                &gitops,
                theirs.config.clone(),
                Foreign::Unlabelled,
            )
            .await;
            p.seed_foreign(
                Kind::AgentService,
                &gitops,
                theirs.service.clone(),
                Foreign::Unlabelled,
            )
            .await;

            let before = p.get_shared(kind, &shared).await.unwrap().unwrap();
            let err = refused(
                format!("{kind:?}: a delete of a shared object that agents name"),
                p.delete_shared(kind, &shared, &before.etag()).await,
            );
            let ObjectsError::Referenced { by, kind: k, name } = &err else {
                panic!("{kind:?}: {err:?}")
            };
            assert_eq!(*k, kind.kind());
            assert_eq!(*name, shared);
            assert_eq!(
                *by,
                vec![a.clone(), b.clone(), gitops.clone()],
                "{kind:?}: the agents that use it, once each, in byte order, GitOps's included"
            );
            assert_eq!(
                p.get_shared(kind, &shared).await.unwrap(),
                Some(before.clone()),
                "{kind:?}: still there"
            );

            // An agent that stops naming it stops blocking the delete.
            for name in [&a, &b] {
                let current = agent(&p, name).await;
                p.put_agent(&sample_agent(name), &Precondition::Matches(current.etag()))
                    .await
                    .unwrap();
            }
            let err = refused(
                format!("{kind:?}: a delete of a shared object that a GitOps agent names"),
                p.delete_shared(kind, &shared, &before.etag()).await,
            );
            let ObjectsError::Referenced { by, .. } = &err else {
                panic!("{kind:?}: {err:?}")
            };
            assert_eq!(
                *by,
                vec![gitops.clone()],
                "{kind:?}: GitOps's agent still names it"
            );
        }
    }

    pub async fn a_shared_object_nobody_names_is_deleted<O: ObjectsUnderTest>(p: O) {
        for kind in [SharedKind::Model, SharedKind::ToolServer] {
            let shared = unique("free");
            let name = unique("ag");
            p.put_shared(
                kind,
                &sample_shared(kind, &shared),
                &Precondition::DoesNotExist,
            )
            .await
            .unwrap();
            let draft = match kind {
                SharedKind::Model => sample_agent_using(&name, Some(&shared), &[]),
                SharedKind::ToolServer => sample_agent_using(&name, None, &[&shared]),
            };
            p.put_agent(&draft, &Precondition::DoesNotExist)
                .await
                .unwrap();
            let token = p.get_shared(kind, &shared).await.unwrap().unwrap().etag();
            assert!(matches!(
                p.delete_shared(kind, &shared, &token).await,
                Err(ObjectsError::Referenced { .. })
            ));
            delete_agent_now(&p, &name).await;
            delete_shared_now(&p, kind, &shared).await;
            assert_eq!(p.get_shared(kind, &shared).await.unwrap(), None);
        }
    }

    pub async fn a_write_does_not_check_what_the_agent_names<O: ObjectsUnderTest>(p: O) {
        // The operator blocks an agent whose referent is missing, naming it; the API validates before it
        // writes. The store only stores.
        let name = unique("ag");
        let draft = sample_agent_using(
            &name,
            Some(&unique("no-such-model")),
            &[&unique("no-such-tool")],
        );
        p.put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
    }

    pub async fn the_status_is_the_operators_and_is_read_back<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let draft = sample_agent(&name);
        p.put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
        let status = json!({ "state": "Ready", "observedGeneration": 1 });
        p.set_status(Kind::AgentService, &name, status.clone())
            .await;
        let seen = agent(&p, &name).await;
        assert_eq!(seen.service.status, Some(status.clone()));
        assert_eq!(seen.service.spec, draft.service, "a status is not a spec");
        // A save by the dashboard leaves what the operator wrote.
        let saved = p
            .put_agent(
                &retitled(&draft, "Saved after the status"),
                &Precondition::Matches(seen.etag()),
            )
            .await
            .unwrap();
        assert_eq!(
            saved.service.status,
            Some(status),
            "the dashboard never writes the status"
        );
    }

    pub async fn offered_secrets_are_references_and_never_values<O: ObjectsUnderTest>(p: O) {
        let (s1, s2, s3) = (unique("sec"), unique("sec"), unique("sec"));
        for (secret, key, external) in [
            (&s2, "TOKEN", "es-two"),
            (&s1, "B_KEY", "es-one"),
            (&s1, "A_KEY", "es-one"),
        ] {
            p.offer(secret, key, external).await;
            p.plant_secret_value(secret, key, SECRET_VALUE).await;
        }
        // It exists, and it is not offered.
        p.decline(&s3, "HIDDEN", "es-three").await;
        p.plant_secret_value(&s3, "HIDDEN", SECRET_VALUE).await;

        let mine: Vec<OfferedSecret> = p
            .offered_secrets()
            .await
            .unwrap()
            .into_iter()
            .filter(|o| [&s1, &s2, &s3].contains(&&o.secret))
            .collect();
        let named = |secret: &str, key: &str, external: &str| OfferedSecret {
            secret: secret.to_owned(),
            key: key.to_owned(),
            external_secret: external.to_owned(),
        };
        let mut expected = vec![
            named(&s1, "A_KEY", "es-one"),
            named(&s1, "B_KEY", "es-one"),
            named(&s2, "TOKEN", "es-two"),
        ];
        expected.sort();
        assert_eq!(
            mine, expected,
            "the offered keys, ordered, and not the one without the label"
        );

        let text = serde_json::to_string(&mine).unwrap();
        assert!(
            !text.contains(SECRET_VALUE),
            "a value is in the answer: {text}"
        );
        for item in serde_json::to_value(&mine).unwrap().as_array().unwrap() {
            let keys: BTreeSet<&str> = item
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect();
            assert_eq!(
                keys,
                BTreeSet::from(["secret", "key", "externalSecret"]),
                "an offer has a name, a key and the ExternalSecret, and no more"
            );
        }
    }

    pub async fn no_read_carries_a_secret_value<O: ObjectsUnderTest>(p: O) {
        // The agent names Secrets; those Secrets hold a value; no read resolves the reference.
        let name = unique("ag");
        let draft = sample_agent(&name);
        for (secret, key) in [
            (super::secret("models"), "MODEL_API_KEY"),
            (super::secret("a2a"), "A2A_BEARER_TOKENS"),
        ] {
            p.plant_secret_value(&secret, key, SECRET_VALUE).await;
            // The keys the agent names are offered, so that an offer is among the answers a value could reach.
            p.offer(&secret, key, "es-sample").await;
        }
        let shared = unique("sh");
        let model = sample_shared(SharedKind::Model, &shared);
        p.plant_secret_value(&super::secret("models"), "GATEWAY_API_KEY", SECRET_VALUE)
            .await;

        let made = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();
        let model = p
            .put_shared(SharedKind::Model, &model, &Precondition::DoesNotExist)
            .await
            .unwrap();
        let offered = p.offered_secrets().await.unwrap();
        assert!(
            offered.iter().any(|o| o.secret == super::secret("models")),
            "the case offers a key, so that there is an offer to read: {offered:?}"
        );
        let everything = serde_json::to_string(&(
            made,
            model,
            p.get_agent(&name).await.unwrap(),
            p.list_agents().await.unwrap(),
            p.list_shared(SharedKind::Model).await.unwrap(),
            offered,
        ))
        .unwrap();
        assert!(
            !everything.contains(SECRET_VALUE),
            "a secret value reached an answer of the store"
        );
        // And the reference is still a reference.
        assert!(everything.contains(&super::secret("models")));
    }

    pub async fn an_unreachable_store_is_unavailable<O: ObjectsUnderTest>(p: O) {
        let Some(off) = p.unreachable().await else {
            skipped("this object store cannot be made unreachable");
            return;
        };
        let name = unique("ag");
        let draft = sample_agent(&name);
        let is_unavailable = |e: &ObjectsError| {
            matches!(e, ObjectsError::Unavailable(_)) && e.class() == ErrorClass::Transient
        };
        assert!(is_unavailable(&off.list_agents().await.unwrap_err()));
        assert!(is_unavailable(&off.get_agent(&name).await.unwrap_err()));
        assert!(is_unavailable(
            &off.put_agent(&draft, &Precondition::DoesNotExist)
                .await
                .unwrap_err()
        ));
        assert!(is_unavailable(
            &off.delete_agent(&name, &ETag::new("1")).await.unwrap_err()
        ));
        assert!(is_unavailable(
            &off.list_shared(SharedKind::Model).await.unwrap_err()
        ));
        assert!(is_unavailable(
            &off.get_shared(SharedKind::ToolServer, &name)
                .await
                .unwrap_err()
        ));
        assert!(is_unavailable(
            &off.put_shared(
                SharedKind::Model,
                &sample_shared(SharedKind::Model, &name),
                &Precondition::DoesNotExist
            )
            .await
            .unwrap_err()
        ));
        assert!(is_unavailable(
            &off.delete_shared(SharedKind::Model, &name, &ETag::new("1"))
                .await
                .unwrap_err()
        ));
        assert!(is_unavailable(&off.offered_secrets().await.unwrap_err()));
    }
}

/// Generate one `#[tokio::test]` per case of the objects suite. `$make` is a path to an
/// `async fn() -> Option<O>` where `O: ObjectsUnderTest`; `None` skips the suite, unless
/// `AAP_TEST_REQUIRE_BACKEND=1`, which fails it instead.
#[macro_export]
macro_rules! agent_objects_conformance {
    ($make:path) => {
        $crate::agent_objects_conformance!(@cases $make;
            an_agent_is_created_and_read_back,
            a_second_create_is_refused_and_changes_nothing,
            a_replace_with_the_current_token_changes_the_agent,
            a_stale_token_is_refused_and_nothing_changes,
            a_refused_write_changes_neither_object,
            a_change_of_either_object_makes_the_token_stale,
            a_token_is_for_the_object_it_was_read_from,
            a_replace_of_an_agent_that_is_not_there_is_not_found,
            a_body_that_is_not_acceptable_is_invalid_with_fields,
            an_agent_is_deleted_with_its_config,
            a_delete_holds_the_token_it_is_given,
            a_gitops_agent_is_read_and_never_written,
            a_gitops_config_blocks_the_create_of_its_name,
            a_pair_with_one_gitops_object_is_gitops,
            a_create_after_a_half_failed_create_takes_the_config_over,
            agents_are_listed_by_name_and_are_independent,
            an_agent_whose_config_is_missing_is_listed_and_read,
            an_agent_whose_config_another_service_names_is_left_alone,
            shared_objects_have_the_same_life,
            shared_objects_are_listed_by_name,
            a_gitops_shared_object_is_read_and_never_written,
            a_shared_object_that_agents_name_is_not_deleted,
            a_shared_object_nobody_names_is_deleted,
            a_write_does_not_check_what_the_agent_names,
            the_status_is_the_operators_and_is_read_back,
            offered_secrets_are_references_and_never_values,
            no_read_carries_a_secret_value,
            an_unreachable_store_is_unavailable,
        );
    };
    (@cases $make:path; $($case:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $case() {
                let Some(objects) = $make().await else {
                    $crate::testkit::skipped(&format!("{}: agent objects not configured", stringify!($case)));
                    return;
                };
                $crate::testkit::objects::cases::$case(objects).await;
            }
        )*
    };
}
