//! The pod-minute meter (ANR-673, AD-069): every period the leader lists the pods that are run pods or an agent's
//! own, works out with [`crate::metering`] the whole minutes each ran that have not been sent, and hands one
//! [`PodMinuteEvent`] per minute to a [`UsageSink`], for the account an [`AccountSource`] names.
//!
//! **Not wired into `operator run` yet** (a later slice, AD-069): [`Operator::with_meter`](crate::Operator::with_meter) turns it
//! on, and nothing calls it from the binary.
//!
//! The cursor of each pod (the first minute not sent) and the last facts seen of each pod are **in memory**. A
//! restart, a new leader and a second replica therefore re-derive the same minutes from the pod's facts, with the
//! same `event_id`s and the same end-of-minute times, and the collector dedupes them: the backfill bound (24 h)
//! limits the repeat.
//!
//! **A vanished pod is closed** ([`Meter::close_vanished`]): a pod missing from a complete listing is sent through
//! the minute that contains `min(last finishedAt seen, last time it was seen Running)`. **The residual loss** is the
//! minutes between the last observation and the real deletion (at most one period plus the partial minute), and
//! the whole life of a pod that began and vanished between two lists, and any time the meter was not running (an
//! outage or a failover gap: the facts are in memory). `now` is this replica's clock: [`MeterConfig::skew_secs`]
//! keeps a minute that is not over on the API server's clock from being sent.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aap_ports::LeaderElection;
use aap_usage::{
    AccountSource, AccountUnavailable, PodKind, PodMinuteEvent, PodRef, UsageError, UsageSink,
    event_id,
};
use k8s_openapi::api::core::v1::Pod;
use kube::api::ListParams;
use kube::{Api, Client};
use tokio::sync::watch;

use crate::metering::{
    self, AGENT_MANAGED_BY, Kind, MANAGED_BY_LABEL, PodFacts, RUN_COMPONENT, RUN_MANAGED_BY,
    Unattributed,
};
use crate::metrics::{Metrics, name};

/// How often the loop looks at the pods when [`MeterConfig::period`] is left alone: well under a minute, so a
/// short run pod is seen.
pub const DEFAULT_PERIOD: Duration = Duration::from_secs(30);

/// The most events one pass sends by default (the rest waits for the next one), so a long backfill after a
/// failover cannot hold the loop. [`MeterConfig::max_events_per_pass`] sets it.
pub const MAX_EVENTS_PER_PASS: usize = 5_000;

/// The default skew margin: a minute is sent only when it ended at least this many seconds ago.
pub const DEFAULT_SKEW_SECS: i64 = 5;

/// A [`UsageSink`] behind a pointer, so the operator is not generic over it.
pub type SinkFn = Arc<
    dyn Fn(PodMinuteEvent) -> Pin<Box<dyn Future<Output = Result<(), UsageError>> + Send>>
        + Send
        + Sync,
>;

/// An [`AccountSource`] behind a pointer.
pub type AccountFn = Arc<
    dyn Fn(
            PodRef,
        )
            -> Pin<Box<dyn Future<Output = Result<Option<String>, AccountUnavailable>> + Send>>
        + Send
        + Sync,
>;

/// Erase the type of `sink`.
pub fn erase_sink<S: UsageSink + 'static>(sink: S) -> SinkFn {
    let sink = Arc::new(sink);
    Arc::new(move |event| {
        let sink = sink.clone();
        Box::pin(async move { sink.send(event).await })
    })
}

/// Erase the type of `accounts`.
pub fn erase_accounts<A: AccountSource + 'static>(accounts: A) -> AccountFn {
    let accounts = Arc::new(accounts);
    Arc::new(move |pod| {
        let accounts = accounts.clone();
        Box::pin(async move { accounts.account(&pod).await })
    })
}

/// What the deployment says about the meter.
#[derive(Clone, Debug)]
pub struct MeterConfig {
    /// The `source` of every event: a constant of the deployment (the same on every replica, so that two replicas'
    /// events are identical).
    pub source: String,
    /// The node label whose value is the node type (`runPods.nodeType.labelKey`). `None`: no node type is ever
    /// known, and no run pod makes a line.
    pub node_type_label_key: Option<String>,
    /// How often the loop looks at the pods.
    pub period: Duration,
    /// The margin, in seconds, between this clock and the API server's: the newest minute sent ended at or before
    /// `now - skew_secs`.
    pub skew_secs: i64,
    /// The most events one pass sends, shared fairly between the pods that have minutes due.
    pub max_events_per_pass: usize,
}

impl Default for MeterConfig {
    fn default() -> Self {
        Self {
            source: "aap-operator".to_owned(),
            node_type_label_key: None,
            period: DEFAULT_PERIOD,
            skew_secs: DEFAULT_SKEW_SECS,
            max_events_per_pass: MAX_EVENTS_PER_PASS,
        }
    }
}

/// What the meter remembers of a pod it has sent minutes for.
#[derive(Clone, Debug, Default)]
struct PodState {
    /// The first minute not sent.
    cursor: Option<u64>,
    /// The end of the minutes already counted as `unknown-account` (so a retried pod is counted once).
    unknown_counted: u64,
    /// Whether the pod's `no-end` was counted.
    no_end_counted: bool,
}

/// The last facts seen of a pod, and when it was last seen Running.
#[derive(Clone, Debug)]
struct Seen {
    facts: PodFacts,
    last_running: Option<i64>,
}

#[derive(Default)]
struct Inner {
    state: HashMap<String, PodState>,
    seen: HashMap<String, Seen>,
    rotor: usize,
}

/// How far one pod got in a pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Progress {
    /// Everything of this pod is sent, or will never be: a closed pod can be forgotten.
    Done,
    /// More to do at the next pass.
    Held,
    /// The sink is down: stop the pass.
    SinkDown,
}

/// The meter: the cursors, and what it sends to and asks.
pub struct Meter {
    sink: SinkFn,
    accounts: AccountFn,
    config: MeterConfig,
    metrics: Arc<Metrics>,
    inner: Mutex<Inner>,
}

impl Meter {
    /// A meter with no cursor: its first pass sends what the pods' facts say, back to the bound.
    pub fn new(
        sink: SinkFn,
        accounts: AccountFn,
        config: MeterConfig,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            sink,
            accounts,
            config,
            metrics,
            inner: Mutex::new(Inner::default()),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn state(&self, uid: &str) -> PodState {
        self.with(|i| i.state.get(uid).cloned().unwrap_or_default())
    }

    fn update(&self, uid: &str, f: impl FnOnce(&mut PodState)) {
        self.with(|i| f(i.state.entry(uid.to_owned()).or_default()));
    }

    fn unattributed(&self, reason: Unattributed, minutes: u64) {
        self.metrics.add(
            name::METER_UNATTRIBUTED,
            &[("reason", reason.as_str())],
            minutes,
        );
    }

    /// What is due of a pod: its kind, the minutes it ran and what of them is not sent. `None` when it is not
    /// metered, never ran, or has nothing due.
    fn due_of(&self, pod: &PodFacts, now: i64) -> Option<(Kind, metering::Ran, metering::Due)> {
        let kind = metering::classify(pod);
        if kind == Kind::Skip {
            return None;
        }
        let ran = metering::minutes(pod, now, self.config.skew_secs).ok()??;
        let due = metering::due(&ran.range, self.state(&pod.uid).cursor, now);
        Some((kind, ran, due))
    }

    /// One pass over the pods' facts at `now` (seconds since the Unix epoch). Returns how many events were sent.
    ///
    /// The cap of events is shared **fairly**: each pod that has minutes due gets an equal share (at least one), and
    /// the pod the pass starts with rotates, so no pod waits behind another's backlog.
    pub async fn pass(&self, pods: &[PodFacts], now: i64) -> usize {
        // Remember what was seen, before anything else: a pod that vanishes is closed from it.
        self.with(|i| {
            for pod in pods {
                if metering::classify(pod) == Kind::Skip {
                    continue;
                }
                let before = i.seen.get(&pod.uid).and_then(|s| s.last_running);
                i.seen.insert(
                    pod.uid.clone(),
                    Seen {
                        facts: pod.clone(),
                        last_running: if pod.phase == "Running" {
                            Some(now)
                        } else {
                            before
                        },
                    },
                );
            }
        });
        let wanting = pods
            .iter()
            .filter(|p| {
                self.due_of(p, now)
                    .is_some_and(|(_, _, d)| !d.send.is_empty())
            })
            .count();
        let share = (self.config.max_events_per_pass / wanting.max(1)).max(1);
        let start = self.with(|i| {
            i.rotor = i.rotor.wrapping_add(1);
            i.rotor
        });
        let mut sent = 0usize;
        for k in 0..pods.len() {
            let pod = &pods[(start + k) % pods.len()];
            if sent >= self.config.max_events_per_pass {
                break;
            }
            if self.process(pod, now, share, &mut sent).await == Progress::SinkDown {
                break;
            }
        }
        sent
    }

    /// Close the pods that are in the meter's memory and not in `listed`, which must be a **complete** listing
    /// of every scope: each is sent through the minute that contains `min(last finishedAt seen, last time it was
    /// seen Running)`, as soon as that minute is over, and then forgotten. Returns how many events were sent.
    pub async fn close_vanished(&self, listed: &HashSet<String>, now: i64) -> usize {
        let gone: Vec<(String, Seen)> = self.with(|i| {
            i.seen
                .iter()
                .filter(|(uid, _)| !listed.contains(*uid))
                .map(|(uid, s)| (uid.clone(), s.clone()))
                .collect()
        });
        let mut sent = 0usize;
        for (uid, seen) in gone {
            // The end, exclusive: the finish, or one second past the last sight (the minute it was seen in counts).
            let end = match (seen.facts.finished_at, seen.last_running) {
                (Some(f), Some(r)) => Some(f.min(r.saturating_add(1))),
                (Some(f), None) => Some(f),
                (None, Some(r)) => Some(r.saturating_add(1)),
                (None, None) => None,
            };
            let Some(end) = end else {
                self.forget(&uid);
                continue;
            };
            let mut facts = seen.facts.clone();
            facts.phase = "Succeeded".to_owned();
            facts.finished_at = Some(end);
            facts.deleted_at = None;
            facts.has_container_status = true;
            let share = self.config.max_events_per_pass;
            match self.process(&facts, now, share, &mut sent).await {
                Progress::Done => self.forget(&uid),
                Progress::Held => {}
                Progress::SinkDown => break,
            }
        }
        sent
    }

    fn forget(&self, uid: &str) {
        self.with(|i| {
            i.seen.remove(uid);
            i.state.remove(uid);
        });
    }

    /// One pod: its due minutes, attributed and sent, at most `share` of them.
    async fn process(&self, pod: &PodFacts, now: i64, share: usize, sent: &mut usize) -> Progress {
        let kind = metering::classify(pod);
        if kind == Kind::Skip {
            return Progress::Done;
        }
        let ran = match metering::minutes(pod, now, self.config.skew_secs) {
            Err(metering::NoEnd) => {
                if !self.state(&pod.uid).no_end_counted {
                    self.unattributed(Unattributed::NoEnd, 1);
                    self.update(&pod.uid, |s| s.no_end_counted = true);
                }
                return Progress::Done;
            }
            // Never ran (Pending, or no start was ever reported): nothing to send now, and nothing to wait for.
            Ok(None) => return Progress::Done,
            Ok(Some(ran)) => ran,
        };
        let after = if ran.complete {
            Progress::Done
        } else {
            Progress::Held
        };
        let state = self.state(&pod.uid);
        let due = metering::due(&ran.range, state.cursor, now);
        if due.skipped > 0 {
            self.metrics
                .add(name::METER_BACKFILL_SKIPPED, &[], due.skipped);
            self.update(&pod.uid, |s| s.cursor = Some(due.send.start));
        }
        if due.send.is_empty() {
            return after;
        }
        let count = due.send.end - due.send.start;
        let (pod_kind, service, run) = match kind {
            Kind::Run { service, run } => (PodKind::Run, service, run),
            Kind::Agent { service } => (PodKind::Agent, service, None),
            Kind::Skip => return Progress::Done,
        };
        // Configuration, not a passing condition: the minutes are given up, and counted.
        let give_up = |reason: Unattributed| {
            self.unattributed(reason, count);
            self.update(&pod.uid, |s| s.cursor = Some(due.send.end));
            after
        };
        let Some(service) = service else {
            return give_up(Unattributed::NoIdentity);
        };
        if pod_kind == PodKind::Run && run.is_none() {
            return give_up(Unattributed::NoIdentity);
        }
        let Some(weight_quarters) = metering::weight_quarters(&pod.annotations) else {
            return give_up(Unattributed::NoWeight);
        };
        let Some(node_type) = self
            .config
            .node_type_label_key
            .as_deref()
            .and_then(|key| metering::node_type(pod.node_affinity.as_ref(), key))
        else {
            return give_up(Unattributed::UnknownNodeType);
        };
        let pod_ref = match &run {
            Some(run) => PodRef::Run {
                service: service.clone(),
                run: run.clone(),
            },
            None => PodRef::Agent {
                service: service.clone(),
            },
        };
        let subject = match (self.accounts)(pod_ref).await {
            Ok(Some(account)) => account,
            Ok(None) => {
                // Not known *yet*: the cursor stays, so that an account that appears later gets its earlier
                // minutes. They are retried every pass until they pass the backfill bound, and counted once.
                let fresh = due
                    .send
                    .end
                    .saturating_sub(due.send.start.max(state.unknown_counted));
                self.unattributed(Unattributed::UnknownAccount, fresh);
                self.update(&pod.uid, |s| s.unknown_counted = due.send.end);
                return Progress::Held;
            }
            Err(AccountUnavailable) => {
                // Transient: the cursor stays, the next pass asks again. The bound is the backfill's.
                self.metrics.inc(name::METER_ACCOUNT_FAILURES, &[]);
                return Progress::Held;
            }
        };
        for minute in due.send.clone().take(share) {
            if *sent >= self.config.max_events_per_pass {
                return Progress::Held;
            }
            let event = PodMinuteEvent {
                source: self.config.source.clone(),
                event_id: event_id(&pod.uid, minute),
                minute,
                subject: subject.clone(),
                weight_quarters,
                node_type: node_type.clone(),
                pod_kind,
                agent_service: service.clone(),
                run: run.clone(),
            };
            match (self.sink)(event).await {
                Ok(()) => {
                    self.metrics.inc(name::METER_EVENTS, &[]);
                    self.update(&pod.uid, |s| s.cursor = Some(minute + 1));
                    *sent += 1;
                }
                Err(UsageError::Unavailable) => {
                    // The sink is down for everyone: hold this minute and every later one, try next pass.
                    self.metrics.inc(name::METER_SEND_FAILURES, &[]);
                    return Progress::SinkDown;
                }
                Err(UsageError::Invalid { field }) => {
                    // A bug, not a transient: never retried for ever. The field is named, not its value.
                    self.metrics.inc(name::METER_INVALID, &[]);
                    tracing::warn!(field, "the meter made an invalid event and dropped it");
                    self.update(&pod.uid, |s| s.cursor = Some(minute + 1));
                }
            }
        }
        let reached = self
            .state(&pod.uid)
            .cursor
            .is_some_and(|c| c >= ran.range.end);
        if reached { after } else { Progress::Held }
    }
}

/// What a pod's facts are, from the Kubernetes object.
pub fn facts_of(pod: &Pod) -> Option<PodFacts> {
    let uid = pod.metadata.uid.clone()?;
    let status = pod.status.as_ref();
    let statuses = status
        .and_then(|s| s.container_statuses.as_deref())
        .unwrap_or_default();
    let inits = status
        .and_then(|s| s.init_container_statuses.as_deref())
        .unwrap_or_default();
    // The earliest start any container ever reported: a restarted container's `state` is the new run, and
    // `lastState` the one before it (Kubernetes keeps only that one).
    let started_at = statuses
        .iter()
        .chain(inits)
        .flat_map(|c| {
            let state = c.state.as_ref();
            let last = c.last_state.as_ref();
            [
                state
                    .and_then(|s| s.running.as_ref())
                    .and_then(|r| r.started_at.as_ref()),
                state
                    .and_then(|s| s.terminated.as_ref())
                    .and_then(|t| t.started_at.as_ref()),
                last.and_then(|s| s.terminated.as_ref())
                    .and_then(|t| t.started_at.as_ref()),
            ]
        })
        .flatten()
        .map(|t| t.0.as_second())
        .min();
    let finished: Vec<Option<i64>> = statuses
        .iter()
        .map(|c| {
            c.state
                .as_ref()
                .and_then(|s| s.terminated.as_ref())
                .and_then(|t| t.finished_at.as_ref())
                .map(|t| t.0.as_second())
        })
        .collect();
    let finished_at = if finished.is_empty() || finished.iter().any(Option::is_none) {
        None
    } else {
        finished.into_iter().flatten().max()
    };
    Some(PodFacts {
        uid,
        labels: pod
            .metadata
            .labels
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect(),
        annotations: pod
            .metadata
            .annotations
            .clone()
            .unwrap_or_default()
            .into_iter()
            .collect(),
        phase: status.and_then(|s| s.phase.clone()).unwrap_or_default(),
        started_at,
        start_time: status
            .and_then(|s| s.start_time.as_ref())
            .map(|t| t.0.as_second()),
        has_container_status: !statuses.is_empty(),
        finished_at,
        deleted_at: pod
            .metadata
            .deletion_timestamp
            .as_ref()
            .map(|t| t.0.as_second()),
        node_affinity: pod
            .spec
            .as_ref()
            .and_then(|s| s.affinity.as_ref())
            .and_then(|a| a.node_affinity.as_ref())
            .and_then(|n| serde_json::to_value(n).ok()),
    })
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// What the loop is made of.
pub(crate) struct MeterLoop {
    pub client: Client,
    pub meter: Arc<Meter>,
    pub scopes: Vec<String>,
    pub leader: Arc<dyn LeaderElection>,
}

/// The pods of one scope that the meter reads: run pods and the operator's own, found by label selectors.
async fn list_scope(client: &Client, scope: &str) -> Result<Vec<PodFacts>, kube::Error> {
    let api: Api<Pod> = if scope.is_empty() {
        Api::all(client.clone())
    } else {
        Api::namespaced(client.clone(), scope)
    };
    let selectors = [
        format!("{MANAGED_BY_LABEL}={RUN_MANAGED_BY},app.kubernetes.io/component={RUN_COMPONENT}"),
        format!("{MANAGED_BY_LABEL}={AGENT_MANAGED_BY}"),
    ];
    let mut out = Vec::new();
    for selector in selectors {
        for pod in api.list(&ListParams::default().labels(&selector)).await? {
            out.extend(facts_of(&pod));
        }
    }
    Ok(out)
}

/// Look at the pods every period while this replica leads, until `stop` is true. A follower lists nothing and
/// sends nothing.
pub(crate) async fn run(l: MeterLoop, period: Duration, mut stop: watch::Receiver<bool>) {
    loop {
        if l.leader.is_leader() {
            let mut all = Vec::new();
            let mut complete = true;
            for scope in &l.scopes {
                match list_scope(&l.client, scope).await {
                    Ok(mut pods) => all.append(&mut pods),
                    Err(e) => {
                        complete = false;
                        tracing::warn!("the meter could not list the pods of {scope:?}: {e}");
                    }
                }
            }
            let now = unix_now();
            l.meter.pass(&all, now).await;
            if complete {
                // Only a complete listing says a pod is gone.
                let listed: HashSet<String> = all.iter().map(|p| p.uid.clone()).collect();
                l.meter.close_vanished(&listed, now).await;
            }
        }
        tokio::select! {
            () = tokio::time::sleep(period) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pod(status: serde_json::Value) -> Pod {
        serde_json::from_value(json!({
            "apiVersion": "v1", "kind": "Pod",
            "metadata": { "name": "p", "namespace": "n", "uid": "u" },
            "status": status,
        }))
        .unwrap()
    }

    fn container(
        name: &str,
        state: serde_json::Value,
        last: serde_json::Value,
    ) -> serde_json::Value {
        let mut c = json!({ "name": name, "image": "x", "imageID": "x", "ready": true, "restartCount": 1, "state": state });
        if !last.is_null() {
            c["lastState"] = last;
        }
        c
    }

    #[test]
    fn a_restarted_container_does_not_move_the_start_forward() {
        let p = pod(json!({ "phase": "Running", "containerStatuses": [container(
            "main",
            json!({ "running": { "startedAt": "2026-11-27T10:30:00Z" } }),
            json!({ "terminated": { "exitCode": 1, "startedAt": "2026-11-27T10:00:00Z", "finishedAt": "2026-11-27T10:29:00Z" } }),
        )]}));
        let f = facts_of(&p).unwrap();
        let ten = "2026-11-27T10:00:00Z"
            .parse::<k8s_openapi::jiff::Timestamp>()
            .unwrap()
            .as_second();
        assert_eq!(
            f.started_at,
            Some(ten),
            "the run before the restart is where the pod began"
        );
    }

    #[test]
    fn a_container_that_is_waiting_does_not_hide_the_one_that_runs_and_init_containers_count() {
        let p = pod(json!({
            "phase": "Running",
            "startTime": "2026-11-27T09:59:00Z",
            "initContainerStatuses": [container(
                "sidecar", json!({ "running": { "startedAt": "2026-11-27T10:00:00Z" } }), json!(null))],
            "containerStatuses": [
                container("a", json!({ "waiting": { "reason": "CrashLoopBackOff" } }), json!(null)),
                container("b", json!({ "running": { "startedAt": "2026-11-27T10:05:00Z" } }), json!(null)),
            ]
        }));
        let f = facts_of(&p).unwrap();
        let start = |t: &str| {
            t.parse::<k8s_openapi::jiff::Timestamp>()
                .unwrap()
                .as_second()
        };
        assert_eq!(f.started_at, Some(start("2026-11-27T10:00:00Z")));
        assert!(f.has_container_status);
        assert_eq!(f.start_time, Some(start("2026-11-27T09:59:00Z")));
        // Every container waiting: no container start at all, so the pod's start time is the anchor.
        let looping = pod(json!({
            "phase": "Running", "startTime": "2026-11-27T09:59:00Z",
            "containerStatuses": [container("a", json!({ "waiting": { "reason": "CrashLoopBackOff" } }), json!(null))]
        }));
        let f = facts_of(&looping).unwrap();
        assert_eq!(f.started_at, None);
        assert_eq!(
            metering::minutes(&f, start("2026-11-27T10:10:00Z"), 0)
                .unwrap()
                .map(|r| r.range.start),
            Some(u64::try_from(start("2026-11-27T09:59:00Z") / 60).unwrap())
        );
    }

    #[test]
    fn finished_needs_every_container_to_have_finished() {
        let done = |t: &str| json!({ "terminated": { "exitCode": 0, "startedAt": "2026-11-27T10:00:00Z", "finishedAt": t } });
        let p = pod(json!({ "phase": "Running", "containerStatuses": [
            container("a", done("2026-11-27T10:05:00Z"), json!(null)),
            container("b", json!({ "running": { "startedAt": "2026-11-27T10:00:00Z" } }), json!(null)),
        ]}));
        assert_eq!(facts_of(&p).unwrap().finished_at, None);
        let p = pod(json!({ "phase": "Succeeded", "containerStatuses": [
            container("a", done("2026-11-27T10:05:00Z"), json!(null)),
            container("b", done("2026-11-27T10:07:00Z"), json!(null)),
        ]}));
        let f = facts_of(&p).unwrap();
        assert!(f.finished_at.unwrap() > f.started_at.unwrap());
    }
}
