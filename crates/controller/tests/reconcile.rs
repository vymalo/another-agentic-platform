//! One pass at a time: the reconcilers of the controller against the `Memory` providers of
//! `aap-ports` and a fake API server (`tests/support`). The real `kube::Client` makes the real
//! requests (the finalizer helper's JSON patches, the server-side apply of the status, the Events), so
//! what is checked is the controller's own code; what a real API server says is the kind job's.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use aap_api::{AgentConfig, AgentService};
use aap_controller::{
    Context, FINALIZER, Metrics, Options, OwnerOf, metric, reconcile_config, reconcile_service,
    service_error_policy,
};
use aap_ports::memory::{MemoryRuntime, MemoryStore};
use aap_ports::{
    Classify, ErrorClass, Issue, IssueReason, OwnerHandle, Phase, Role, RuntimeId, RuntimeStatus,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use kube::runtime::controller::Action;
use serde_json::{Value, json};
use support::Fake;
use support::scripted::Scripted;

const NS: &str = "another-agentic-system";
const SERVICES: &str = "agentservices";
const CONFIGS: &str = "agentconfigs";
const ENDPOINTS: &str = "modelendpoints";
const PROVIDERS: &str = "toolproviders";

// ------------------------------------------------------------------ fixtures

/// An example as its two documents.
fn example(name: &str) -> (Value, Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut service = None;
    let mut config = None;
    for doc in serde_yaml::Deserializer::from_str(&text) {
        let d: Value = serde::Deserialize::deserialize(doc).unwrap();
        match d["kind"].as_str() {
            Some("AgentService") => service = Some(d),
            _ => config = Some(d),
        }
    }
    (service.unwrap(), config.unwrap())
}

/// The cluster, the providers and the context of one test. Every test builds its own, so tests share
/// nothing (the lesson of S4's first CI run, in memory).
struct World {
    fake: Fake,
    runtime: MemoryRuntime,
    store: MemoryStore,
    ctx: Arc<Context<Scripted, MemoryStore>>,
    metrics: Arc<Metrics>,
    name: String,
}

fn owner() -> OwnerOf {
    Arc::new(|svc: &AgentService| {
        OwnerHandle::new(format!(
            "owner:{}",
            svc.metadata.uid.clone().unwrap_or_default()
        ))
    })
}

fn clock() -> aap_controller::Clock {
    Arc::new(|| Time("2026-10-05T10:00:00Z".parse().unwrap()))
}

impl World {
    /// The service and config of `example`, applied.
    fn new(example_name: &str) -> Self {
        Self::with(example_name, MemoryRuntime::new(), MemoryStore::new())
    }

    fn with(example_name: &str, runtime: MemoryRuntime, store: MemoryStore) -> Self {
        Self::build(example_name, Scripted::wrapping(runtime), store)
    }

    fn with_runtime(example_name: &str, runtime: Scripted) -> Self {
        Self::build(example_name, runtime, MemoryStore::new())
    }

    fn build(example_name: &str, scripted: Scripted, store: MemoryStore) -> Self {
        Self::build_with(example_name, scripted, store, Options::default())
    }

    /// A world whose registry is served (or not), as the composition root says.
    fn with_options(example_name: &str, options: Options) -> Self {
        Self::build_with(
            example_name,
            Scripted::wrapping(MemoryRuntime::new()),
            MemoryStore::new(),
            options,
        )
    }

    fn build_with(
        example_name: &str,
        scripted: Scripted,
        store: MemoryStore,
        options: Options,
    ) -> Self {
        let runtime = scripted.inner.clone();
        let (service, config) = example(example_name);
        let fake = Fake::new();
        let name = service["metadata"]["name"].as_str().unwrap().to_owned();
        fake.put(SERVICES, service);
        fake.put(CONFIGS, config);
        let metrics = Arc::new(Metrics::new());
        let ctx = Arc::new(
            Context::new(fake.client(), scripted, store.clone(), owner(), options)
                .with_clock(clock())
                .with_metrics(metrics.clone()),
        );
        Self {
            fake,
            runtime,
            store,
            ctx,
            metrics,
            name,
        }
    }

    fn id(&self) -> RuntimeId {
        RuntimeId::new(NS, &self.name)
    }

    fn service(&self) -> Value {
        self.fake
            .get(SERVICES, NS, &self.name)
            .expect("the service")
    }

    /// One pass over the object as the cluster has it now.
    async fn pass(&self) -> Result<Action, aap_controller::Error> {
        let svc: AgentService = serde_json::from_value(self.service()).unwrap();
        reconcile_service(Arc::new(svc), self.ctx.clone()).await
    }

    /// Two passes: the first adds the finalizer (and the patch is what brings the second).
    async fn settle(&self) -> Action {
        self.pass().await.unwrap();
        self.pass().await.unwrap()
    }

    fn status(&self) -> Value {
        self.service()["status"].clone()
    }

    fn edit_service(&self, change: impl FnOnce(&mut Value)) {
        let mut s = self.service();
        change(&mut s);
        self.fake.put(SERVICES, s);
    }

    fn edit_config(&self, change: impl FnOnce(&mut Value)) {
        let mut c = self.fake.get(CONFIGS, NS, &self.name).unwrap();
        change(&mut c);
        self.fake.put(CONFIGS, c);
    }
}

/// `(status, reason)` of a condition.
fn cond(status: &Value, type_: &str) -> (String, String) {
    let c = status["conditions"]
        .as_array()
        .and_then(|cs| cs.iter().find(|c| c["type"] == type_))
        .unwrap_or_else(|| panic!("no condition {type_} in {status}"));
    (
        c["status"].as_str().unwrap().to_owned(),
        c["reason"].as_str().unwrap().to_owned(),
    )
}

fn is(status: &Value, type_: &str, truth: &str, reason: &str) {
    assert_eq!(
        cond(status, type_),
        (truth.to_owned(), reason.to_owned()),
        "{type_} in {status}"
    );
}

fn requeue_after(action: &Action) -> Duration {
    // `Action` has no accessor; its Debug form is `Action { requeue_after: Some(300s) }`.
    let text = format!("{action:?}");
    let secs = text
        .split("Some(")
        .nth(1)
        .and_then(|t| t.split('s').next())
        .unwrap_or_else(|| panic!("no requeue in {text}"));
    Duration::from_secs(secs.parse().unwrap())
}

// -------------------------------------------------------------------- tests

#[tokio::test]
async fn the_first_pass_only_adds_the_finalizer() {
    let w = World::new("chat");
    w.pass().await.unwrap();
    assert_eq!(
        w.service()["metadata"]["finalizers"],
        json!([FINALIZER]),
        "the finalizer is there before anything is created"
    );
    assert!(
        w.runtime.ids().is_empty(),
        "nothing is created before the finalizer"
    );
    assert!(w.service().get("status").is_none());
    // The finalizer is added with a JSON patch that tests what it found, never a blind overwrite.
    let patch = w.fake.writes().pop().unwrap();
    assert!(patch.content_type.contains("json-patch"));
    assert_eq!(patch.body.unwrap()[0]["op"], "test");
}

#[tokio::test]
async fn a_served_registry_lists_a_ready_service_and_a_full_one_lists_nothing() {
    use std::sync::atomic::Ordering;

    let full = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let w = World::with_options(
        "chat",
        Options {
            registry: aap_controller::RegistryMode::Enabled,
            registry_full: full.clone(),
            ..Options::default()
        },
    );
    w.settle().await;
    let status = w.status();
    is(&status, "Listed", "True", "Listed");
    is(&status, "Ready", "True", "Reconciled");

    // The registry says it would pass a limit: the next pass says so, and the agent stays Ready.
    full.store(true, Ordering::Release);
    w.settle().await;
    let status = w.status();
    is(&status, "Listed", "False", "RegistryFull");
    is(&status, "Ready", "True", "Reconciled");
    assert_eq!(
        status["state"], "Ready",
        "Listed informs and never gates Ready"
    );

    full.store(false, Ordering::Release);
    w.settle().await;
    is(&w.status(), "Listed", "True", "Listed");
}

#[tokio::test]
async fn a_new_service_gets_a_runtime_a_store_and_a_ready_status() {
    let w = World::new("chat");
    let next = w.settle().await;

    let spec = w.runtime.spec(&w.id()).expect("the runtime was ensured");
    let status = w.status();
    assert_eq!(status["observedGeneration"], 1);
    assert_eq!(status["state"], "Ready");
    is(&status, "ConfigResolved", "True", "Resolved");
    is(&status, "StoreReady", "True", "SecretReferenced");
    is(&status, "RuntimeReady", "True", "Ready");
    is(&status, "Ready", "True", "Reconciled");
    // The registry is S7: nothing lists the agent, and the status says why instead of staying silent.
    is(&status, "Listed", "False", "RegistryDisabled");
    assert_eq!(
        status["runtime"],
        json!({"provider": "memory", "phase": "Ready", "replicas": 1})
    );
    assert_eq!(status["config"]["name"], "chat");
    assert_eq!(status["config"]["digest"], json!(spec.digest));
    assert_eq!(status["config"]["observedGeneration"], 1);
    assert_eq!(
        status["endpoints"]["agentCard"],
        format!("http://chat.{NS}.svc:8080/.well-known/agent-card.json")
    );
    assert_eq!(
        status["endpoints"]["a2a"],
        format!("http://chat.{NS}.svc:8080/")
    );
    assert_eq!(
        requeue_after(&next),
        Duration::from_secs(300),
        "settled: the timer is the net"
    );
    // The store was asked, with the service's identity.
    assert!(w.store.spec(&aap_ports::StoreId::new(NS, "chat")).is_some());
    // The owner is the object's own, as the owner function made it.
    assert_eq!(spec.owner.token(), "owner:uid-chat");
    // One Event: Ready.
    let events = w.fake.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0]["type"], "Normal");
    assert_eq!(events[0]["reason"], "Reconciled");
    assert_eq!(events[0]["regarding"]["name"], "chat");
    assert_eq!(
        w.metrics.get(metric::STATE_CHANGES, &[("state", "Ready")]),
        1
    );
}

#[tokio::test]
async fn a_pass_that_finds_nothing_new_writes_nothing() {
    let w = World::new("chat");
    w.settle().await;
    w.fake.clear_calls();
    w.pass().await.unwrap();
    assert!(
        w.fake.writes().is_empty(),
        "no status patch, no Event: {:?}",
        w.fake.writes()
    );
    assert_eq!(w.fake.events().len(), 1);
}

#[tokio::test]
async fn a_changed_config_is_a_new_digest_and_a_rollout() {
    let w = World::new("chat");
    w.settle().await;
    let before = w.runtime.spec(&w.id()).unwrap().digest;

    w.edit_config(|c| {
        c["spec"]["harness"]["adam"]["agent"]["folder"]["files"]["instructions.md"] =
            json!("---\nname: chat\n---\nA different instruction.");
    });
    w.pass().await.unwrap();

    let after = w.runtime.spec(&w.id()).unwrap().digest;
    assert_ne!(
        before, after,
        "adam reads its folder at startup only: a changed folder must roll the pods"
    );
    let status = w.status();
    assert_eq!(status["config"]["digest"], json!(after));
    assert_eq!(
        status["config"]["observedGeneration"], 2,
        "the generation of the config it resolved"
    );
    assert_eq!(
        status["observedGeneration"], 1,
        "the service itself did not change"
    );
}

#[tokio::test]
async fn an_invalid_config_blocks_the_service_and_leaves_what_runs_untouched() {
    let w = World::new("chat");
    w.settle().await;
    let running = w.runtime.spec(&w.id()).unwrap();

    w.edit_config(|c| {
        // A folder with no instructions: the CRD's schema accepts it, the reconciler's validation does not.
        c["spec"]["harness"]["adam"]["agent"]["folder"]["files"] =
            json!({"README.md": "no instructions"});
    });
    w.pass().await.unwrap();

    assert_eq!(
        w.runtime.spec(&w.id()).unwrap(),
        running,
        "what runs is left as it is"
    );
    let status = w.status();
    assert_eq!(status["state"], "Blocked");
    is(&status, "ConfigResolved", "False", "ConfigInvalid");
    is(&status, "Ready", "False", "ConfigInvalid");
    is(&status, "StoreReady", "Unknown", "ConfigNotResolved");
    // What runs is still what was last observed, and was resolved from the old config.
    assert_eq!(status["runtime"]["phase"], "Ready");
    assert_eq!(status["config"]["digest"], json!(running.digest));
    let message = status["conditions"][0]["message"].as_str().unwrap();
    assert!(message.contains("instructions.md"), "{message}");
    let last = w.fake.events().pop().unwrap();
    assert_eq!(
        (last["type"].as_str(), last["reason"].as_str()),
        (Some("Warning"), Some("ConfigInvalid"))
    );

    // Fixing the config unblocks it with no help.
    w.edit_config(|c| {
        c["spec"]["harness"]["adam"]["agent"]["folder"]["files"] =
            json!({"instructions.md": "---\nname: chat\n---\nFixed."});
    });
    w.pass().await.unwrap();
    assert_eq!(w.status()["state"], "Ready");
    assert_ne!(w.runtime.spec(&w.id()).unwrap().digest, running.digest);
}

// ------------------------------------------------ the shared objects of the dashboard (D2)

/// The `ModelEndpoint` and the `ToolProvider` of an example file.
fn shared(name: &str) -> (Value, Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(path).unwrap();
    let docs: Vec<Value> = serde_yaml::Deserializer::from_str(&text)
        .map(|d| serde::Deserialize::deserialize(d).unwrap())
        .collect();
    let of = |kind: &str| docs.iter().find(|d| d["kind"] == kind).cloned().unwrap();
    (of("ModelEndpoint"), of("ToolProvider"))
}

/// The image the operator is given as its default, in these tests.
const DEFAULT_IMAGE: &str = "ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e@sha256:c3c178b7d73599f40d53f798616e510346abd3300a622b09b2bc233109f12b44";

/// The §60a example (`coder-me`): a config that references an endpoint and a provider and names no
/// image, on an operator that has a default one. Its two objects are **not** applied: each test says when.
fn dashboard_world() -> World {
    World::with_options(
        "coder-me",
        Options {
            default_agent_image: Some(DEFAULT_IMAGE.to_owned()),
            ..Options::default()
        },
    )
}

impl World {
    fn put_shared(&self) {
        let (endpoint, provider) = shared("coder-me");
        self.fake.put(ENDPOINTS, endpoint);
        self.fake.put(PROVIDERS, provider);
    }

    fn edit_shared(&self, plural: &str, name: &str, change: impl FnOnce(&mut Value)) {
        let mut o = self.fake.get(plural, NS, name).unwrap();
        change(&mut o);
        self.fake.put(plural, o);
    }

    fn message(&self) -> String {
        self.status()["conditions"][0]["message"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    fn digest(&self) -> String {
        self.runtime.spec(&self.id()).unwrap().digest
    }
}

fn env_of(spec: &aap_ports::RuntimeSpec, name: &str) -> Option<aap_ports::EnvValue> {
    spec.workloads[0]
        .container
        .env
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.value.clone())
}

#[tokio::test]
async fn a_reference_to_an_object_that_is_not_there_blocks_the_service_and_names_it() {
    let w = dashboard_world();
    w.settle().await;

    let status = w.status();
    assert_eq!(status["state"], "Blocked");
    is(&status, "ConfigResolved", "False", "ConfigInvalid");
    is(&status, "Ready", "False", "ConfigInvalid");
    let message = w.message();
    assert!(
        message.contains("ModelEndpoint \"gateway\"") && message.contains(NS),
        "{message}"
    );
    assert!(message.contains("ToolProvider \"websearch\""), "{message}");
    // Nothing is made out of half an agent: no runtime, no store.
    assert!(
        w.runtime.ids().is_empty(),
        "nothing in the pod spec: {:?}",
        w.runtime.ids()
    );
    assert!(
        w.store
            .spec(&aap_ports::StoreId::new(NS, &w.name))
            .is_none()
    );
    let last = w.fake.events().pop().unwrap();
    assert_eq!(
        (last["type"].as_str(), last["reason"].as_str()),
        (Some("Warning"), Some("ConfigInvalid"))
    );
    assert!(last["note"].as_str().unwrap().contains("gateway"));
}

#[tokio::test]
async fn a_reference_to_a_kind_whose_crd_is_not_installed_blocks_the_service_and_says_so() {
    // The API server answers a kind it does not serve with the plain text `404 page not found`, not with
    // a Status that names an object, and kube's `get_opt` calls that an error: the pass would fail, back
    // off and retry, and the status would never say anything. The pass reads it as what it is.
    let w = dashboard_world();
    w.fake.absent(&[ENDPOINTS, PROVIDERS]);
    let next = w.settle().await;

    let status = w.status();
    assert_eq!(status["state"], "Blocked");
    is(&status, "ConfigResolved", "False", "ConfigInvalid");
    let message = w.message();
    for who in ["ModelEndpoint \"gateway\"", "ToolProvider \"websearch\""] {
        assert!(
            message.contains(who)
                && message.contains("is not installed")
                && message.contains("deploy/operator-crds"),
            "{who} in {message}"
        );
    }
    assert!(
        w.runtime.ids().is_empty(),
        "nothing is made from half an agent"
    );
    // The pass found something to say, so it is not a failed pass: no error backoff, a timer.
    assert!(requeue_after(&next) >= Duration::from_secs(1));

    // The CRDs are installed, but the objects are not created yet: now they are only missing.
    w.fake.absent(&[]);
    w.pass().await.unwrap();
    assert!(w.message().contains("was not found"), "{}", w.message());
    assert!(!w.message().contains("is not installed"), "{}", w.message());

    // And created: the agent is made.
    w.put_shared();
    w.pass().await.unwrap();
    assert_eq!(w.status()["state"], "Ready", "{}", w.status());
}

#[tokio::test]
async fn a_referent_created_after_the_config_unblocks_it() {
    let w = dashboard_world();
    w.settle().await;
    assert_eq!(w.status()["state"], "Blocked");

    // The endpoint appears first: the provider is still named, alone.
    let (endpoint, provider) = shared("coder-me");
    w.fake.put(ENDPOINTS, endpoint);
    w.pass().await.unwrap();
    assert_eq!(w.status()["state"], "Blocked");
    let message = w.message();
    assert!(
        !message.contains("ModelEndpoint") && message.contains("ToolProvider \"websearch\""),
        "{message}"
    );
    assert!(w.runtime.ids().is_empty());

    // The provider appears: the agent is made, from both.
    w.fake.put(PROVIDERS, provider);
    w.pass().await.unwrap();
    let status = w.status();
    assert_eq!(status["state"], "Ready", "{status}");
    is(&status, "ConfigResolved", "True", "Resolved");
    let spec = w.runtime.spec(&w.id()).expect("the runtime was ensured");
    assert_eq!(status["config"]["digest"], json!(spec.digest));
    // What the pod gets is what the objects say: the endpoint's Secret keys, the operator's image.
    assert_eq!(
        env_of(&spec, "MODEL_BASE_URL"),
        Some(aap_ports::EnvValue::Secret(aap_ports::SecretRef::new(
            "agent-models",
            "GATEWAY_BASE_URL"
        )))
    );
    assert_eq!(spec.workloads[0].container.image, DEFAULT_IMAGE);
    assert!(
        spec.file_sets
            .iter()
            .any(|f| f.name.ends_with("-mcp") && f.files["mcp.json"].contains("websearch")),
        "{:?}",
        spec.file_sets
    );
}

#[tokio::test]
async fn an_edit_of_a_referent_rolls_the_digest() {
    let w = dashboard_world();
    w.put_shared();
    w.settle().await;
    assert_eq!(w.status()["state"], "Ready");
    let before = w.digest();

    // An endpoint's key moves to another Secret: the pods must read the new one, so they roll.
    w.edit_shared(ENDPOINTS, "gateway", |e| {
        e["spec"]["apiKeySecretRef"]["name"] = json!("rotated-models");
    });
    w.pass().await.unwrap();
    let after_endpoint = w.digest();
    assert_ne!(
        before, after_endpoint,
        "an endpoint's edit rolls its agents"
    );
    assert_eq!(w.status()["config"]["digest"], json!(after_endpoint));
    assert_eq!(
        env_of(&w.runtime.spec(&w.id()).unwrap(), "MODEL_API_KEY"),
        Some(aap_ports::EnvValue::Secret(aap_ports::SecretRef::new(
            "rotated-models",
            "GATEWAY_API_KEY"
        )))
    );
    // The config itself did not change.
    assert_eq!(w.status()["config"]["observedGeneration"], 1);

    // A provider's URL, the same.
    w.edit_shared(PROVIDERS, "websearch", |p| {
        p["spec"]["mcp"]["url"] =
            json!("http://another-search.another-agentic-system.svc:8080/mcp");
    });
    w.pass().await.unwrap();
    let after_provider = w.digest();
    assert_ne!(
        after_endpoint, after_provider,
        "a provider's edit rolls its agents"
    );

    // What only a person reads reaches no pod: the digest stays and nothing is written.
    w.fake.clear_calls();
    w.edit_shared(ENDPOINTS, "gateway", |e| {
        e["spec"]["title"] = json!("A new title");
        e["spec"]["models"] = json!(["another-alias"]);
    });
    w.edit_shared(PROVIDERS, "websearch", |p| {
        p["spec"]["title"] = json!("A new title");
    });
    w.fake.clear_calls();
    w.pass().await.unwrap();
    assert_eq!(w.digest(), after_provider);
    assert!(w.fake.writes().is_empty(), "{:?}", w.fake.writes());
}

#[tokio::test]
async fn deleting_a_referent_blocks_the_service_and_leaves_what_runs_untouched() {
    let w = dashboard_world();
    w.put_shared();
    w.settle().await;
    assert_eq!(w.status()["state"], "Ready");
    let running = w.runtime.spec(&w.id()).unwrap();

    w.fake.delete(ENDPOINTS, NS, "gateway");
    w.pass().await.unwrap();

    let status = w.status();
    assert_eq!(status["state"], "Blocked");
    is(&status, "ConfigResolved", "False", "ConfigInvalid");
    let message = w.message();
    assert!(message.contains("ModelEndpoint \"gateway\""), "{message}");
    assert!(
        !message.contains("ToolProvider"),
        "only what is missing: {message}"
    );
    // Nothing of the half-resolved agent reached the pods: what runs is what ran.
    assert_eq!(w.runtime.spec(&w.id()).unwrap(), running);
    assert_eq!(status["config"]["digest"], json!(running.digest));
    let last = w.fake.events().pop().unwrap();
    assert_eq!(last["reason"], "ConfigInvalid");

    // The same for a provider.
    let (endpoint, _) = shared("coder-me");
    w.fake.put(ENDPOINTS, endpoint);
    w.pass().await.unwrap();
    assert_eq!(w.status()["state"], "Ready");
    assert_eq!(
        w.digest(),
        running.digest,
        "the same objects, the same agent"
    );
    w.fake.delete(PROVIDERS, NS, "websearch");
    w.pass().await.unwrap();
    assert_eq!(w.status()["state"], "Blocked");
    assert!(w.message().contains("ToolProvider \"websearch\""));
    assert_eq!(w.runtime.spec(&w.id()).unwrap(), running);
}

#[tokio::test]
async fn a_config_with_no_image_is_blocked_without_a_default_and_runs_the_default_with_one() {
    // No default: the message says which flag.
    let w = World::with_options("coder-me", Options::default());
    w.put_shared();
    w.settle().await;
    assert_eq!(w.status()["state"], "Blocked");
    let message = w.message();
    assert!(
        message.contains("spec.environment.image") && message.contains("--default-agent-image"),
        "{message}"
    );
    assert!(w.runtime.ids().is_empty());

    // The same objects on an operator with a default run it, and a bump of the default rolls an agent
    // that names none and none that pins its own.
    let other = "ghcr.io/vymalo/another-adam-rs/coder:sha-aaaaaaa@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let digest_with = |default: &str, pinned: bool| {
        let default = default.to_owned();
        async move {
            let w = World::with_options(
                "coder-me",
                Options {
                    default_agent_image: Some(default),
                    ..Options::default()
                },
            );
            w.put_shared();
            if pinned {
                w.edit_config(|c| {
                    c["spec"]["environment"]["image"] = json!({"ref": "ghcr.io/x/y:own"});
                });
            }
            w.settle().await;
            assert_eq!(w.status()["state"], "Ready");
            w.runtime.spec(&w.id()).unwrap()
        }
    };
    let (a, b) = (
        digest_with(DEFAULT_IMAGE, false).await,
        digest_with(other, false).await,
    );
    assert_eq!(a.workloads[0].container.image, DEFAULT_IMAGE);
    assert_eq!(b.workloads[0].container.image, other);
    assert_ne!(
        a.digest, b.digest,
        "a new default rolls the agents that name none"
    );
    let (pa, pb) = (
        digest_with(DEFAULT_IMAGE, true).await,
        digest_with(other, true).await,
    );
    assert_eq!(pa.workloads[0].container.image, "ghcr.io/x/y:own");
    assert_eq!(
        pa.digest, pb.digest,
        "an agent that pins its own image is not rolled"
    );
}

#[tokio::test]
async fn an_inline_object_is_unaffected_by_the_operators_default_and_the_shared_kinds() {
    // The same `chat` under an operator with a default image and with none: the same agent.
    let plain = World::new("chat");
    plain.settle().await;
    let with_default = World::with_options(
        "chat",
        Options {
            default_agent_image: Some(DEFAULT_IMAGE.to_owned()),
            ..Options::default()
        },
    );
    with_default.settle().await;
    assert_eq!(plain.digest(), with_default.digest());
    // and it reads no ModelEndpoint or ToolProvider: it names none.
    assert!(
        with_default
            .fake
            .calls()
            .iter()
            .all(|c| !c.path.contains("modelendpoints") && !c.path.contains("toolproviders")),
        "{:?}",
        with_default.fake.calls()
    );
}

#[tokio::test]
async fn a_missing_config_is_a_condition_and_creates_nothing() {
    let w = World::new("chat");
    w.fake.delete(CONFIGS, NS, "chat");
    let next = w.settle().await;
    let status = w.status();
    assert_eq!(status["state"], "Blocked");
    is(&status, "ConfigResolved", "False", "ConfigNotFound");
    is(&status, "Ready", "False", "ConfigNotFound");
    assert!(
        w.runtime.ids().is_empty(),
        "nothing is made without a config"
    );
    assert_eq!(
        requeue_after(&next),
        Duration::from_secs(300),
        "the config's watch brings the change"
    );

    // The config appears.
    let (_, config) = example("chat");
    w.fake.put(CONFIGS, config);
    w.pass().await.unwrap();
    assert_eq!(w.status()["state"], "Ready");
}

#[tokio::test]
async fn a_missing_secret_is_a_condition_from_what_the_runtime_reports() {
    // The operator cannot read Secrets (AD-024): the kubelet's word about the pod is how it learns,
    // and the runtime provider reports it as an issue.
    let runtime = Scripted::new();
    let w = World::with_runtime("chat", runtime.clone());
    runtime.script(Some(RuntimeStatus {
        phase: Phase::Failed,
        replicas: 0,
        issues: vec![Issue {
            role: Role::All,
            reason: IssueReason::MissingSecret {
                name: "chat-secrets".to_owned(),
            },
            message: "CreateContainerConfigError".to_owned(),
        }],
    }));
    w.settle().await;
    let status = w.status();
    assert_eq!(
        status["state"], "Degraded",
        "the desired state was applied; the workload is not well"
    );
    is(&status, "RuntimeReady", "False", "MissingSecret");
    is(&status, "Ready", "False", "MissingSecret");
    is(&status, "ConfigResolved", "True", "Resolved");
    let message = status["conditions"][2]["message"].as_str().unwrap();
    assert!(message.contains("chat-secrets"), "{message}");

    // The Secret appears, the pod starts: the next pass sees Ready.
    runtime.script(None);
    w.pass().await.unwrap();
    assert_eq!(w.status()["state"], "Ready");
    // And the Events said both.
    let reasons: Vec<_> = w
        .fake
        .events()
        .iter()
        .map(|e| e["reason"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(reasons, ["MissingSecret", "Reconciled"]);
}

#[tokio::test]
async fn each_issue_the_runtime_can_report_has_its_condition() {
    let cases = [
        (IssueReason::ConfigRejected, "ConfigRejected"),
        (IssueReason::DependencyUnavailable, "DependencyUnavailable"),
        (IssueReason::ImagePull, "ImagePull"),
        (IssueReason::CrashLoop, "CrashLoop"),
    ];
    for (issue, reason) in cases {
        let runtime = Scripted::new();
        let w = World::with_runtime("chat", runtime.clone());
        runtime.script(Some(RuntimeStatus {
            phase: Phase::Failed,
            replicas: 0,
            issues: vec![Issue {
                role: Role::All,
                reason: issue,
                message: "x".to_owned(),
            }],
        }));
        w.settle().await;
        is(&w.status(), "RuntimeReady", "False", reason);
        assert_eq!(w.status()["state"], "Degraded", "{reason}");
    }
}

#[tokio::test]
async fn a_rollout_in_progress_is_degraded_with_the_reason_provisioning() {
    let runtime = Scripted::new();
    let w = World::with_runtime("chat", runtime.clone());
    runtime.script(Some(RuntimeStatus {
        phase: Phase::Provisioning,
        replicas: 0,
        issues: vec![],
    }));
    let next = w.settle().await;
    assert_eq!(w.status()["state"], "Degraded");
    is(&w.status(), "RuntimeReady", "False", "Provisioning");
    assert_eq!(
        requeue_after(&next),
        Duration::from_secs(15),
        "a rollout is looked at soon"
    );
    let last = w.fake.events().pop().unwrap();
    assert_eq!(last["type"], "Normal", "a rollout is not a warning");
}

#[tokio::test]
async fn a_name_held_by_an_object_that_is_not_ours_is_a_blocked_condition() {
    let w = World::new("chat");
    w.runtime.set_foreign(&w.id(), true);
    let next = w.settle().await;
    let status = w.status();
    assert_eq!(status["state"], "Blocked");
    is(&status, "RuntimeReady", "False", "NameConflict");
    is(&status, "Ready", "False", "NameConflict");
    is(&status, "ConfigResolved", "True", "Resolved");
    assert!(
        w.runtime.spec(&w.id()).is_none(),
        "the adoption guard changed nothing"
    );
    assert_eq!(
        requeue_after(&next),
        Duration::from_secs(15),
        "the foreign object is not watched: look again"
    );
    let last = w.fake.events().pop().unwrap();
    assert_eq!(
        (last["type"].as_str(), last["reason"].as_str()),
        (Some("Warning"), Some("NameConflict"))
    );

    // Its owner removes it (a Helm release pruned): the runtime is made, with no help.
    w.runtime.set_foreign(&w.id(), false);
    w.pass().await.unwrap();
    assert_eq!(w.status()["state"], "Ready");
    assert!(w.runtime.spec(&w.id()).is_some());
}

#[tokio::test]
async fn suspend_scales_to_zero_and_resume_wakes() {
    let w = World::new("chat");
    w.settle().await;

    w.edit_service(|s| s["spec"]["suspend"] = json!(true));
    w.pass().await.unwrap();
    assert!(w.runtime.spec(&w.id()).unwrap().suspend);
    let status = w.status();
    assert_eq!(status["state"], "Suspended");
    is(&status, "RuntimeReady", "False", "Suspended");
    assert_eq!(status["observedGeneration"], 2);
    assert_eq!(status["runtime"]["phase"], "Suspended");
    let last = w.fake.events().pop().unwrap();
    assert_eq!(last["type"], "Normal", "Suspended is healthy");

    w.edit_service(|s| s["spec"]["suspend"] = json!(false));
    w.pass().await.unwrap();
    assert_eq!(w.status()["state"], "Ready");
}

#[tokio::test]
async fn a_provider_without_suspend_blocks_a_suspended_service() {
    let w = World::with(
        "chat",
        MemoryRuntime::new().without_suspend(),
        MemoryStore::new(),
    );
    w.edit_service(|s| s["spec"]["suspend"] = json!(true));
    w.settle().await;
    let status = w.status();
    assert_eq!(status["state"], "Blocked");
    is(&status, "ConfigResolved", "False", "ConfigInvalid");
    let message = status["conditions"][0]["message"].as_str().unwrap();
    assert!(message.contains("suspend"), "{message}");
}

#[tokio::test]
async fn a_spec_the_provider_refuses_is_a_blocked_condition_with_its_words() {
    let runtime = Scripted::new();
    let w = World::with_runtime("chat", runtime.clone());
    runtime.refuse("a StatefulSet's volumeClaimTemplates cannot change");
    w.settle().await;
    let status = w.status();
    assert_eq!(status["state"], "Blocked");
    is(&status, "ConfigResolved", "False", "ConfigInvalid");
    let message = status["conditions"][0]["message"].as_str().unwrap();
    assert!(message.contains("volumeClaimTemplates"), "{message}");
}

#[tokio::test]
async fn a_cluster_store_without_cloudnativepg_is_a_condition() {
    for store in [MemoryStore::new().without_cnpg(), {
        let s = MemoryStore::new();
        s.set_cnpg_installed(false);
        s
    }] {
        let w = World::with("chat", MemoryRuntime::new(), store);
        w.edit_service(|s| {
            s["spec"]["store"] =
                json!({"postgres": {"cnpg": {"instances": 1, "storage": {"size": "5Gi"}}}});
        });
        let next = w.settle().await;
        let status = w.status();
        assert_eq!(status["state"], "Blocked");
        is(&status, "StoreReady", "False", "CNPGNotInstalled");
        is(&status, "Ready", "False", "CNPGNotInstalled");
        assert!(w.runtime.ids().is_empty(), "no workload without its ledger");
        assert_eq!(
            requeue_after(&next),
            Duration::from_secs(60),
            "someone has to install it"
        );
    }
}

#[tokio::test]
async fn a_cluster_that_is_not_ready_holds_the_runtime_back() {
    let store = MemoryStore::new();
    store.set_cluster_ready(false);
    let w = World::with("chat", MemoryRuntime::new(), store.clone());
    w.edit_service(|s| {
        s["spec"]["store"] =
            json!({"postgres": {"cnpg": {"instances": 1, "storage": {"size": "5Gi"}}}});
    });
    let next = w.settle().await;
    is(&w.status(), "StoreReady", "False", "ClusterNotReady");
    assert_eq!(w.status()["state"], "Blocked");
    assert!(w.runtime.ids().is_empty());
    assert_eq!(requeue_after(&next), Duration::from_secs(15));

    store.set_cluster_ready(true);
    w.pass().await.unwrap();
    is(&w.status(), "StoreReady", "True", "ClusterReady");
    assert_eq!(w.status()["state"], "Ready");
}

// ------------------------------------------------------------------ deleting

#[tokio::test]
async fn deleting_under_retain_removes_the_compute_and_keeps_the_volumes() {
    let w = World::new("coder");
    w.settle().await;
    assert!(w.runtime.spec(&w.id()).is_some());

    w.fake.delete(SERVICES, NS, "coder");
    assert!(
        w.fake.get(SERVICES, NS, "coder").is_some(),
        "the finalizer holds the object"
    );
    w.pass().await.unwrap();

    assert!(w.runtime.ids().is_empty(), "the compute is gone");
    assert!(
        w.fake.get(SERVICES, NS, "coder").is_none(),
        "the finalizer was removed and the object went"
    );
    let last = w.fake.events().pop().unwrap();
    assert_eq!(last["reason"], "Deleted");
    let note = last["note"].as_str().unwrap();
    assert!(note.contains("kept the volumes work"), "{note}");
    assert_eq!(w.metrics.get(metric::SERVICES_DELETED, &[]), 1);
}

#[tokio::test]
async fn deleting_under_delete_takes_the_data_too() {
    let w = World::new("coder");
    w.edit_service(|s| s["spec"]["deletionPolicy"] = json!("Delete"));
    w.settle().await;
    assert_eq!(
        w.runtime.spec(&w.id()).unwrap().deletion,
        aap_ports::DeletionPolicy::Delete,
        "the policy reached the provider in its neutral type"
    );
    w.fake.delete(SERVICES, NS, "coder");
    w.pass().await.unwrap();
    assert!(w.fake.get(SERVICES, NS, "coder").is_none());
    let note = w.fake.events().pop().unwrap()["note"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!note.contains("kept"), "{note}");
}

#[tokio::test]
async fn a_policy_changed_just_before_the_delete_is_the_one_honoured() {
    // Delete -> Retain and a delete in one breath: no pass between them. The data must stay.
    let w = World::new("coder");
    w.edit_service(|s| s["spec"]["deletionPolicy"] = json!("Delete"));
    w.settle().await;
    w.edit_service(|s| s["spec"]["deletionPolicy"] = json!("Retain"));
    w.fake.delete(SERVICES, NS, "coder");
    w.pass().await.unwrap();
    assert!(w.fake.get(SERVICES, NS, "coder").is_none());
    let note = w.fake.events().pop().unwrap()["note"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(note.contains("kept the volumes work"), "{note}");
}

#[tokio::test]
async fn deleting_a_service_that_never_got_a_runtime_still_completes() {
    let w = World::new("chat");
    w.fake.delete(CONFIGS, NS, "chat");
    w.settle().await; // blocked: ConfigNotFound
    w.fake.delete(SERVICES, NS, "chat");
    w.pass().await.unwrap();
    assert!(w.fake.get(SERVICES, NS, "chat").is_none());
}

#[tokio::test]
async fn a_failed_delete_keeps_the_finalizer() {
    let w = World::new("chat");
    w.settle().await;
    w.fake.delete(SERVICES, NS, "chat");
    w.runtime.set_unavailable(true);
    let err = w.pass().await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Transient);
    assert!(
        w.fake.get(SERVICES, NS, "chat").is_some(),
        "the object stays until the compute is gone"
    );
    w.runtime.set_unavailable(false);
    w.pass().await.unwrap();
    assert!(w.fake.get(SERVICES, NS, "chat").is_none());
}

// ------------------------------------------------------------------- failure

#[tokio::test]
async fn an_unreachable_provider_is_a_failed_pass_that_backs_off_by_class() {
    let w = World::new("chat");
    w.settle().await;
    let before = w.status();
    w.runtime.set_unavailable(true);

    let err = w.pass().await.unwrap_err();
    assert_eq!(err.class(), ErrorClass::Transient);
    assert_eq!(
        w.status(),
        before,
        "a failed pass does not rewrite what it did not learn"
    );

    let svc: Arc<AgentService> = Arc::new(serde_json::from_value(w.service()).unwrap());
    let waits: Vec<_> = (0..3)
        .map(|_| requeue_after(&service_error_policy(svc.clone(), &err, w.ctx.clone())))
        .collect();
    assert_eq!(
        waits,
        [5, 10, 20].map(Duration::from_secs),
        "doubling from 5 s"
    );
    assert_eq!(
        w.metrics.get(
            metric::ERRORS,
            &[("controller", "agentservice"), ("class", "transient")]
        ),
        3
    );

    // A pass that succeeds starts the count again.
    w.runtime.set_unavailable(false);
    w.pass().await.unwrap();
    w.runtime.set_unavailable(true);
    let err = w.pass().await.unwrap_err();
    assert_eq!(
        requeue_after(&service_error_policy(svc, &err, w.ctx.clone())),
        Duration::from_secs(5)
    );
}

#[tokio::test]
async fn an_api_server_that_fails_is_a_failed_pass_with_a_class() {
    let w = World::new("chat");
    w.fake.fail_with(Some(503));
    assert_eq!(w.pass().await.unwrap_err().class(), ErrorClass::Transient);
    w.fake.fail_with(Some(403));
    assert_eq!(
        w.pass().await.unwrap_err().class(),
        ErrorClass::Internal,
        "RBAC is a person's to fix"
    );
    w.fake.fail_with(None);
    w.settle().await;
    assert_eq!(w.status()["state"], "Ready");
}

// ------------------------------------------------------------------- configs

#[tokio::test]
async fn an_agentconfig_gets_its_valid_condition() {
    let (_, config) = example("chat");
    let fake = Fake::new();
    fake.put(CONFIGS, config);
    let ctx = Arc::new(
        aap_controller::ConfigContext::new(fake.client(), Arc::new(Metrics::new()))
            .with_clock(clock()),
    );
    let pass = || async {
        let c: AgentConfig =
            serde_json::from_value(fake.get(CONFIGS, NS, "chat").unwrap()).unwrap();
        reconcile_config(Arc::new(c), ctx.clone()).await.unwrap()
    };

    pass().await;
    let status = fake.get(CONFIGS, NS, "chat").unwrap()["status"].clone();
    assert_eq!(status["observedGeneration"], 1);
    is(&status, "Valid", "True", "Valid");

    // Nothing changed: nothing is written.
    fake.clear_calls();
    pass().await;
    assert!(fake.writes().is_empty());

    // An invalid config says why, with the field.
    let mut c = fake.get(CONFIGS, NS, "chat").unwrap();
    c["spec"]["harness"]["adam"]["agent"]["folder"]["files"] = json!({"README.md": "x"});
    fake.put(CONFIGS, c);
    pass().await;
    let status = fake.get(CONFIGS, NS, "chat").unwrap()["status"].clone();
    assert_eq!(status["observedGeneration"], 2);
    is(&status, "Valid", "False", "ConfigInvalid");
    assert!(
        status["conditions"][0]["message"]
            .as_str()
            .unwrap()
            .contains("instructions.md")
    );
}
