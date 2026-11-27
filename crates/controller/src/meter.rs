//! The pod-minute meter (ANR-673, AD-069): every period the leader lists the pods that are run pods or an agent's
//! own, works out with [`crate::metering`] the whole minutes each ran that have not been sent, and hands one
//! [`PodMinuteEvent`] per minute to a [`UsageSink`], for the account an [`AccountSource`] names.
//!
//! [`Operator::with_meter`](crate::Operator::with_meter) turns it on, and `operator run` calls it only when the file's
//! `metering.enabled` is true (ANR-1103; off by default).
//!
//! The cursor of each pod (the first minute not sent) and the last facts seen of each pod are **in memory**. A
//! restart, a new leader and a second replica therefore re-derive the same minutes of a pod that is still listed
//! from the pod's facts, with the same `event_id`s and the same end-of-minute times, and the collector dedupes them:
//! the backfill bound (24 h) limits the repeat.
//!
//! **A pod that has vanished is the one the API server no longer remembers**, so its facts are persisted
//! (ANR-1198, AD-069 amended): when [`Meter::close_vanished`] finds a pod gone with minutes that are not known
//! delivered, it writes one [`Tombstone`] (before it sends anything), and deletes it when those minutes are
//! acknowledged or the 24 h bound has passed. A new leader reads the tombstones ([`Meter::restore`]) and sends the
//! minutes again, **repeating only by `event_id`**. Nothing is written for a pod that is still listed, and nothing
//! per period.
//!
//! **A vanished pod is closed** ([`Meter::close_vanished`]): a pod missing from a complete listing is sent through
//! the minute that contains `min(last finishedAt seen, last time it was seen Running)`. **The residual loss** is the
//! minutes between the last observation and the real deletion (at most one period plus the partial minute), and
//! the whole life of a pod that began and vanished between two lists, and any time the meter was not running (an
//! outage or a failover gap: the facts are in memory). `now` is this replica's clock: [`MeterConfig::skew_secs`]
//! keeps a minute that is not over on the API server's clock from being sent.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aap_ports::LeaderElection;
use aap_usage::{
    AccountSource, AccountUnavailable, Fate, Loaded, MAX_TOMBSTONES, Outcome, PodKind,
    PodMinuteEvent, PodRef, Tombstone, TombstoneError, TombstoneStore, UsageError, UsageSink,
    event_id, is_line_text,
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

type TombstoneFuture<T> = Pin<Box<dyn Future<Output = Result<T, TombstoneError>> + Send>>;

/// A [`TombstoneStore`] behind pointers, so the operator is not generic over it.
#[derive(Clone)]
pub struct TombstonesFn {
    load: Arc<dyn Fn(String) -> TombstoneFuture<Loaded> + Send + Sync>,
    save: Arc<dyn Fn(String, Vec<Tombstone>) -> TombstoneFuture<()> + Send + Sync>,
}

/// Erase the type of `store`.
pub fn erase_tombstones<T: TombstoneStore + 'static>(store: T) -> TombstonesFn {
    let store = Arc::new(store);
    let reader = store.clone();
    TombstonesFn {
        load: Arc::new(move |scope| {
            let store = reader.clone();
            Box::pin(async move { store.load(&scope).await })
        }),
        save: Arc::new(move |namespace, set| {
            let store = store.clone();
            Box::pin(async move { store.save(&namespace, &set).await })
        }),
    }
}

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
    /// The tombstones of vanished pods, by pod uid (ANR-1198): what is persisted, plus what is about to be.
    tombs: HashMap<String, Tombstone>,
    /// The namespaces whose set differs from what the store holds, so the next flush writes them.
    tomb_dirty: BTreeSet<String>,
    /// The scopes (a namespace, or the empty scope for every one) whose set was **read at the start of this pass**.
    /// Nothing of a namespace is written before it is, and a stale writer is found when the set is read again each
    /// pass: a write replaces the set, so one made unread would delete what another leader left.
    restored: HashSet<String>,
    /// The scopes the store is forbidden for (no right to the `ConfigMap`): memory only for the rest of the term.
    memory_only: HashSet<String>,
    /// The pods whose tombstone is known to be in the store (read or written).
    persisted: HashSet<String>,
    /// Whether this term already looked for the sets of namespaces that are no longer watched.
    swept: bool,
}

/// The most vanished pods the meter holds in memory because their namespace's store cannot be read or written; the
/// excess is sent from memory like any pod with no tombstone (counted as unbacked).
const MAX_HELD: usize = 2_048;

fn scope_has(set: &HashSet<String>, namespace: &str) -> bool {
    set.contains("") || set.contains(namespace)
}

/// A minute the sink took and the collector has not acknowledged.
#[derive(Clone, Debug)]
struct Pending {
    uid: String,
    minute: u64,
    /// When it was handed over (seconds since the Unix epoch), so that an acknowledgement that never comes expires.
    at: i64,
}

/// What a pod is for a line, once its facts say so.
struct Attributed {
    pod_kind: PodKind,
    service: String,
    run: Option<String>,
    weight_quarters: u32,
    node_type: String,
}

/// How [`Meter::emit`] ended.
enum Emit {
    /// Every minute asked for was handled.
    Finished,
    /// The period's budget ran out: the rest waits.
    BudgetOut,
    /// The sink is down: stop the pass.
    SinkDown,
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
    /// Where the facts of vanished pods are kept (ANR-1198); none: they are in memory only.
    tombstones: Option<TombstonesFn>,
}

impl Meter {
    /// Track the sink's acknowledgements (ANR-1151): which minutes handed over the collector has acknowledged or
    /// refused. **This observes; it does not hold anything back:** the cursor that decides what is sent still moves
    /// when the sink takes the event. It tells the tombstones (ANR-1198) what is safe to drop. A sink that answers `None` to its
    /// outcomes is detected at the first settle and tracking stops.
    #[must_use]
    pub fn with_outcomes(mut self, f: OutcomesFn) -> Self {
        self.outcomes = Some(f);
        self
    }

    /// Keep the facts of vanished pods in `store` (ANR-1198): a pod found gone with minutes not known delivered gets a
    /// tombstone before anything is sent, and a replica that begins to lead reads them ([`restore`](Self::restore)).
    /// Without a store the facts are in memory only and a crash or a failover loses those minutes.
    #[must_use]
    pub fn with_tombstones(mut self, store: TombstonesFn) -> Self {
        self.tombstones = Some(store);
        self
    }

    /// How many tombstones of `namespace` the meter holds (written, or about to be).
    pub fn tombstones_held(&self, namespace: &str) -> usize {
        self.with(|i| {
            i.tombs
                .values()
                .filter(|t| t.namespace == namespace)
                .count()
        })
    }

    /// This replica no longer leads: the next term reads the store again before it writes.
    pub fn lose_leadership(&self) {
        self.with(|i| {
            i.restored.clear();
            i.memory_only.clear();
            i.swept = false;
        });
    }

    fn leading(&self) -> bool {
        self.keep_going.as_ref().is_none_or(|f| f())
    }

    /// Whether the set of `namespace` was read at the start of this pass (so a write over it is not a blind one).
    fn can_write(&self, namespace: &str) -> bool {
        self.with(|i| scope_has(&i.restored, namespace))
    }

    /// Whether the store may not be used for `namespace` at all in this term (forbidden).
    fn memory_only(&self, namespace: &str) -> bool {
        self.with(|i| scope_has(&i.memory_only, namespace))
    }

    /// Read the tombstone sets of `scopes` (an empty scope is every namespace) and take what they hold as the facts
    /// of pods that are gone: their minutes are sent again from the first one not known delivered, with the same
    /// `event_id`s. **Called at the start of every pass while leading**, not only when a term begins: an overlapping
    /// leader that wrote after the last read is found by the next one (one `get` per namespace), and a tombstone the
    /// other leader released is let go. Each scope is read **on its own**: one that fails is not written or
    /// replayed this pass and the others go on, and one the store is forbidden for is memory-only for the term.
    /// Returns whether every scope was read.
    pub async fn restore(&self, scopes: &[String]) -> bool {
        let Some(store) = self.tombstones.clone() else {
            return true;
        };
        let mut all_read = true;
        let mut read = HashSet::new();
        for scope in scopes {
            if self.with(|i| i.memory_only.contains(scope)) {
                continue;
            }
            match (store.load)(scope.clone()).await {
                Ok(loaded) => {
                    self.merge(scope, loaded);
                    read.insert(scope.clone());
                }
                Err(TombstoneError::Forbidden) => {
                    self.with(|i| i.memory_only.insert(scope.clone()));
                    self.metrics
                        .inc(name::METER_TOMBSTONE_ERRORS, &[("op", "load")]);
                    tracing::warn!(
                        "the meter may not read its tombstones of a scope: vanished pods there are metered from memory only"
                    );
                }
                Err(_) => {
                    all_read = false;
                    self.metrics
                        .inc(name::METER_TOMBSTONE_ERRORS, &[("op", "load")]);
                    tracing::warn!(
                        "the meter could not read its tombstones of a scope: nothing of it is written or replayed this pass"
                    );
                }
            }
        }
        self.with(|i| i.restored = read);
        if all_read {
            self.sweep(scopes).await;
        }
        all_read
    }

    /// Take what the store holds for `scope` into memory.
    fn merge(&self, scope: &str, loaded: Loaded) {
        self.metrics
            .add(name::METER_TOMBSTONES_UNREADABLE, &[], loaded.unreadable);
        self.metrics
            .add(name::METER_TOMBSTONES_REFUSED, &[], loaded.refused);
        let (mut restored, mut invalid) = (0u64, 0u64);
        self.with(|i| {
            let in_scope = |ns: &str| scope.is_empty() || ns == scope;
            let on_disk: HashSet<String> =
                loaded.tombstones.iter().map(|t| t.uid.clone()).collect();
            for t in loaded.tombstones {
                if !tombstone_is_valid(&t) {
                    // Left out of the next write (the set is replaced whole) and counted.
                    i.tomb_dirty.insert(t.namespace.clone());
                    invalid += 1;
                    continue;
                }
                i.persisted.insert(t.uid.clone());
                if i.tombs.contains_key(&t.uid) {
                    continue;
                }
                // From the first minute not known delivered: what is acknowledged again is a repeat by event id.
                i.state.entry(t.uid.clone()).or_default().cursor = Some(t.first_minute);
                i.seen.remove(&t.uid);
                i.tombs.insert(t.uid.clone(), t);
                restored += 1;
            }
            // One that was in the store and no longer is was released by another leader (its minutes were acknowledged
            // there): let it go, unless this replica has a change of its own waiting to be written.
            let released: Vec<String> = i
                .tombs
                .values()
                .filter(|t| {
                    in_scope(&t.namespace)
                        && i.persisted.contains(&t.uid)
                        && !on_disk.contains(&t.uid)
                        && !i.tomb_dirty.contains(&t.namespace)
                })
                .map(|t| t.uid.clone())
                .collect();
            for uid in released {
                i.tombs.remove(&uid);
                i.persisted.remove(&uid);
            }
            // What this replica holds and the store does not is written at the next flush.
            let ahead: Vec<String> = i
                .tombs
                .values()
                .filter(|t| in_scope(&t.namespace) && !on_disk.contains(&t.uid))
                .map(|t| t.namespace.clone())
                .collect();
            i.tomb_dirty.extend(ahead);
        });
        self.metrics
            .add(name::METER_TOMBSTONES_RESTORED, &[], restored);
        self.metrics
            .add(name::METER_TOMBSTONES_INVALID, &[], invalid);
    }

    /// Once a term: the set of a namespace that is no longer watched, every record of which is past the 24 h bound, is
    /// deleted (so a namespace taken out of `watch.namespaces` leaves nothing behind for ever).
    async fn sweep(&self, scopes: &[String]) {
        let Some(store) = self.tombstones.clone() else {
            return;
        };
        if self.with(|i| i.swept) || scopes.iter().any(String::is_empty) || !self.leading() {
            return;
        }
        let now = unix_now();
        let bound = i64::try_from(metering::BACKFILL_MINUTES * 60).unwrap_or(i64::MAX);
        match (store.load)(String::new()).await {
            Ok(all) => {
                let mut by_ns: BTreeMap<String, Vec<Tombstone>> = BTreeMap::new();
                for t in all.tombstones {
                    if !scopes.contains(&t.namespace) {
                        by_ns.entry(t.namespace.clone()).or_default().push(t);
                    }
                }
                for (namespace, set) in by_ns {
                    if set.iter().all(|t| now.saturating_sub(t.written_at) > bound)
                        && (store.save)(namespace, Vec::new()).await.is_ok()
                    {
                        self.metrics
                            .add(name::METER_TOMBSTONES_EXPIRED, &[], set.len() as u64);
                    }
                }
                self.with(|i| i.swept = true);
            }
            Err(TombstoneError::Forbidden) => self.with(|i| i.swept = true),
            Err(_) => {}
        }
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
            tombstones: None,
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
    ///
    /// With a tombstone store (ANR-1198) a pod that still has minutes to send, or minutes not known delivered, is
    /// first written as one tombstone (a batch per namespace, **before** anything is sent) and then sent from it by
    /// [`replay`](Self::replay), so a crash or a failover in between loses nothing. A pod whose tombstone could not
    /// be written because the store is down is **held** (it stays in memory and is tried next pass); one whose
    /// namespace is full, or that cannot be attributed, is sent from memory as without a store.
    pub async fn close_vanished(&self, listed: &HashSet<String>, now: i64) -> usize {
        self.settle(now);
        let gone: Vec<(String, Seen)> = self.with(|i| {
            i.seen
                .iter()
                .filter(|(uid, _)| !listed.contains(*uid))
                .map(|(uid, s)| (uid.clone(), s.clone()))
                .collect()
        });
        let durable = self.tombstones.is_some();
        if durable && !self.leading() {
            // A replica that lost the lease writes nothing and closes nothing: the leader reads the store again.
            return 0;
        }
        // The end, exclusive, and the facts of each pod as a pod that has finished.
        let mut closed: Vec<(String, PodFacts)> = Vec::new();
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
            closed.push((uid, facts));
        }
        let mut held: HashSet<String> = HashSet::new();
        if durable {
            let mut candidates = Vec::new();
            for (uid, facts) in &closed {
                if self.memory_only(&facts.namespace) {
                    // The store is forbidden for this namespace: from memory, as without a store (counted at load).
                    continue;
                }
                if !self.can_write(&facts.namespace) {
                    // Its set was not read this pass: nothing of it is written, and the pod waits in memory.
                    held.insert(uid.clone());
                    continue;
                }
                // A pod with nothing outstanding, or that is not attributable, gets none: the in-memory close below
                // counts it and forgets it. So does a pod the store has no room for (`unbacked`).
                candidates.extend(self.tombstone_of(facts, now));
            }
            let (backed, _unbacked, down) = self.persist(candidates).await;
            self.with(|i| {
                for uid in &backed {
                    // The tombstone is the pod's facts from here on; its cursor stays in `state`.
                    i.seen.remove(uid);
                }
            });
            held.extend(down);
            closed.retain(|(uid, _)| !backed.contains(uid));
            self.bound_held(&mut held, &mut closed, now);
        }
        let mut sent = 0usize;
        let mut remaining = closed.len();
        for (uid, facts) in closed {
            if held.contains(&uid) {
                continue;
            }
            let share = (self.budget() / remaining.max(1)).max(1);
            remaining = remaining.saturating_sub(1);
            match self.process(&facts, now, share, &mut sent).await {
                Progress::Done => self.forget(&uid),
                Progress::Held => {}
                Progress::SinkDown => break,
            }
        }
        sent + self.replay(now).await
    }

    /// What is held for want of a store is bounded: a pod whose minutes are all past the 24 h bound is forgotten
    /// (counted), and past [`MAX_HELD`] the excess is let go to the in-memory close like a pod with no tombstone.
    fn bound_held(
        &self,
        held: &mut HashSet<String>,
        closed: &mut Vec<(String, PodFacts)>,
        now: i64,
    ) {
        let bound = i64::try_from(metering::BACKFILL_MINUTES * 60).unwrap_or(i64::MAX);
        let mut expired = Vec::new();
        for (uid, facts) in closed.iter() {
            if held.contains(uid)
                && facts
                    .finished_at
                    .is_some_and(|end| now.saturating_sub(end) > bound)
            {
                expired.push(uid.clone());
            }
        }
        for uid in &expired {
            held.remove(uid);
            self.forget(uid);
        }
        closed.retain(|(uid, _)| !expired.contains(uid));
        self.metrics
            .add(name::METER_HELD_EXPIRED, &[], expired.len() as u64);
        if held.len() > MAX_HELD {
            let mut ordered: Vec<String> = held.iter().cloned().collect();
            ordered.sort();
            let excess = ordered.split_off(MAX_HELD);
            self.metrics
                .add(name::METER_TOMBSTONES_UNBACKED, &[], excess.len() as u64);
            for uid in excess {
                held.remove(&uid);
            }
        }
    }

    /// The tombstone of a pod found gone, if it needs one: it has minutes still to send or minutes not known
    /// delivered, its facts attribute it to an account's line, and every text of it is one a line may carry.
    fn tombstone_of(&self, closed: &PodFacts, now: i64) -> Option<Tombstone> {
        let kind = metering::classify(closed);
        let started_at = closed.started_at?;
        let ended_at = closed.finished_at?;
        let outstanding = self.with(|i| i.pending.values().any(|p| p.uid == closed.uid))
            || self
                .due_of(closed, now)
                .is_some_and(|(_, ran, due)| !due.send.is_empty() || !ran.complete);
        if !outstanding {
            return None;
        }
        let a = self.attribute(closed, &kind).ok()?;
        let subject = a.run.as_ref().and_then(|run| {
            self.with(|i| i.accounts.get(&(a.service.clone(), run.clone())).cloned())
        });
        let t = Tombstone {
            namespace: closed.namespace.clone(),
            uid: closed.uid.clone(),
            service: a.service,
            run: a.run,
            pod_kind: a.pod_kind,
            subject,
            weight_quarters: a.weight_quarters,
            node_type: a.node_type,
            started_at,
            ended_at: ended_at.max(started_at),
            first_minute: self
                .acknowledged_through(&closed.uid)
                .unwrap_or_else(|| metering::to_minute(started_at)),
            written_at: now,
        };
        tombstone_is_valid(&t).then_some(t)
    }

    /// Write `new` (grouped by namespace, one write each) and say what became of each: **backed** (written: send
    /// from the tombstone), **unbacked** (no room: send from memory, counted) or **down** (the store could not be
    /// written: hold the pod).
    async fn persist(
        &self,
        new: Vec<Tombstone>,
    ) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
        let (mut backed, mut unbacked, mut down) = (HashSet::new(), HashSet::new(), HashSet::new());
        let mut groups: BTreeMap<String, Vec<Tombstone>> = BTreeMap::new();
        for t in new {
            groups.entry(t.namespace.clone()).or_default().push(t);
        }
        for (namespace, group) in groups {
            let room = MAX_TOMBSTONES.saturating_sub(self.tombstones_held(&namespace));
            let (fit, over): (Vec<_>, Vec<_>) =
                group.into_iter().enumerate().partition(|(k, _)| *k < room);
            let fit: Vec<Tombstone> = fit.into_iter().map(|(_, t)| t).collect();
            if !over.is_empty() {
                self.metrics
                    .add(name::METER_TOMBSTONES_UNBACKED, &[], over.len() as u64);
                tracing::warn!(
                    namespace,
                    "the meter's tombstone set is full: pods that vanish now are metered from memory only"
                );
                unbacked.extend(over.into_iter().map(|(_, t)| t.uid));
            }
            if fit.is_empty() {
                continue;
            }
            let was_dirty = self.with(|i| {
                let was = i.tomb_dirty.contains(&namespace);
                for t in &fit {
                    i.tombs.insert(t.uid.clone(), t.clone());
                }
                i.tomb_dirty.insert(namespace.clone());
                was
            });
            let result = self.save_namespace(&namespace).await;
            if result.is_ok() {
                self.metrics
                    .add(name::METER_TOMBSTONES_WRITTEN, &[], fit.len() as u64);
                self.with(|i| i.persisted.extend(fit.iter().map(|t| t.uid.clone())));
                backed.extend(fit.into_iter().map(|t| t.uid));
                continue;
            }
            // Nothing was written: the memory goes back to what the store holds.
            self.with(|i| {
                for t in &fit {
                    i.tombs.remove(&t.uid);
                }
                if !was_dirty {
                    i.tomb_dirty.remove(&namespace);
                }
            });
            if matches!(
                result,
                Err(TombstoneError::TooLarge | TombstoneError::Forbidden)
            ) {
                self.metrics
                    .add(name::METER_TOMBSTONES_UNBACKED, &[], fit.len() as u64);
                unbacked.extend(fit.into_iter().map(|t| t.uid));
            } else {
                down.extend(fit.into_iter().map(|t| t.uid));
            }
        }
        (backed, unbacked, down)
    }

    /// Write the whole set of `namespace`. **Only while this replica still leads**, checked right before the write: a
    /// stale leader's write would succeed (its version is current) and a replica that read the set earlier would
    /// never see it.
    async fn save_namespace(&self, namespace: &str) -> Result<(), TombstoneError> {
        let Some(store) = self.tombstones.clone() else {
            return Ok(());
        };
        if !self.leading() {
            return Err(TombstoneError::Unavailable);
        }
        let set: Vec<Tombstone> = self.with(|i| {
            let mut v: Vec<Tombstone> = i
                .tombs
                .values()
                .filter(|t| t.namespace == namespace)
                .cloned()
                .collect();
            v.sort_by(|a, b| a.uid.cmp(&b.uid));
            v
        });
        match (store.save)(namespace.to_owned(), set).await {
            Ok(()) => {
                self.with(|i| i.tomb_dirty.remove(namespace));
                Ok(())
            }
            Err(e) => {
                self.metrics
                    .inc(name::METER_TOMBSTONE_ERRORS, &[("op", "save")]);
                tracing::warn!("the meter could not write its tombstones of a namespace");
                match e {
                    TombstoneError::Conflict => {
                        // Another writer, or the object is gone: this namespace is read again before the next write.
                        self.with(|i| {
                            i.restored.remove(namespace);
                            i.restored.remove("");
                        });
                    }
                    TombstoneError::Forbidden => {
                        self.with(|i| i.memory_only.insert(namespace.to_owned()));
                    }
                    _ => {}
                }
                Err(e)
            }
        }
    }

    /// Send what the tombstones say is not sent, and delete each tombstone whose minutes are all sent **and**
    /// acknowledged (or when the 24 h bound has passed since it was written). One write per namespace that
    /// changed, at the end. Returns how many events were sent. Does nothing until [`restore`](Self::restore) read
    /// the store.
    pub async fn replay(&self, now: i64) -> usize {
        if self.tombstones.is_none() {
            return 0;
        }
        self.settle(now);
        if !self.leading() {
            return 0;
        }
        // A namespace whose set was not read this pass waits for the next one (nothing of it is sent or deleted).
        let mut tombs: Vec<Tombstone> = self.with(|i| {
            i.tombs
                .values()
                .filter(|t| scope_has(&i.restored, &t.namespace))
                .cloned()
                .collect()
        });
        tombs.sort_by(|a, b| a.uid.cmp(&b.uid));
        let bound = i64::try_from(metering::BACKFILL_MINUTES * 60).unwrap_or(i64::MAX);
        let mut sent = 0usize;
        let mut remaining = tombs.len();
        for t in tombs {
            if self.must_stop() {
                break;
            }
            if now.saturating_sub(t.written_at) > bound {
                // Every minute of it is past the backfill bound by now, and so is every acknowledgement.
                self.metrics.inc(name::METER_TOMBSTONES_EXPIRED, &[]);
                self.release(&t);
                continue;
            }
            let share = (self.budget() / remaining.max(1)).max(1);
            remaining = remaining.saturating_sub(1);
            match self.replay_one(&t, now, share, &mut sent).await {
                Progress::SinkDown => break,
                Progress::Held => {}
                Progress::Done => {
                    if self.unacknowledged(&t.uid) == 0 {
                        self.metrics.inc(name::METER_TOMBSTONES_RELEASED, &[]);
                        self.release(&t);
                    }
                }
            }
        }
        // The deletions of this pass, batched: one write per namespace that changed.
        let dirty: Vec<String> = self.with(|i| {
            i.tomb_dirty
                .iter()
                .filter(|ns| scope_has(&i.restored, ns))
                .cloned()
                .collect()
        });
        for namespace in dirty {
            let _ = self.save_namespace(&namespace).await;
        }
        sent
    }

    fn release(&self, t: &Tombstone) {
        self.with(|i| {
            i.tombs.remove(&t.uid);
            i.persisted.remove(&t.uid);
            i.state.remove(&t.uid);
            i.tomb_dirty.insert(t.namespace.clone());
        });
    }

    /// One tombstone: the minutes it still owes, from its own facts.
    async fn replay_one(
        &self,
        t: &Tombstone,
        now: i64,
        share: usize,
        sent: &mut usize,
    ) -> Progress {
        let closed = PodFacts {
            uid: t.uid.clone(),
            phase: "Succeeded".to_owned(),
            started_at: Some(t.started_at),
            finished_at: Some(t.ended_at),
            has_container_status: true,
            ..PodFacts::default()
        };
        let ran = match metering::minutes(&closed, now, self.config.skew_secs) {
            Ok(Some(ran)) => ran,
            // A tombstone is only written for a pod that ran and ended: nothing more to wait for.
            _ => return Progress::Done,
        };
        let after = if ran.complete {
            Progress::Done
        } else {
            Progress::Held
        };
        let cursor = self.state(&t.uid).cursor.or(Some(t.first_minute));
        let due = metering::due(&ran.range, cursor, now);
        self.update(&t.uid, |s| s.cursor = Some(due.send.start));
        if due.skipped > 0 {
            self.metrics
                .add(name::METER_BACKFILL_SKIPPED, &[], due.skipped);
        }
        if due.send.is_empty() {
            return after;
        }
        let a = Attributed {
            pod_kind: t.pod_kind,
            service: t.service.clone(),
            run: t.run.clone(),
            weight_quarters: t.weight_quarters,
            node_type: t.node_type.clone(),
        };
        let asked = match &t.subject {
            Some(subject) => Ok(Some(subject.clone())),
            None => self.account_for(&a).await,
        };
        let subject = match asked {
            Ok(Some(subject)) => subject,
            Ok(None) => {
                let fresh = due
                    .send
                    .end
                    .saturating_sub(due.send.start.max(self.state(&t.uid).unknown_counted));
                self.unattributed(Unattributed::UnknownAccount, fresh);
                self.update(&t.uid, |s| s.unknown_counted = due.send.end);
                return Progress::Held;
            }
            Err(AccountUnavailable) => {
                self.metrics.inc(name::METER_ACCOUNT_FAILURES, &[]);
                return Progress::Held;
            }
        };
        match self
            .emit(&t.uid, &a, &subject, due.send.clone(), share, now, sent)
            .await
        {
            Emit::SinkDown => return Progress::SinkDown,
            Emit::BudgetOut => return Progress::Held,
            Emit::Finished => {}
        }
        if self
            .state(&t.uid)
            .cursor
            .is_some_and(|c| c >= ran.range.end)
        {
            after
        } else {
            Progress::Held
        }
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
        // Configuration, not a passing condition: the minutes are given up, and counted.
        let attributed = match self.attribute(pod, &kind) {
            Ok(a) => a,
            Err(reason) => {
                self.unattributed(reason, count);
                self.update(&pod.uid, |s| s.cursor = Some(due.send.end));
                return after;
            }
        };
        let subject = match self.account_for(&attributed).await {
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
        match self
            .emit(
                &pod.uid,
                &attributed,
                &subject,
                due.send.clone(),
                share,
                now,
                sent,
            )
            .await
        {
            Emit::SinkDown => return Progress::SinkDown,
            Emit::BudgetOut => return Progress::Held,
            Emit::Finished => {}
        }
        let reached = self
            .state(&pod.uid)
            .cursor
            .is_some_and(|c| c >= ran.range.end);
        if reached { after } else { Progress::Held }
    }

    /// What a pod is for a line: its kind, service, run, weight and node type, or why it has none.
    fn attribute(&self, pod: &PodFacts, kind: &Kind) -> Result<Attributed, Unattributed> {
        let (pod_kind, service, run) = match kind {
            Kind::Run { service, run } => (PodKind::Run, service.clone(), run.clone()),
            Kind::Agent { service } => (PodKind::Agent, service.clone(), None),
            Kind::Skip => return Err(Unattributed::NoIdentity),
        };
        let Some(service) = service else {
            return Err(Unattributed::NoIdentity);
        };
        if pod_kind == PodKind::Run && run.is_none() {
            return Err(Unattributed::NoIdentity);
        }
        let Some(weight_quarters) = metering::weight_quarters(&pod.annotations) else {
            return Err(Unattributed::NoWeight);
        };
        let Some(node_type) = self
            .config
            .node_type_label_key
            .as_deref()
            .and_then(|key| metering::node_type(pod.node_affinity.as_ref(), key))
        else {
            return Err(Unattributed::UnknownNodeType);
        };
        Ok(Attributed {
            pod_kind,
            service,
            run,
            weight_quarters,
            node_type,
        })
    }

    /// The account of an attributed pod: the cache of bound runs, or the source, cut at the pass deadline.
    async fn account_for(&self, a: &Attributed) -> Result<Option<String>, AccountUnavailable> {
        let pod_ref = match &a.run {
            Some(run) => PodRef::Run {
                service: a.service.clone(),
                run: run.clone(),
            },
            None => PodRef::Agent {
                service: a.service.clone(),
            },
        };
        let cache_key = a.run.as_ref().map(|run| (a.service.clone(), run.clone()));
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
        if let (Ok(Some(account)), Some(k)) = (&asked, cache_key) {
            // A bound run's account never changes: remembered for the life of the process.
            self.with(|i| i.accounts.insert(k, account.clone()));
        }
        asked
    }

    /// Hand the sink the events of `minutes` (at most `share` of them), moving the pod's cursor and recording each
    /// as pending when the sink reports acknowledgements.
    #[allow(clippy::too_many_arguments)]
    async fn emit(
        &self,
        uid: &str,
        a: &Attributed,
        subject: &str,
        minutes: std::ops::Range<u64>,
        share: usize,
        now: i64,
        sent: &mut usize,
    ) -> Emit {
        for minute in minutes.take(share) {
            if self.budget() == 0 {
                return Emit::BudgetOut;
            }
            let event = PodMinuteEvent {
                source: self.config.source.clone(),
                event_id: event_id(uid, minute),
                minute,
                subject: subject.to_owned(),
                weight_quarters: a.weight_quarters,
                node_type: a.node_type.clone(),
                pod_kind: a.pod_kind,
                agent_service: a.service.clone(),
                run: a.run.clone(),
            };
            match (self.sink)(event).await {
                Ok(()) => {
                    self.metrics.inc(name::METER_EVENTS, &[]);
                    self.update(uid, |s| s.cursor = Some(minute + 1));
                    if self.outcomes.is_some() && !self.with(|i| i.untracked) {
                        let id = event_id(uid, minute);
                        self.with(|i| {
                            i.pending.insert(
                                id,
                                Pending {
                                    uid: uid.to_owned(),
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
                    return Emit::SinkDown;
                }
                Err(UsageError::Invalid { field }) => {
                    // A bug, not a transient: never retried for ever. The field is named, not its value.
                    self.metrics.inc(name::METER_INVALID, &[]);
                    tracing::warn!(field, "the meter made an invalid event and dropped it");
                    self.update(uid, |s| s.cursor = Some(minute + 1));
                }
            }
        }
        Emit::Finished
    }
}

/// Whether a tombstone read from the store (or about to be written) is one the meter can send a line from.
fn tombstone_is_valid(t: &Tombstone) -> bool {
    !t.namespace.is_empty()
        && !t.uid.is_empty()
        && t.uid.len() <= 128
        && is_line_text(&t.service)
        && is_line_text(&t.node_type)
        && t.run.as_deref().is_none_or(is_line_text)
        && t.subject.as_deref().is_none_or(is_line_text)
        && aap_usage::WEIGHT_GRID.contains(&t.weight_quarters)
        && (t.pod_kind == PodKind::Run) == t.run.is_some()
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
        namespace: pod.metadata.namespace.clone().unwrap_or_default(),
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
        // Only a complete listing says a pod is gone. It also replays the tombstones.
        let listed: HashSet<String> = all.iter().map(|p| p.uid.clone()).collect();
        meter.close_vanished(&listed, now).await;
    } else {
        // What the tombstones owe does not depend on the listing.
        meter.replay(now).await;
    }
}

/// Look at the pods every period while this replica leads, until `stop` is true. A follower lists nothing and
/// sends nothing.
pub(crate) async fn run(l: MeterLoop, period: Duration, mut stop: watch::Receiver<bool>) {
    loop {
        // Acknowledgements keep arriving whoever leads: a follower drains them (nothing is listed or sent).
        l.meter.settle(unix_now());
        if !l.leader.is_leader() {
            // The next term reads the tombstones again before it writes any.
            l.meter.lose_leadership();
        } else {
            // Read the tombstones a crash or a failover left. Until they are read nothing is written or replayed,
            // and a pod that vanishes waits in memory; the live pods are metered all the same.
            l.meter.restore(&l.scopes).await;
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
