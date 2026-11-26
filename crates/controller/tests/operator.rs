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
    AgentDirectory, Issue, IssueReason, LeaderElection, OwnerHandle, Phase, Role, RuntimeId,
    RuntimeStatus,
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

async fn start(namespaces: &[&str]) -> Running {
    start_on(Fake::new(), namespaces, true).await
}

/// The operator on `fake`, watching `namespaces` (empty: every namespace). With `shared` false it does not wait
/// for the watches of the two shared kinds, because those are not there to open (`Fake::absent`).
async fn start_on(fake: Fake, namespaces: &[&str], shared: bool) -> Running {
    let watched = namespaces
        .iter()
        .map(|ns| (*ns).to_owned())
        .collect::<Vec<_>>();
    start_where(fake, namespaces, move |fake| {
        // The watches are open, one per namespace (or one of the whole cluster), so nothing below races the
        // list that precedes them.
        let kinds = if watched.is_empty() {
            fake.watchers(SERVICES) >= 1 && fake.watchers(CONFIGS) >= 2
        } else {
            watched
                .iter()
                .all(|ns| fake.watchers_in(SERVICES, ns) >= 1 && fake.watchers_in(CONFIGS, ns) >= 2)
        };
        kinds && (!shared || (fake.watchers(ENDPOINTS) >= 1 && fake.watchers(PROVIDERS) >= 1))
    })
    .await
}

/// The operator on `fake`, started when `ready` says its watches are where the case needs them: a namespace that
/// the fake refuses never opens one, and the case that checks that says so.
async fn start_where(fake: Fake, namespaces: &[&str], ready: impl Fn(&Fake) -> bool) -> Running {
    let options = Options {
        watch_namespaces: (!namespaces.is_empty())
            .then(|| namespaces.iter().map(|ns| (*ns).to_owned()).collect()),
        // Far-away timers: what is asserted here arrives by a trigger or not at all, and a timer
        // that fired in the time we wait would hide a missing trigger.
        resync: Resync {
            settled: Duration::from_secs(3600),
            pending: Duration::from_secs(3600),
            not_installed: Duration::from_secs(3600),
        },
        default_agent_image: Some(DEFAULT_IMAGE.to_owned()),
        // Only a test that stalls a request meets it; the others finish in milliseconds.
        pass_timeout: Duration::from_secs(30),
        ..Options::default()
    };
    start_options(fake, options, ready).await
}

/// The operator on `fake` with `options`, started when `ready` says its watches are where the case needs them: the
/// cases that are about the settings themselves (no namespace at all) compose their own.
async fn start_options(fake: Fake, options: Options, ready: impl Fn(&Fake) -> bool) -> Running {
    start_with_activity(fake, options, ready, None).await
}

async fn start_with_activity(
    fake: Fake,
    options: Options,
    ready: impl Fn(&Fake) -> bool,
    activity: Option<aap_ports::memory::MemoryActivity>,
) -> Running {
    let runtime = Scripted::new();
    let owner: OwnerOf = Arc::new(|_| OwnerHandle::none());
    let mut operator = Operator::new(
        fake.client(),
        runtime.clone(),
        MemoryStore::new(),
        owner,
        options,
    );
    if let Some(source) = activity {
        operator = operator.with_activity(source);
    }
    let directory = operator.directory();
    let readiness = operator.readiness();
    let metrics = operator.metrics();
    let (stop, stopped) = oneshot::channel::<()>();
    let operator_done = tokio::spawn(operator.run(async move {
        let _ = stopped.await;
    }));
    eventually("the controllers' watches", || ready(&fake)).await;
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
    let r = start(&["team"]).await;
    eventually("the caches to sync", || r.readiness.is_ready()).await;
    assert!(r.directory.list().await.unwrap().is_empty());

    let (mut service, config) = example("chat", "team");
    // An audience (AD-028) set on the object reaches the directory entry: the registry publishes from it.
    service["spec"]["access"]["audience"] = json!(["agent.use:chat", "agent.use:admin"]);
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
    assert_eq!(
        entry.audience,
        ["agent.use:chat", "agent.use:admin"],
        "from the object, through the list and the watch, as stated"
    );
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
    let r = start(&[]).await;
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
    let r = start(&[]).await;
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
    let r = start(&[]).await;
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
    let r = start(&[]).await;
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
    let r = start(&[]).await;
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
    let r = start(&[]).await;
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
    let r = start(&["team"]).await;
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
    let r = start(&["team"]).await;
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

/// K6b: one set of controllers and caches per namespace, reconciled independently, and never a request of the whole
/// cluster: what an operator with a `Role` in each namespace may do.
#[tokio::test]
async fn two_namespaces_are_reconciled_independently_and_nothing_is_asked_of_the_whole_cluster() {
    let r = start(&["team", "other"]).await;
    eventually("every cache to sync", || r.readiness.is_ready()).await;

    // One watch of each kind per namespace, and no watch of the whole cluster.
    for ns in ["team", "other"] {
        assert!(
            r.fake.watchers_in(SERVICES, ns) >= 1,
            "the services of {ns}"
        );
        assert!(r.fake.watchers_in(CONFIGS, ns) >= 2, "the configs of {ns}");
    }
    assert_eq!(
        r.fake.watchers(SERVICES),
        r.fake.watchers_in(SERVICES, "team") + r.fake.watchers_in(SERVICES, "other"),
        "a watch of every namespace"
    );

    let (service, config) = example("chat", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    let (service, config) = example("coder", "other");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    eventually("both namespaces Ready", || {
        r.state("team", "chat").as_deref() == Some("Ready")
            && r.state("other", "coder").as_deref() == Some("Ready")
    })
    .await;

    // The directory is the union of the shards' caches, in one order: by scope, then name.
    for _ in 0..1000 {
        if r.directory.list().await.unwrap().len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let names: Vec<String> = r
        .directory
        .list()
        .await
        .unwrap()
        .iter()
        .map(|e| format!("{}/{}", e.scope, e.name))
        .collect();
    assert_eq!(names, ["other/coder", "team/chat"]);

    // Every request of the group was namespaced: a namespaced `Role` is all this operator needs (AD-055).
    for call in r.fake.calls() {
        let Some(rest) = call.path.strip_prefix("/apis/agents.vymalo.com/v1alpha1") else {
            continue;
        };
        assert!(
            rest.starts_with("/namespaces/team/") || rest.starts_with("/namespaces/other/"),
            "a request of the whole cluster: {} {}",
            call.method,
            call.path
        );
    }
    r.stop().await;
}

/// K6b: a namespace the operator may not read (a `Role` that is not there, a namespace that is gone) fails its own
/// caches and nothing else: the other namespace still reconciles, and `/readyz` is false, because an operator that
/// cannot see a namespace it was told to watch is not ready.
#[tokio::test(start_paused = true)]
async fn a_forbidden_namespace_leaves_the_other_reconciling_and_readiness_false() {
    let fake = Fake::new();
    fake.fail_path(None, "/namespaces/blocked/", 403);
    let r = start_where(fake, &["team", "blocked"], |fake| {
        fake.watchers_in(SERVICES, "team") >= 1 && fake.watchers_in(CONFIGS, "team") >= 2
    })
    .await;
    let (service, config) = example("chat", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    within(
        Duration::from_secs(30),
        "the namespace the operator may read to be Ready",
        || r.state("team", "chat").as_deref() == Some("Ready"),
    )
    .await;

    // The refused namespace never opened a watch, and no cache of it ever listed.
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert_eq!(r.fake.watchers_in(SERVICES, "blocked"), 0);
    assert!(
        !r.readiness.is_ready(),
        "a namespace whose caches never synced is not ready"
    );

    // And the other one still reconciles a change: its shard is not held back by the failing one.
    let before = r.digest("team", "chat");
    let mut config = r.fake.get(CONFIGS, "team", "chat").unwrap();
    config["spec"]["harness"]["adam"]["agent"]["folder"]["files"]["instructions.md"] =
        json!("---\nname: chat\n---\nChanged while the other namespace was refused.");
    r.fake.put(CONFIGS, config);
    within(
        Duration::from_secs(30),
        "the readable namespace to follow its config",
        || r.digest("team", "chat") != before,
    )
    .await;
    r.stop().await;
}

/// K6b: readiness waits for the **config** caches too. A namespace whose `Role` lists the services but not the
/// configs would reconcile every agent of it against an empty config cache (each one `Blocked`, or worse, a stale
/// config read from nowhere) while `/readyz` said the operator was ready: the services' own caches listing is not
/// enough to know the operator can see the fleet.
#[tokio::test(start_paused = true)]
async fn a_namespace_whose_config_caches_have_not_listed_keeps_the_operator_unready() {
    let fake = Fake::new();
    fake.fail_path(None, "/namespaces/blocked/agentconfigs", 403);
    let r = start_where(fake, &["team", "blocked"], |fake| {
        fake.watchers_in(SERVICES, "team") >= 1 && fake.watchers_in(CONFIGS, "team") >= 2
    })
    .await;
    // The readable namespace is fully reconciled — which needs its service cache *and* its config cache to have
    // listed — so the only cache that never lists is the refused namespace's.
    let (service, config) = example("chat", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);
    within(
        Duration::from_secs(30),
        "the readable namespace to be Ready",
        || r.state("team", "chat").as_deref() == Some("Ready"),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert!(
        !r.readiness.is_ready(),
        "a namespace whose config cache never listed is not ready, whatever its services' cache did"
    );
    r.stop().await;
}

/// K6b: `Options::watch_namespaces = Some([])` is **no shard at all**, never every namespace: an operator composed
/// that way asks the cluster nothing, reconciles nothing, and never says it is ready. The composition root refuses
/// such a list; this is the library's own promise, and the fallback it must not have (every namespace is `Api::all`,
/// which the chart's namespaced `Role` cannot allow).
#[tokio::test(start_paused = true)]
async fn an_operator_with_no_namespace_is_never_ready_and_asks_the_cluster_nothing() {
    let fake = Fake::new();
    let options = Options {
        watch_namespaces: Some(Vec::new()),
        ..Options::default()
    };
    let r = start_options(fake, options, |_| true).await;
    tokio::time::sleep(Duration::from_secs(300)).await;
    assert!(
        !r.readiness.is_ready(),
        "no shard listed anything, so the operator is not ready"
    );
    assert!(
        r.fake.calls().is_empty(),
        "an operator with no namespace watches nothing, and asks nothing: {:?}",
        r.fake.calls()
    );
    r.stop().await;
}

#[tokio::test]
async fn deleting_a_running_service_runs_the_finalizer_through_the_controller() {
    let r = start(&[]).await;
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
    let r = start(&[]).await;
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
    let r = start_on(fake, &[], false).await;

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
    let r = start_on(fake, &[], false).await;
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

// ------------------------------------------------ a request that is never answered

/// A request that is received and never answered holds the service's slot in kube's `Controller` for as
/// long as the request lives, and nothing else reaches the service meanwhile: not a watch event, not the
/// timer. It happens when a call is queued behind a watch on a pooled connection that hyper 1.11.1 let go
/// of too early (*seen 2026-10-16* on kind v1.35.8: a new service with no status for the 60 s or 240 s a
/// case waited, nothing in the log; the call went out when the watch ended, 290 s later). The first pass
/// of a service is one request, the finalizer's patch, so a service whose patch is never answered is the
/// shape of it. Without a limit on a pass this service has no status after any amount of time.
#[tokio::test(start_paused = true)]
async fn a_request_that_is_never_answered_does_not_leave_a_new_service_without_a_status() {
    let fake = Fake::new();
    fake.stall_next("PATCH", "/agentservices/chat", 1);
    let r = start_on(fake, &["team"], true).await;

    let (service, config) = example("chat", "team");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service);

    // The limit cuts the first pass at 30 s, the error policy waits 5 s, the second pass adds the finalizer
    // and its patch brings the third.
    within(
        Duration::from_secs(90),
        "the service to be Ready although the request of its first pass was never answered",
        || r.state("team", "chat").as_deref() == Some("Ready"),
    )
    .await;
    assert_eq!(
        r.metrics.get(
            aap_controller::metric::PASS_TIMEOUTS,
            &[("controller", "agentservice")]
        ),
        1,
        "one pass was cut short, and it is counted"
    );
    r.stop().await;
}

/// K2: the shared kinds get a status from the controllers that `Operator::run` wires, and follow the agents: a service
/// that becomes `Ready`, a config that stops naming an endpoint (the endpoint it left is told, though the new config
/// does not name it), a service that goes.
#[tokio::test]
async fn a_shared_object_gets_its_status_and_follows_the_agents_that_use_it() {
    let r = start(&[]).await;
    running_dashboard(&r, "team").await;
    let status =
        |plural: &str, name: &str| r.fake.get(plural, "team", name).unwrap()["status"].clone();
    eventually("the endpoint to list its user, Ready", || {
        let s = status(ENDPOINTS, "gateway");
        s["usedBy"] == json!(["coder-me"])
            && s["conditions"].as_array().is_some_and(|c| {
                c.iter()
                    .any(|c| c["type"] == "AgentsReady" && c["status"] == "True")
            })
    })
    .await;
    eventually("the provider to list its user", || {
        status(PROVIDERS, "websearch")["usedBy"] == json!(["coder-me"])
    })
    .await;

    // The config goes inline for its model: the endpoint is no longer named, the provider still is.
    let mut config = r.fake.get(CONFIGS, "team", "coder-me").unwrap();
    config["spec"]["model"] = json!({
        "baseUrl": { "value": "https://models.example/v1" },
        "apiKeySecretRef": { "name": "agent-models", "key": "GATEWAY_API_KEY" },
        "model": "coding-model"
    });
    r.fake.put(CONFIGS, config);
    eventually("the endpoint it left to say so", || {
        status(ENDPOINTS, "gateway")["usedBy"] == json!([])
    })
    .await;
    assert_eq!(
        status(PROVIDERS, "websearch")["usedBy"],
        json!(["coder-me"])
    );

    // The service is deleted: the provider follows.
    r.fake.delete(SERVICES, "team", "coder-me");
    eventually("the provider to lose its user", || {
        status(PROVIDERS, "websearch")["usedBy"] == json!([])
    })
    .await;
    r.stop().await;
}

fn service_passes(r: &Running) -> u64 {
    ["ok", "error"]
        .iter()
        .map(|result| {
            r.metrics.get(
                aap_controller::metric::RECONCILES,
                &[("controller", "agentservice"), ("result", result)],
            )
        })
        .sum()
}

/// K2: the operator writes a status on a shared object after every change of an agent that uses it. That moves the
/// object's `resourceVersion` and not its `generation`, and must not reconcile every service that names it (one flip of
/// `AgentsReady` under two hundred users would be two hundred passes).
#[tokio::test]
async fn a_status_write_on_a_shared_object_reconciles_no_service_and_a_spec_change_does() {
    let r = start(&[]).await;
    running_dashboard(&r, "team").await;
    eventually("the statuses", || {
        r.fake.get(ENDPOINTS, "team", "gateway").unwrap()["status"]["usedBy"] == json!(["coder-me"])
            && r.fake.get(PROVIDERS, "team", "websearch").unwrap()["status"]["usedBy"]
                == json!(["coder-me"])
    })
    .await;
    // Let what the first statuses woke settle.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let before = service_passes(&r);
    for (plural, name) in [(ENDPOINTS, "gateway"), (PROVIDERS, "websearch")] {
        let mut other = r.fake.get(plural, "team", name).unwrap()["status"].clone();
        other["usedBy"] = json!(["someone-else"]);
        r.fake.put_status(plural, "team", name, other);
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        service_passes(&r),
        before,
        "a status-only change of a shared object reconciled a service"
    );

    // The control: the same watch does carry a change of the spec.
    let mut endpoint = r.fake.get(ENDPOINTS, "team", "gateway").unwrap();
    endpoint["spec"]["apiKeySecretRef"]["name"] = json!("rotated-models");
    r.fake.put(ENDPOINTS, endpoint);
    eventually("a pass for the spec change", || service_passes(&r) > before).await;
    r.stop().await;
}

/// The tracer reaches the passes of both controllers when the operator is composed as `bin/operator` composes it
/// (`Operator::new(..).with_tracer(..)`, then `run`): a service and its config that appear are a root span each.
#[tokio::test]
async fn the_tracer_given_to_the_operator_reaches_the_passes_of_both_controllers() {
    let fake = Fake::new();
    let (tracer, memory) = aap_trace::Tracer::in_memory(aap_trace::Sampler::AlwaysOn);
    let owner: OwnerOf = Arc::new(|_| OwnerHandle::none());
    let options = Options {
        watch_namespaces: Some(vec!["team".to_owned()]),
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
        Scripted::new(),
        MemoryStore::new(),
        owner,
        options,
    )
    .with_tracer(tracer);
    let readiness = operator.readiness();
    let (stop, stopped) = oneshot::channel::<()>();
    let done = tokio::spawn(operator.run(async move {
        let _ = stopped.await;
    }));
    eventually("the watches", || {
        fake.watchers(SERVICES) >= 1 && fake.watchers(CONFIGS) >= 2
    })
    .await;
    eventually("the caches to sync", || readiness.is_ready()).await;
    let (service, config) = example("chat", "team");
    fake.put(CONFIGS, config);
    fake.put(SERVICES, service);
    eventually("a root span of each controller", || {
        memory.named("reconcile agentservice").is_some()
            && memory.named("reconcile agentconfig").is_some()
    })
    .await;
    let _ = stop.send(());
    tokio::time::timeout(Duration::from_secs(10), done)
        .await
        .expect("the operator stops when told to")
        .unwrap();
}

// ------------------------------------------------------------- leader election (K7a)

/// An operator on `fake` whose passes are gated by `leader`, started when `ready` says its watches are where the
/// case needs them.
async fn start_led_by(
    fake: Fake,
    options: Options,
    leader: Arc<dyn aap_ports::LeaderElection>,
    ready: impl Fn(&Fake) -> bool,
) -> Running {
    let runtime = Scripted::new();
    let owner: OwnerOf = Arc::new(|_| OwnerHandle::none());
    let operator = Operator::new(
        fake.client(),
        runtime.clone(),
        MemoryStore::new(),
        owner,
        options,
    )
    .with_leader(leader);
    let directory = operator.directory();
    let readiness = operator.readiness();
    let metrics = operator.metrics();
    let (stop, stopped) = oneshot::channel::<()>();
    let operator_done = tokio::spawn(operator.run(async move {
        let _ = stopped.await;
    }));
    eventually("the controllers' watches", || ready(&fake)).await;
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

/// The options of the leader-election cases (K7a): one namespace, timers an hour away (only a trigger or the
/// follower's own recheck makes a pass), the default image.
fn leader_case_options() -> Options {
    Options {
        watch_namespaces: Some(vec!["team".to_owned()]),
        resync: Resync {
            settled: Duration::from_secs(3600),
            pending: Duration::from_secs(3600),
            not_installed: Duration::from_secs(3600),
        },
        default_agent_image: Some(DEFAULT_IMAGE.to_owned()),
        pass_timeout: Duration::from_secs(30),
        ..Options::default()
    }
}

/// K7a: two replicas of one operator, one election. Both keep their caches and watch the cluster; **only the
/// holder's passes write**. When the holder is killed, the other one takes the election over, and it is the one
/// that writes the next change.
///
/// The campaigns are started by hand, as `bin/operator` runs them beside `Operator::run` (the sixth arm of its
/// `select!`).
#[tokio::test(start_paused = true)]
async fn two_replicas_one_leader() {
    use aap_ports::memory::MemoryElection;
    use aap_ports::testkit::{LeaderElectionUnderTest, unique};

    let fake = Fake::new();
    let election = MemoryElection::new(&unique("election"), "a");
    let follower = LeaderElectionUnderTest::candidate(&election, "b");

    // The first replica's campaign, alone, holds the election.
    let (stop_campaign_a, stopped_a) = oneshot::channel::<()>();
    let campaign_a = tokio::spawn(election.clone().run(Box::pin(async move {
        let _ = stopped_a.await;
    })));
    eventually("the first candidate to hold the election", || {
        election.is_leader()
    })
    .await;

    let first = start_led_by(
        fake.clone(),
        leader_case_options(),
        Arc::new(election.clone()),
        |fake| fake.watchers_in(SERVICES, "team") >= 1 && fake.watchers_in(CONFIGS, "team") >= 2,
    )
    .await;
    let (service, config) = example("chat", "team");
    fake.put(CONFIGS, config);
    fake.put(SERVICES, service);
    eventually("the holder's pass to make the agent Ready", || {
        first.state("team", "chat").as_deref() == Some("Ready")
    })
    .await;

    // The second replica joins: it lists and watches the cluster too, and writes nothing. Its runtime reports a
    // **failure**, so its pass would write a different status if it ran at all: what stops it is the election,
    // and not the fact that the status it would write is the one that is there.
    fake.clear_calls();
    let (stop_campaign_b, stopped_b) = oneshot::channel::<()>();
    let campaign_b = tokio::spawn(follower.clone().run(Box::pin(async move {
        let _ = stopped_b.await;
    })));
    let second = start_led_by(
        fake.clone(),
        leader_case_options(),
        Arc::new(follower.clone()),
        |fake| fake.watchers_in(SERVICES, "team") >= 2 && fake.watchers_in(CONFIGS, "team") >= 4,
    )
    .await;
    second.runtime.script(Some(RuntimeStatus {
        phase: Phase::Failed,
        replicas: 0,
        issues: vec![Issue {
            role: Role::All,
            reason: IssueReason::CrashLoop,
            message: "the follower would write this".to_owned(),
        }],
    }));
    assert!(
        second.readiness.is_ready(),
        "a follower's caches list, and it is ready: /readyz is not the election"
    );
    // ... and its registry serves the agents from those caches, so a failover does not blank it.
    let entry = second
        .directory
        .get("team", "chat")
        .await
        .expect("the follower's directory is synced")
        .expect("the follower's registry serves the agent from its own cache");
    assert!(
        !entry.blocked,
        "the holder made the agent Ready, and the follower's cache says so"
    );
    // Several of the follower's 30 s rechecks, and several renewals of the holder's.
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert!(!follower.is_leader(), "the second replica holds it too");
    assert!(
        fake.writes().is_empty(),
        "the follower wrote: {:?}",
        fake.writes()
    );
    second.runtime.script(None);

    // The holder is killed: the second replica takes the election over.
    election.kill().await;
    eventually("the second replica to hold the election", || {
        follower.is_leader()
    })
    .await;
    fake.clear_calls();

    // And it is the one that writes the next change: the first replica's passes write nothing, for ever after.
    let before = first.digest("team", "chat");
    let mut config = fake.get(CONFIGS, "team", "chat").unwrap();
    config["spec"]["harness"]["adam"]["agent"]["folder"]["files"]["instructions.md"] =
        json!("---\nname: chat\n---\nChanged while the second replica held the election.");
    fake.put(CONFIGS, config);
    within(
        Duration::from_secs(120),
        "the new holder to write the change",
        || {
            fake.writes()
                .iter()
                .any(|call| call.method == "PATCH" && call.path.contains("/agentservices/"))
        },
    )
    .await;
    assert_ne!(
        first.digest("team", "chat"),
        before,
        "the change reached the service"
    );
    // The first replica is a follower for ever after: every write there is, is the second one's.
    assert!(
        fake.writes()
            .iter()
            .any(|call| call.method == "PATCH" && call.path.contains("/agentservices/")),
        "the new holder wrote the status: {:?}",
        fake.writes()
    );
    assert!(!election.is_leader(), "the killed holder writes nothing");

    second.stop().await;
    first.stop().await;
    let _ = stop_campaign_b.send(());
    let _ = stop_campaign_a.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), campaign_b).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), campaign_a).await;
}

/// K7a: a replica that is not the holder writes **nothing at all**, for each of the four controllers: the whole
/// dashboard (an agent and the two objects its config names) appears while the replica watches, and after several
/// of its rechecks the service has no finalizer and no status, and the two shared kinds have none either.
///
/// The control is the same replica once its campaign runs: it holds the election and writes the status.
#[tokio::test(start_paused = true)]
async fn a_follower_replica_writes_nothing() {
    use aap_ports::memory::MemoryElection;
    use aap_ports::testkit::unique;

    let fake = Fake::new();
    // A candidate whose campaign nobody runs: it never holds the election, and it is not dead either.
    let election = MemoryElection::new(&unique("election"), "follower");
    let follower = start_led_by(
        fake.clone(),
        leader_case_options(),
        Arc::new(election.clone()),
        |fake| fake.watchers_in(SERVICES, "team") >= 1 && fake.watchers_in(CONFIGS, "team") >= 2,
    )
    .await;

    let (service, config, endpoint, provider) = dashboard("team");
    fake.put(ENDPOINTS, endpoint);
    fake.put(PROVIDERS, provider);
    fake.put(CONFIGS, config);
    fake.put(SERVICES, service);
    // Several of the follower's 30 s rechecks: every object is reconciled again and again, and nothing is written.
    tokio::time::sleep(Duration::from_secs(120)).await;
    assert!(!election.is_leader());
    assert!(
        follower.readiness.is_ready(),
        "a follower's caches list, and it is ready: /readyz is not the election"
    );
    assert!(
        fake.writes().is_empty(),
        "a follower wrote: {:?}",
        fake.writes()
    );
    let svc = fake.get(SERVICES, "team", "coder-me").expect("the service");
    assert!(
        svc["metadata"]["finalizers"].is_null(),
        "a follower added the finalizer: {svc}"
    );
    assert!(
        svc["status"].is_null(),
        "a follower wrote the service's status: {svc}"
    );
    for (plural, name) in [
        (CONFIGS, "coder-me"),
        (ENDPOINTS, "gateway"),
        (PROVIDERS, "websearch"),
    ] {
        assert!(
            fake.get(plural, "team", name).expect(name)["status"].is_null(),
            "a follower wrote the status of {plural}/{name}"
        );
    }

    // The control: the campaign starts, the replica holds the election, and the same objects are written.
    let (stop, stopped) = oneshot::channel::<()>();
    let campaign = tokio::spawn(election.clone().run(Box::pin(async move {
        let _ = stopped.await;
    })));
    eventually("the replica to hold the election", || election.is_leader()).await;
    within(
        Duration::from_secs(120),
        "the holder to write the service's status",
        || {
            fake.get(SERVICES, "team", "coder-me")
                .is_some_and(|svc| !svc["status"].is_null())
        },
    )
    .await;
    assert!(
        !fake.writes().is_empty(),
        "the holder wrote nothing either, so the case proves nothing"
    );
    follower.stop().await;
    let _ = stop.send(());
    let _ = tokio::time::timeout(Duration::from_secs(10), campaign).await;
}

fn phase_of(r: &Running, ns: &str, name: &str) -> Option<String> {
    r.fake
        .get(SERVICES, ns, name)?
        .pointer("/status/runtime/phase")?
        .as_str()
        .map(str::to_owned)
}

#[tokio::test]
async fn an_idle_agent_sleeps_and_a_removed_policy_wakes_it() {
    let source = aap_ports::memory::MemoryActivity::new();
    let id = RuntimeId::new("team", "chat");
    source.set(
        &id,
        aap_ports::Activity {
            busy: 1,
            next_due_at: None,
        },
    );
    let options = Options {
        watch_namespaces: Some(vec!["team".to_owned()]),
        resync: Resync {
            settled: Duration::from_secs(3600),
            pending: Duration::from_secs(3600),
            not_installed: Duration::from_secs(3600),
        },
        default_agent_image: Some(DEFAULT_IMAGE.to_owned()),
        sleep_poll: Duration::from_millis(50),
        ..Options::default()
    };
    let r = start_with_activity(
        Fake::new(),
        options,
        |f| f.watchers_in(SERVICES, "team") >= 1 && f.watchers_in(CONFIGS, "team") >= 2,
        Some(source.clone()),
    )
    .await;
    eventually("the caches to sync", || r.readiness.is_ready()).await;

    let (mut service, config) = example("chat", "team");
    // The fake does not run the CRD's CEL, so a one-second timeout stands for ten minutes.
    service["spec"]["scaling"]["idleTimeout"] = json!("1s");
    r.fake.put(CONFIGS, config);
    r.fake.put(SERVICES, service.clone());
    eventually("the service to be Ready", || {
        r.state("team", "chat").as_deref() == Some("Ready")
    })
    .await;

    // Busy: it is asked, and it stays up.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!source.asked().is_empty(), "the loop polled");
    assert_eq!(r.state("team", "chat").as_deref(), Some("Ready"));

    // Idle for the timeout: scaled to zero, shown as Sleeping, and an ensure does not wake it.
    source.set(&id, aap_ports::Activity::IDLE);
    // The scripted runtime's `watch` speaks only when the test says so; a real provider's does when the workload scales.
    within(Duration::from_secs(15), "the agent to sleep", || {
        r.runtime.signal(&id);
        phase_of(&r, "team", "chat").as_deref() == Some("Sleeping")
    })
    .await;
    assert_eq!(r.state("team", "chat").as_deref(), Some("Suspended"));
    assert_eq!(
        r.metrics.get(aap_controller::metric::SLEEPS, &[]),
        1,
        "slept once"
    );
    r.runtime.signal(&id);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(phase_of(&r, "team", "chat").as_deref(), Some("Sleeping"));

    // The person takes the policy away: the next pass ensures it running again.
    service["spec"]["scaling"]["idleTimeout"] = Value::Null;
    r.fake.put(SERVICES, service);
    within(Duration::from_secs(15), "the agent to wake", || {
        phase_of(&r, "team", "chat").as_deref() == Some("Ready")
    })
    .await;
    r.stop().await;
}
