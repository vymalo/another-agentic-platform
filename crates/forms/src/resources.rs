//! An agent form to its two custom resources and back (§60a, "Applying").
//!
//! [`to_resources`] is total: every form, valid or not, gives objects (validation is
//! [`validate_agent`](crate::validate_agent)'s). [`load_agent`] reads the form back out of an
//! [`Agent`] or says it cannot ([`Representation::YamlOnly`]); the two are inverses on every agent a form can
//! say, and that is a test (`tests/round_trip.rs`), including for arbitrary forms.

use std::collections::BTreeMap;

use aap_api::{
    A2a, A2aEnv, A2aInterface, Access, Adam, AgentConfig, AgentConfigSpec, AgentService,
    AgentServiceSpec, AgentSource, Binary, Coder, DeletionPolicy, Empty, Environment, Folder,
    Github, GithubApp, Harness, HarnessType, Image, Interfaces, McpServer, Model, NameRef,
    Registry, RunPods, Scaling, Tools, UnsupportedInterface,
};
use aap_ports::{Agent, AgentDraft, Kind, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::{
    A2aEnvForm, A2aSettings, AgentForm, AgentKind, AgentKindForm, CoderSettings, FolderSettings,
    GithubAppForm, ModelChoice,
};
use crate::defaults::{CoderDeployment, Deployment};
use crate::represent::{Loaded, Representation, Unrepresentable, differences, dropped};
use crate::secret::SecretPick;

/// The two objects of an agent, as the dashboard would write them: the objects `aap-domain::validate` reads,
/// and whose `spec`s are the [`AgentDraft`] a store takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Resources {
    /// The addressable agent.
    pub service: AgentService,
    /// What it runs.
    pub config: AgentConfig,
}

impl Resources {
    /// What to give [`AgentObjects::put_agent`](aap_ports::AgentObjects::put_agent): the two `spec`s. The
    /// store adds the label; the metadata here is for validation and for the YAML a person previews.
    pub fn draft(&self) -> AgentDraft {
        AgentDraft {
            name: self.service.metadata.name.clone().unwrap_or_default(),
            service: json(&self.service.spec),
            config: json(&self.config.spec),
        }
    }
}

/// An existing agent read as a form: the form, and the deployment's part its objects hold, which a save
/// writes back (see [`Defaults`](crate::Defaults)).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentView {
    /// What an administrator edits.
    pub form: AgentForm,
    /// What no form edits, shown read-only under *Deployment defaults*.
    pub deployment: Deployment,
}

/// Serialising the types of `aap-api` cannot fail: every map has string keys.
pub(crate) fn json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn meta(name: &str, namespace: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_owned()),
        namespace: Some(namespace.to_owned()),
        labels: Some(BTreeMap::from([(
            MANAGED_BY_LABEL.to_owned(),
            MANAGED_BY_DASHBOARD.to_owned(),
        )])),
        ..ObjectMeta::default()
    }
}

/// The objects of `form`, with `deployment` filled in, in `namespace`. Both carry the label
/// `app.kubernetes.io/managed-by: dashboard.agents.vymalo.com`.
///
/// What no form and no deployment field says is the schema's default: one combined workload, A2A at the default
/// address, `deletionPolicy: Retain`, no `extraEnv`, no tunable of the coder. That is what [`load_agent`] holds an
/// object to.
pub fn to_resources(form: &AgentForm, deployment: &Deployment, namespace: &str) -> Resources {
    let service_spec = AgentServiceSpec {
        description: form.description.clone(),
        config_ref: NameRef {
            name: form.name.clone(),
        },
        interfaces: Interfaces {
            a2a: A2aInterface {
                enabled: true,
                bearer_tokens_secret_ref: deployment.a2a_bearer.clone(),
                public_url: None,
            },
            responses: UnsupportedInterface::default(),
            mcp: UnsupportedInterface::default(),
        },
        scaling: Scaling::default(),
        suspend: form.suspend,
        store: deployment.store.clone(),
        access: Access {
            allow_from: deployment.allow_from.clone(),
            audience: form.audience.clone(),
        },
        registry: Registry {
            title: Some(form.title.clone()),
            tags: form.tags.clone(),
        },
        deletion_policy: DeletionPolicy::Retain,
    };

    let harness = match &form.agent {
        AgentKindForm::Coder(c) => Harness {
            kind: HarnessType::AdamRs,
            adam: Some(Adam {
                binary: Binary::AdamCoder,
                agent: AgentSource {
                    folder: None,
                    embedded: Some(Empty {}),
                },
                coder: Some(Coder {
                    workers: c.workers,
                    max_check_cycles: None,
                    check_timeout_secs: None,
                    workspace_sweep_secs: None,
                    workspace_placement: None,
                    allowed_repo_hosts: deployment.coder.allowed_repo_hosts.clone(),
                    github_api_url: deployment.coder.github_api_url.clone(),
                    pr_draft: c.pr_draft,
                    git_author: deployment.coder.git_author.clone(),
                    create_repo_owners: c.create_repo_owners.clone(),
                    opencode_model: c.opencode_model.clone(),
                    max_access_tier: c.max_access_tier,
                    run_pods: c.run_pod_size_class.as_ref().map(|size_class| RunPods {
                        size_class: size_class.clone(),
                    }),
                    github: Github {
                        app: Some(GithubApp {
                            id: c.github.app_id.clone(),
                            owners: Some(c.github.owners.clone()),
                            installation_id: None,
                            private_key_secret_ref: c.github.private_key.to_ref(),
                        }),
                        token: None,
                    },
                }),
            }),
            a2a: None,
        },
        AgentKindForm::Folder(f) => Harness {
            kind: HarnessType::AdamRs,
            adam: Some(Adam {
                binary: Binary::AdamAgent,
                agent: AgentSource {
                    folder: Some(Folder {
                        files: Some(f.files.clone()),
                        config_map_ref: None,
                    }),
                    embedded: None,
                },
                coder: None,
            }),
            a2a: None,
        },
        AgentKindForm::A2a(a) => Harness {
            kind: HarnessType::A2a,
            adam: None,
            a2a: Some(A2a {
                image: Image {
                    reference: a.image.clone(),
                },
                port: a.port,
                card_path: a.card_path.clone(),
                health_path: a.health_path.clone(),
                env: a
                    .env
                    .iter()
                    .map(|e| A2aEnv {
                        name: e.name.clone(),
                        value: e.value.clone(),
                        secret_key_ref: e.secret.as_ref().map(SecretPick::to_ref),
                    })
                    .collect(),
            }),
        },
    };

    let config_spec = AgentConfigSpec {
        harness,
        model: form.model.as_ref().map(|m| Model {
            model: m.model.clone(),
            endpoint_ref: Some(NameRef {
                name: m.endpoint.clone(),
            }),
            base_url: None,
            api_key_secret_ref: None,
            extra_body: None,
            echo_reasoning: None,
            context_window: None,
        }),
        tools: Tools {
            github_mcp: deployment.github_mcp.clone(),
            mcp_servers: form
                .tool_servers
                .iter()
                .map(|name| {
                    (
                        name.clone(),
                        McpServer {
                            provider_ref: Some(NameRef { name: name.clone() }),
                            url: None,
                            headers: BTreeMap::new(),
                            optional: false,
                        },
                    )
                })
                .collect(),
            allow_insecure_http: form.allow_insecure_http,
        },
        environment: Environment {
            image: form.image.clone().map(|reference| Image { reference }),
            cache: None,
            resources: deployment.resources.clone(),
            volumes: deployment.volumes.clone(),
            termination_grace_period_seconds: None,
        },
        security: deployment.security.clone(),
        extra_env: BTreeMap::new(),
    };

    let mut service = AgentService::new(&form.name, service_spec);
    service.metadata = meta(&form.name, namespace);
    let mut config = AgentConfig::new(&form.name, config_spec);
    config.metadata = meta(&form.name, namespace);
    Resources { service, config }
}

/// A spec read through its type, and what a form would lose by it: the typed spec is `None` only when the
/// JSON is not the shape of the kind at all; the members this version does not know are reasons, and the
/// rest of the spec is still read, so that all the reasons are found at once.
fn decode<T: DeserializeOwned + Serialize>(
    kind: Kind,
    spec: &Value,
) -> (Option<T>, Vec<Unrepresentable>) {
    match serde_json::from_value::<T>(spec.clone()) {
        Ok(typed) => {
            let lost = dropped(kind, spec, &json(&typed));
            (Some(typed), lost)
        }
        // The message of the error would quote a value; the class of the mistake is enough.
        Err(e) => (
            None,
            vec![Unrepresentable {
                object: kind,
                path: "spec".to_owned(),
                reason: format!("is not the shape of a {kind} ({:?})", e.classify()),
            }],
        ),
    }
}

/// Read an agent as a form.
///
/// The form and the deployment's part are read out of the two objects, rendered back with [`to_resources`], and
/// compared: **the agent is a form exactly when that gives the objects it was read from**. Otherwise the
/// result is [`Representation::YamlOnly`] with every field that differs, and nothing is dropped silently,
/// including a member of the stored `spec` that this version does not know. The `AgentConfig` that is the one
/// the service names must be there.
pub fn load_agent(agent: &Agent) -> Loaded<AgentView> {
    let managed_by = agent.owner();
    let Some(config) = &agent.config else {
        return Loaded::yaml_only(
            managed_by,
            vec![Unrepresentable {
                object: Kind::AgentService,
                path: "spec.configRef.name".to_owned(),
                reason: "names an AgentConfig that is not there".to_owned(),
            }],
        );
    };
    let (service, mut reasons) =
        decode::<AgentServiceSpec>(Kind::AgentService, &agent.service.spec);
    let (config_spec, more) = decode::<AgentConfigSpec>(Kind::AgentConfig, &config.spec);
    reasons.extend(more);
    let (Some(service), Some(config_spec)) = (service, config_spec) else {
        return Loaded::yaml_only(managed_by, reasons);
    };

    // Every harness a form can say: the adam-rs ones (a coder, a folder agent) and an image that serves
    // A2A itself (K11b, AD-054). A harness of one type that lacks its own block is an object a form cannot
    // say, and says so: the form always writes the block of the type it names.
    let Some((form, deployment)) = extract(&agent.service.name, &service, &config_spec) else {
        reasons.push(Unrepresentable {
            object: Kind::AgentConfig,
            path: format!("spec.harness.{}", harness_block(&config_spec)),
            reason: "the harness names a type and has no block of it: edit the YAML".to_owned(),
        });
        return Loaded::yaml_only(managed_by, reasons);
    };

    let again = to_resources(&form, &deployment, &agent.service.namespace);
    differences(
        Kind::AgentService,
        &json(&service),
        &json(&again.service.spec),
        &mut reasons,
    );
    differences(
        Kind::AgentConfig,
        &json(&config_spec),
        &json(&again.config.spec),
        &mut reasons,
    );
    if !reasons.is_empty() {
        return Loaded::yaml_only(managed_by, reasons);
    }
    Loaded {
        managed_by,
        representation: Representation::Form(AgentView { form, deployment }),
    }
}

/// The block a harness of `config`'s type must have: what an object is missing, for the reason that names it.
fn harness_block(config: &AgentConfigSpec) -> &'static str {
    match config.harness.kind {
        HarnessType::AdamRs => "adam",
        HarnessType::A2a => "a2a",
    }
}

/// The form and the deployment's part an agent's objects hold, as best they can be said; `None` when the
/// harness names a type and has no block of it. Where an object has no such thing (a coder without a GitHub
/// App) the form holds the empty one, and the comparison of [`load_agent`] finds it.
fn extract(
    name: &str,
    service: &AgentServiceSpec,
    config: &AgentConfigSpec,
) -> Option<(AgentForm, Deployment)> {
    let adam = config.harness.adam.as_ref();
    let coder = adam.and_then(|a| a.coder.as_ref());
    let kind = match config.harness.kind {
        HarnessType::AdamRs => match adam?.binary {
            Binary::AdamCoder => {
                let app = coder.and_then(|c| c.github.app.as_ref());
                AgentKindForm::Coder(CoderSettings {
                    github: GithubAppForm {
                        app_id: app.map(|a| a.id.clone()).unwrap_or_default(),
                        private_key: app.map_or_else(
                            || SecretPick::new("", ""),
                            |a| SecretPick::from_ref(&a.private_key_secret_ref),
                        ),
                        owners: app.and_then(|a| a.owners.clone()).unwrap_or_default(),
                    },
                    create_repo_owners: coder
                        .map(|c| c.create_repo_owners.clone())
                        .unwrap_or_default(),
                    workers: coder.and_then(|c| c.workers),
                    pr_draft: coder.and_then(|c| c.pr_draft),
                    opencode_model: coder.and_then(|c| c.opencode_model.clone()),
                    max_access_tier: coder.and_then(|c| c.max_access_tier),
                    run_pod_size_class: coder
                        .and_then(|c| c.run_pods.as_ref())
                        .map(|r| r.size_class.clone()),
                })
            }
            Binary::AdamAgent => AgentKindForm::Folder(FolderSettings {
                files: adam
                    .and_then(|a| a.agent.folder.as_ref())
                    .and_then(|f| f.files.clone())
                    .unwrap_or_default(),
            }),
        },
        HarnessType::A2a => {
            let a2a = config.harness.a2a.as_ref()?;
            AgentKindForm::A2a(A2aSettings {
                image: a2a.image.reference.clone(),
                port: a2a.port,
                card_path: a2a.card_path.clone(),
                health_path: a2a.health_path.clone(),
                env: a2a
                    .env
                    .iter()
                    .map(|e| A2aEnvForm {
                        name: e.name.clone(),
                        value: e.value.clone(),
                        secret: e.secret_key_ref.as_ref().map(SecretPick::from_ref),
                    })
                    .collect(),
            })
        }
    };
    let form = AgentForm {
        name: name.to_owned(),
        title: service.registry.title.clone().unwrap_or_default(),
        description: service.description.clone(),
        tags: service.registry.tags.clone(),
        suspend: service.suspend,
        model: config.model.as_ref().map(|m| ModelChoice {
            endpoint: m
                .endpoint_ref
                .as_ref()
                .map(|r| r.name.clone())
                .unwrap_or_default(),
            model: m.model.clone(),
        }),
        tool_servers: config.tools.mcp_servers.keys().cloned().collect(),
        allow_insecure_http: config.tools.allow_insecure_http,
        image: config
            .environment
            .image
            .as_ref()
            .map(|i| i.reference.clone()),
        audience: service.access.audience.clone(),
        agent: kind,
    };
    let deployment = Deployment {
        store: service.store.clone(),
        a2a_bearer: service.interfaces.a2a.bearer_tokens_secret_ref.clone(),
        allow_from: service.access.allow_from.clone(),
        security: config.security.clone(),
        resources: config.environment.resources.clone(),
        volumes: config.environment.volumes.clone(),
        github_mcp: config.tools.github_mcp.clone(),
        coder: coder.map_or_else(CoderDeployment::default, |c| CoderDeployment {
            git_author: c.git_author.clone(),
            allowed_repo_hosts: c.allowed_repo_hosts.clone(),
            github_api_url: c.github_api_url.clone(),
        }),
    };
    Some((form, deployment))
}

impl AgentView {
    /// Which kind of agent.
    pub fn kind(&self) -> AgentKind {
        self.form.kind()
    }
}
