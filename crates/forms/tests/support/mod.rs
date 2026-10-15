//! Forms, defaults and stores for the tests of `aap-forms`: examples, and the strategies of the property tests.
#![allow(dead_code)] // each test file uses part of it

use std::collections::{BTreeMap, BTreeSet};

use aap_api::{
    Cnpg, CnpgStorage, GitAuthor, GithubMcp, PersistentVolume, SecretKeyRef, Security, Volume,
    VolumeScope, VolumeSource,
};
use aap_forms::{
    AgentForm, AgentKind, AgentKindForm, BaseUrlForm, CoderDefaults, CoderDeployment,
    CoderSettings, Defaults, Deployment, FolderDefaults, FolderSettings, GithubAppForm, HeaderForm,
    ModelChoice, ModelEndpointForm, OfferedKeys, Resources, SecretPick, StoreDefault,
    ToolServerForm,
};
use aap_ports::{
    Agent, Kind, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, OfferedSecret, ResourceVersion, Stored,
};
use k8s_openapi::api::core::v1::ResourceRequirements;
use k8s_openapi::api::networking::v1::NetworkPolicyPeer;
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
use proptest::prelude::*;
use serde_json::{Value, json};

pub const NS: &str = "another-agentic-system";

/// A string no form, default or default may let reach an object as a value.
pub const SENTINEL: &str = "aap-forms-sentinel-secret-value-5c1f";

pub fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

// ------------------------------------------------------------ the defaults

pub fn orchestrator_peer() -> NetworkPolicyPeer {
    NetworkPolicyPeer {
        namespace_selector: Some(LabelSelector {
            match_labels: Some(BTreeMap::from([(
                "kubernetes.io/metadata.name".to_owned(),
                NS.to_owned(),
            )])),
            match_expressions: None,
        }),
        ip_block: None,
        pod_selector: None,
    }
}

pub fn work_volume() -> Volume {
    Volume {
        name: "work".to_owned(),
        scope: VolumeScope::Agent,
        mount_path: "/work".to_owned(),
        source: VolumeSource {
            persistent: PersistentVolume {
                size: Quantity("20Gi".to_owned()),
                storage_class: Some("longhorn".to_owned()),
                per_replica: true,
            },
        },
    }
}

pub fn requests(cpu: &str, memory: &str) -> ResourceRequirements {
    ResourceRequirements {
        requests: Some(BTreeMap::from([
            ("cpu".to_owned(), Quantity(cpu.to_owned())),
            ("memory".to_owned(), Quantity(memory.to_owned())),
        ])),
        ..ResourceRequirements::default()
    }
}

/// The Platform API's defaults, as home-os would set them.
pub fn defaults() -> Defaults {
    Defaults {
        namespace: NS.to_owned(),
        offer_label: aap_ports::OFFER_LABEL.to_owned(),
        store: StoreDefault::Cnpg(Cnpg {
            instances: 1,
            storage: CnpgStorage {
                size: Quantity("5Gi".to_owned()),
                storage_class: None,
            },
        }),
        a2a_bearer: SecretKeyRef {
            name: "agent-a2a".to_owned(),
            key: "AGENT_REGISTRY_AGENT_TOKEN".to_owned(),
        },
        allow_from: vec![orchestrator_peer()],
        security: Security {
            run_as_user: Some(10001),
            run_as_group: Some(10001),
            fs_group: Some(10001),
            fs_group_change_policy: None,
        },
        coder: CoderDefaults {
            git_author: Some(GitAuthor {
                name: "bored-giant-panda[bot]".to_owned(),
                email: "337696346+bored-giant-panda[bot]@users.noreply.github.com".to_owned(),
            }),
            allowed_repo_hosts: vec!["github.com".to_owned()],
            github_api_url: Some("https://api.github.com".to_owned()),
            github_mcp: Some(GithubMcp {
                sidecar: true,
                port: 8082,
                host: None,
            }),
            work_volume: Some(work_volume()),
            resources: Some(requests("500m", "1Gi")),
        },
        folder: FolderDefaults {
            resources: Some(requests("100m", "256Mi")),
        },
    }
}

// ---------------------------------------------------------------- the forms

pub fn coder_form(name: &str) -> AgentForm {
    AgentForm {
        name: name.to_owned(),
        title: "Coder (me)".to_owned(),
        description: Some("Coding task to verified pull request.".to_owned()),
        tags: vec!["coding".to_owned(), "git".to_owned()],
        suspend: false,
        model: ModelChoice {
            endpoint: "gateway".to_owned(),
            model: "coding-model".to_owned(),
        },
        tool_servers: ["websearch".to_owned()].into(),
        allow_insecure_http: true,
        image: None,
        audience: Some(vec![format!("agent.use:{name}")]),
        agent: AgentKindForm::Coder(CoderSettings {
            github: GithubAppForm {
                app_id: "Iv1.example".to_owned(),
                private_key: SecretPick::new("github-app", "private-key.pem"),
                owners: vec!["stephane-segning".to_owned()],
            },
            create_repo_owners: Vec::new(),
            workers: Some(2),
            pr_draft: Some(true),
            opencode_model: Some("coding-model".to_owned()),
        }),
    }
}

pub fn folder_form(name: &str) -> AgentForm {
    AgentForm {
        name: name.to_owned(),
        title: "Chat".to_owned(),
        description: None,
        tags: vec!["chat".to_owned()],
        suspend: false,
        model: ModelChoice {
            endpoint: "gateway".to_owned(),
            model: "chat-model".to_owned(),
        },
        tool_servers: BTreeSet::new(),
        allow_insecure_http: false,
        image: None,
        audience: Some(vec!["*".to_owned()]),
        agent: AgentKindForm::Folder(FolderSettings {
            files: BTreeMap::from([
                (
                    "instructions.md".to_owned(),
                    "---\nname: chat\n---\nYour name is Chat.\n".to_owned(),
                ),
                ("skills/review/SKILL.md".to_owned(), "# Review\n".to_owned()),
            ]),
        }),
    }
}

pub fn model_form(name: &str) -> ModelEndpointForm {
    ModelEndpointForm {
        name: name.to_owned(),
        title: "Gateway".to_owned(),
        base_url: BaseUrlForm::Secret(SecretPick::new("agent-models", "GATEWAY_BASE_URL")),
        api_key: SecretPick::new("agent-models", "GATEWAY_API_KEY"),
        models: vec!["coding-model".to_owned(), "chat-model".to_owned()],
    }
}

pub fn tool_form(name: &str) -> ToolServerForm {
    ToolServerForm {
        name: name.to_owned(),
        title: "Web search".to_owned(),
        url: "http://another-agentic-websearch.another-agentic-system.svc:8080/mcp".to_owned(),
        headers: BTreeMap::from([(
            "Authorization".to_owned(),
            HeaderForm {
                prefix: Some("Bearer ".to_owned()),
                secret: SecretPick::new("agent-tools", "SEARCH_MCP_TOKEN"),
            },
        )]),
        tools: Vec::new(),
        optional: true,
    }
}

/// Every key the example forms name.
pub fn offered() -> OfferedKeys {
    OfferedKeys::from_offered(
        aap_ports::OFFER_LABEL,
        &[
            offer("github-app", "private-key.pem"),
            offer("agent-models", "GATEWAY_BASE_URL"),
            offer("agent-models", "GATEWAY_API_KEY"),
            offer("agent-tools", "SEARCH_MCP_TOKEN"),
        ],
    )
}

pub fn offer(secret: &str, key: &str) -> OfferedSecret {
    OfferedSecret {
        secret: secret.to_owned(),
        key: key.to_owned(),
        external_secret: format!("{secret}-es"),
    }
}

// ------------------------------------------------------------- the stores

/// An object as a store would hold it, made of a spec.
pub fn stored(kind: Kind, name: &str, spec: Value, dashboard: bool) -> Stored {
    let mut labels = BTreeMap::new();
    if dashboard {
        labels.insert(MANAGED_BY_LABEL.to_owned(), MANAGED_BY_DASHBOARD.to_owned());
    }
    Stored {
        kind,
        namespace: NS.to_owned(),
        name: name.to_owned(),
        version: ResourceVersion::new("7"),
        generation: 1,
        labels,
        annotations: BTreeMap::new(),
        spec,
        status: None,
    }
}

/// The agent a store would hold for `resources`.
pub fn agent_of(resources: &Resources, dashboard: bool) -> Agent {
    let draft = resources.draft();
    Agent {
        service: stored(Kind::AgentService, &draft.name, draft.service, dashboard),
        config: Some(stored(
            Kind::AgentConfig,
            &draft.name,
            draft.config,
            dashboard,
        )),
    }
}

/// An agent read from the YAML of `examples/<file>.yaml`: its `AgentService` and `AgentConfig`.
pub fn example_agent(file: &str, dashboard: bool) -> Agent {
    let path = repo_root().join("examples").join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    let mut service = None;
    let mut config = None;
    for doc in serde_yaml::Deserializer::from_str(&text) {
        let value: Value = serde::Deserialize::deserialize(doc).unwrap();
        let name = value["metadata"]["name"].as_str().unwrap().to_owned();
        match value["kind"].as_str() {
            Some("AgentService") => {
                service = Some(stored(
                    Kind::AgentService,
                    &name,
                    value["spec"].clone(),
                    dashboard,
                ));
            }
            Some("AgentConfig") => {
                config = Some(stored(
                    Kind::AgentConfig,
                    &name,
                    value["spec"].clone(),
                    dashboard,
                ));
            }
            _ => {}
        }
    }
    Agent {
        service: service.unwrap(),
        config,
    }
}

// ------------------------------------------------------------- strategies

fn ident() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9]{0,8}(-[a-z0-9]{1,4}){0,2}"
}

fn text() -> impl Strategy<Value = String> {
    "[A-Za-z0-9][ -~]{0,20}"
}

fn alias() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9.-]{0,14}"
}

fn pick() -> impl Strategy<Value = SecretPick> {
    (ident(), "[A-Za-z][A-Za-z0-9_.-]{0,12}").prop_map(|(s, k)| SecretPick::new(s, k))
}

pub fn arb_audience() -> impl Strategy<Value = Option<Vec<String>>> {
    prop_oneof![
        Just(None),
        Just(Some(Vec::new())),
        Just(Some(vec!["*".to_owned()])),
        prop::collection::vec("[a-z][a-z.:-]{0,20}", 1..4).prop_map(Some),
    ]
}

fn arb_coder_settings() -> impl Strategy<Value = CoderSettings> {
    (
        "[A-Za-z0-9.]{1,12}",
        pick(),
        prop::collection::btree_set("[A-Za-z0-9][A-Za-z0-9-]{0,10}", 1..4),
        prop::collection::vec(any::<bool>(), 3),
        prop::option::of(1i32..=16),
        prop::option::of(any::<bool>()),
        prop::option::of(alias()),
    )
        .prop_map(
            |(app_id, key, owners, subset, workers, pr_draft, opencode_model)| {
                let owners: Vec<String> = owners.into_iter().collect();
                let create_repo_owners = owners
                    .iter()
                    .zip(subset.iter().cycle())
                    .filter(|(_, keep)| **keep)
                    .map(|(o, _)| o.clone())
                    .collect();
                CoderSettings {
                    github: GithubAppForm {
                        app_id,
                        private_key: key,
                        owners,
                    },
                    create_repo_owners,
                    workers,
                    pr_draft,
                    opencode_model,
                }
            },
        )
}

fn arb_folder_settings() -> impl Strategy<Value = FolderSettings> {
    prop::collection::btree_map("[a-z]{1,6}(/[a-z]{1,6}){0,2}\\.md", "[ -~]{0,30}", 0..4).prop_map(
        |mut files| {
            files.insert(
                "instructions.md".to_owned(),
                "---\nname: x\n---\nHi\n".to_owned(),
            );
            FolderSettings { files }
        },
    )
}

pub fn arb_agent_form() -> impl Strategy<Value = AgentForm> {
    let agent = prop_oneof![
        arb_coder_settings().prop_map(AgentKindForm::Coder),
        arb_folder_settings().prop_map(AgentKindForm::Folder),
    ];
    (
        (ident(), text(), prop::option::of("[ -~]{0,60}")),
        (
            prop::collection::btree_set("[a-z][a-z0-9]{0,6}(-[a-z0-9]{1,3}){0,2}", 0..4),
            any::<bool>(),
        ),
        (
            ident(),
            alias(),
            prop::collection::btree_set(ident(), 0..3),
            any::<bool>(),
        ),
        (
            prop::option::of("[a-z]{3,8}\\.io/[a-z]{3,8}:[a-z0-9]{1,6}"),
            arb_audience(),
        ),
        agent,
    )
        .prop_map(
            |(
                (name, title, description),
                (tags, suspend),
                (endpoint, model, tool_servers, insecure),
                (image, audience),
                agent,
            )| {
                AgentForm {
                    name,
                    title,
                    description,
                    tags: tags.into_iter().collect(),
                    suspend,
                    model: ModelChoice { endpoint, model },
                    tool_servers,
                    allow_insecure_http: insecure,
                    image,
                    audience,
                    agent,
                }
            },
        )
}

fn arb_quantity() -> impl Strategy<Value = Quantity> {
    "[1-9][0-9]{0,2}(Mi|Gi|m)".prop_map(Quantity)
}

fn arb_resources() -> impl Strategy<Value = Option<ResourceRequirements>> {
    prop::option::of((arb_quantity(), arb_quantity(), any::<bool>()).prop_map(
        |(cpu, memory, limit)| ResourceRequirements {
            requests: Some(BTreeMap::from([
                ("cpu".to_owned(), cpu),
                ("memory".to_owned(), memory.clone()),
            ])),
            limits: limit.then(|| BTreeMap::from([("memory".to_owned(), memory)])),
            claims: None,
        },
    ))
}

fn arb_peer() -> impl Strategy<Value = NetworkPolicyPeer> {
    ("[a-z]{1,8}", "[a-z]{1,8}").prop_map(|(k, v)| NetworkPolicyPeer {
        namespace_selector: Some(LabelSelector {
            match_labels: Some(BTreeMap::from([(k, v)])),
            match_expressions: None,
        }),
        ip_block: None,
        pod_selector: None,
    })
}

fn arb_store() -> impl Strategy<Value = aap_api::Store> {
    prop_oneof![
        (ident(), "[a-z]{1,6}").prop_map(|(name, key)| aap_api::Store {
            postgres: aap_api::PostgresStore {
                secret_ref: Some(SecretKeyRef { name, key }),
                cnpg: None,
            },
        }),
        (1i32..4, arb_quantity(), prop::option::of("[a-z]{1,8}")).prop_map(
            |(instances, size, storage_class)| {
                aap_api::Store {
                    postgres: aap_api::PostgresStore {
                        secret_ref: None,
                        cnpg: Some(Cnpg {
                            instances,
                            storage: CnpgStorage {
                                size,
                                storage_class,
                            },
                        }),
                    },
                }
            }
        ),
    ]
}

fn arb_volume() -> impl Strategy<Value = Volume> {
    (
        ident(),
        "(/[a-z]{1,6}){1,2}",
        arb_quantity(),
        prop::option::of("[a-z]{1,8}"),
        any::<bool>(),
    )
        .prop_map(
            |(name, mount_path, size, storage_class, per_replica)| Volume {
                name,
                scope: VolumeScope::Agent,
                mount_path,
                source: VolumeSource {
                    persistent: PersistentVolume {
                        size,
                        storage_class,
                        per_replica,
                    },
                },
            },
        )
}

pub fn arb_deployment(kind: AgentKind) -> impl Strategy<Value = Deployment> {
    let coder = match kind {
        AgentKind::Coder => (
            prop::option::of(
                ("[A-Za-z ]{1,10}", "[a-z]{1,6}@[a-z]{1,6}\\.invalid")
                    .prop_map(|(name, email)| GitAuthor { name, email }),
            ),
            prop::collection::vec("[a-z]{1,8}\\.com", 0..3),
            prop::option::of("https://[a-z]{1,8}\\.invalid"),
        )
            .prop_map(
                |(git_author, allowed_repo_hosts, github_api_url)| CoderDeployment {
                    git_author,
                    allowed_repo_hosts,
                    github_api_url,
                },
            )
            .boxed(),
        AgentKind::Folder => Just(CoderDeployment::default()).boxed(),
    };
    (
        arb_store(),
        prop::option::of(
            (ident(), "[A-Z_]{1,10}").prop_map(|(name, key)| SecretKeyRef { name, key }),
        ),
        prop::collection::vec(arb_peer(), 0..3),
        (prop::option::of(1i64..20000), prop::option::of(1i64..20000)),
        arb_resources(),
        prop::collection::vec(arb_volume(), 0..2),
        prop::option::of(
            (any::<bool>(), 1i32..65535).prop_map(|(sidecar, port)| GithubMcp {
                sidecar,
                port,
                host: None,
            }),
        ),
        coder,
    )
        .prop_map(
            |(
                store,
                a2a_bearer,
                allow_from,
                (run_as_user, fs_group),
                resources,
                volumes,
                github_mcp,
                coder,
            )| {
                Deployment {
                    store,
                    a2a_bearer,
                    allow_from,
                    security: Security {
                        run_as_user,
                        run_as_group: None,
                        fs_group,
                        fs_group_change_policy: None,
                    },
                    resources,
                    volumes,
                    github_mcp,
                    coder,
                }
            },
        )
}

/// A form and a deployment that fit each other (a folder agent has no coder block).
pub fn arb_agent() -> impl Strategy<Value = (AgentForm, Deployment)> {
    arb_agent_form().prop_flat_map(|form| {
        let kind = form.kind();
        (Just(form), arb_deployment(kind))
    })
}

pub fn arb_model_form() -> impl Strategy<Value = ModelEndpointForm> {
    (
        ident(),
        text(),
        prop_oneof![
            "https://[a-z]{1,8}\\.invalid/v1".prop_map(BaseUrlForm::Value),
            pick().prop_map(BaseUrlForm::Secret),
        ],
        pick(),
        prop::collection::vec(alias(), 0..4),
    )
        .prop_map(
            |(name, title, base_url, api_key, models)| ModelEndpointForm {
                name,
                title,
                base_url,
                api_key,
                models,
            },
        )
}

pub fn arb_tool_form() -> impl Strategy<Value = ToolServerForm> {
    (
        ident(),
        text(),
        "https?://[a-z]{1,8}\\.invalid(:[0-9]{2,4})?/mcp",
        prop::collection::btree_map(
            "[A-Za-z][A-Za-z0-9-]{0,10}",
            (prop::option::of("[A-Za-z ]{0,8}"), pick())
                .prop_map(|(prefix, secret)| HeaderForm { prefix, secret }),
            0..3,
        ),
        prop::collection::vec("[a-z][a-z0-9_-]{0,10}", 0..4),
        any::<bool>(),
    )
        .prop_map(
            |(name, title, url, headers, tools, optional)| ToolServerForm {
                name,
                title,
                url,
                headers,
                tools,
                optional,
            },
        )
}

// ------------------------------------------------------------------ JSON

/// Every string leaf of a JSON value (not the member names).
pub fn string_leaves(value: &Value, out: &mut BTreeSet<String>) {
    match value {
        Value::String(s) => {
            out.insert(s.clone());
        }
        Value::Array(a) => a.iter().for_each(|v| string_leaves(v, out)),
        Value::Object(m) => m.values().for_each(|v| string_leaves(v, out)),
        _ => {}
    }
}

/// Set the value at a JSON pointer (the parent must exist).
pub fn set(doc: &mut Value, pointer: &str, value: Value) {
    let (parent, last) = pointer.rsplit_once('/').unwrap();
    let parent = if parent.is_empty() {
        doc
    } else {
        doc.pointer_mut(parent).unwrap()
    };
    match parent {
        Value::Object(m) => {
            m.insert(last.to_owned(), value);
        }
        other => panic!("{pointer}: the parent is {other}"),
    }
}

pub fn remove(doc: &mut Value, pointer: &str) {
    let (parent, last) = pointer.rsplit_once('/').unwrap();
    let parent = if parent.is_empty() {
        Some(doc)
    } else {
        doc.pointer_mut(parent)
    };
    if let Some(Value::Object(m)) = parent {
        m.remove(last);
    }
}

pub fn j<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap()
}

pub fn example_json_of_a_coder_form() -> Value {
    json!({
        "name": "coder-me",
        "title": "Coder (me)",
        "description": "Coding task to verified pull request.",
        "tags": ["coding", "git"],
        "suspend": false,
        "model": { "endpoint": "gateway", "model": "coding-model" },
        "toolServers": ["websearch"],
        "allowInsecureHttp": true,
        "image": null,
        "audience": ["agent.use:coder-me"],
        "agent": {
            "kind": "coder",
            "github": {
                "appId": "Iv1.example",
                "privateKey": { "secret": "github-app", "key": "private-key.pem" },
                "owners": ["stephane-segning"]
            },
            "createRepoOwners": [],
            "workers": 2,
            "prDraft": true,
            "opencodeModel": "coding-model"
        }
    })
}
