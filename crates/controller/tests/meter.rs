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
                "adam.vymalo.com/run": run,
            },
            "annotations": { "agents.vymalo.com/size-weight": "2" },
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
    accounts.set_agent("coder", "acct-agent");
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
        ("adam.vymalo.com/run", "run-1"),
    ];
    PodFacts {
        uid: uid.into(),
        labels: labels
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
        annotations: [("agents.vymalo.com/size-weight".to_owned(), "1".to_owned())].into(),
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
async fn an_account_that_becomes_known_later_gets_the_earlier_minutes() {
    let d = direct(|_| {});
    d.accounts.set_broken(false);
    let mut pod = facts("uid-9", 600);
    pod.labels
        .insert("adam.vymalo.com/run".into(), "run-late".into());
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
    pod.labels
        .insert("adam.vymalo.com/run".into(), "run-never".into());
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
    stranger
        .labels
        .insert("adam.vymalo.com/run".into(), "run-unknown".into());
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
    pod.labels
        .insert("adam.vymalo.com/run".into(), "run-late".into());
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
