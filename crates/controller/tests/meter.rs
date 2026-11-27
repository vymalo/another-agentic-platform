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
    let mut pod = finished_run("r1", "uid-1", "run-1", 0, 0);
    pod["status"] = json!({ "phase": "Pending" });
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
