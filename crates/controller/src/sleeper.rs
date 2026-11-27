//! The idle loop (AD-058, P2b): poll each agent that may sleep, and scale the one that has been idle long enough
//! to zero.
//!
//! [`sleep`](crate::sleep) decides; this module asks, remembers and acts. [`Sleepers`] is what the loop and the
//! service reconciler share: the idle clock of each agent and the proposals to sleep.
//!
//! Sleeping is **stored** (P3a-1): the loop only proposes (a [`Mark`]); the reconcile writes `status.sleep`, then
//! holds the workload at zero, and reads the record back after a restart. Waking is [`wake`](crate::wake).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aap_api::AgentService;
use aap_ports::{
    Activity, ActivityError, ActivitySource, LeaderElection, RuntimeId, RuntimeProvider,
};
use kube::{Api, Client, ResourceExt};
use tokio::sync::{mpsc, watch};

use crate::sleep::{Decision, IdleState, SleepPolicy, decide};
use crate::wake::Mark;

/// How often the loop polls when [`Options::sleep_poll`](crate::Options) is left alone.
pub const DEFAULT_POLL: Duration = Duration::from_secs(30);

/// An [`ActivitySource`] behind a pointer, so the operator is not generic over it.
pub type ActivityFn = Arc<
    dyn Fn(RuntimeId) -> Pin<Box<dyn Future<Output = Result<Activity, ActivityError>> + Send>>
        + Send
        + Sync,
>;

/// Erase the type of `source`.
pub fn erase<A: ActivitySource + 'static>(source: A) -> ActivityFn {
    let source = Arc::new(source);
    Arc::new(move |id| {
        let source = source.clone();
        Box::pin(async move { source.poll(&id).await })
    })
}

/// The closed set of kinds a failed poll is told apart by in the log: the agent refused the question (a token that does not
/// match: 401 or 403), it answered what the operator cannot read, or it could not be reached (timeout, refused connection, a
/// NetworkPolicy that drops the packets, a 5xx).
fn failure_kind(e: &ActivityError) -> &'static str {
    match e {
        ActivityError::Invalid(m) if m.contains("refused the question") => "refused",
        ActivityError::Invalid(_) => "invalid",
        ActivityError::Unavailable(_) => "unreachable",
    }
}

/// What the loop and the reconciler share about sleep. Cheap to share in an `Arc`.
///
/// It holds the idle clock of each agent and the loop's **proposals** (a [`Mark`]). It does not hold who is asleep:
/// that is `status.sleep` of the object, written by the reconcile and read back after a restart (P3a-1).
#[derive(Debug, Default)]
pub struct Sleepers {
    inner: Mutex<Inner>,
    /// Where the loop says "reconcile this one now" (set by the operator when it runs).
    nudge: Mutex<Option<mpsc::UnboundedSender<RuntimeId>>>,
}

#[derive(Debug, Default)]
struct Inner {
    idle: HashMap<String, IdleState>,
    marks: HashMap<String, (Mark, bool)>,
    /// Agents that ask to sleep and have no activity listener (the a2a harness): the loop never polls them.
    listenerless: std::collections::HashSet<String>,
    /// The kind of the last failed poll of each agent, while it keeps failing the same way (to warn once per kind).
    poll_failures: HashMap<String, &'static str>,
}

fn key(id: &RuntimeId) -> String {
    format!("{}/{}", id.scope(), id.name())
}

impl Sleepers {
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The channel on which the loop asks for a pass of the reconciler. One reader: the operator's.
    pub(crate) fn nudges(&self) -> mpsc::UnboundedReceiver<RuntimeId> {
        let (tx, rx) = mpsc::unbounded_channel();
        *self.nudge.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx);
        rx
    }

    /// Whether the loop has proposed to put this agent to sleep and the reconcile has not yet finished with it. A mark
    /// older than `max_age` at `now` is **removed** and does not count: the agent is polled again.
    pub fn is_marked(&self, id: &RuntimeId, now: i64, max_age: Duration) -> bool {
        self.with(|i| {
            let k = key(id);
            let stale = i.marks.get(&k).is_some_and(|(m, recorded)| {
                !recorded
                    && now.saturating_sub(m.made_at)
                        > i64::try_from(max_age.as_secs()).unwrap_or(i64::MAX)
            });
            if stale {
                i.marks.remove(&k);
                tracing::debug!(service = %id.name(), reason = "stale", "a mark was dropped: the agent is polled again");
            }
            i.marks.contains_key(&k)
        })
    }

    /// Drop every proposal: this replica is not the leader (any longer), and what it proposed while it was is no
    /// statement about an agent it no longer watches over.
    pub fn clear_marks(&self) {
        self.with(|i| i.marks.clear());
    }

    /// The loop's proposal for this agent, and whether the reconcile has claimed it.
    pub(crate) fn mark(&self, id: &RuntimeId) -> Option<(Mark, bool)> {
        self.with(|i| {
            i.marks
                .get(&key(id))
                .map(|(m, claimed)| (m.clone(), *claimed))
        })
    }

    /// Claim the proposal **before** its record is written, so that aging cannot drop it while the write is in flight
    /// (a loop lap in that window would otherwise poll an agent whose record is not visible yet, and the sleep would
    /// go uncounted). True the first time only: that pass counts the sleep once its write succeeded.
    pub(crate) fn claim(&self, id: &RuntimeId) -> bool {
        self.with(|i| {
            i.marks
                .get_mut(&key(id))
                .is_some_and(|(_, claimed)| !std::mem::replace(claimed, true))
        })
    }

    /// Give a claim back because the write failed: the mark ages again, so a retry a long time later is no longer
    /// acted on (the safe choice: a failing write must not keep an old poll alive for ever).
    pub(crate) fn unclaim(&self, id: &RuntimeId) {
        self.with(|i| {
            if let Some((_, claimed)) = i.marks.get_mut(&key(id)) {
                *claimed = false;
            }
        });
    }

    /// Remove the mark only if it is the one a pass read (`made_at`): the loop may have made a newer one since.
    pub(crate) fn drop_mark(&self, id: &RuntimeId, made_at: i64) {
        self.with(|i| {
            if i.marks
                .get(&key(id))
                .is_some_and(|(m, _)| m.made_at == made_at)
            {
                i.marks.remove(&key(id));
            }
        });
    }

    /// Record whether an agent asks to sleep (`scaling.idleTimeout`) but has **no activity listener**: an A2A image says nothing
    /// of how busy it is, so it never sleeps and the loop does not ask it. True when this call **changed** the answer to
    /// "yes", which is when the reconcile says so, once, rather than on every pass.
    pub(crate) fn set_listenerless(&self, id: &RuntimeId, listenerless: bool) -> bool {
        self.with(|i| {
            if listenerless {
                i.listenerless.insert(key(id))
            } else {
                i.listenerless.remove(&key(id));
                false
            }
        })
    }

    /// Record the outcome of a poll: `Some(kind)` for a failure, `None` for an answer. True when the failure is **news**: a
    /// kind this agent was not already failing with. A success clears it, so the next failure is news again.
    pub(crate) fn note_poll(&self, id: &RuntimeId, failure: Option<&'static str>) -> bool {
        self.with(|i| match failure {
            Some(kind) => i.poll_failures.insert(key(id), kind) != Some(kind),
            None => {
                i.poll_failures.remove(&key(id));
                false
            }
        })
    }

    fn is_listenerless(&self, id: &RuntimeId) -> bool {
        self.with(|i| i.listenerless.contains(&key(id)))
    }

    /// Forget everything about an agent: it was deleted, suspended by a person, or no longer asks to sleep.
    pub fn forget(&self, id: &RuntimeId) {
        self.with(|i| {
            i.idle.remove(&key(id));
            i.marks.remove(&key(id));
            i.poll_failures.remove(&key(id));
        });
    }

    /// Propose that an agent sleep, and ask the reconciler for a pass. The reconcile writes `status.sleep` first and
    /// scales second, so a crash in between leaves the agent asleep on the record, never awake by accident.
    pub fn put_to_sleep(&self, id: &RuntimeId, mark: Mark) {
        self.with(|i| {
            i.idle.remove(&key(id));
            i.marks.insert(key(id), (mark, false));
        });
        if let Some(tx) = &*self.nudge.lock().unwrap_or_else(PoisonError::into_inner) {
            // A reader that has stopped is the operator stopping.
            let _ = tx.send(id.clone());
        }
    }

    /// The agent is awake again (or never slept): drop the proposal, so the next pass does not put it back.
    pub fn wake(&self, id: &RuntimeId) {
        self.with(|i| {
            i.marks.remove(&key(id));
        });
    }

    fn idle_state(&self, id: &RuntimeId) -> IdleState {
        self.with(|i| i.idle.get(&key(id)).copied().unwrap_or_default())
    }

    fn set_idle(&self, id: &RuntimeId, state: IdleState) {
        self.with(|i| {
            i.idle.insert(key(id), state);
        });
    }
}

/// What one step did to one agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing changed: no policy, a person's suspend, busy, not idle long enough, or already asleep or waking.
    Stayed,
    /// The agent was proposed for sleep: the reconcile writes the record and scales it to zero.
    Marked,
}

/// One pass of the idle loop over one service at `now` (seconds since the Unix epoch).
///
/// The agent is asked twice before it is proposed for sleep: once to run the idle clock, and again right before (the
/// drain: a run claimed in between keeps the agent up, and so does an error). Whatever `ActivitySource` says,
/// `spec.suspend` and a missing policy win. **An agent whose `status.sleep` is present is never asked**, asleep or
/// waking: a poll goes through the activator, and would wake it (P3a-1). The loop does not scale and does not write
/// the status: the reconcile does both, from the mark (the status has one writer, because it applies the whole).
pub async fn step(
    activity: &ActivityFn,
    sleepers: &Sleepers,
    service: &AgentService,
    now: i64,
    mark_max_age: Duration,
) -> Outcome {
    let Some(ns) = service.namespace() else {
        return Outcome::Stayed;
    };
    let id = RuntimeId::new(&ns, service.name_any());
    let policy = SleepPolicy::from_scaling(&service.spec.scaling);
    let Some(policy) =
        policy.filter(|_| !service.spec.suspend && service.metadata.deletion_timestamp.is_none())
    else {
        sleepers.forget(&id);
        return Outcome::Stayed;
    };
    if sleepers.is_marked(&id, now, mark_max_age)
        || service.status.as_ref().is_some_and(|s| s.sleep.is_some())
    {
        return Outcome::Stayed;
    }
    if sleepers.is_listenerless(&id) {
        // Nothing to ask: the agent's pod serves no listener, and a refused connection would count as busy for ever.
        return Outcome::Stayed;
    }
    let polled = activity(id.clone()).await;
    // A failed poll counts as busy, so a wrong token, a missing Secret or a `activity.to` that misses the agents' namespace
    // would keep every agent awake in silence: the first failure of each kind says so, once, at `warn`.
    let failure = polled.as_ref().err().map(failure_kind);
    if let (Err(e), Some(kind)) = (&polled, failure) {
        if sleepers.note_poll(&id, Some(kind)) {
            tracing::warn!(
                service = %id.name(), namespace = %id.scope(), kind,
                "activity poll failed, counted as busy: {e} (the agent stays awake until polls succeed; logged once per kind)"
            );
        } else {
            tracing::debug!(service = %id.name(), kind, "activity poll failed again, counted as busy: {e}");
        }
    } else {
        sleepers.note_poll(&id, None);
    }
    let step = decide(&policy, sleepers.idle_state(&id), false, &polled, now);
    sleepers.set_idle(&id, step.state);
    if step.decision != Decision::Sleep {
        return Outcome::Stayed;
    }
    // Drain: ask again, and let the same rules say whether it is still idle.
    let again = activity(id.clone()).await;
    let confirmed = decide(&policy, step.state, false, &again, now);
    sleepers.set_idle(&id, confirmed.state);
    if confirmed.decision != Decision::Sleep {
        return Outcome::Stayed;
    }
    // The schedule the confirmed poll named is what a scheduled wake is later due by.
    let next_due_at = again.as_ref().ok().and_then(|a| a.next_due_at);
    let wake_at_seen = service
        .annotations()
        .get(aap_api::WAKE_AT_ANNOTATION)
        .map(|v| crate::wake::seen_of(v));
    sleepers.put_to_sleep(
        &id,
        Mark {
            next_due_at,
            wake_at_seen,
            made_at: now,
        },
    );
    tracing::info!(service = %id.name(), "idle for {:?}, proposed for sleep", policy.idle_timeout);
    Outcome::Marked
}

/// What the idle loop is made of.
pub(crate) struct IdleLoop {
    pub client: Client,
    pub activity: ActivityFn,
    pub sleepers: Arc<Sleepers>,
    pub scopes: Vec<String>,
    pub period: Duration,
    /// How old a mark may be and still count ([`crate::wake::mark_max_age`]).
    pub mark_max_age: Duration,
    /// The clock the reconcile uses, so a mark's age means the same in both.
    pub clock: crate::Clock,
    pub leader: Arc<dyn LeaderElection>,
}

/// What the loop does with its marks on each lap: a replica that is not the leader holds none. Done on every lap
/// that is not led (so it is the false edge and every lap after it), before anything is polled.
pub(crate) fn on_leadership(sleepers: &Sleepers, is_leader: bool) {
    if !is_leader {
        sleepers.clear_marks();
    }
}

/// Poll every service of `scopes` every `period` while this replica leads, until `stop` is true.
pub(crate) async fn run<R>(l: IdleLoop, runtime: R, mut stop: watch::Receiver<bool>)
where
    R: RuntimeProvider,
{
    if !runtime.capabilities().suspend {
        tracing::warn!("the runtime provider cannot scale to zero: no agent will sleep");
        return;
    }
    loop {
        let led = l.leader.is_leader();
        on_leadership(&l.sleepers, led);
        if led {
            for scope in &l.scopes {
                let api: Api<AgentService> = if scope.is_empty() {
                    Api::all(l.client.clone())
                } else {
                    Api::namespaced(l.client.clone(), scope)
                };
                match api.list(&Default::default()).await {
                    Ok(list) => {
                        for service in &list.items {
                            step(
                                &l.activity,
                                &l.sleepers,
                                service,
                                (l.clock)().0.as_second(),
                                l.mark_max_age,
                            )
                            .await;
                        }
                    }
                    Err(e) => tracing::warn!("idle loop could not list services of {scope:?}: {e}"),
                }
            }
        }
        tokio::select! {
            () = tokio::time::sleep(l.period) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use aap_ports::memory::MemoryActivity;
    use serde_json::json;

    const NS: &str = "team";
    const MAX_AGE: Duration = Duration::from_secs(90);

    fn service(scaling: serde_json::Value, suspend: bool) -> AgentService {
        serde_json::from_value(json!({
            "apiVersion": "agents.vymalo.com/v1alpha1",
            "kind": "AgentService",
            "metadata": { "name": "a", "namespace": NS },
            "spec": {
                "configRef": { "name": "a" },
                "suspend": suspend,
                "scaling": scaling,
                "store": { "postgres": { "secretRef": { "name": "s", "key": "uri" } } },
            }
        }))
        .unwrap()
    }

    struct Rig {
        source: MemoryActivity,
        activity: ActivityFn,
        sleepers: Sleepers,
        id: RuntimeId,
    }

    fn rig() -> Rig {
        let source = MemoryActivity::new();
        Rig {
            activity: erase(source.clone()),
            source,
            sleepers: Sleepers::default(),
            id: RuntimeId::new(NS, "a"),
        }
    }

    impl Rig {
        async fn at(&self, svc: &AgentService, now: i64) -> Outcome {
            step(&self.activity, &self.sleepers, svc, now, MAX_AGE).await
        }
    }

    fn sleepy() -> serde_json::Value {
        json!({ "idleTimeout": "10m", "wakeAhead": "2m" })
    }

    /// R1 (ANR-1030b): a failing poll is news once per kind, not on every lap, and an answer ends the run of failures.
    #[tokio::test]
    async fn a_failed_poll_is_news_once_per_kind_until_an_answer() {
        let r = rig();
        assert!(r.sleepers.note_poll(&r.id, Some("refused")), "first");
        assert!(!r.sleepers.note_poll(&r.id, Some("refused")), "repeat");
        assert!(
            r.sleepers.note_poll(&r.id, Some("unreachable")),
            "kind changed"
        );
        assert!(!r.sleepers.note_poll(&r.id, Some("unreachable")));
        assert!(!r.sleepers.note_poll(&r.id, None), "an answer is no news");
        assert!(
            r.sleepers.note_poll(&r.id, Some("unreachable")),
            "after an answer it is news again"
        );

        // The kinds come from the error, and `step` records them (a repeat of the recorded kind is no news).
        assert_eq!(
            failure_kind(&ActivityError::Invalid(
                "the agent refused the question (401 Unauthorized)".into()
            )),
            "refused"
        );
        assert_eq!(
            failure_kind(&ActivityError::Invalid("the answer is not JSON".into())),
            "invalid"
        );
        assert_eq!(
            failure_kind(&ActivityError::Unavailable("x".into())),
            "unreachable"
        );
        let refusing: ActivityFn = Arc::new(|_| {
            Box::pin(async {
                Err(ActivityError::Invalid(
                    "the agent refused the question (401 Unauthorized)".into(),
                ))
            })
        });
        let sleepers = Sleepers::default();
        let svc = service(sleepy(), false);
        step(&refusing, &sleepers, &svc, 0, MAX_AGE).await;
        assert!(
            !sleepers.note_poll(&r.id, Some("refused")),
            "step recorded the first failure"
        );
        assert!(sleepers.note_poll(&r.id, Some("invalid")));
    }

    #[tokio::test]
    async fn idle_for_the_timeout_proposes_a_sleep_once_and_touches_nothing_else() {
        let r = rig();
        let mut nudges = r.sleepers.nudges();
        let svc = service(sleepy(), false);
        assert_eq!(r.at(&svc, 0).await, Outcome::Stayed, "the clock starts");
        assert_eq!(r.at(&svc, 599).await, Outcome::Stayed);
        assert!(!r.sleepers.is_marked(&r.id, 0, MAX_AGE));
        assert_eq!(r.at(&svc, 600).await, Outcome::Marked);
        assert!(r.sleepers.is_marked(&r.id, 0, MAX_AGE));
        assert_eq!(
            nudges.try_recv().unwrap(),
            r.id,
            "the reconcile is asked for"
        );
        // Marked: not asked again, not proposed again.
        let asked = r.source.asked().len();
        assert_eq!(r.at(&svc, 650).await, Outcome::Stayed);
        assert_eq!(r.source.asked().len(), asked);
    }

    #[tokio::test]
    async fn the_confirmed_polls_schedule_and_the_annotation_are_in_the_mark() {
        let r = rig();
        let mut svc = service(sleepy(), false);
        svc.metadata.annotations = Some(
            [(
                aap_api::WAKE_AT_ANNOTATION.to_owned(),
                "2026-01-01T00:00:00Z".to_owned(),
            )]
            .into(),
        );
        r.source.set(
            &r.id,
            Activity {
                busy: 0,
                next_due_at: Some(50_000),
            },
        );
        r.at(&svc, 0).await;
        r.at(&svc, 600).await;
        assert_eq!(
            r.sleepers.mark(&r.id),
            Some((
                Mark {
                    next_due_at: Some(50_000),
                    wake_at_seen: Some("2026-01-01T00:00:00Z".to_owned()),
                    made_at: 600,
                },
                false
            ))
        );
    }

    #[tokio::test]
    async fn an_agent_whose_status_holds_a_sleep_record_is_never_asked() {
        let r = rig();
        let mut svc = service(sleepy(), false);
        svc.status = Some(
            serde_json::from_value(json!({
                "sleep": { "since": "2026-01-01T00:00:00Z" },
                "conditions": [],
            }))
            .unwrap(),
        );
        for now in [0, 600, 6_000] {
            assert_eq!(r.at(&svc, now).await, Outcome::Stayed);
        }
        assert!(r.source.asked().is_empty());
    }

    #[tokio::test]
    async fn a_busy_agent_restarts_the_clock() {
        let r = rig();
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        r.source.set(
            &r.id,
            Activity {
                busy: 1,
                next_due_at: None,
            },
        );
        assert_eq!(r.at(&svc, 500).await, Outcome::Stayed);
        r.source.set(&r.id, Activity::IDLE);
        assert_eq!(
            r.at(&svc, 700).await,
            Outcome::Stayed,
            "idle since 700, not 0"
        );
        assert_eq!(r.at(&svc, 1299).await, Outcome::Stayed);
        assert_eq!(r.at(&svc, 1300).await, Outcome::Marked);
    }

    #[tokio::test]
    async fn a_failed_poll_never_proposes_a_sleep() {
        let r = rig();
        let svc = service(sleepy(), false);
        r.source.break_source(&r.id);
        for now in [0, 600, 6_000, 600_000] {
            assert_eq!(r.at(&svc, now).await, Outcome::Stayed);
        }
        assert!(!r.sleepers.is_marked(&r.id, 0, MAX_AGE));
    }

    #[tokio::test]
    async fn a_schedule_inside_wake_ahead_never_proposes_a_sleep() {
        let r = rig();
        let svc = service(sleepy(), false);
        r.source.set(
            &r.id,
            Activity {
                busy: 0,
                next_due_at: Some(10_000 + 119),
            },
        );
        r.at(&svc, 9_000).await;
        assert_eq!(r.at(&svc, 10_000).await, Outcome::Stayed);
        // Due far away: idle counts again.
        r.source.set(
            &r.id,
            Activity {
                busy: 0,
                next_due_at: Some(50_000),
            },
        );
        r.at(&svc, 10_001).await;
        assert_eq!(r.at(&svc, 10_601).await, Outcome::Marked);
    }

    #[tokio::test]
    async fn a_run_claimed_between_the_two_asks_keeps_the_agent_up() {
        // The first answer is idle, the second (the drain check) is busy: the loop asks twice before it proposes.
        let r = rig();
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let flip = {
            let calls = calls.clone();
            let source = r.source.clone();
            let id = r.id.clone();
            let inner = erase(source.clone());
            Arc::new(move |i: RuntimeId| {
                if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1 {
                    source.set(
                        &id,
                        Activity {
                            busy: 1,
                            next_due_at: None,
                        },
                    );
                }
                inner(i)
            }) as ActivityFn
        };
        let out = step(&flip, &r.sleepers, &svc, 600, MAX_AGE).await;
        assert_eq!(out, Outcome::Stayed);
        assert!(!r.sleepers.is_marked(&r.id, 0, MAX_AGE));
    }

    #[tokio::test]
    async fn suspend_and_a_missing_policy_win() {
        let r = rig();
        let suspended = service(sleepy(), true);
        let never = service(json!({}), false);
        for now in [0, 600, 6_000] {
            assert_eq!(r.at(&suspended, now).await, Outcome::Stayed);
            assert_eq!(r.at(&never, now).await, Outcome::Stayed);
        }
        assert!(r.source.asked().is_empty(), "no policy, nothing asked");
        assert!(!r.sleepers.is_marked(&r.id, 0, MAX_AGE));
    }

    #[tokio::test]
    async fn removing_the_policy_forgets_the_proposal() {
        let r = rig();
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        r.at(&svc, 600).await;
        assert!(r.sleepers.is_marked(&r.id, 0, MAX_AGE));
        r.at(&service(json!({}), false), 601).await;
        assert!(!r.sleepers.is_marked(&r.id, 0, MAX_AGE));
    }

    #[tokio::test]
    async fn a_mark_older_than_the_bound_is_dropped_and_the_agent_is_polled_again() {
        let r = rig();
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        r.at(&svc, 600).await;
        assert!(r.sleepers.is_marked(&r.id, 600, MAX_AGE));
        assert!(
            r.sleepers.is_marked(&r.id, 690, MAX_AGE),
            "at the bound it stands"
        );
        let asked = r.source.asked().len();
        assert!(!r.sleepers.is_marked(&r.id, 691, MAX_AGE), "past it, gone");
        // The loop polls it again, with a clock that starts afresh.
        r.at(&svc, 700).await;
        assert!(r.source.asked().len() > asked);
    }

    #[tokio::test]
    async fn losing_leadership_drops_the_marks() {
        let r = rig();
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        r.at(&svc, 600).await;
        assert!(r.sleepers.is_marked(&r.id, 600, MAX_AGE));
        on_leadership(&r.sleepers, true);
        assert!(
            r.sleepers.is_marked(&r.id, 600, MAX_AGE),
            "the leader keeps its marks"
        );
        on_leadership(&r.sleepers, false);
        assert!(!r.sleepers.is_marked(&r.id, 600, MAX_AGE));
    }

    #[tokio::test]
    async fn a_claimed_mark_older_than_the_bound_is_not_dropped_by_aging() {
        let r = rig();
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        r.at(&svc, 600).await;
        assert!(r.sleepers.claim(&r.id), "the first claim is new");
        assert!(!r.sleepers.claim(&r.id), "the second is not");
        assert!(
            r.sleepers.is_marked(&r.id, 600 + 10_000, MAX_AGE),
            "a record is being written for it"
        );
        // A failed write gives the claim back: it ages again.
        r.sleepers.unclaim(&r.id);
        assert!(!r.sleepers.is_marked(&r.id, 600 + 10_000, MAX_AGE));
    }

    #[tokio::test]
    async fn a_newer_mark_is_not_removed_by_a_pass_that_read_an_older_one() {
        let r = rig();
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        r.at(&svc, 600).await;
        r.sleepers.drop_mark(&r.id, 599);
        assert!(r.sleepers.is_marked(&r.id, 600, MAX_AGE));
        r.sleepers.drop_mark(&r.id, 600);
        assert!(!r.sleepers.is_marked(&r.id, 600, MAX_AGE));
    }
}
