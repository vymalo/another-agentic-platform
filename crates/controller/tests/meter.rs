//! The pod-minute meter (ANR-673, AD-069), running: `Operator::with_meter` against the fake API server's pod list,
//! a `MemoryUsageSink` and a fake `AccountSource`. What these show is the wiring: which pods are read, what a line
//! carries, what makes no line and is counted, that a restart or a second replica makes the same events, and that a
//! follower sends nothing. The counting rules are `metering`'s own unit tests.
//!
//! Times: the meter reads the real clock. A case that needs an exact answer uses a **finished** pod (its minutes do
//! not depend on `now`); the Running case asserts a prefix.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use aap_controller::{MeterConfig, Operator, Options, OwnerOf};
use aap_ports::memory::MemoryStore;
use aap_ports::{LeaderElection, OwnerHandle};
use aap_usage::{
    AccountSource, AccountUnavailable, MemoryAccounts, MemoryUsageSink, PodKind, PodMinuteEvent,
    PodRef, UsageError, UsageSink, event_id,
};
use serde_json::{Value, json};
use support::Fake;
use support::scripted::Scripted;
use tokio::sync::oneshot;

const NS: &str = "team";
const KEY: &str = "node-type";

/// A sink or a source that the case keeps a handle on while the operator owns the other.
struct Shared<T>(Arc<T>);
impl<T: UsageSink> UsageSink for Shared<T> {
    async fn send(&self, e: PodMinuteEvent) -> Result<(), UsageError> {
        self.0.send(e).await
    }
    fn outcomes(&self) -> Option<Vec<aap_usage::Outcome>> {
        self.0.outcomes()
    }
}
impl<T: AccountSource> AccountSource for Shared<T> {
    async fn account(&self, p: &PodRef) -> Result<Option<String>, AccountUnavailable> {
        self.0.account(p).await
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn ts(secs: i64) -> String {
    k8s_openapi::jiff::Timestamp::from_second(secs)
        .unwrap()
        .to_string()
}

/// The annotation that carries a run pod's full id (ANR-1160).
const RUN_ID: &str = "adam.vymalo.com/run-id";

/// adam-rs's `adam-env-kubernetes` `names::run_hash` (at `e8809b4d`), copied for the fixtures: the label a coder
/// puts on a run pod is this hash of the run id, never the id.
fn run_hash(run: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(run.as_bytes())
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Makes `pod` a pod of the run `run`, as the coder labels and annotates it.
fn set_run(pod: &mut aap_controller::metering::PodFacts, run: &str) {
    pod.labels
        .insert("adam.vymalo.com/run".into(), run_hash(run));
    pod.annotations.insert(RUN_ID.into(), run.into());
}

fn affinity(values: &[&str]) -> Value {
    json!({ "requiredDuringSchedulingIgnoredDuringExecution": { "nodeSelectorTerms": [
        { "matchExpressions": [{ "key": KEY, "operator": "In", "values": values }] }
    ]}})
}

/// A run pod of the service `coder`, run `run`, finished: it ran `start..end` (seconds since the epoch).
fn finished_run(name: &str, uid: &str, run: &str, start: i64, end: i64) -> Value {
    json!({
        "apiVersion": "v1", "kind": "Pod",
        "metadata": {
            "name": name, "namespace": NS, "uid": uid,
            "labels": {
                "app.kubernetes.io/managed-by": "adam-coder",
                "app.kubernetes.io/component": "run",
                "app.kubernetes.io/name": "coder-run",
                "adam.vymalo.com/run": run_hash(run),
            },
            "annotations": {
                "agents.vymalo.com/size-weight": "2",
                "adam.vymalo.com/run-id": run,
            },
        },
        "spec": { "affinity": { "nodeAffinity": affinity(&["big"]) } },
        "status": {
            "phase": "Succeeded",
            "containerStatuses": [{
                "name": "run", "image": "x", "imageID": "x", "ready": false, "restartCount": 0,
                "state": { "terminated": {
                    "exitCode": 0, "startedAt": ts(start), "finishedAt": ts(end),
                }},
            }],
        },
    })
}

fn running_container(start: i64) -> Value {
    json!([{
        "name": "run", "image": "x", "imageID": "x", "ready": true, "restartCount": 0,
        "state": { "running": { "startedAt": ts(start) } },
    }])
}

struct Rig {
    fake: Fake,
    sink: Arc<MemoryUsageSink>,
    accounts: Arc<MemoryAccounts>,
    metrics: Arc<aap_controller::Metrics>,
    stop: oneshot::Sender<()>,
    done: tokio::task::JoinHandle<()>,
}

fn options() -> Options {
    Options {
        watch_namespaces: Some(vec![NS.to_owned()]),
        default_agent_image: Some("x".to_owned()),
        pass_timeout: Duration::from_secs(30),
        ..Options::default()
    }
}

fn config() -> MeterConfig {
    MeterConfig {
        source: "meter-test".to_owned(),
        node_type_label_key: Some(KEY.to_owned()),
        period: Duration::from_millis(20),
        ..MeterConfig::default()
    }
}

async fn start(fake: &Fake, leader: Option<Arc<dyn LeaderElection>>) -> Rig {
    let sink = Arc::new(MemoryUsageSink::new());
    let accounts = Arc::new(MemoryAccounts::new());
    accounts.set_run("coder", "run-1", "acct-1");
    accounts.set_agent(NS, "coder", "acct-agent");
    let owner: OwnerOf = Arc::new(|_| OwnerHandle::none());
    let mut operator = Operator::new(
        fake.client(),
        Scripted::new(),
        MemoryStore::new(),
        owner,
        options(),
    )
    .with_meter(Shared(sink.clone()), Shared(accounts.clone()))
    .with_meter_config(config());
    if let Some(leader) = leader {
        operator = operator.with_leader(leader);
    }
    let metrics = operator.metrics();
    let (stop, stopped) = oneshot::channel::<()>();
    let done = tokio::spawn(operator.run(async move {
        let _ = stopped.await;
    }));
    Rig {
        fake: fake.clone(),
        sink,
        accounts,
        metrics,
        stop,
        done,
    }
}

impl Rig {
    async fn stop(self) {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(10), self.done)
            .await
            .expect("the operator stops")
            .unwrap();
    }
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// Let the loop pass at least `n` more times.
async fn passes(n: u64) {
    tokio::time::sleep(Duration::from_millis(20 * n + 60)).await;
}

fn sorted(mut events: Vec<PodMinuteEvent>) -> Vec<PodMinuteEvent> {
    events.sort_by(|a, b| (&a.event_id, a.minute).cmp(&(&b.event_id, b.minute)));
    events
}

#[tokio::test]
async fn a_finished_run_pod_gives_one_line_per_minute_with_the_right_fields() {
    let fake = Fake::new();
    // 12:00:30 to 12:03:10 relative: minutes m0, m0+1, m0+2 and m0+3.
    let m0 = (now() / 60 - 30) as u64;
    let start = m0 as i64 * 60 + 30;
    let end = (m0 as i64 + 3) * 60 + 10;
    fake.put_pod(finished_run("r1", "uid-1", "run-1", start, end));
    let rig = start_rig(&fake).await;
    eventually("four lines", || rig.sink.taken().len() >= 4).await;
    passes(3).await;
    let got = rig.sink.taken();
    assert_eq!(
        got.len(),
        4,
        "one line per minute, and each minute once: {got:?}"
    );
    for (i, e) in got.iter().enumerate() {
        let minute = m0 + i as u64;
        assert_eq!(
            e,
            &PodMinuteEvent {
                source: "meter-test".into(),
                event_id: event_id("uid-1", minute),
                minute,
                subject: "acct-1".into(),
                weight_quarters: 8,
                node_type: "big".into(),
                pod_kind: PodKind::Run,
                agent_service: "coder".into(),
                run: Some("run-1".into()),
            }
        );
        assert_eq!(e.end_unix_nanos(), (minute + 1) * 60 * 1_000_000_000);
    }
    rig.stop().await;
}

async fn start_rig(fake: &Fake) -> Rig {
    start(fake, None).await
}

#[tokio::test]
async fn a_running_run_pod_gives_a_line_for_each_minute_that_is_over() {
    let fake = Fake::new();
    let m_now = now() / 60;
    let m0 = (m_now - 5) as u64;
    let mut pod = finished_run("r1", "uid-1", "run-1", 0, 0);
    pod["status"] =
        json!({ "phase": "Running", "containerStatuses": running_container(m0 as i64 * 60 + 5) });
    fake.put_pod(pod);
    let rig = start_rig(&fake).await;
    eventually("five minutes", || rig.sink.taken().len() >= 5).await;
    let got = rig.sink.taken();
    for (i, e) in got.iter().take(5).enumerate() {
        assert_eq!(e.minute, m0 + i as u64);
        assert_eq!(e.event_id, event_id("uid-1", m0 + i as u64));
    }
    // The minute in progress is not sent before it is over.
    assert!(
        got.iter().all(|e| (e.minute + 1) * 60 <= now() as u64),
        "{got:?}"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_pending_pod_gives_no_line() {
    let fake = Fake::new();
    // A Pending pod whose container nevertheless reports a start: only the Pending rule keeps it out.
    let started = (now() / 60 - 10) * 60;
    let mut pod = finished_run("r1", "uid-1", "run-1", 0, 0);
    pod["status"] = json!({ "phase": "Pending", "containerStatuses": running_container(started) });
    fake.put_pod(pod);
    let rig = start_rig(&fake).await;
    passes(5).await;
    assert!(rig.sink.taken().is_empty());
    rig.stop().await;
}

#[tokio::test]
async fn the_egress_proxy_gives_no_line() {
    let fake = Fake::new();
    let m0 = now() / 60 - 10;
    let mut proxy = finished_run("proxy", "uid-proxy", "run-1", m0 * 60, (m0 + 2) * 60);
    proxy["metadata"]["labels"] = json!({
        "app.kubernetes.io/managed-by": "aap-operator",
        "app.kubernetes.io/component": "egress-proxy",
        "app.kubernetes.io/instance": "coder",
        "app.kubernetes.io/name": "coder-egress",
    });
    fake.put_pod(proxy);
    // A real run pod beside it, so that "nothing" is not "the meter did not run".
    fake.put_pod(finished_run("r1", "uid-1", "run-1", m0 * 60, (m0 + 2) * 60));
    let rig = start_rig(&fake).await;
    eventually("the run pod's lines", || rig.sink.taken().len() >= 2).await;
    passes(3).await;
    let got = rig.sink.taken();
    assert!(!got.is_empty());
    assert!(
        got.iter()
            .all(|e| e.event_id != event_id("uid-proxy", e.minute)),
        "the proxy was metered: {got:?}"
    );
    assert_eq!(got.len(), 2);
    rig.stop().await;
}

#[tokio::test]
async fn an_unknown_account_gives_no_line_and_is_counted() {
    let fake = Fake::new();
    let m0 = now() / 60 - 10;
    fake.put_pod(finished_run(
        "r9",
        "uid-9",
        "run-unknown",
        m0 * 60,
        (m0 + 3) * 60,
    ));
    let rig = start_rig(&fake).await;
    let metrics_rig = rig;
    passes(6).await;
    assert!(metrics_rig.sink.taken().is_empty());
    // Three minutes, counted once each, whatever the number of passes.
    assert_eq!(
        metrics_rig.metrics.get(
            aap_controller::metric::METER_UNATTRIBUTED,
            &[("reason", "unknown-account")]
        ),
        3
    );
    assert_eq!(
        metrics_rig
            .metrics
            .get(aap_controller::metric::METER_EVENTS, &[]),
        0
    );
    assert!(metrics_rig.accounts.asked().contains(&PodRef::Run {
        service: "coder".into(),
        run: "run-unknown".into()
    }));
    metrics_rig.stop().await;
}

#[tokio::test]
async fn an_unavailable_account_source_holds_the_minutes_and_sends_them_later() {
    let fake = Fake::new();
    let m0 = now() / 60 - 10;
    fake.put_pod(finished_run("r1", "uid-1", "run-1", m0 * 60, (m0 + 2) * 60));
    let sink = Arc::new(MemoryUsageSink::new());
    let accounts = Arc::new(MemoryAccounts::new());
    accounts.set_run("coder", "run-1", "acct-1");
    accounts.set_broken(true);
    let owner: OwnerOf = Arc::new(|_| OwnerHandle::none());
    let operator = Operator::new(
        fake.client(),
        Scripted::new(),
        MemoryStore::new(),
        owner,
        options(),
    )
    .with_meter(Shared(sink.clone()), Shared(accounts.clone()))
    .with_meter_config(config());
    let metrics = operator.metrics();
    let (stop, stopped) = oneshot::channel::<()>();
    let done = tokio::spawn(operator.run(async move {
        let _ = stopped.await;
    }));
    passes(5).await;
    assert!(
        sink.taken().is_empty(),
        "nothing is sent while the source is down"
    );
    assert!(metrics.get(aap_controller::metric::METER_ACCOUNT_FAILURES, &[]) >= 2);
    accounts.set_broken(false);
    eventually("the held minutes", || sink.taken().len() == 2).await;
    let _ = stop.send(());
    done.await.unwrap();
    assert_eq!(
        metrics.get(
            aap_controller::metric::METER_UNATTRIBUTED,
            &[("reason", "unknown-account")]
        ),
        0
    );
}

#[tokio::test]
async fn a_node_type_line_carries_it_and_several_admitted_values_give_none() {
    let fake = Fake::new();
    let m0 = now() / 60 - 10;
    fake.put_pod(finished_run(
        "one",
        "uid-one",
        "run-1",
        m0 * 60,
        (m0 + 1) * 60,
    ));
    let mut two = finished_run("two", "uid-two", "run-1", m0 * 60, (m0 + 1) * 60);
    two["spec"]["affinity"]["nodeAffinity"] = affinity(&["big", "std"]);
    fake.put_pod(two);
    let rig = start_rig(&fake).await;
    eventually("the line of the pod with one node type", || {
        !rig.sink.taken().is_empty()
    })
    .await;
    passes(4).await;
    let got = rig.sink.taken();
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].node_type, "big");
    assert_eq!(got[0].event_id, event_id("uid-one", m0 as u64));
    assert_eq!(
        rig.metrics.get(aap_controller::metric::METER_EVENTS, &[]),
        1
    );
    assert_eq!(
        rig.metrics.get(
            aap_controller::metric::METER_UNATTRIBUTED,
            &[("reason", "unknown-node-type")]
        ),
        1,
        "the pod that admits two node types: its one minute, counted"
    );
    rig.stop().await;
}

#[tokio::test]
async fn an_agent_pod_has_no_weight_yet_and_gives_no_line() {
    let fake = Fake::new();
    let m0 = now() / 60 - 10;
    let mut agent = finished_run("agent-0", "uid-agent", "run-1", m0 * 60, (m0 + 2) * 60);
    agent["metadata"]["labels"] = json!({
        "app.kubernetes.io/managed-by": "aap-operator",
        "app.kubernetes.io/component": "agent",
        "app.kubernetes.io/instance": "coder",
        "app.kubernetes.io/name": "coder",
    });
    agent["metadata"]["annotations"] = json!({});
    fake.put_pod(agent.clone());
    let rig = start_rig(&fake).await;
    passes(5).await;
    assert!(
        rig.sink.taken().is_empty(),
        "an agent pod has no size weight: a later slice"
    );
    assert_eq!(
        rig.metrics.get(
            aap_controller::metric::METER_UNATTRIBUTED,
            &[("reason", "no-weight")]
        ),
        2
    );
    // When the annotation is there (a later slice), the same pod is metered, as an agent pod with no run.
    agent["metadata"]["annotations"] = json!({ "agents.vymalo.com/size-weight": "1" });
    agent["metadata"]["name"] = json!("agent-1");
    agent["metadata"]["uid"] = json!("uid-agent-2");
    fake.put_pod(agent);
    eventually("the weighted agent pod's lines", || {
        rig.sink.taken().len() >= 2
    })
    .await;
    let got = rig.sink.taken();
    assert!(
        got.iter()
            .all(|e| e.pod_kind == PodKind::Agent && e.run.is_none() && e.subject == "acct-agent")
    );
    assert!(got.iter().all(|e| e.weight_quarters == 4));
    rig.stop().await;
}

#[tokio::test]
async fn a_restart_and_a_second_replica_make_the_same_events() {
    let fake = Fake::new();
    let m0 = now() / 60 - 20;
    fake.put_pod(finished_run("r1", "uid-1", "run-1", m0 * 60, (m0 + 4) * 60));
    fake.put_pod(finished_run(
        "r2",
        "uid-2",
        "run-1",
        (m0 + 1) * 60 + 7,
        (m0 + 3) * 60 + 7,
    ));

    let first = start_rig(&fake).await;
    eventually("the first replica's lines", || {
        first.sink.taken().len() >= 7
    })
    .await;
    passes(3).await;
    let before = sorted(first.sink.taken());
    assert_eq!(before.len(), 7, "{before:?}");
    first.stop().await;

    // A new operator over the same pods (a restart), and at the same time another one (a second replica).
    let restarted = start_rig(&fake).await;
    let second = start_rig(&fake).await;
    eventually("the restarted replica's lines", || {
        restarted.sink.taken().len() >= 7
    })
    .await;
    eventually("the second replica's lines", || {
        second.sink.taken().len() >= 7
    })
    .await;
    passes(3).await;
    assert_eq!(sorted(restarted.sink.taken()), before);
    assert_eq!(sorted(second.sink.taken()), before);
    restarted.stop().await;
    second.stop().await;
}

#[tokio::test]
async fn a_pod_that_finished_while_the_operator_was_down_is_closed_from_its_finish() {
    let fake = Fake::new();
    let m0 = now() / 60 - 15;
    // A pod the first operator saw running, then finished at m0+2 while nobody watched.
    let mut pod = finished_run("r1", "uid-1", "run-1", m0 * 60, m0 * 60);
    pod["status"] = json!({ "phase": "Running", "containerStatuses": running_container(m0 * 60) });
    fake.put_pod(pod);
    let first = start_rig(&fake).await;
    eventually("the running pod's lines", || first.sink.taken().len() >= 10).await;
    first.stop().await;
    // It ended 2 minutes after it began; its status says so. The new operator ends it there, not at `now`.
    fake.put_pod(finished_run("r1", "uid-1", "run-1", m0 * 60, (m0 + 2) * 60));
    let second = start_rig(&fake).await;
    eventually("the closed lines", || second.sink.taken().len() >= 2).await;
    passes(4).await;
    let minutes: Vec<u64> = second.sink.taken().iter().map(|e| e.minute).collect();
    assert_eq!(
        minutes,
        vec![m0 as u64, m0 as u64 + 1],
        "closed at its finishedAt, not at now"
    );
    second.stop().await;
}

#[tokio::test]
async fn backfill_is_bounded_to_a_day_and_counted() {
    let fake = Fake::new();
    let m_now = now() / 60;
    // Finished 2 days ago, ran 3 minutes: all older than the bound.
    let m_old = m_now - 2 * 24 * 60;
    fake.put_pod(finished_run(
        "old",
        "uid-old",
        "run-1",
        m_old * 60,
        (m_old + 3) * 60,
    ));
    // One that straddles the bound: 24h + 2 minutes ago to 24h - 2 minutes ago.
    let edge = m_now - 24 * 60;
    fake.put_pod(finished_run(
        "edge",
        "uid-edge",
        "run-1",
        (edge - 2) * 60,
        (edge + 2) * 60,
    ));
    let sink = Arc::new(MemoryUsageSink::new());
    let accounts = Arc::new(MemoryAccounts::new());
    accounts.set_run("coder", "run-1", "acct-1");
    let owner: OwnerOf = Arc::new(|_| OwnerHandle::none());
    let operator = Operator::new(
        fake.client(),
        Scripted::new(),
        MemoryStore::new(),
        owner,
        options(),
    )
    .with_meter(Shared(sink.clone()), Shared(accounts))
    .with_meter_config(config());
    let metrics = operator.metrics();
    let (stop, stopped) = oneshot::channel::<()>();
    let done = tokio::spawn(operator.run(async move {
        let _ = stopped.await;
    }));
    eventually("the lines inside the bound", || !sink.taken().is_empty()).await;
    passes(4).await;
    let _ = stop.send(());
    done.await.unwrap();
    let got = sink.taken();
    assert!(
        got.iter()
            .all(|e| e.event_id != event_id("uid-old", e.minute)),
        "{got:?}"
    );
    assert!(got.iter().all(|e| e.minute >= m_now as u64 - 24 * 60 - 1));
    let skipped = metrics.get(aap_controller::metric::METER_BACKFILL_SKIPPED, &[]);
    assert!(
        skipped >= 3 + 2,
        "the old pod's 3 and the edge pod's 2 older minutes: {skipped}"
    );
}

#[tokio::test]
async fn a_follower_sends_nothing_and_lists_nothing() {
    use aap_ports::memory::MemoryElection;
    use aap_ports::testkit::unique;
    let fake = Fake::new();
    let m0 = now() / 60 - 10;
    fake.put_pod(finished_run("r1", "uid-1", "run-1", m0 * 60, (m0 + 2) * 60));
    // Nobody campaigns for this election, so this replica is never the leader.
    let election = MemoryElection::new(&unique("meter-election"), "follower");
    assert!(!election.is_leader());
    let rig = start(&fake, Some(Arc::new(election.clone()))).await;
    passes(8).await;
    assert!(rig.sink.taken().is_empty(), "a follower writes nothing");
    assert!(
        !rig.fake.calls().iter().any(|c| c.path.contains("/pods")),
        "and reads no pod"
    );
    assert!(rig.accounts.asked().is_empty());
    rig.stop().await;
    // The same cluster with a leader: it sends.
    let leader_election = MemoryElection::new(&unique("meter-election-2"), "leader");
    let (stop_campaign, stopped) = oneshot::channel::<()>();
    let campaign = tokio::spawn(leader_election.clone().run(Box::pin(async move {
        let _ = stopped.await;
    })));
    eventually("the election", || leader_election.is_leader()).await;
    let led = start(&fake, Some(Arc::new(leader_election.clone()))).await;
    eventually("the leader's lines", || led.sink.taken().len() >= 2).await;
    led.stop().await;
    let _ = stop_campaign.send(());
    let _ = campaign.await;
}

#[tokio::test]
async fn nothing_is_asked_of_pods_the_meter_does_not_select() {
    let fake = Fake::new();
    let m0 = now() / 60 - 10;
    let mut other = finished_run("other", "uid-other", "run-1", m0 * 60, (m0 + 2) * 60);
    other["metadata"]["labels"] = json!({ "app": "something-else" });
    fake.put_pod(other);
    let rig = start_rig(&fake).await;
    passes(5).await;
    assert!(rig.sink.taken().is_empty());
    assert!(rig.accounts.asked().is_empty());
    // The two selectors the meter lists with: run pods of the coder's manager, and the operator's own.
    let selectors: Vec<String> = rig
        .fake
        .calls()
        .iter()
        .filter(|c| c.path.ends_with("/pods"))
        .map(|c| c.query.clone())
        .collect();
    assert!(
        selectors.iter().any(|q| q.contains("adam-coder")),
        "{selectors:?}"
    );
    assert!(
        selectors.iter().any(|q| q.contains("aap-operator")),
        "{selectors:?}"
    );
    rig.stop().await;
}

// ---- the meter driven directly, at times of the case's own choosing

use aap_controller::metering::{self, PodFacts};
use aap_controller::{Meter, Metrics, erase_accounts, erase_sink};

struct Direct {
    sink: Arc<MemoryUsageSink>,
    accounts: Arc<MemoryAccounts>,
    metrics: Arc<Metrics>,
    meter: Meter,
}

fn direct(configure: impl FnOnce(&mut MeterConfig)) -> Direct {
    let sink = Arc::new(MemoryUsageSink::new());
    let accounts = Arc::new(MemoryAccounts::new());
    accounts.set_run("coder", "run-1", "acct-1");
    let metrics = Arc::new(Metrics::new());
    let mut cfg = config();
    cfg.skew_secs = 0;
    configure(&mut cfg);
    let meter = Meter::new(
        erase_sink(Shared(sink.clone())),
        erase_accounts(Shared(accounts.clone())),
        cfg,
        metrics.clone(),
    );
    Direct {
        sink,
        accounts,
        metrics,
        meter,
    }
}

/// A run pod's facts, Running since `start`.
fn facts(uid: &str, start: i64) -> PodFacts {
    let labels = [
        ("app.kubernetes.io/managed-by", "adam-coder"),
        ("app.kubernetes.io/component", "run"),
        ("app.kubernetes.io/name", "coder-run"),
    ];
    let mut labels: std::collections::BTreeMap<String, String> = labels
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    labels.insert("adam.vymalo.com/run".into(), run_hash("run-1"));
    PodFacts {
        namespace: NS.into(),
        uid: uid.into(),
        labels,
        annotations: [
            ("agents.vymalo.com/size-weight".to_owned(), "1".to_owned()),
            (RUN_ID.to_owned(), "run-1".to_owned()),
        ]
        .into(),
        phase: "Running".into(),
        started_at: Some(start),
        has_container_status: true,
        node_affinity: Some(affinity(&["big"])),
        ..PodFacts::default()
    }
}

fn minutes_of(d: &Direct, uid: &str) -> Vec<u64> {
    d.sink
        .taken()
        .iter()
        .filter(|e| e.event_id == event_id(uid, e.minute))
        .map(|e| e.minute)
        .collect()
}

#[tokio::test]
async fn a_pod_that_vanishes_is_closed_through_the_minute_it_was_last_seen_running() {
    let d = direct(|_| {});
    let pod = facts("uid-1", 600); // minute 10
    d.meter.pass(std::slice::from_ref(&pod), 1_000).await; // minute 16: 10..=15 are over
    assert_eq!(minutes_of(&d, "uid-1"), (10..16).collect::<Vec<_>>());
    d.meter.pass(&[pod], 1_100).await; // seen Running in minute 18 (second 1100); 16 and 17 are over
    assert_eq!(minutes_of(&d, "uid-1"), (10..18).collect::<Vec<_>>());
    // Gone from a complete listing, at 1_300 (minute 21): closed through minute 18, the one it was last seen in.
    let n = d.meter.close_vanished(&Default::default(), 1_300).await;
    assert_eq!(n, 1);
    assert_eq!(minutes_of(&d, "uid-1"), (10..19).collect::<Vec<_>>());
    // Closed once: nothing more, however often it is asked.
    assert_eq!(d.meter.close_vanished(&Default::default(), 1_400).await, 0);
    d.meter.pass(&[], 1_500).await;
    assert_eq!(minutes_of(&d, "uid-1"), (10..19).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_vanished_pod_whose_last_minute_is_not_over_waits_for_it() {
    let d = direct(|_| {});
    d.meter.pass(&[facts("uid-1", 600)], 1_100).await; // last seen in minute 18
    // Vanished at 1_110, still inside minute 18: that minute is not over, so it is not billed yet.
    assert_eq!(d.meter.close_vanished(&Default::default(), 1_110).await, 0);
    assert_eq!(minutes_of(&d, "uid-1").last(), Some(&17));
    // Once it is over, it is.
    assert_eq!(d.meter.close_vanished(&Default::default(), 1_141).await, 1);
    assert_eq!(minutes_of(&d, "uid-1").last(), Some(&18));
}

#[tokio::test]
async fn a_pod_still_listed_is_not_closed() {
    let d = direct(|_| {});
    let pod = facts("uid-1", 600);
    d.meter.pass(std::slice::from_ref(&pod), 1_100).await;
    let listed = ["uid-1".to_owned()].into_iter().collect();
    assert_eq!(d.meter.close_vanished(&listed, 1_300).await, 0);
    assert_eq!(minutes_of(&d, "uid-1").last(), Some(&17));
}

#[tokio::test]
async fn a_terminating_pod_is_not_billed_into_the_minute_its_grace_ends_in() {
    let d = direct(|c| c.skew_secs = 5);
    let mut pod = facts("uid-1", 120); // minute 2
    pod.deleted_at = Some(270); // the grace ends in minute 4
    d.meter.pass(&[pod.clone()], 200).await; // minute 3 is under way
    assert_eq!(minutes_of(&d, "uid-1"), vec![2]);
    d.meter.pass(&[pod.clone()], 250).await; // minute 4 is under way, the grace end is still ahead
    assert_eq!(minutes_of(&d, "uid-1"), vec![2, 3]);
    d.meter.pass(&[pod], 400).await; // the grace end has passed: it ended in minute 4
    assert_eq!(minutes_of(&d, "uid-1"), vec![2, 3, 4]);
}

#[tokio::test]
async fn a_run_pod_is_asked_for_by_its_run_id_annotation_and_never_by_its_hash_label() {
    // ANR-1160: the coder labels a run pod with the 12-digit hash of the run id and annotates it with the id; the
    // broker binds the id, so the meter asks by the annotation and sends it on the line.
    let id = "0f3c2a9e-1b7d-8e4a-9c21-6d5b7a8e9f10";
    let d = direct(|_| {});
    d.accounts.set_run("coder", id, "acct-id");
    // A source that also knew the hash would hand it out: it must never be asked for it.
    d.accounts.set_run("coder", &run_hash(id), "acct-hash");
    let mut pod = facts("uid-id", 600);
    set_run(&mut pod, id);
    assert_ne!(pod.labels["adam.vymalo.com/run"], id);
    d.meter.pass(&[pod], 600 + 3 * 60).await;
    assert_eq!(minutes_of(&d, "uid-id"), vec![10, 11, 12]);
    let lines = d.sink.taken();
    assert!(
        lines
            .iter()
            .all(|e| e.subject == "acct-id" && e.run.as_deref() == Some(id))
    );
    assert_eq!(
        d.accounts.asked(),
        vec![PodRef::Run {
            service: "coder".into(),
            run: id.into()
        }]
    );

    // The hash label alone is no identity: no line, no question, the minutes counted as `no-identity`.
    let d = direct(|_| {});
    d.accounts.set_run("coder", &run_hash("run-1"), "acct-hash");
    let mut only_label = facts("uid-label", 600);
    only_label.annotations.remove(RUN_ID);
    d.meter.pass(&[only_label], 600 + 3 * 60).await;
    assert!(d.sink.taken().is_empty());
    assert!(d.accounts.asked().is_empty());
    assert_eq!(
        d.metrics.get(
            aap_controller::metric::METER_UNATTRIBUTED,
            &[("reason", "no-identity")]
        ),
        3
    );
}

#[tokio::test]
async fn an_account_that_becomes_known_later_gets_the_earlier_minutes() {
    let d = direct(|_| {});
    d.accounts.set_broken(false);
    let mut pod = facts("uid-9", 600);
    set_run(&mut pod, "run-late");
    d.meter.pass(&[pod.clone()], 1_000).await;
    d.meter.pass(&[pod.clone()], 1_020).await;
    assert!(d.sink.taken().is_empty());
    // Counted once, however many passes retried it.
    assert_eq!(
        d.metrics.get(
            aap_controller::metric::METER_UNATTRIBUTED,
            &[("reason", "unknown-account")]
        ),
        7
    );
    d.accounts.set_run("coder", "run-late", "acct-late");
    d.meter.pass(&[pod], 1_040).await;
    assert_eq!(minutes_of(&d, "uid-9"), (10..17).collect::<Vec<_>>());
    assert!(d.sink.taken().iter().all(|e| e.subject == "acct-late"));
}

#[tokio::test]
async fn an_unknown_account_is_given_up_when_its_minutes_pass_the_backfill_bound() {
    let d = direct(|_| {});
    let day = 24 * 60 * 60;
    let mut pod = facts("uid-9", 600);
    set_run(&mut pod, "run-never");
    d.meter.pass(&[pod.clone()], 1_000).await;
    d.meter.pass(&[pod], 1_000 + day + 600).await;
    assert!(d.sink.taken().is_empty());
    assert!(
        d.metrics
            .get(aap_controller::metric::METER_BACKFILL_SKIPPED, &[])
            >= 6
    );
}

#[tokio::test]
async fn two_pods_with_big_backlogs_both_progress_in_one_pass() {
    let d = direct(|c| c.max_events_per_pass = 10);
    let (a, b) = (facts("uid-a", 600), facts("uid-b", 600));
    let sent = d.meter.pass(&[a, b], 600 + 100 * 60).await; // 100 minutes each
    assert_eq!(sent, 10);
    assert_eq!(minutes_of(&d, "uid-a").len(), 5);
    assert_eq!(minutes_of(&d, "uid-b").len(), 5);
}

#[tokio::test]
async fn a_finished_pod_that_says_when_nowhere_gives_no_line_once_counted() {
    let d = direct(|_| {});
    let mut pod = facts("uid-1", 600);
    pod.phase = "Failed".into();
    d.meter.pass(&[pod.clone()], 5_000).await;
    d.meter.pass(&[pod], 5_100).await;
    assert!(d.sink.taken().is_empty());
    assert_eq!(
        d.metrics.get(
            aap_controller::metric::METER_UNATTRIBUTED,
            &[("reason", "no-end")]
        ),
        1
    );
}

#[tokio::test]
async fn an_invalid_event_is_dropped_counted_and_not_retried() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let calls = Arc::new(AtomicUsize::new(0));
    let metrics = Arc::new(Metrics::new());
    let counter = calls.clone();
    let sink: aap_controller::SinkFn = Arc::new(move |_e| {
        counter.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(UsageError::Invalid { field: "subject" }) })
    });
    let accounts = Arc::new(MemoryAccounts::new());
    accounts.set_run("coder", "run-1", "acct-1");
    let mut cfg = config();
    cfg.skew_secs = 0;
    let meter = Meter::new(sink, erase_accounts(Shared(accounts)), cfg, metrics.clone());
    let pod = facts("uid-1", 600);
    meter.pass(std::slice::from_ref(&pod), 1_000).await;
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    meter.pass(&[pod], 1_000).await;
    assert_eq!(
        calls.load(Ordering::SeqCst),
        6,
        "an invalid minute is never retried"
    );
    assert_eq!(metrics.get(aap_controller::metric::METER_INVALID, &[]), 6);
    assert_eq!(
        metrics.get(aap_controller::metric::METER_SEND_FAILURES, &[]),
        0
    );
    let _ = metering::BACKFILL_MINUTES;
}

#[tokio::test]
async fn a_pod_that_vanishes_while_terminating_is_billed_only_through_its_last_seen_minute() {
    let d = direct(|_| {});
    let mut pod = facts("uid-1", 600); // minute 10
    pod.deleted_at = Some(5_000); // the grace period ends far ahead (minute 83)
    d.meter.pass(std::slice::from_ref(&pod), 1_100).await; // seen Running in minute 18
    d.meter.close_vanished(&Default::default(), 9_000).await;
    let minutes = minutes_of(&d, "uid-1");
    assert_eq!(
        minutes.last(),
        Some(&18),
        "never to the grace end: {minutes:?}"
    );
}

#[tokio::test]
async fn a_period_never_sends_more_than_its_cap_between_the_pass_and_the_close() {
    let d = direct(|c| c.max_events_per_pass = 4);
    // B is seen once (minute 10 is over at 700), then vanishes.
    d.meter.pass(&[facts("uid-b", 600)], 700).await;
    assert_eq!(minutes_of(&d, "uid-b"), vec![10]);
    let before = d.sink.taken().len();
    // The next period: A has a backlog of 6 and takes the whole budget of 4; B's close gets nothing this period.
    let a = facts("uid-a", 600);
    let sent = d.meter.pass(std::slice::from_ref(&a), 1_000).await;
    let closed = d
        .meter
        .close_vanished(&["uid-a".to_owned()].into_iter().collect(), 1_000)
        .await;
    assert_eq!((sent, closed), (4, 0));
    assert_eq!(d.sink.taken().len() - before, 4);
    // The period after: A's last two minutes, then what is left goes to B's close.
    let sent = d.meter.pass(std::slice::from_ref(&a), 1_000).await;
    let closed = d
        .meter
        .close_vanished(&["uid-a".to_owned()].into_iter().collect(), 1_000)
        .await;
    assert_eq!((sent, closed), (2, 1));
    assert_eq!(minutes_of(&d, "uid-b"), vec![10, 11]);
}

#[tokio::test]
async fn a_pod_that_sends_nothing_leaves_its_share_to_the_others() {
    let d = direct(|c| c.max_events_per_pass = 10);
    let mut stranger = facts("uid-s", 600);
    set_run(&mut stranger, "run-unknown");
    let sent = d
        .meter
        .pass(
            &[stranger, facts("uid-a", 600), facts("uid-b", 600)],
            600 + 100 * 60,
        )
        .await;
    assert_eq!(sent, 10, "the whole budget is spent");
    assert_eq!(minutes_of(&d, "uid-a").len(), 5);
    assert_eq!(minutes_of(&d, "uid-b").len(), 5);
}

#[tokio::test]
async fn a_vanished_pod_whose_account_is_not_known_is_kept_until_it_is() {
    let d = direct(|_| {});
    let mut pod = facts("uid-9", 600);
    set_run(&mut pod, "run-late");
    d.meter.pass(std::slice::from_ref(&pod), 1_100).await;
    d.meter.close_vanished(&Default::default(), 1_300).await;
    assert!(d.sink.taken().is_empty());
    d.accounts.set_run("coder", "run-late", "acct-late");
    d.meter.close_vanished(&Default::default(), 1_400).await;
    assert_eq!(minutes_of(&d, "uid-9"), (10..19).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_pod_whose_app_container_never_started_is_counted_as_no_start_once() {
    let d = direct(|_| {});
    let mut pod = facts("uid-1", 600);
    pod.started_at = None;
    d.meter.pass(std::slice::from_ref(&pod), 5_000).await;
    d.meter.pass(std::slice::from_ref(&pod), 5_100).await;
    assert!(d.sink.taken().is_empty());
    assert_eq!(
        d.metrics.get(
            aap_controller::metric::METER_UNATTRIBUTED,
            &[("reason", "no-start")]
        ),
        1
    );
}

// ---- delivery survives a restart (ANR-1151): the cursor that is known delivered moves on the collector's answer

/// A meter that tracks the sink's acknowledgements, over a sink the case keeps a handle on.
fn acked(sink: &Arc<MemoryUsageSink>, metrics: &Arc<Metrics>) -> Meter {
    let accounts = Arc::new(MemoryAccounts::new());
    accounts.set_run("coder", "run-1", "acct-1");
    let mut cfg = config();
    cfg.skew_secs = 0;
    let (send, outcomes) = aap_controller::erase_sink_with_outcomes(Shared(sink.clone()));
    Meter::new(send, erase_accounts(Shared(accounts)), cfg, metrics.clone()).with_outcomes(outcomes)
}

fn ids(from: u64, to: u64) -> Vec<String> {
    (from..to).map(|m| event_id("uid-1", m)).collect()
}

#[tokio::test]
async fn a_silent_collector_leaves_the_acknowledged_cursor_where_it_was() {
    let sink = Arc::new(MemoryUsageSink::new());
    sink.hold_outcomes(aap_usage::Fate::Delivered); // taken, not acknowledged
    let metrics = Arc::new(Metrics::new());
    let meter = acked(&sink, &metrics);
    meter.pass(&[facts("uid-1", 600)], 1_000).await; // minutes 10..16 are over and handed over
    assert_eq!(sink.taken().len(), 6);
    // Nothing was acknowledged: the first minute not known delivered is still the first.
    assert_eq!(meter.acknowledged_through("uid-1"), Some(10));
    assert_eq!(meter.unacknowledged("uid-1"), 6);
    // Passes go by with the collector still silent: the cursor does not move, and nothing is sent twice.
    meter.pass(&[facts("uid-1", 600)], 1_010).await;
    meter.settle(1_020);
    assert_eq!(meter.acknowledged_through("uid-1"), Some(10));
    assert_eq!(sink.taken().len(), 6);
    assert_eq!(metrics.get(aap_controller::metric::METER_DELIVERED, &[]), 0);
}

#[tokio::test]
async fn an_acknowledgement_moves_the_cursor() {
    let sink = Arc::new(MemoryUsageSink::new());
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    let metrics = Arc::new(Metrics::new());
    let meter = acked(&sink, &metrics);
    meter.pass(&[facts("uid-1", 600)], 1_000).await;
    assert_eq!(sink.settle(Some(2)), 2); // a 200 for the first two
    meter.settle(1_001);
    assert_eq!(meter.acknowledged_through("uid-1"), Some(12));
    assert_eq!(sink.settle(None), 4); // and for the rest
    meter.settle(1_002);
    assert_eq!(meter.acknowledged_through("uid-1"), Some(16));
    assert_eq!(meter.unacknowledged("uid-1"), 0);
    assert_eq!(metrics.get(aap_controller::metric::METER_DELIVERED, &[]), 6);
}

#[tokio::test]
async fn a_refusal_for_good_is_counted_and_never_sent_again() {
    let sink = Arc::new(MemoryUsageSink::new());
    sink.hold_outcomes(aap_usage::Fate::Refused);
    let metrics = Arc::new(Metrics::new());
    let meter = acked(&sink, &metrics);
    meter.pass(&[facts("uid-1", 600)], 1_000).await;
    sink.settle(None);
    meter.settle(1_001);
    assert_eq!(meter.unacknowledged("uid-1"), 0);
    assert_eq!(metrics.get(aap_controller::metric::METER_REFUSED, &[]), 6);
    meter.pass(&[facts("uid-1", 600)], 1_002).await;
    assert_eq!(sink.taken().len(), 6, "a refused minute is not sent again");
}

#[tokio::test]
async fn a_restart_in_the_middle_of_a_batch_leaves_no_gap_for_a_listed_pod_and_repeats_only_by_id()
{
    // The collector keeps what it was given across the operator's restart; the operator does not.
    let sink = Arc::new(MemoryUsageSink::new());
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    let first = acked(&sink, &Arc::new(Metrics::new()));
    let pod = facts("uid-1", 600);
    first.pass(std::slice::from_ref(&pod), 1_000).await; // minutes 10..16 handed over
    sink.settle(Some(3)); // three acknowledged, then the operator dies with three in flight
    drop(first);
    // The restarted operator has no cursor; the pod is still listed, so it re-derives its minutes.
    let second = acked(&sink, &Arc::new(Metrics::new()));
    second.pass(&[pod], 1_100).await; // minutes 10..18
    let taken = sink.taken();
    let mut by_id: std::collections::BTreeMap<String, PodMinuteEvent> = Default::default();
    for e in &taken {
        // A repeat is the same event (the account of a run pod is the same): the collector dedupes it on its id.
        if let Some(before) = by_id.insert(e.event_id.clone(), e.clone()) {
            assert_eq!(&before, e);
        }
    }
    let mut want = ids(10, 18);
    want.sort();
    assert_eq!(by_id.keys().cloned().collect::<Vec<_>>(), want, "no gap");
    assert!(taken.len() > by_id.len(), "the restart repeated minutes");
}

#[tokio::test]
async fn a_minute_never_acknowledged_expires_at_the_backfill_bound_and_is_counted() {
    let sink = Arc::new(MemoryUsageSink::new());
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    let metrics = Arc::new(Metrics::new());
    let meter = acked(&sink, &metrics);
    meter.pass(&[facts("uid-1", 600)], 1_000).await;
    meter.settle(1_000 + 24 * 3600 - 1);
    assert_eq!(meter.unacknowledged("uid-1"), 6, "still within the bound");
    meter.settle(1_000 + 24 * 3600 + 1);
    assert_eq!(meter.unacknowledged("uid-1"), 0);
    assert_eq!(
        metrics.get(aap_controller::metric::METER_UNACKNOWLEDGED, &[]),
        6
    );
}

/// A sink written as before the outcomes existed: it keeps the default `outcomes()`.
struct Plain(Arc<MemoryUsageSink>);
impl UsageSink for Plain {
    async fn send(&self, e: PodMinuteEvent) -> Result<(), UsageError> {
        self.0.send(e).await
    }
}

#[tokio::test]
async fn a_sink_that_reports_no_outcomes_is_not_tracked_and_leaks_nothing() {
    // Through the function `Operator::with_meter` calls.
    let inner = Arc::new(MemoryUsageSink::new());
    let metrics = Arc::new(Metrics::new());
    let (send, outcomes) = aap_controller::erase_sink_with_outcomes(Plain(inner.clone()));
    let accounts = Arc::new(MemoryAccounts::new());
    accounts.set_run("coder", "run-1", "acct-1");
    let mut cfg = config();
    cfg.skew_secs = 0;
    let meter = Meter::new(send, erase_accounts(Shared(accounts)), cfg, metrics.clone())
        .with_outcomes(outcomes);
    meter.pass(&[facts("uid-1", 600)], 1_000).await;
    assert_eq!(inner.taken().len(), 6);
    assert_eq!(
        meter.unacknowledged("uid-1"),
        0,
        "nothing is pending: the sink says nothing"
    );
    meter.settle(1_000 + 25 * 3600);
    assert_eq!(
        metrics.get(aap_controller::metric::METER_UNACKNOWLEDGED, &[]),
        0
    );
    // A pod that vanishes is closed and forgotten as it always was.
    assert_eq!(meter.close_vanished(&Default::default(), 1_300).await, 1);
    assert_eq!(meter.acknowledged_through("uid-1"), None);
}

#[tokio::test]
async fn acknowledgements_that_arrive_while_no_pass_runs_are_drained_by_settle_alone() {
    // A replica that follows lists and sends nothing, but still settles (`run` does): what the collector
    // acknowledged meanwhile clears, and a pass after the change of leader sends nothing again.
    let sink = Arc::new(MemoryUsageSink::new());
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    let metrics = Arc::new(Metrics::new());
    let meter = acked(&sink, &metrics);
    meter.pass(&[facts("uid-1", 600)], 1_000).await;
    sink.settle(None); // acknowledged while this replica did not lead
    meter.settle(1_030);
    assert_eq!(meter.unacknowledged("uid-1"), 0);
    assert_eq!(metrics.get(aap_controller::metric::METER_DELIVERED, &[]), 6);
    meter.pass(&[facts("uid-1", 600)], 1_010).await;
    assert_eq!(sink.taken().len(), 6);
}

// ---- the facts of a vanished pod are persisted (ANR-1198)

use aap_usage::{MemoryTombstones, Tombstone, TombstoneError, TombstoneStore};

/// A tombstone store the case keeps a handle on while the meter owns the other.
struct SharedStore(Arc<MemoryTombstones>);
impl TombstoneStore for SharedStore {
    async fn load(&self, scope: &str) -> Result<aap_usage::Loaded, TombstoneError> {
        self.0.load(scope).await
    }
    async fn save(&self, namespace: &str, set: &[Tombstone]) -> Result<(), TombstoneError> {
        self.0.save(namespace, set).await
    }
}

fn scopes() -> Vec<String> {
    vec![NS.to_owned()]
}

/// A meter that reports acknowledgements and keeps tombstones in `store`; `skew` is zero so times are the case's.
fn durable(
    sink: &Arc<MemoryUsageSink>,
    store: &Arc<MemoryTombstones>,
    metrics: &Arc<Metrics>,
) -> Meter {
    acked(sink, metrics)
        .with_tombstones(aap_controller::erase_tombstones(SharedStore(store.clone())))
}

fn minutes_on(sink: &MemoryUsageSink, uid: &str) -> Vec<u64> {
    let mut m: Vec<u64> = sink
        .taken()
        .iter()
        .filter(|e| e.event_id == event_id(uid, e.minute))
        .map(|e| e.minute)
        .collect();
    m.sort_unstable();
    m.dedup();
    m
}

/// A pod seen Running at 1_000, listed no more at 1_100, whose minutes 10..=16 are handed over and not acknowledged.
async fn vanish(meter: &Meter, uid: &str) {
    let pod = facts(uid, 600);
    meter.pass(std::slice::from_ref(&pod), 1_000).await;
    meter.close_vanished(&HashSet::new(), 1_100).await;
}

use std::collections::HashSet;

#[tokio::test]
async fn a_vanished_pod_is_written_down_before_its_minutes_are_sent() {
    // The collector is down: nothing is taken. The tombstone is there all the same, and a crash now loses nothing.
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    let meter = durable(&sink, &store, &metrics);
    assert!(meter.restore(&scopes()).await);
    let pod = facts("uid-1", 600);
    meter.pass(std::slice::from_ref(&pod), 1_000).await; // 10..16 taken
    sink.set_down(true);
    meter.close_vanished(&HashSet::new(), 1_100).await; // minute 16 cannot be sent
    let stored = store.stored(NS);
    assert_eq!(stored.len(), 1, "one tombstone");
    let t = &stored[0];
    assert_eq!(
        (t.uid.as_str(), t.namespace.as_str(), t.subject.as_deref()),
        ("uid-1", NS, Some("acct-1"))
    );
    // The collector acknowledged 10..16 at once, so the first minute not known delivered is the one still to send.
    assert_eq!((t.started_at, t.ended_at, t.first_minute), (600, 1_001, 16));
    assert_eq!(
        (t.weight_quarters, t.pod_kind, t.node_type.as_str()),
        (4, PodKind::Run, "big")
    );
    assert_eq!(
        metrics.get(aap_controller::metric::METER_TOMBSTONES_WRITTEN, &[]),
        1
    );
}

#[tokio::test]
async fn a_crash_loses_no_minute_of_a_vanished_pod() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered); // the collector has acknowledged nothing
    let first = durable(&sink, &store, &metrics);
    first.restore(&scopes()).await;
    vanish(&first, "uid-1").await;
    assert_eq!(minutes_on(&sink, "uid-1"), (10..=16).collect::<Vec<_>>());
    assert_eq!(store.stored(NS).len(), 1);
    // The operator dies with those minutes in its queue: the queue, the cursors and the meter are gone.
    drop(first);
    let lost = Arc::new(MemoryUsageSink::new()); // what the restarted operator's own sink holds: nothing
    let second_metrics = Arc::new(Metrics::new());
    let second = durable(&lost, &store, &second_metrics);
    assert!(second.restore(&scopes()).await);
    assert_eq!(
        second_metrics.get(aap_controller::metric::METER_TOMBSTONES_RESTORED, &[]),
        1
    );
    second.replay(1_200).await;
    assert_eq!(
        minutes_on(&lost, "uid-1"),
        (10..=16).collect::<Vec<_>>(),
        "every minute from the first one not acknowledged, none lost"
    );
    // Repeats are only by event id: the same event (a run's account is recorded in the tombstone; an agent pod's is the
    // last one its source named, or asked again when it named none).
    let before = sink.taken();
    for e in lost.taken() {
        let same = before
            .iter()
            .find(|b| b.event_id == e.event_id)
            .expect("a repeat of an id");
        assert_eq!(same, &e);
    }
}

#[tokio::test]
async fn a_failover_loses_no_minute_and_the_old_leader_writes_nothing_more() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    let old = durable(&sink, &store, &metrics);
    old.restore(&scopes()).await;
    vanish(&old, "uid-1").await;
    // The lease moves: the old leader is a follower now, and a pod that vanishes in its memory is not written.
    old.lose_leadership();
    let other = facts("uid-2", 600);
    old.pass(std::slice::from_ref(&other), 1_000).await;
    let before = store.stored(NS);
    assert_eq!(old.close_vanished(&HashSet::new(), 1_100).await, 0);
    assert_eq!(
        store.stored(NS),
        before,
        "a replica that does not lead, or has not read the store, writes nothing"
    );
    // The new leader reads the set and sends what the old one's queue took with it.
    let fresh = Arc::new(MemoryUsageSink::new());
    let new = durable(&fresh, &store, &Arc::new(Metrics::new()));
    new.restore(&scopes()).await;
    new.replay(1_200).await;
    assert_eq!(minutes_on(&fresh, "uid-1"), (10..=16).collect::<Vec<_>>());
    assert!(
        minutes_on(&fresh, "uid-2").is_empty(),
        "uid-2 was never written down"
    );
}

#[tokio::test]
async fn a_tombstone_is_deleted_when_its_minutes_are_acknowledged() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    let meter = durable(&sink, &store, &metrics);
    meter.restore(&scopes()).await;
    vanish(&meter, "uid-1").await;
    assert_eq!(store.stored(NS).len(), 1);
    meter.replay(1_200).await;
    assert_eq!(
        store.stored(NS).len(),
        1,
        "unacknowledged: the tombstone stays"
    );
    sink.settle(Some(3));
    meter.replay(1_300).await;
    assert_eq!(
        store.stored(NS).len(),
        1,
        "three of seven acknowledged is not all"
    );
    sink.settle(None);
    meter.replay(1_400).await;
    assert!(
        store.stored(NS).is_empty(),
        "acknowledged: the tombstone is deleted"
    );
    assert_eq!(meter.tombstones_held(NS), 0);
    assert_eq!(
        metrics.get(aap_controller::metric::METER_TOMBSTONES_RELEASED, &[]),
        1
    );
}

#[tokio::test]
async fn a_refusal_for_good_also_releases_the_tombstone() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Refused);
    let meter = durable(&sink, &store, &metrics);
    meter.restore(&scopes()).await;
    vanish(&meter, "uid-1").await;
    sink.settle(None);
    meter.replay(1_400).await;
    assert!(store.stored(NS).is_empty());
}

#[tokio::test]
async fn a_pod_with_nothing_outstanding_gets_no_tombstone_and_no_write() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    let meter = durable(&sink, &store, &metrics);
    meter.restore(&scopes()).await;
    // Everything is sent and acknowledged (the sink answers at once) before the pod is gone: nothing to remember.
    let pod = facts("uid-1", 600);
    meter.pass(std::slice::from_ref(&pod), 1_000).await;
    let mut finished = pod.clone();
    finished.phase = "Succeeded".into();
    finished.finished_at = Some(960);
    meter.pass(&[finished], 1_030).await; // all of it handed over, in one more pass
    meter.close_vanished(&HashSet::new(), 1_100).await;
    assert_eq!(store.calls().0, 0, "no write at all");
    assert!(store.stored(NS).is_empty());
}

#[tokio::test]
async fn a_tombstone_older_than_the_backfill_bound_is_deleted_without_a_line() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    let now: i64 = 10_000_000;
    let mut t = aap_usage::testkit::tombstones::tombstone(NS, "uid-old");
    t.written_at = now - 24 * 3600 - 1;
    t.started_at = now - 25 * 3600;
    t.ended_at = now - 24 * 3600;
    t.first_minute = u64::try_from(t.started_at / 60).unwrap();
    store.seed(NS, vec![t]);
    let meter = durable(&sink, &store, &metrics);
    assert!(meter.restore(&scopes()).await);
    meter.replay(now).await;
    assert!(sink.taken().is_empty());
    assert!(store.stored(NS).is_empty());
    assert_eq!(
        metrics.get(aap_controller::metric::METER_TOMBSTONES_EXPIRED, &[]),
        1
    );
}

#[tokio::test]
async fn a_full_namespace_keeps_metering_from_memory_and_counts_what_has_no_tombstone() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    let meter = durable(&sink, &store, &metrics);
    meter.restore(&scopes()).await;
    let n = aap_usage::MAX_TOMBSTONES + 1;
    let pods: Vec<PodFacts> = (0..n).map(|i| facts(&format!("uid-{i:04}"), 600)).collect();
    meter.pass(&pods, 1_000).await;
    meter.close_vanished(&HashSet::new(), 1_100).await;
    assert_eq!(
        store.stored(NS).len(),
        aap_usage::MAX_TOMBSTONES,
        "the set stops at the bound"
    );
    assert_eq!(
        metrics.get(aap_controller::metric::METER_TOMBSTONES_UNBACKED, &[]),
        1
    );
    for i in 0..n {
        assert_eq!(
            minutes_on(&sink, &format!("uid-{i:04}")),
            (10..=16).collect::<Vec<_>>(),
            "pod {i} is metered whether or not it has a tombstone"
        );
    }
}

#[tokio::test]
async fn a_store_that_cannot_hold_the_set_leaves_the_pods_metered_from_memory() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    store.set_capacity(Some(1));
    let meter = durable(&sink, &store, &metrics);
    meter.restore(&scopes()).await;
    let pods = [facts("uid-1", 600), facts("uid-2", 600)];
    meter.pass(&pods, 1_000).await;
    meter.close_vanished(&HashSet::new(), 1_100).await;
    assert!(
        store.stored(NS).is_empty(),
        "a refused write changes nothing"
    );
    assert_eq!(
        metrics.get(aap_controller::metric::METER_TOMBSTONES_UNBACKED, &[]),
        2
    );
    assert_eq!(minutes_on(&sink, "uid-1"), (10..=16).collect::<Vec<_>>());
    assert_eq!(minutes_on(&sink, "uid-2"), (10..=16).collect::<Vec<_>>());
}

#[tokio::test]
async fn a_store_that_is_down_holds_the_pod_until_it_is_written() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    let meter = durable(&sink, &store, &metrics);
    meter.restore(&scopes()).await;
    let pod = facts("uid-1", 600);
    meter.pass(std::slice::from_ref(&pod), 1_000).await; // 10..16
    store.set_down(true);
    meter.close_vanished(&HashSet::new(), 1_100).await;
    assert_eq!(
        minutes_on(&sink, "uid-1"),
        (10..16).collect::<Vec<_>>(),
        "minute 16 waits: nothing is sent unwritten"
    );
    assert!(
        metrics.get(
            aap_controller::metric::METER_TOMBSTONE_ERRORS,
            &[("op", "save")]
        ) >= 1
    );
    store.set_down(false);
    meter.close_vanished(&HashSet::new(), 1_160).await;
    assert_eq!(store.stored(NS).len(), 1);
    assert_eq!(minutes_on(&sink, "uid-1"), (10..=16).collect::<Vec<_>>());
}

#[tokio::test]
async fn nothing_is_written_before_the_store_was_read_and_a_conflict_makes_it_read_again() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    // Another leader's tombstone is in the store; this meter has not read it.
    let theirs = aap_usage::testkit::tombstones::tombstone(NS, "uid-theirs");
    store.seed(NS, vec![theirs]);
    let meter = durable(&sink, &store, &metrics);
    let pod = facts("uid-1", 600);
    meter.pass(std::slice::from_ref(&pod), 1_000).await;
    assert_eq!(meter.close_vanished(&HashSet::new(), 1_100).await, 0);
    assert_eq!(
        store.stored(NS).len(),
        1,
        "unread: not a byte written over theirs"
    );
    // A write that finds the set changed asks to read again, and the next pass does.
    assert!(meter.restore(&scopes()).await);
    store.fail_next_save(TombstoneError::Conflict);
    meter.close_vanished(&HashSet::new(), 1_100).await;
    assert_eq!(store.stored(NS).len(), 1, "the conflict wrote nothing");
    assert!(meter.restore(&scopes()).await, "it reads again");
    meter.close_vanished(&HashSet::new(), 1_160).await;
    let uids: Vec<String> = store.stored(NS).into_iter().map(|t| t.uid).collect();
    assert_eq!(uids, ["uid-1", "uid-theirs"], "both are kept");
}

// ---- the same, through the operator and the fake API server's ConfigMaps

use aap_controller::KubeTombstones;

/// The operator's own namespace: where the tombstones are kept.
const HOME: &str = "ops";

/// An operator with the meter and a ConfigMap store, over `fake`, on `sink`.
async fn start_durable(fake: &Fake, sink: Arc<MemoryUsageSink>) -> Rig {
    let accounts = Arc::new(MemoryAccounts::new());
    accounts.set_run("coder", "run-1", "acct-1");
    let owner: OwnerOf = Arc::new(|_| OwnerHandle::none());
    let operator = Operator::new(
        fake.client(),
        Scripted::new(),
        MemoryStore::new(),
        owner,
        options(),
    )
    .with_meter(Shared(sink.clone()), Shared(accounts.clone()))
    .with_meter_tombstones(KubeTombstones::new(fake.client(), HOME))
    .with_meter_config(config());
    let metrics = operator.metrics();
    let (stop, stopped) = oneshot::channel::<()>();
    let done = tokio::spawn(operator.run(async move {
        let _ = stopped.await;
    }));
    Rig {
        fake: fake.clone(),
        sink,
        accounts,
        metrics,
        stop,
        done,
    }
}

fn tombstone_keys(fake: &Fake) -> Vec<String> {
    fake.configmap(HOME, &aap_controller::tombstone_configmap_name(NS))
        .and_then(|cm| cm["data"].as_object().cloned())
        .map(|d| d.keys().cloned().collect())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_crash_and_a_failover_with_queued_minutes_of_a_vanished_pod_lose_none() {
    let fake = Fake::new();
    let m0 = now() / 60 - 30;
    fake.put_pod(finished_run("r1", "uid-1", "run-1", m0 * 60, (m0 + 5) * 60));
    // The first operator hands the five minutes over; the collector acknowledges none of them.
    let queue = Arc::new(MemoryUsageSink::new());
    queue.hold_outcomes(aap_usage::Fate::Delivered);
    let first = start_durable(&fake, queue.clone()).await;
    eventually("the five minutes handed over", || queue.taken().len() >= 5).await;
    // The pod's object goes (the coder's own cleanup), while its minutes are still queued.
    fake.remove_pod(NS, "r1");
    eventually("the tombstone", || tombstone_keys(&fake) == ["uid-1"]).await;
    // It dies with its queue (nothing is stopped gracefully: the sink is simply never read again), and a second
    // operator, another replica that wins the lease, has an empty sink of its own.
    first.stop().await;
    let fresh = Arc::new(MemoryUsageSink::new());
    let second = start_durable(&fake, fresh.clone()).await;
    eventually("the minutes sent again", || fresh.taken().len() >= 5).await;
    let want: Vec<u64> = (m0..m0 + 5).map(|m| m as u64).collect();
    assert_eq!(
        minutes_on(&fresh, "uid-1"),
        want,
        "no minute of the vanished pod is lost"
    );
    // The same events (a run's account is in the tombstone): the collector dedupes them by `event_id`.
    for e in fresh.taken() {
        let before = queue
            .taken()
            .into_iter()
            .find(|b| b.event_id == e.event_id)
            .unwrap();
        assert_eq!(before, e);
    }
    // The fresh sink acknowledges at once, so the tombstone goes.
    eventually("the tombstone is deleted", || {
        tombstone_keys(&fake).is_empty()
    })
    .await;
    second.stop().await;
}

#[tokio::test]
async fn the_meter_uses_only_the_verbs_the_operators_role_holds_on_configmaps() {
    let fake = Fake::new();
    let m0 = now() / 60 - 30;
    fake.put_pod(finished_run("r1", "uid-1", "run-1", m0 * 60, (m0 + 3) * 60));
    let queue = Arc::new(MemoryUsageSink::new());
    queue.hold_outcomes(aap_usage::Fate::Delivered);
    let rig = start_durable(&fake, queue.clone()).await;
    eventually("the minutes", || queue.taken().len() >= 3).await;
    fake.remove_pod(NS, "r1");
    eventually("the tombstone", || tombstone_keys(&fake) == ["uid-1"]).await;
    queue.settle(None);
    eventually("the tombstone is deleted", || {
        tombstone_keys(&fake).is_empty()
    })
    .await;
    rig.stop().await;
    let calls = fake.calls();
    let on_configmaps: std::collections::BTreeSet<String> = calls
        .iter()
        .filter(|c| c.path.contains("/configmaps"))
        .map(|c| c.method.clone())
        .collect();
    assert!(!on_configmaps.is_empty());
    // The Role: get, list, patch, create, delete. This adapter uses get, create, patch and delete, and never update.
    assert!(
        on_configmaps.is_subset(&["GET", "POST", "PATCH", "DELETE"].map(String::from).into()),
        "{on_configmaps:?}"
    );
    assert!(
        !calls.iter().any(|c| c.path.contains("/secrets")),
        "and no Secret is touched"
    );
    // One write per change of the set, never one per pod per period: a tombstone in, a tombstone out.
    let writes = calls
        .iter()
        .filter(|c| c.path.contains("/configmaps") && c.method != "GET")
        .count();
    assert_eq!(
        writes, 2,
        "{writes} writes for one pod's life: created with the tombstone, patched when it went"
    );
}

#[tokio::test]
async fn two_writers_cannot_lose_each_others_tombstones() {
    let fake = Fake::new();
    let a = KubeTombstones::new(fake.client(), HOME);
    let b = KubeTombstones::new(fake.client(), HOME);
    let t = |uid: &str| aap_usage::testkit::tombstones::tombstone(NS, uid);
    assert!(a.load(NS).await.unwrap().tombstones.is_empty());
    assert!(b.load(NS).await.unwrap().tombstones.is_empty());
    a.save(NS, &[t("uid-a")]).await.unwrap();
    // B never saw A's ConfigMap: its create finds one there.
    assert_eq!(
        b.save(NS, &[t("uid-b")]).await,
        Err(TombstoneError::Conflict)
    );
    // B reads, then writes over what it read.
    assert_eq!(b.load(NS).await.unwrap().tombstones.len(), 1);
    b.save(NS, &[t("uid-a"), t("uid-b")]).await.unwrap();
    // A's next write is over a version that moved.
    assert_eq!(a.save(NS, &[]).await, Err(TombstoneError::Conflict));
    assert_eq!(
        tombstone_keys(&fake),
        ["uid-a", "uid-b"],
        "nothing was lost"
    );
    // And the whole set of a namespace is one ConfigMap, found by its label across namespaces.
    assert_eq!(
        KubeTombstones::new(fake.client(), HOME)
            .load("")
            .await
            .unwrap()
            .tombstones
            .len(),
        2
    );
}

#[tokio::test]
async fn a_set_that_does_not_fit_in_a_configmap_is_refused_and_changes_nothing() {
    let fake = Fake::new();
    let store = KubeTombstones::new(fake.client(), HOME);
    store.load(NS).await.unwrap();
    let t = aap_usage::testkit::tombstones::tombstone(NS, "uid-0");
    store.save(NS, std::slice::from_ref(&t)).await.unwrap();
    // Far past what the meter ever holds (MAX_TOMBSTONES): the byte bound is the second fence.
    let many: Vec<Tombstone> = (0..4_000)
        .map(|i| aap_usage::testkit::tombstones::tombstone(NS, &format!("{i:064}")))
        .collect();
    assert_eq!(store.save(NS, &many).await, Err(TombstoneError::TooLarge));
    assert_eq!(tombstone_keys(&fake), ["uid-0"]);
    // And at the meter's own bound the set is well under the 1 MiB the API server enforces.
    let at_bound: Vec<Tombstone> = (0..aap_usage::MAX_TOMBSTONES)
        .map(|i| {
            let mut t = aap_usage::testkit::tombstones::tombstone(NS, &format!("{i:036}"));
            t.service = "s".repeat(128);
            t.node_type = "n".repeat(128);
            t.subject = Some("a".repeat(128));
            t.run = Some("r".repeat(128));
            t
        })
        .collect();
    store.save(NS, &at_bound).await.unwrap();
    let size: usize = fake
        .configmap(HOME, &aap_controller::tombstone_configmap_name(NS))
        .unwrap()["data"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_str().unwrap().len())
        .sum();
    assert!(size < aap_controller::TOMBSTONE_MAX_BYTES, "{size}");
}

// ---- review of #140: overlapping leaders, a store in the operator's namespace, per-namespace failures

use std::sync::atomic::Ordering;

fn in_ns(mut pod: PodFacts, ns: &str) -> PodFacts {
    pod.namespace = ns.to_owned();
    pod
}

fn with_store(sink: &Arc<MemoryUsageSink>, store: KubeTombstones, metrics: &Arc<Metrics>) -> Meter {
    acked(sink, metrics).with_tombstones(aap_controller::erase_tombstones(store))
}

#[tokio::test]
async fn an_overlapping_leaders_tombstone_is_found_by_the_next_read_and_nothing_is_lost() {
    let fake = Fake::new();
    let (sink_a, sink_b) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryUsageSink::new()),
    );
    sink_a.hold_outcomes(aap_usage::Fate::Delivered);
    let a = with_store(
        &sink_a,
        KubeTombstones::new(fake.client(), HOME),
        &Arc::new(Metrics::new()),
    );
    let b = with_store(
        &sink_b,
        KubeTombstones::new(fake.client(), HOME),
        &Arc::new(Metrics::new()),
    );
    // A leads and has read the (empty) set. The lease moves to B, which reads the set too.
    assert!(a.restore(&scopes()).await);
    assert!(b.restore(&scopes()).await);
    // A has not noticed yet: it finds a pod gone and writes its tombstone. The write succeeds, because A's version of
    // the set is still the current one, and B has already read.
    vanish(&a, "uid-1").await;
    assert_eq!(tombstone_keys(&fake), ["uid-1"]);
    // B reads the set again at the start of its next pass, and sends what A's queue took with it.
    assert!(b.restore(&scopes()).await);
    b.replay(1_200).await;
    assert_eq!(minutes_on(&sink_b, "uid-1"), (10..=16).collect::<Vec<_>>());
    // Two writers on one set cannot lose each other's records: B writes its own, and A's next write is over a version
    // that moved, so it reads again first and keeps B's.
    b.pass(std::slice::from_ref(&facts("uid-2", 600)), 1_000)
        .await;
    b.close_vanished(&HashSet::new(), 1_100).await;
    a.pass(std::slice::from_ref(&facts("uid-3", 600)), 1_000)
        .await;
    a.close_vanished(&HashSet::new(), 1_100).await; // conflict: held, and read again
    assert!(a.restore(&scopes()).await);
    a.close_vanished(&HashSet::new(), 1_160).await;
    // uid-1 was acknowledged at B's sink and released there, and A let it go when it read the set again; the two
    // records written by the two writers are both there.
    assert_eq!(tombstone_keys(&fake), ["uid-2", "uid-3"]);
}

#[tokio::test]
async fn a_replica_that_lost_the_lease_writes_and_closes_nothing() {
    use std::sync::atomic::AtomicUsize;
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    // The lease answers true this many more times, and then false.
    let allowed = Arc::new(AtomicUsize::new(usize::MAX));
    let left = allowed.clone();
    let meter = durable(&sink, &store, &metrics).with_continue(move || {
        left.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
    });
    meter.restore(&scopes()).await;
    let pod = facts("uid-1", 600);
    meter.pass(std::slice::from_ref(&pod), 1_000).await;
    // Lost before the pass starts: it closes nothing.
    allowed.store(0, Ordering::SeqCst);
    assert_eq!(meter.close_vanished(&HashSet::new(), 1_100).await, 0);
    assert_eq!((store.calls().0, store.stored(NS).len()), (0, 0));
    // Lost between the start of the pass and the write: checked again right before it, so the write never happens.
    allowed.store(1, Ordering::SeqCst);
    meter.close_vanished(&HashSet::new(), 1_100).await;
    assert_eq!(
        (store.calls().0, store.stored(NS).len()),
        (0, 0),
        "the lease is checked right before the write"
    );
}

#[tokio::test]
async fn a_namespace_that_cannot_be_read_is_held_and_the_others_go_on() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    store.set_unreadable_in("team", true);
    let meter = durable(&sink, &store, &metrics);
    let both = vec!["team".to_owned(), "other".to_owned()];
    assert!(!meter.restore(&both).await, "one namespace was not read");
    let pods = [
        in_ns(facts("uid-team", 600), "team"),
        in_ns(facts("uid-other", 600), "other"),
    ];
    meter.pass(&pods, 1_000).await;
    meter.close_vanished(&HashSet::new(), 1_100).await;
    assert_eq!(
        store.stored("other").len(),
        1,
        "the readable namespace is closed as usual"
    );
    assert_eq!(
        minutes_on(&sink, "uid-other"),
        (10..=16).collect::<Vec<_>>()
    );
    assert!(
        store.stored("team").is_empty(),
        "nothing is written over a set that was not read"
    );
    assert_eq!(
        minutes_on(&sink, "uid-team"),
        (10..16).collect::<Vec<_>>(),
        "held, not lost"
    );
    // It comes back: the held pod is written down and sent.
    store.set_unreadable_in("team", false);
    assert!(meter.restore(&both).await);
    meter.close_vanished(&HashSet::new(), 1_160).await;
    assert_eq!(store.stored("team").len(), 1);
    assert_eq!(minutes_on(&sink, "uid-team"), (10..=16).collect::<Vec<_>>());
}

#[tokio::test]
async fn what_is_held_for_want_of_a_store_expires_with_its_minutes() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    store.set_down(true);
    let meter = durable(&sink, &store, &metrics);
    assert!(!meter.restore(&scopes()).await);
    let pod = facts("uid-1", 600);
    meter.pass(std::slice::from_ref(&pod), 1_000).await;
    meter.close_vanished(&HashSet::new(), 1_100).await;
    // A day and more later its minutes are all past the backfill bound: it is forgotten, and counted.
    meter
        .close_vanished(&HashSet::new(), 1_100 + 25 * 3600)
        .await;
    assert_eq!(
        metrics.get(aap_controller::metric::METER_HELD_EXPIRED, &[]),
        1
    );
    // Not held again: a second look finds nothing left to count.
    meter
        .close_vanished(&HashSet::new(), 1_100 + 26 * 3600)
        .await;
    assert_eq!(
        metrics.get(aap_controller::metric::METER_HELD_EXPIRED, &[]),
        1
    );
}

#[tokio::test]
async fn a_store_that_is_forbidden_falls_back_to_memory_and_is_counted_once() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    sink.hold_outcomes(aap_usage::Fate::Delivered);
    let meter = durable(&sink, &store, &metrics);
    assert!(meter.restore(&scopes()).await);
    // The right is taken away after the read: the write is forbidden.
    store.set_forbidden_in(NS, true);
    let pod = facts("uid-1", 600);
    meter.pass(std::slice::from_ref(&pod), 1_000).await;
    meter.close_vanished(&HashSet::new(), 1_100).await;
    assert_eq!(
        minutes_on(&sink, "uid-1"),
        (10..=16).collect::<Vec<_>>(),
        "metered from memory"
    );
    assert_eq!(
        metrics.get(aap_controller::metric::METER_TOMBSTONES_UNBACKED, &[]),
        1
    );
    // Memory-only for the term: no more calls are made for it.
    let (saves, loads) = store.calls();
    meter.restore(&scopes()).await;
    meter.pass(&[facts("uid-2", 600)], 1_000).await;
    meter.close_vanished(&HashSet::new(), 1_100).await;
    assert_eq!(store.calls(), (saves, loads), "nothing is retried for ever");
}

#[tokio::test]
async fn the_set_of_a_namespace_that_is_no_longer_watched_is_deleted_when_it_is_old() {
    let (sink, store, metrics) = (
        Arc::new(MemoryUsageSink::new()),
        Arc::new(MemoryTombstones::new()),
        Arc::new(Metrics::new()),
    );
    let mut old = aap_usage::testkit::tombstones::tombstone("gone", "uid-old");
    old.written_at = 1_000;
    let mut recent = aap_usage::testkit::tombstones::tombstone("kept", "uid-new");
    recent.written_at = now();
    store.seed("gone", vec![old]);
    store.seed("kept", vec![recent]);
    let meter = durable(&sink, &store, &metrics);
    assert!(meter.restore(&scopes()).await);
    assert!(
        store.stored("gone").is_empty(),
        "every record of it is past the bound"
    );
    assert_eq!(
        store.stored("kept").len(),
        1,
        "one that may still be owed is left"
    );
    assert_eq!(
        metrics.get(aap_controller::metric::METER_TOMBSTONES_EXPIRED, &[]),
        1
    );
}

fn cm(name: &str, labels: Value, owner: Option<Value>, data: Value) -> Value {
    let mut cm = json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": { "name": name, "namespace": HOME, "labels": labels },
        "data": data,
    });
    if let Some(owner) = owner {
        cm["metadata"]["ownerReferences"] = owner;
    }
    cm
}

fn ours(ns: &str) -> Value {
    json!({
        "app.kubernetes.io/component": "meter-tombstones",
        "app.kubernetes.io/managed-by": "aap-operator",
        "app.kubernetes.io/name": "aap-meter",
        "agents.vymalo.com/meter-namespace": ns,
    })
}

#[tokio::test]
async fn a_record_this_version_cannot_read_is_kept_and_counted_never_deleted() {
    let fake = Fake::new();
    let valid = aap_usage::testkit::tombstones::tombstone(NS, "uid-ok");
    let store = KubeTombstones::new(fake.client(), HOME);
    store.load(NS).await.unwrap();
    store.save(NS, std::slice::from_ref(&valid)).await.unwrap();
    // A newer replica adds a record with a field and a kind this version does not know.
    let name = aap_controller::tombstone_configmap_name(NS);
    let mut object = fake.configmap(HOME, &name).unwrap();
    object["data"]["uid-future"] = json!(r#"{"uid":"uid-future","podKind":"gpu","shape":"new"}"#);
    fake.put_configmap(object);
    let loaded = store.load(NS).await.unwrap();
    assert_eq!(loaded.tombstones.len(), 1);
    assert_eq!(loaded.unreadable, 1);
    // Saving an empty set deletes what it understood, and nothing it did not: the ConfigMap stays.
    store.save(NS, &[]).await.unwrap();
    let after = fake.configmap(HOME, &name).expect("the ConfigMap is kept");
    let keys: Vec<&String> = after["data"].as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        ["uid-future"],
        "the record that was not understood is untouched"
    );
}

#[tokio::test]
async fn a_configmap_the_operator_did_not_make_is_refused_counted_and_never_written_over() {
    let fake = Fake::new();
    let t = aap_usage::testkit::tombstones::tombstone("a", "uid-a");
    let record = |t: &Tombstone| {
        json!({ t.uid.clone(): format!(
            r#"{{"uid":"{}","service":"coder","run":"run-1","podKind":"run","subject":"forged","weightQuarters":4,"nodeType":"std","startedAt":0,"endedAt":86400,"firstMinute":0,"writtenAt":1}}"#, t.uid) })
    };
    // No labels at all, an owner the operator did not set, and a name that says another namespace.
    let name = |ns: &str| aap_controller::tombstone_configmap_name(ns);
    fake.put_configmap(cm(&name("a"), json!({}), None, record(&t)));
    fake.put_configmap(cm(
        &name("b"),
        ours("b"),
        Some(json!([{"apiVersion":"v1","kind":"Pod","name":"p","uid":"u"}])),
        record(&t),
    ));
    fake.put_configmap(cm(&name("c"), ours("a"), None, record(&t)));
    let store = KubeTombstones::new(fake.client(), HOME);
    for ns in ["a", "b", "c"] {
        let loaded = store.load(ns).await.unwrap();
        assert!(
            loaded.tombstones.is_empty(),
            "{ns}: a forged record is not trusted"
        );
        assert_eq!(loaded.refused, 1, "{ns}");
        assert_eq!(
            store
                .save(
                    ns,
                    &[aap_usage::testkit::tombstones::tombstone(ns, "uid-x")]
                )
                .await,
            Err(TombstoneError::Forbidden),
            "{ns}: never written over"
        );
    }
    // And the watch-all read, a list in the operator's namespace, refuses them the same.
    let all = KubeTombstones::new(fake.client(), HOME)
        .load("")
        .await
        .unwrap();
    assert!(all.tombstones.is_empty());
    assert!(all.refused >= 2);
    // A tenant's namespace holds nothing of the meter's: a ConfigMap there is not read at all.
    let mut tenant = cm(&name("a"), ours("a"), None, record(&t));
    tenant["metadata"]["namespace"] = json!(NS);
    fake.put_configmap(tenant);
    let again = KubeTombstones::new(fake.client(), HOME)
        .load("")
        .await
        .unwrap();
    assert!(again.tombstones.is_empty());
}

#[tokio::test]
async fn the_configmap_store_passes_the_conformance_suite_on_the_fake_api_server() {
    aap_usage::testkit::tombstones::run(|| {
        let fake = Fake::new();
        (
            KubeTombstones::new(fake.client(), HOME),
            KubeTombstones::new(fake.client(), HOME),
        )
    })
    .await
    .unwrap();
}
