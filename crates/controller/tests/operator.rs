//! The whole controller, running: `Operator::run` against the fake API server's list and watch, the
//! `Memory` store and a scripted runtime. What these show is the wiring that no single pass can: that
//! an object that appears is reconciled, that a changed `AgentConfig` reconciles the services that name
//! it, that a changed or deleted `ModelEndpoint` or `ToolProvider` reconciles the services whose config
//! references it (and only those), that an id from `RuntimeProvider::watch()` reconciles its service,
//! that another namespace is left alone, that a shared kind whose CRD is not installed stalls nothing, and that
//! the controllers stop when told to. A real API server is the kind job's.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use aap_controller::{Operator, Options, OwnerOf, Resync};
use aap_ports::memory::MemoryStore;
use aap_ports::{
    AgentDirectory, Issue, IssueReason, OwnerHandle, Phase, Role, RuntimeId, RuntimeStatus,
};
use serde_json::{Value, json};
use support::Fake;
use support::scripted::Scripted;
use tokio::sync::oneshot;

const SERVICES: &str = "agentservices";
const CONFIGS: &str = "agentconfigs";
const ENDPOINTS: &str = "modelendpoints";
const PROVIDERS: &str = "toolproviders";

const DEFAULT_IMAGE: &str = "ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e@sha256:c3c178b7d73599f40d53f798616e510346abd3300a622b09b2bc233109f12b44";

/// The §60a example (`coder-me`) in `ns`: its service, its config and the two objects the config names.
fn dashboard(ns: &str) -> (Value, Value, Value, Value) {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/coder-me.yaml");
    let text = std::fs::read_to_string(path).unwrap();
    let mut docs: Vec<Value> = serde_yaml::Deserializer::from_str(&text)
        .map(|d| serde::Deserialize::deserialize(d).unwrap())
        .collect();
    for d in &mut docs {
        d["metadata"]["namespace"] = json!(ns);
    }
    let of = |kind: &str| docs.iter().find(|d| d["kind"] == kind).cloned().unwrap();
    (
        of("AgentService"),
        of("AgentConfig"),
        of("ModelEndpoint"),
        of("ToolProvider"),
    )
}

fn example(name: &str, ns: &str) -> (Value, Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(path).unwrap();
    let mut docs: Vec<Value> = serde_yaml::Deserializer::from_str(&text)
        .map(|d| serde::Deserialize::deserialize(d).unwrap())
        .collect();
    for d in &mut docs {
        d["metadata"]["namespace"] = json!(ns);
    }
    let service = docs
        .iter()
        .find(|d| d["kind"] == "AgentService")
        .cloned()
        .unwrap();
    let config = docs
        .iter()
        .find(|d| d["kind"] == "AgentConfig")
        .cloned()
        .unwrap();
    (service, config)
}

/// Poll `check` until it holds, or fail after ten seconds with `what`.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..1000 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {what}");
}

struct Running {
    fake: Fake,
    runtime: Scripted,
    operator_done: tokio::task::JoinHandle<()>,
    stop: oneshot::Sender<()>,
    directory: aap_controller::ReflectorDirectory,
    readiness: aap_controller::Readiness,
    metrics: Arc<aap_controller::Metrics>,
}

async fn start(namespace: Option<&str>) -> Running {
    start_on(Fake::new(), namespace, true).await
}

/// The operator on `fake`. With `shared` false it does not wait for the watches of the two shared kinds,
/// because those are not there to open (`Fake::absent`).
async fn start_on(fake: Fake, namespace: Option<&str>, shared: bool) -> Running {
    let runtime = Scripted::new();
    let owner: OwnerOf = Arc::new(|_| OwnerHandle::none());
    let options = Options {
        watch_namespace: namespace.map(str::to_owned),
        // Far-away timers: what is asserted here arrives by a trigger or not at all, and a timer
        // that fired in the time we wait would hide a missing trigger.
        resync: Resync {
            settled: Duration::from_secs(3600),
            pending: Duration::from_secs(3600),
            not_installed: Duration::from_secs(3600),
        },
        default_agent_image: Some(DEFAULT_IMAGE.to_owned()),
        ..Options::default()
    };
    let operator = Operator::new(
        fake.client(),
        runtime.clone(),
        MemoryStore::new(),
        owner,
        options,
    );
    let directory = operator.directory();
    let readiness = operator.readiness();
    let metrics = operator.metrics();
    let (stop, stopped) = oneshot::channel::<()>();
    let operator_done = tokio::spawn(operator.run(async move {
        let _ = stopped.await;
    }));
    // The watches are open: nothing below races the list that precedes them.
    eventually("the controllers' watches", || {
        fake.watchers(SERVICES) >= 1
            && fake.watchers(CONFIGS) >= 2
            && (!shared || (fake.watchers(ENDPOINTS) >= 1 && fake.watchers(PROVIDERS) >= 1))
    })
    .await;
    Running {
        fake,
        runtime,
        operator_done,
        stop,
        directory,
        readiness,
        metrics,
    }
}

impl Running {
    fn state(&self, ns: &str, name: &str) -> Option<String> {
        self.fake
            .get(SERVICES, ns, name)?
            .pointer("/status/state")?
            .as_str()
            .map(str::to_owned)
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(10), self.operator_done)
            .await
            .expect("the operator stops when told to")
            .unwrap();
    }
}

#[tokio::test]
async fn a_service_that_appears_is_reconciled_and_listed_in_the_directory() {
    let r = start(Some("team")).await;
    eventually("the caches to sync", || r.readiness.is_ready()).await;
    assert!(r.directory.list().await.unwrap().is_empty());

    let (service, config) = example("chat", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    eventually("the service to be Ready", || {
        r.state("team", "chat").as_deref() == Some("Ready")
    })
    .await;

    let svc = r.fake.get(SERVICES, "team", "chat").unwrap();
    assert_eq!(
        svc["metadata"]["finalizers"],
        json!(["agents.vymalo.com/runtime"])
    );
    assert!(
        r.runtime
            .inner
            .spec(&RuntimeId::new("team", "chat"))
            .is_some()
    );
    // The config's own status, from the other controller.
    eventually("the config to be validated", || {
        r.fake.get(CONFIGS, "team", "chat").unwrap()["status"]["conditions"][0]["status"] == "True"
    })
    .await;
    // The directory reads the cache the status landed in.
    for _ in 0..1000 {
        let entry = r.directory.get("team", "chat").await.unwrap();
        if entry.is_some_and(|e| !e.blocked && e.agent_card.is_some()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let entry = r.directory.get("team", "chat").await.unwrap().unwrap();
    assert!(entry.listed(), "{entry:?}");
    assert!(
        r.metrics.get(
            aap_controller::metric::RECONCILES,
            &[("controller", "agentservice"), ("result", "ok")]
        ) >= 2
    );
    r.stop().await;
}

#[tokio::test]
async fn a_changed_config_reconciles_the_services_that_name_it() {
    let r = start(None).await;
    let (service, config) = example("chat", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    eventually("Ready", || {
        r.state("team", "chat").as_deref() == Some("Ready")
    })
    .await;
    let digest = |r: &Running| {
        r.fake.get(SERVICES, "team", "chat").unwrap()["status"]["config"]["digest"].clone()
    };
    let before = digest(&r);

    let mut config = r.fake.get(CONFIGS, "team", "chat").unwrap();
    config["spec"]["harness"]["adam"]["agent"]["folder"]["files"]["instructions.md"] =
        json!("---\nname: chat\n---\nChanged while it ran.");
    r.fake.put(CONFIGS, config);
    eventually("the service to follow its config", || digest(&r) != before).await;

    // And an invalid one blocks, without anyone touching the service.
    let mut config = r.fake.get(CONFIGS, "team", "chat").unwrap();
    config["spec"]["harness"]["adam"]["agent"]["folder"]["files"] = json!({"README.md": "x"});
    r.fake.put(CONFIGS, config);
    eventually("Blocked", || {
        r.state("team", "chat").as_deref() == Some("Blocked")
    })
    .await;
    r.stop().await;
}

impl Running {
    fn digest(&self, ns: &str, name: &str) -> Value {
        self.fake.get(SERVICES, ns, name).unwrap()["status"]["config"]["digest"].clone()
    }

    fn message(&self, ns: &str, name: &str) -> String {
        self.fake.get(SERVICES, ns, name).unwrap()["status"]["conditions"][0]["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    }

    fn reconciles(&self) -> u64 {
        self.metrics.get(
            aap_controller::metric::RECONCILES,
            &[("controller", "agentservice"), ("result", "ok")],
        )
    }
}

/// The §60a example with its objects, running: the service is `Ready`.
async fn running_dashboard(r: &Running, ns: &str) {
    let (service, config, endpoint, provider) = dashboard(ns);
    r.fake.put(ENDPOINTS, endpoint);
    r.fake.put(PROVIDERS, provider);
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    eventually("Ready", || {
        r.state(ns, "coder-me").as_deref() == Some("Ready")
    })
    .await;
}

#[tokio::test]
async fn a_changed_model_endpoint_reconciles_the_services_that_use_it() {
    let r = start(None).await;
    running_dashboard(&r, "team").await;
    let before = r.digest("team", "coder-me");

    // The service and its config do not change: only the endpoint they reference does.
    let mut endpoint = r.fake.get(ENDPOINTS, "team", "gateway").unwrap();
    endpoint["spec"]["apiKeySecretRef"]["name"] = json!("rotated-models");
    r.fake.put(ENDPOINTS, endpoint);
    eventually("the service to follow its endpoint", || {
        r.digest("team", "coder-me") != before
    })
    .await;
    let spec = r
        .runtime
        .inner
        .spec(&RuntimeId::new("team", "coder-me"))
        .unwrap();
    assert!(
        spec.workloads[0]
            .container
            .env
            .iter()
            .any(|e| e.name == "MODEL_API_KEY"
                && e.value
                    == aap_ports::EnvValue::Secret(aap_ports::SecretRef::new(
                        "rotated-models",
                        "GATEWAY_API_KEY"
                    ))),
        "the pods read the new Secret"
    );
    r.stop().await;
}

#[tokio::test]
async fn a_changed_tool_provider_reconciles_the_services_that_use_it() {
    let r = start(None).await;
    running_dashboard(&r, "team").await;
    let before = r.digest("team", "coder-me");

    let mut provider = r.fake.get(PROVIDERS, "team", "websearch").unwrap();
    provider["spec"]["mcp"]["optional"] = json!(false);
    r.fake.put(PROVIDERS, provider);
    eventually("the service to follow its provider", || {
        r.digest("team", "coder-me") != before
    })
    .await;
    r.stop().await;
}

#[tokio::test]
async fn a_deleted_referent_blocks_the_services_that_use_it_and_a_new_one_unblocks_them() {
    let r = start(None).await;
    running_dashboard(&r, "team").await;
    let running = r.digest("team", "coder-me");

    r.fake.delete(ENDPOINTS, "team", "gateway");
    eventually("Blocked by the deletion", || {
        r.state("team", "coder-me").as_deref() == Some("Blocked")
    })
    .await;
    let message = r.message("team", "coder-me");
    assert!(message.contains("ModelEndpoint \"gateway\""), "{message}");
    assert_eq!(
        r.digest("team", "coder-me"),
        running,
        "what runs is what ran"
    );

    // It comes back (created again, as a person would): the service follows, with no help.
    let (_, _, endpoint, _) = dashboard("team");
    r.fake.put(ENDPOINTS, endpoint);
    eventually("Ready again", || {
        r.state("team", "coder-me").as_deref() == Some("Ready")
    })
    .await;
    assert_eq!(r.digest("team", "coder-me"), running);
    r.stop().await;
}

#[tokio::test]
async fn a_referent_created_after_the_service_unblocks_it() {
    let r = start(None).await;
    let (service, config, endpoint, provider) = dashboard("team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    eventually("Blocked: the objects are not there", || {
        r.state("team", "coder-me").as_deref() == Some("Blocked")
    })
    .await;
    r.fake.put(ENDPOINTS, endpoint);
    eventually("the provider alone", || {
        let m = r.message("team", "coder-me");
        !m.contains("ModelEndpoint") && m.contains("ToolProvider")
    })
    .await;
    r.fake.put(PROVIDERS, provider);
    eventually("Ready", || {
        r.state("team", "coder-me").as_deref() == Some("Ready")
    })
    .await;
    r.stop().await;
}

#[tokio::test]
async fn a_referent_nobody_references_leaves_every_service_alone() {
    let r = start(None).await;
    running_dashboard(&r, "team").await;
    let (service, config) = example("chat", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    eventually("chat Ready", || {
        r.state("team", "chat").as_deref() == Some("Ready")
    })
    .await;
    // The timers are an hour: only a trigger can make another pass. Let the passes settle first.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let passes = r.reconciles();

    // An object nothing references, a same-named one in another namespace, and one of each kind.
    let (_, _, mut endpoint, mut provider) = dashboard("team");
    endpoint["metadata"]["name"] = json!("unused");
    provider["metadata"]["name"] = json!("unused");
    r.fake.put(ENDPOINTS, endpoint);
    r.fake.put(PROVIDERS, provider);
    let (_, _, elsewhere, other_provider) = dashboard("elsewhere");
    r.fake.put(ENDPOINTS, elsewhere);
    r.fake.put(PROVIDERS, other_provider);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        r.reconciles(),
        passes,
        "no service was reconciled for an object nobody references"
    );

    // And the control: the object that is referenced does make a pass.
    let mut endpoint = r.fake.get(ENDPOINTS, "team", "gateway").unwrap();
    endpoint["spec"]["baseUrl"] = json!({"value": "https://other.example.invalid/v1"});
    r.fake.put(ENDPOINTS, endpoint);
    eventually("a pass for the referenced one", || r.reconciles() > passes).await;
    r.stop().await;
}

#[tokio::test]
async fn the_runtime_providers_signal_reconciles_its_service() {
    let r = start(Some("team")).await;
    let (service, config) = example("chat", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    eventually("Ready", || {
        r.state("team", "chat").as_deref() == Some("Ready")
    })
    .await;

    // A pod starts failing: no object of ours changes, the provider says the runtime did.
    r.runtime.script(Some(RuntimeStatus {
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
    r.runtime.signal(&RuntimeId::new("team", "chat"));
    eventually("Degraded", || {
        r.state("team", "chat").as_deref() == Some("Degraded")
    })
    .await;
    let svc = r.fake.get(SERVICES, "team", "chat").unwrap();
    assert!(
        svc["status"]["conditions"]
            .to_string()
            .contains("MissingSecret")
    );

    // It recovers, and the signal of an unrelated runtime is heard and ignored.
    r.runtime.script(None);
    r.runtime.signal(&RuntimeId::new("other", "chat"));
    r.runtime.signal(&RuntimeId::new("team", "unknown"));
    r.runtime.signal(&RuntimeId::new("team", "chat"));
    eventually("Ready again", || {
        r.state("team", "chat").as_deref() == Some("Ready")
    })
    .await;
    r.stop().await;
}

#[tokio::test]
async fn a_namespaced_operator_leaves_other_namespaces_alone() {
    let r = start(Some("team")).await;
    let (service, config) = example("chat", "elsewhere");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    let (service, config) = example("coder", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    eventually("the watched namespace", || {
        r.state("team", "coder").as_deref() == Some("Ready")
    })
    .await;
    // Give an unwatched object every chance to be (wrongly) picked up.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        r.fake
            .get(SERVICES, "elsewhere", "chat")
            .unwrap()
            .get("status")
            .is_none()
    );
    assert!(
        r.runtime
            .inner
            .spec(&RuntimeId::new("elsewhere", "chat"))
            .is_none()
    );
    r.stop().await;
}

#[tokio::test]
async fn deleting_a_running_service_runs_the_finalizer_through_the_controller() {
    let r = start(None).await;
    let (service, config) = example("coder", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    eventually("Ready", || {
        r.state("team", "coder").as_deref() == Some("Ready")
    })
    .await;

    r.fake.delete(SERVICES, "team", "coder");
    eventually("the object to go", || {
        r.fake.get(SERVICES, "team", "coder").is_none()
    })
    .await;
    assert!(r.runtime.inner.ids().is_empty(), "the compute went with it");
    r.stop().await;
}

#[tokio::test]
async fn the_operator_stops_when_told_to_even_with_nothing_to_do() {
    let r = start(None).await;
    r.stop().await;
}

// ------------------------------------------------ the shared kinds' CRDs are not installed

/// Poll `check` until it holds, or fail after `limit` with `what`. In the tests below time is virtual
/// (`start_paused`): it moves when every task waits, so a backoff of thirty seconds costs nothing to
/// sit through, and a limit is how long the operator itself would have taken.
async fn within(limit: Duration, what: &str, mut check: impl FnMut() -> bool) {
    let started = tokio::time::Instant::now();
    while !check() {
        assert!(
            started.elapsed() < limit,
            "not within {limit:?}: {what} (virtual time)"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// An operator rolled out before the CRDs chart (or with an older one), or without the right to list the
/// shared kinds, has two watches that fail their list for as long as that lasts. They must not hold the
/// services back: kube's `Controller` runs one backoff around every trigger it has, so a failing one
/// that is left among them slows them all. With the two watches as `Controller::watches` made them, the
/// first assertion fails (the service that existed at start is Ready after 11 to 16 s of virtual time)
/// and so would the next (27 to 42 s).
#[tokio::test(start_paused = true)]
async fn kinds_whose_crds_are_not_installed_stall_no_service() {
    let fake = Fake::new();
    fake.absent(&[ENDPOINTS, PROVIDERS]);
    // One service exists before the operator starts.
    let (service, config) = example("chat", "team");
    fake.put(CONFIGS, config);
    fake.put(SERVICES, service);
    let r = start_on(fake, None, false).await;

    let promptly = Duration::from_secs(2);
    within(
        promptly,
        "the service that existed at start to be Ready",
        || r.state("team", "chat").as_deref() == Some("Ready"),
    )
    .await;

    // Let the failing watches back off as far as they go: this is the worst moment for a new object.
    tokio::time::sleep(Duration::from_secs(300)).await;

    // A service created now is reconciled at once.
    let (mut service, mut config) = example("chat", "team");
    service["metadata"]["name"] = json!("later");
    service["spec"]["configRef"]["name"] = json!("later");
    config["metadata"]["name"] = json!("later");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    within(promptly, "a service created later to be Ready", || {
        r.state("team", "later").as_deref() == Some("Ready")
    })
    .await;

    // A change of a config is followed at once.
    let before = r.digest("team", "later");
    let mut config = r.fake.get(CONFIGS, "team", "later").unwrap();
    config["spec"]["harness"]["adam"]["agent"]["folder"]["files"]["instructions.md"] =
        json!("---\nname: later\n---\nChanged while the CRDs were absent.");
    r.fake.put(CONFIGS, config);
    within(promptly, "the service to follow its config", || {
        r.digest("team", "later") != before
    })
    .await;

    // A service whose config references a ModelEndpoint and a ToolProvider is blocked at once. The
    // message names both, and says why nobody can create them yet: the kinds are not installed. (A GET
    // of such a kind is not a 404 that names an object, and `Api::get_opt` would have called it an error:
    // the pass would have failed and retried, and the status would have said nothing.)
    let (service, config, _, _) = dashboard("team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    within(promptly, "the dashboard service to be Blocked", || {
        r.state("team", "coder-me").as_deref() == Some("Blocked")
    })
    .await;
    let message = r.message("team", "coder-me");
    for kind in ["ModelEndpoint \"gateway\"", "ToolProvider \"websearch\""] {
        assert!(message.contains(kind), "{kind} in {message}");
    }
    assert!(
        message.contains("is not installed") && message.contains("deploy/operator-crds"),
        "{message}"
    );
    assert!(
        r.runtime
            .inner
            .spec(&RuntimeId::new("team", "coder-me"))
            .is_none(),
        "nothing is made out of half an agent"
    );
    r.stop().await;
}

/// The same operator, when the CRDs are installed afterwards: the watches that failed find the kinds on
/// their next try, and the service that was blocked follows without anyone touching it. (The timers are
/// an hour here, so only a trigger can do it.)
#[tokio::test(start_paused = true)]
async fn a_watch_that_failed_finds_its_kind_when_the_crds_are_installed() {
    let fake = Fake::new();
    fake.absent(&[ENDPOINTS, PROVIDERS]);
    let (service, config, endpoint, provider) = dashboard("team");
    fake.put(CONFIGS, config);
    fake.put(SERVICES, service);
    let r = start_on(fake, None, false).await;
    within(Duration::from_secs(2), "Blocked", || {
        r.state("team", "coder-me").as_deref() == Some("Blocked")
    })
    .await;
    tokio::time::sleep(Duration::from_secs(300)).await;

    // The CRDs are installed and the objects created. The watchers' next list answers, at most one
    // ceiling of their backoff later (thirty seconds, with jitter).
    r.fake.absent(&[]);
    r.fake.put(ENDPOINTS, endpoint);
    r.fake.put(PROVIDERS, provider);
    within(Duration::from_secs(90), "the watches to open", || {
        r.fake.watchers(ENDPOINTS) >= 1 && r.fake.watchers(PROVIDERS) >= 1
    })
    .await;
    within(Duration::from_secs(90), "the service to be Ready", || {
        r.state("team", "coder-me").as_deref() == Some("Ready")
    })
    .await;

    // And they are watches: a change of the endpoint now reaches the service.
    let before = r.digest("team", "coder-me");
    let mut endpoint = r.fake.get(ENDPOINTS, "team", "gateway").unwrap();
    endpoint["spec"]["apiKeySecretRef"]["name"] = json!("rotated-models");
    r.fake.put(ENDPOINTS, endpoint);
    within(
        Duration::from_secs(2),
        "the service to follow its endpoint",
        || r.digest("team", "coder-me") != before,
    )
    .await;
    r.stop().await;
}
