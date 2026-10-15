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

/// Cases of the objects suite, as plain async functions for harnesses that do not use the macro.
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
        let err = p
            .put_agent(&retitled(&draft, "Other"), &Precondition::DoesNotExist)
            .await
            .unwrap_err();
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
        let err = p
            .put_agent(
                &retitled(&draft, "Saved second"),
                &Precondition::Matches(first.etag()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(
            agent(&p, &name).await,
            second,
            "the stale write changed nothing"
        );
        // A token no object ever had is as stale.
        let err = p
            .put_agent(&draft, &Precondition::Matches(ETag::new("0.nothing")))
            .await
            .unwrap_err();
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
    }

    pub async fn a_change_of_either_object_makes_the_token_stale<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let draft = sample_agent(&name);
        let first = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap();

        // Only the config changes.
        let mut config_only = draft.clone();
        config_only.config["model"]["model"] = json!("another-model");
        let second = p
            .put_agent(&config_only, &Precondition::Matches(first.etag()))
            .await
            .unwrap();
        assert_ne!(second.etag(), first.etag(), "the config moved the token");
        assert_eq!(second.service.spec, first.service.spec);

        // Only the service changes.
        let mut service_only = config_only.clone();
        service_only.service["registry"]["tags"] = json!(["sample", "more"]);
        let third = p
            .put_agent(&service_only, &Precondition::Matches(second.etag()))
            .await
            .unwrap();
        assert_ne!(third.etag(), second.etag(), "the service moved the token");
        assert_eq!(config_of(&third).spec, config_of(&second).spec);
    }

    pub async fn a_replace_of_an_agent_that_is_not_there_is_not_found<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        let err = p
            .put_agent(
                &sample_agent(&name),
                &Precondition::Matches(ETag::new("1:1.1:1")),
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
        assert_eq!(p.get_agent(&name).await.unwrap(), None, "nothing was made");
    }

    pub async fn a_body_that_is_not_acceptable_is_invalid_with_fields<O: ObjectsUnderTest>(p: O) {
        let name = unique("ag");
        // The config of an agent has the agent's name.
        let mut draft = sample_agent(&name);
        draft.service["configRef"]["name"] = json!("somebody-else");
        let err = p
            .put_agent(&draft, &Precondition::DoesNotExist)
            .await
            .unwrap_err();
        let ObjectsError::Invalid(fields) = &err else {
            panic!("{err:?}")
        };
        assert!(
            fields
                .iter()
                .any(|f| f.field == "AgentService spec.configRef.name"),
            "{fields:?}"
        );
        assert_eq!(p.get_agent(&name).await.unwrap(), None, "nothing was made");

        // What the backend itself refuses.
        for kind in [Kind::AgentService, Kind::AgentConfig] {
            let Some((spec, field)) = p.refused_spec(kind) else {
                continue;
            };
            let mut draft = sample_agent(&name);
            match kind {
                Kind::AgentService => draft.service = spec,
                _ => draft.config = spec,
            }
            let err = p
                .put_agent(&draft, &Precondition::DoesNotExist)
                .await
                .unwrap_err();
            let ObjectsError::Invalid(fields) = &err else {
                panic!("{kind}: {err:?}")
            };
            assert!(
                fields.iter().any(|f| f.field.contains(&field)),
                "{kind}: a field names {field}: {fields:?}"
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
        p.delete_agent(&name, None).await.unwrap();
        assert_eq!(p.get_agent(&name).await.unwrap(), None);
        assert!(!listed(&p, &name).await.contains(&name));
        let err = p.delete_agent(&name, None).await.unwrap_err();
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
        let err = p
            .delete_agent(&name, Some(&first.etag()))
            .await
            .unwrap_err();
        assert!(matches!(err, ObjectsError::PreconditionFailed), "{err:?}");
        assert_eq!(
            agent(&p, &name).await,
            second,
            "the stale delete changed nothing"
        );
        p.delete_agent(&name, Some(&second.etag())).await.unwrap();
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
            let err = p
                .put_agent(&changed, &Precondition::Matches(seen.etag()))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ObjectsError::NotOwned { .. }),
                "{how:?}: {err:?}"
            );
            let err = p
                .put_agent(&changed, &Precondition::DoesNotExist)
                .await
                .unwrap_err();
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
            let err = p.delete_agent(&name, None).await.unwrap_err();
            assert!(
                matches!(err, ObjectsError::NotOwned { .. }),
                "{how:?}: {err:?}"
            );
            let err = p.delete_agent(&name, Some(&seen.etag())).await.unwrap_err();
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
            let err = p
                .put_agent(&draft, &Precondition::DoesNotExist)
                .await
                .unwrap_err();
            assert!(
                matches!(
                    &err,
                    ObjectsError::NotOwned {
                        kind: Kind::AgentConfig,
                        ..
                    }
                ),
                "{how:?}: {err:?}"
            );
            assert_eq!(
                p.get_agent(&name).await.unwrap(),
                None,
                "{how:?}: no service was made"
            );
        }
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
        p.delete_agent(&names[1], None).await.unwrap();
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

            let err = p
                .put_shared(kind, &draft, &Precondition::DoesNotExist)
                .await
                .unwrap_err();
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
            let err = p
                .put_shared(kind, &draft, &Precondition::Matches(made.etag()))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ObjectsError::PreconditionFailed),
                "{kind:?}: {err:?}"
            );
            assert_eq!(
                p.get_shared(kind, &name).await.unwrap(),
                Some(second.clone())
            );

            let err = p
                .put_shared(
                    kind,
                    &sample_shared(kind, &unique("sh")),
                    &Precondition::Matches(second.etag()),
                )
                .await
                .unwrap_err();
            assert!(
                matches!(err, ObjectsError::NotFound { .. }),
                "{kind:?}: {err:?}"
            );

            let err = p
                .delete_shared(kind, &name, Some(&made.etag()))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ObjectsError::PreconditionFailed),
                "{kind:?}: {err:?}"
            );
            p.delete_shared(kind, &name, Some(&second.etag()))
                .await
                .unwrap();
            assert_eq!(p.get_shared(kind, &name).await.unwrap(), None);
            let err = p.delete_shared(kind, &name, None).await.unwrap_err();
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
                let err = p
                    .put_shared(kind, &changed, &Precondition::Matches(seen.etag()))
                    .await
                    .unwrap_err();
                assert!(
                    matches!(err, ObjectsError::NotOwned { .. }),
                    "{kind:?} {how:?}: {err:?}"
                );
                let err = p
                    .put_shared(kind, &changed, &Precondition::DoesNotExist)
                    .await
                    .unwrap_err();
                assert!(
                    matches!(err, ObjectsError::AlreadyExists { .. }),
                    "{kind:?} {how:?}: {err:?}"
                );
                let err = p.delete_shared(kind, &name, None).await.unwrap_err();
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
            let err = p.delete_shared(kind, &shared, None).await.unwrap_err();
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
                Some(before),
                "{kind:?}: still there"
            );

            // An agent that stops naming it stops blocking the delete.
            for name in [&a, &b] {
                let current = agent(&p, name).await;
                p.put_agent(&sample_agent(name), &Precondition::Matches(current.etag()))
                    .await
                    .unwrap();
            }
            let err = p.delete_shared(kind, &shared, None).await.unwrap_err();
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
            assert!(matches!(
                p.delete_shared(kind, &shared, None).await,
                Err(ObjectsError::Referenced { .. })
            ));
            p.delete_agent(&name, None).await.unwrap();
            p.delete_shared(kind, &shared, None).await.unwrap();
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
        let everything = serde_json::to_string(&(
            made,
            model,
            p.get_agent(&name).await.unwrap(),
            p.list_agents().await.unwrap(),
            p.list_shared(SharedKind::Model).await.unwrap(),
            p.offered_secrets().await.unwrap(),
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
            &off.delete_agent(&name, None).await.unwrap_err()
        ));
        assert!(is_unavailable(
            &off.list_shared(SharedKind::Model).await.unwrap_err()
        ));
        assert!(is_unavailable(
            &off.get_shared(SharedKind::ToolServer, &name)
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
            a_change_of_either_object_makes_the_token_stale,
            a_replace_of_an_agent_that_is_not_there_is_not_found,
            a_body_that_is_not_acceptable_is_invalid_with_fields,
            an_agent_is_deleted_with_its_config,
            a_delete_holds_the_token_it_is_given,
            a_gitops_agent_is_read_and_never_written,
            a_gitops_config_blocks_the_create_of_its_name,
            agents_are_listed_by_name_and_are_independent,
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
