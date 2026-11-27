//! The pod-minute meter (ANR-673, AD-069): every period the leader lists the pods that are run pods or an agent's
//! own, works out with [`crate::metering`] the whole minutes each ran that have not been sent, and hands one
//! [`PodMinuteEvent`] per minute to a [`UsageSink`], for the account an [`AccountSource`] names.
//!
//! **Not wired into `operator run` yet** (a later slice, AD-069): [`Operator::with_meter`](crate::Operator::with_meter) turns it
//! on, and nothing calls it from the binary.
//!
//! The cursor of each pod (the first minute not sent) is **in memory**. A restart, a new leader and a second
//! replica therefore re-derive the same minutes from the pod's facts, with the same `event_id`s and the same
//! end-of-minute times, and the collector dedupes them: the backfill bound (24 h) is what limits the repeat.
//! A pod that lives and vanishes between two lists is not seen (the period is 30 s by default).

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

/// The most events one pass sends (the rest waits for the next one), so a long backfill after a failover
/// cannot hold the loop.
pub const MAX_EVENTS_PER_PASS: usize = 5_000;

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
}

impl Default for MeterConfig {
    fn default() -> Self {
        Self {
            source: "aap-operator".to_owned(),
            node_type_label_key: None,
            period: DEFAULT_PERIOD,
        }
    }
}

/// The meter: the cursors, and what it sends to and asks.
pub struct Meter {
    sink: SinkFn,
    accounts: AccountFn,
    config: MeterConfig,
    metrics: Arc<Metrics>,
    cursors: Mutex<HashMap<String, u64>>,
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
            cursors: Mutex::new(HashMap::new()),
        }
    }

    fn cursor(&self, uid: &str) -> Option<u64> {
        self.cursors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(uid)
            .copied()
    }

    fn set_cursor(&self, uid: &str, minute: u64) {
        self.cursors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(uid.to_owned(), minute);
    }

    /// Forget the pods that are not in `seen`: they are gone. Call it only with a complete listing.
    pub fn prune(&self, seen: &HashSet<String>) {
        self.cursors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|uid, _| seen.contains(uid));
    }

    fn unattributed(&self, reason: Unattributed, minutes: u64) {
        self.metrics.add(
            name::METER_UNATTRIBUTED,
            &[("reason", reason.as_str())],
            minutes,
        );
    }

    /// One pass over the pods' facts at `now` (seconds since the Unix epoch). Returns how many events were sent.
    pub async fn pass(&self, pods: &[PodFacts], now: i64) -> usize {
        let mut sent = 0usize;
        for pod in pods {
            if sent >= MAX_EVENTS_PER_PASS {
                break;
            }
            let kind = metering::classify(pod);
            if kind == Kind::Skip {
                continue;
            }
            let Some(ran) = metering::minutes(pod, now) else {
                continue;
            };
            let due = metering::due(&ran, self.cursor(&pod.uid), now);
            if due.skipped > 0 {
                self.metrics
                    .add(name::METER_BACKFILL_SKIPPED, &[], due.skipped);
                self.set_cursor(&pod.uid, due.send.start);
            }
            if due.send.is_empty() {
                continue;
            }
            let count = due.send.end - due.send.start;
            let (pod_kind, service, run) = match kind {
                Kind::Run { service, run } => (PodKind::Run, service, run),
                Kind::Agent { service } => (PodKind::Agent, service, None),
                Kind::Skip => continue,
            };
            // The cheap checks first: a pod that cannot make a line is never asked about.
            let hold = |reason: Unattributed| {
                self.unattributed(reason, count);
                self.set_cursor(&pod.uid, due.send.end);
            };
            let Some(service) = service else {
                hold(Unattributed::NoIdentity);
                continue;
            };
            if pod_kind == PodKind::Run && run.is_none() {
                hold(Unattributed::NoIdentity);
                continue;
            }
            let Some(weight_quarters) = metering::weight_quarters(&pod.annotations) else {
                hold(Unattributed::NoWeight);
                continue;
            };
            let Some(node_type) = self
                .config
                .node_type_label_key
                .as_deref()
                .and_then(|key| metering::node_type(pod.node_affinity.as_ref(), key))
            else {
                hold(Unattributed::UnknownNodeType);
                continue;
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
                    hold(Unattributed::UnknownAccount);
                    continue;
                }
                Err(AccountUnavailable) => {
                    // Transient: the cursor stays, the next pass asks again. The bound is the backfill's.
                    self.metrics.inc(name::METER_ACCOUNT_FAILURES, &[]);
                    continue;
                }
            };
            for minute in due.send.clone() {
                if sent >= MAX_EVENTS_PER_PASS {
                    break;
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
                        self.set_cursor(&pod.uid, minute + 1);
                        sent += 1;
                    }
                    Err(UsageError::Unavailable) => {
                        // The sink is down for everyone: hold this minute and every later one, try next pass.
                        self.metrics.inc(name::METER_SEND_FAILURES, &[]);
                        return sent;
                    }
                    Err(UsageError::Invalid { field }) => {
                        // A bug, not a transient: never retried for ever. The field is named, not its value.
                        self.metrics.inc(name::METER_SEND_FAILURES, &[]);
                        tracing::warn!(field, "the meter made an invalid event and dropped it");
                        self.set_cursor(&pod.uid, minute + 1);
                    }
                }
            }
        }
        sent
    }
}

/// What a pod's facts are, from the Kubernetes object.
pub fn facts_of(pod: &Pod) -> Option<PodFacts> {
    let uid = pod.metadata.uid.clone()?;
    let statuses = pod
        .status
        .as_ref()
        .and_then(|s| s.container_statuses.as_deref())
        .unwrap_or_default();
    let started_at = statuses
        .iter()
        .filter_map(|c| {
            let state = c.state.as_ref()?;
            state
                .running
                .as_ref()
                .and_then(|r| r.started_at.as_ref())
                .or_else(|| {
                    state
                        .terminated
                        .as_ref()
                        .and_then(|t| t.started_at.as_ref())
                })
                .map(|t| t.0.as_second())
        })
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
        phase: pod
            .status
            .as_ref()
            .and_then(|s| s.phase.clone())
            .unwrap_or_default(),
        started_at,
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
            l.meter.pass(&all, unix_now()).await;
            if complete {
                l.meter.prune(&all.iter().map(|p| p.uid.clone()).collect());
            }
        }
        tokio::select! {
            () = tokio::time::sleep(period) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
    }
}
