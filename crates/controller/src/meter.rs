//! The pod-minute meter (ANR-673, AD-069): every period the leader lists the pods that are run pods or an agent's
//! own, works out with [`crate::metering`] the whole minutes each ran that have not been sent, and hands one
//! [`PodMinuteEvent`] per minute to a [`UsageSink`], for the account an [`AccountSource`] names.
//!
//! [`Operator::with_meter`](crate::Operator::with_meter) turns it on, and `operator run` calls it only when the file's
//! `metering.enabled` is true (ANR-1103; off by default).
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
    AccountSource, AccountUnavailable, Fate, Outcome, PodKind, PodMinuteEvent, PodRef, UsageError,
    UsageSink, event_id,
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

/// The default of [`MeterConfig::pass_deadline`].
pub const DEFAULT_PASS_DEADLINE: Duration = Duration::from_secs(25);

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

/// What a sink says became of the events it took (ANR-1151), behind a pointer; `None`: it cannot say.
pub type OutcomesFn = Arc<dyn Fn() -> Option<Vec<Outcome>> + Send + Sync>;

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

/// Erase the type of `sink`, keeping its acknowledgements ([`UsageSink::outcomes`]): what the operator uses.
pub fn erase_sink_with_outcomes<S: UsageSink + 'static>(sink: S) -> (SinkFn, OutcomesFn) {
    let sink = Arc::new(sink);
    let reader = sink.clone();
    let send: SinkFn = Arc::new(move |event| {
        let sink = sink.clone();
        Box::pin(async move { sink.send(event).await })
    });
    (send, Arc::new(move || reader.outcomes()))
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
    /// How long one pass may take: past it the pass stops at the next pod (and a question to the account source is cut
    /// at it), so a hung broker cannot stretch a pass. The pods left wait for the next one.
    pub pass_deadline: Duration,
}

impl Default for MeterConfig {
    fn default() -> Self {
        Self {
            source: "aap-operator".to_owned(),
            node_type_label_key: None,
            period: DEFAULT_PERIOD,
            skew_secs: DEFAULT_SKEW_SECS,
            max_events_per_pass: MAX_EVENTS_PER_PASS,
            pass_deadline: DEFAULT_PASS_DEADLINE,
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
    /// Whether the pod's `no-end` or `no-start` was counted.
    unmetered_counted: bool,
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
    /// What is left of the period's cap of events, shared by [`Meter::pass`] and [`Meter::close_vanished`].
    budget: usize,
    /// When the running pass must stop.
    deadline: Option<tokio::time::Instant>,
    /// The account of a run that is bound (it never changes), by service and run: the broker is asked once per run.
    accounts: HashMap<(String, String), String>,
    /// Minutes handed to the sink and not yet acknowledged, by event id (ANR-1151). Empty when the sink reports no
    /// outcomes.
    pending: HashMap<String, Pending>,
    /// Set the first time the sink answers `None` to [`UsageSink::outcomes`]: it reports nothing, so nothing is
    /// tracked from then on (taking an event is the delivery) and `pending` is empty.
    untracked: bool,
}

/// A minute the sink took and the collector has not acknowledged.
#[derive(Clone, Debug)]
struct Pending {
    uid: String,
    minute: u64,
    /// When it was handed over (seconds since the Unix epoch), so that an acknowledgement that never comes expires.
    at: i64,
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
    /// Asked between pods: false stops the pass (this replica lost the lease).
    keep_going: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    /// Where the sink says what became of what it took; none: taking an event is delivering it.
    outcomes: Option<OutcomesFn>,
}

impl Meter {
    /// Track the sink's acknowledgements (ANR-1151): which minutes handed over the collector has acknowledged or
    /// refused. **This observes; it does not hold anything back:** the cursor that decides what is sent still moves
    /// when the sink takes the event. It is the groundwork of the durable part (ANR-1198), which needs to know what
    /// is safe to drop. A sink that answers `None` to its outcomes is detected at the first settle and tracking stops.
    #[must_use]
    pub fn with_outcomes(mut self, f: OutcomesFn) -> Self {
        self.outcomes = Some(f);
        self
    }

    /// How many minutes of `uid` the sink took and the collector has not yet acknowledged.
    pub fn unacknowledged(&self, uid: &str) -> usize {
        self.with(|i| i.pending.values().filter(|p| p.uid == uid).count())
    }

    /// The first minute of `uid` that is **not known delivered**: the oldest unacknowledged minute, or, when none
    /// is pending, the cursor of what was handed over. It moves only on the collector's acknowledgement (it is
    /// **not** the cursor that decides what is sent).
    pub fn acknowledged_through(&self, uid: &str) -> Option<u64> {
        self.with(|i| {
            i.pending
                .values()
                .filter(|p| p.uid == uid)
                .map(|p| p.minute)
                .min()
                .or_else(|| i.state.get(uid).and_then(|s| s.cursor))
        })
    }

    /// Read what the sink says became of its events at `now`: an acknowledged or refused minute stops being
    /// pending, and a minute unacknowledged past the backfill bound is forgotten (counted), so a lost
    /// acknowledgement cannot keep a minute pending for ever.
    pub fn settle(&self, now: i64) {
        let Some(read) = self.outcomes.clone() else {
            return;
        };
        let Some(outcomes) = read() else {
            // The sink reports nothing, and never will: stop tracking, free what was kept.
            self.with(|i| {
                i.untracked = true;
                i.pending.clear();
            });
            return;
        };
        let (mut delivered, mut refused) = (0u64, 0u64);
        let mut expired = 0u64;
        self.with(|i| {
            for Outcome { event_id, fate } in outcomes {
                if i.pending.remove(&event_id).is_some() {
                    match fate {
                        Fate::Delivered => delivered += 1,
                        Fate::Refused => refused += 1,
                    }
                }
            }
            let bound = i64::try_from(metering::BACKFILL_MINUTES * 60).unwrap_or(i64::MAX);
            let before = i.pending.len();
            i.pending.retain(|_, p| now.saturating_sub(p.at) <= bound);
            expired = (before - i.pending.len()) as u64;
        });
        self.metrics.add(name::METER_DELIVERED, &[], delivered);
        self.metrics.add(name::METER_REFUSED, &[], refused);
        self.metrics.add(name::METER_UNACKNOWLEDGED, &[], expired);
        if refused > 0 {
            tracing::warn!(refused, "the collector refused pod-minute events for good");
        }
    }

    /// Stop a pass between two pods when `f` says false (for example, when this replica no longer leads).
    #[must_use]
    pub fn with_continue(mut self, f: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.keep_going = Some(Arc::new(f));
        self
    }

    fn must_stop(&self) -> bool {
        self.keep_going.as_ref().is_some_and(|f| !f())
            || self
                .with(|i| i.deadline)
                .is_some_and(|d| tokio::time::Instant::now() >= d)
    }

    fn budget(&self) -> usize {
        self.with(|i| i.budget)
    }

    /// A meter with no cursor: its first pass sends what the pods' facts say, back to the bound.
    pub fn new(
        sink: SinkFn,
        accounts: AccountFn,
        config: MeterConfig,
        metrics: Arc<Metrics>,
    ) -> Self {
        let config_cap = config.max_events_per_pass;
        Self {
            sink,
            accounts,
            config,
            metrics,
            inner: Mutex::new(Inner {
                budget: config_cap,
                ..Inner::default()
            }),
            keep_going: None,
            outcomes: None,
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
    /// The cap of events is **one budget per period**, shared by this pass and by [`close_vanished`](Self::close_vanished)
    /// (which uses what is left): a period never sends more than `max_events_per_pass`. It is shared **fairly**: each
    /// pod that has minutes due gets an equal part of what is left (at least one), the pod the pass starts with
    /// rotates, and what a pod that sends nothing leaves is shared again by the others in the same pass.
    pub async fn pass(&self, pods: &[PodFacts], now: i64) -> usize {
        self.settle(now);
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
        // A new period: a fresh budget, which `close_vanished` then shares.
        let cap = self.config.max_events_per_pass;
        let start = self.with(|i| {
            i.budget = cap;
            i.deadline = Some(tokio::time::Instant::now() + self.config.pass_deadline);
            i.rotor = i.rotor.wrapping_add(1);
            i.rotor
        });
        let n = pods.len();
        let wants = |pod: &PodFacts| {
            self.due_of(pod, now)
                .is_some_and(|(_, _, d)| !d.send.is_empty())
        };
        let mut idle: HashSet<usize> = HashSet::new();
        let mut sent = 0usize;
        // Rounds: what a pod gets is an equal part of what is left for the pods still to be served, and what a pod
        // that sent nothing (an unknown account, say) leaves unspent is shared again by the pods that did send.
        loop {
            let before = sent;
            let live: Vec<usize> = (0..n)
                .map(|k| (start + k) % n)
                .filter(|i| !idle.contains(i))
                .collect();
            let mut candidates = live.iter().filter(|i| wants(&pods[**i])).count();
            for i in live {
                if self.must_stop() {
                    return sent;
                }
                let pod = &pods[i];
                let share = (self.budget() / candidates.max(1)).max(1);
                if wants(pod) {
                    candidates = candidates.saturating_sub(1);
                }
                let sent_here = sent;
                if self.process(pod, now, share, &mut sent).await == Progress::SinkDown {
                    return sent;
                }
                if sent == sent_here {
                    idle.insert(i);
                }
            }
            if sent == before || self.budget() == 0 {
                break;
            }
        }
        sent
    }

    /// Close the pods that are in the meter's memory and not in `listed`, which must be a **complete** listing
    /// of every scope: each is sent through the minute that contains `min(last finishedAt seen, last time it was
    /// seen Running)`, as soon as that minute is over, and then forgotten. Returns how many events were sent.
    pub async fn close_vanished(&self, listed: &HashSet<String>, now: i64) -> usize {
        self.settle(now);
        let gone: Vec<(String, Seen)> = self.with(|i| {
            i.seen
                .iter()
                .filter(|(uid, _)| !listed.contains(*uid))
                .map(|(uid, s)| (uid.clone(), s.clone()))
                .collect()
        });
        let mut sent = 0usize;
        let mut remaining = gone.len();
        for (uid, seen) in gone {
            let share = (self.budget() / remaining.max(1)).max(1);
            remaining = remaining.saturating_sub(1);
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
            Err(why) => {
                if !self.state(&pod.uid).unmetered_counted {
                    let reason = match why {
                        metering::Unmetered::NoEnd => Unattributed::NoEnd,
                        metering::Unmetered::NoStart => Unattributed::NoStart,
                    };
                    self.unattributed(reason, 1);
                    self.update(&pod.uid, |s| s.unmetered_counted = true);
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
        let cache_key = run.as_ref().map(|run| (service.clone(), run.clone()));
        let cached = cache_key
            .as_ref()
            .and_then(|k| self.with(|i| i.accounts.get(k).cloned()));
        let asked = match cached {
            Some(account) => Ok(Some(account)),
            None => {
                let left = self
                    .with(|i| i.deadline)
                    .map_or(self.config.pass_deadline, |d| {
                        d.saturating_duration_since(tokio::time::Instant::now())
                    });
                match tokio::time::timeout(left, (self.accounts)(pod_ref)).await {
                    Ok(answer) => answer,
                    // The pass ran out of time in the question: held like any failure to answer.
                    Err(_) => Err(AccountUnavailable),
                }
            }
        };
        let subject = match asked {
            Ok(Some(account)) => {
                // A bound run's account never changes: remembered for the life of the process.
                if let Some(k) = cache_key {
                    self.with(|i| i.accounts.insert(k, account.clone()));
                }
                account
            }
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
            if self.budget() == 0 {
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
                    if self.outcomes.is_some() && !self.with(|i| i.untracked) {
                        let id = event_id(&pod.uid, minute);
                        self.with(|i| {
                            i.pending.insert(
                                id,
                                Pending {
                                    uid: pod.uid.clone(),
                                    minute,
                                    at: now,
                                },
                            );
                        });
                    }
                    self.with(|i| i.budget = i.budget.saturating_sub(1));
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
    // The earliest start any **app** container ever reported (init containers and native sidecars never set it, and
    // neither does `status.startTime`, which includes Pending and the pull): a restarted container's `state` is the new run, and
    // `lastState` the one before it (Kubernetes keeps only that one).
    let started_at = statuses
        .iter()
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

/// One look at the pods: the clock is read **once, before the listing**, and that value stamps what the listing shows
/// and closes what it does not. A pod listed Running was Running at some instant at or after the moment the list
/// began, and the minute rule needs no more than that; a later time (the list can be slow) would bill a minute the
/// pod may not have lived. `list` answers the pods and whether the listing is complete.
pub(crate) async fn observe<F>(meter: &Meter, clock: &(dyn Fn() -> i64 + Send + Sync), list: F)
where
    F: Future<Output = (Vec<PodFacts>, bool)>,
{
    let now = clock();
    let (all, complete) = list.await;
    meter.pass(&all, now).await;
    if complete {
        // Only a complete listing says a pod is gone.
        let listed: HashSet<String> = all.iter().map(|p| p.uid.clone()).collect();
        meter.close_vanished(&listed, now).await;
    }
}

/// Look at the pods every period while this replica leads, until `stop` is true. A follower lists nothing and
/// sends nothing.
pub(crate) async fn run(l: MeterLoop, period: Duration, mut stop: watch::Receiver<bool>) {
    loop {
        // Acknowledgements keep arriving whoever leads: a follower drains them (nothing is listed or sent).
        l.meter.settle(unix_now());
        if l.leader.is_leader() {
            observe(&l.meter, &unix_now, async {
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
                (all, complete)
            })
            .await;
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

    fn at(t: &str) -> i64 {
        t.parse::<k8s_openapi::jiff::Timestamp>()
            .unwrap()
            .as_second()
    }

    #[test]
    fn a_container_that_is_waiting_does_not_hide_the_one_that_runs() {
        let p = pod(json!({
            "phase": "Running",
            "containerStatuses": [
                container("a", json!({ "waiting": { "reason": "CrashLoopBackOff" } }), json!(null)),
                container("b", json!({ "running": { "startedAt": "2026-11-27T10:05:00Z" } }), json!(null)),
            ]
        }));
        let f = facts_of(&p).unwrap();
        assert_eq!(f.started_at, Some(at("2026-11-27T10:05:00Z")));
        assert!(f.has_container_status);
    }

    #[test]
    fn an_init_container_that_ran_while_pending_does_not_start_the_billing() {
        // The init container ran 12:00 to 12:05 while the pod was Pending; the app container started at 12:08.
        let p = pod(json!({
            "phase": "Running",
            "startTime": "2026-11-27T11:59:00Z",
            "initContainerStatuses": [container(
                "init",
                json!({ "terminated": { "exitCode": 0, "startedAt": "2026-11-27T12:00:00Z", "finishedAt": "2026-11-27T12:05:00Z" } }),
                json!(null))],
            "containerStatuses": [container(
                "main", json!({ "running": { "startedAt": "2026-11-27T12:08:00Z" } }), json!(null))],
        }));
        assert_eq!(
            facts_of(&p).unwrap().started_at,
            Some(at("2026-11-27T12:08:00Z"))
        );
    }

    #[test]
    fn a_native_sidecar_that_started_before_the_main_container_does_not_move_the_start() {
        let p = pod(json!({
            "phase": "Running",
            "initContainerStatuses": [container(
                "sidecar", json!({ "running": { "startedAt": "2026-11-27T12:01:00Z" } }), json!(null))],
            "containerStatuses": [container(
                "main", json!({ "running": { "startedAt": "2026-11-27T12:08:00Z" } }), json!(null))],
        }));
        assert_eq!(
            facts_of(&p).unwrap().started_at,
            Some(at("2026-11-27T12:08:00Z"))
        );
    }

    #[test]
    fn a_crash_loop_that_never_started_has_no_start_and_the_pod_start_time_is_not_borrowed() {
        let p = pod(json!({
            "phase": "Running", "startTime": "2026-11-27T09:59:00Z",
            "containerStatuses": [container("a", json!({ "waiting": { "reason": "CrashLoopBackOff" } }), json!(null))]
        }));
        let f = facts_of(&p).unwrap();
        assert_eq!(f.started_at, None);
        assert_eq!(
            metering::minutes(&f, at("2026-11-27T10:10:00Z"), 0),
            Err(metering::Unmetered::NoStart)
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

    use aap_usage::{MemoryAccounts, MemoryUsageSink};
    use std::sync::atomic::{AtomicI64, Ordering};

    fn run_pod_facts(start: i64) -> PodFacts {
        PodFacts {
            uid: "uid-1".into(),
            labels: [
                ("app.kubernetes.io/managed-by", "adam-coder"),
                ("app.kubernetes.io/component", "run"),
                ("app.kubernetes.io/name", "coder-run"),
                // The coder's hash label; the run is the annotation.
                ("adam.vymalo.com/run", "0123456789ab"),
            ]
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect(),
            annotations: [
                ("agents.vymalo.com/size-weight".to_owned(), "1".to_owned()),
                ("adam.vymalo.com/run-id".to_owned(), "run-1".to_owned()),
            ]
            .into(),
            phase: "Running".into(),
            started_at: Some(start),
            has_container_status: true,
            node_affinity: Some(json!({ "requiredDuringSchedulingIgnoredDuringExecution": {
                "nodeSelectorTerms": [{ "matchExpressions": [
                    { "key": "node-type", "operator": "In", "values": ["big"] }] }] } })),
            ..PodFacts::default()
        }
    }

    #[tokio::test]
    async fn a_slow_listing_stamps_the_time_before_it_and_a_pod_that_vanishes_is_billed_to_that_minute()
     {
        let sink = Arc::new(MemoryUsageSink::new());
        struct S(Arc<MemoryUsageSink>);
        impl UsageSink for S {
            async fn send(&self, e: PodMinuteEvent) -> Result<(), UsageError> {
                self.0.send(e).await
            }
        }
        struct A;
        impl AccountSource for A {
            async fn account(&self, _: &PodRef) -> Result<Option<String>, AccountUnavailable> {
                Ok(Some("acct".into()))
            }
        }
        let _ = MemoryAccounts::new();
        let meter = Meter::new(
            erase_sink(S(sink.clone())),
            erase_accounts(A),
            MeterConfig {
                node_type_label_key: Some("node-type".into()),
                skew_secs: 0,
                ..MeterConfig::default()
            },
            Arc::new(Metrics::new()),
        );
        let clock = Arc::new(AtomicI64::new(1_020)); // minute 17
        let c = clock.clone();
        let read = move || c.load(Ordering::SeqCst);
        // The list starts at 1_020 (minute 17), takes until 1_100 (minute 18), and shows the pod Running.
        let c2 = clock.clone();
        observe(&meter, &read, async move {
            c2.store(1_100, Ordering::SeqCst);
            (vec![run_pod_facts(600)], true)
        })
        .await;
        // The next look sees nothing: the pod is gone. Long after, so every minute is over.
        clock.store(5_000, Ordering::SeqCst);
        observe(&meter, &read, async { (vec![], true) }).await;
        let minutes: Vec<u64> = sink.taken().iter().map(|e| e.minute).collect();
        assert_eq!(
            minutes.last(),
            Some(&17),
            "billed through the minute that holds the time before the list, not the time after it: {minutes:?}"
        );
        assert!(!minutes.contains(&18));
    }

    /// A meter over a counting account source and a memory sink, for the next tests.
    fn counting(
        config: MeterConfig,
        delay: Duration,
    ) -> (
        Meter,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<MemoryUsageSink>,
    ) {
        struct S(Arc<MemoryUsageSink>);
        impl UsageSink for S {
            async fn send(&self, e: PodMinuteEvent) -> Result<(), UsageError> {
                self.0.send(e).await
            }
        }
        struct A(Arc<std::sync::atomic::AtomicUsize>, Duration);
        impl AccountSource for A {
            async fn account(&self, _: &PodRef) -> Result<Option<String>, AccountUnavailable> {
                self.0.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(self.1).await;
                Ok(Some("acct".into()))
            }
        }
        let sink = Arc::new(MemoryUsageSink::new());
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let meter = Meter::new(
            erase_sink(S(sink.clone())),
            erase_accounts(A(asked.clone(), delay)),
            MeterConfig {
                node_type_label_key: Some("node-type".into()),
                skew_secs: 0,
                ..config
            },
            Arc::new(Metrics::new()),
        );
        (meter, asked, sink)
    }

    #[tokio::test]
    async fn a_bound_runs_account_is_asked_once_for_the_life_of_the_meter() {
        let (meter, asked, sink) = counting(MeterConfig::default(), Duration::ZERO);
        meter.pass(&[run_pod_facts(600)], 1_000).await;
        meter.pass(&[run_pod_facts(600)], 1_200).await;
        meter.pass(&[run_pod_facts(600)], 1_500).await;
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        assert!(sink.taken().len() > 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_account_source_is_cut_at_the_pass_deadline_and_the_minutes_are_held() {
        let (meter, _asked, sink) = counting(
            MeterConfig {
                pass_deadline: Duration::from_secs(2),
                ..MeterConfig::default()
            },
            Duration::from_secs(3_600),
        );
        // Returns at the deadline (the paused clock advances), with nothing sent and the minutes still due.
        assert_eq!(meter.pass(&[run_pod_facts(600)], 1_000).await, 0);
        assert!(sink.taken().is_empty());
    }

    #[tokio::test]
    async fn a_pass_stops_between_pods_once_leadership_is_lost() {
        let (meter, asked, _sink) = counting(MeterConfig::default(), Duration::ZERO);
        let meter = meter.with_continue(|| false);
        assert_eq!(meter.pass(&[run_pod_facts(600)], 1_000).await, 0);
        assert_eq!(asked.load(Ordering::SeqCst), 0);
    }
}
