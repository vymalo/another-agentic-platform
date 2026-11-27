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

    /// Whether the loop has proposed to put this agent to sleep and the reconcile has not yet finished with it.
    pub fn is_marked(&self, id: &RuntimeId) -> bool {
        self.with(|i| i.marks.contains_key(&key(id)))
    }

    /// The loop's proposal for this agent.
    pub(crate) fn mark(&self, id: &RuntimeId) -> Option<Mark> {
        self.with(|i| i.marks.get(&key(id)).map(|(m, _)| m.clone()))
    }

    /// Say the proposal was written to the status: true the first time only (so the metric counts one sleep).
    pub(crate) fn note_recorded(&self, id: &RuntimeId) -> bool {
        self.with(|i| {
            i.marks
                .get_mut(&key(id))
                .is_some_and(|(_, recorded)| !std::mem::replace(recorded, true))
        })
    }

    /// Forget everything about an agent: it was deleted, suspended by a person, or no longer asks to sleep.
    pub fn forget(&self, id: &RuntimeId) {
        self.with(|i| {
            i.idle.remove(&key(id));
            i.marks.remove(&key(id));
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

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
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
    if sleepers.is_marked(&id) || service.status.as_ref().is_some_and(|s| s.sleep.is_some()) {
        return Outcome::Stayed;
    }
    let polled = activity(id.clone()).await;
    if let Err(e) = &polled {
        tracing::debug!(service = %id.name(), "activity poll failed, counted as busy: {e}");
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
        .cloned();
    sleepers.put_to_sleep(
        &id,
        Mark {
            next_due_at,
            wake_at_seen,
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
    pub leader: Arc<dyn LeaderElection>,
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
        if l.leader.is_leader() {
            for scope in &l.scopes {
                let api: Api<AgentService> = if scope.is_empty() {
                    Api::all(l.client.clone())
                } else {
                    Api::namespaced(l.client.clone(), scope)
                };
                match api.list(&Default::default()).await {
                    Ok(list) => {
                        for service in &list.items {
                            step(&l.activity, &l.sleepers, service, unix_now()).await;
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
            step(&self.activity, &self.sleepers, svc, now).await
        }
    }

    fn sleepy() -> serde_json::Value {
        json!({ "idleTimeout": "10m", "wakeAhead": "2m" })
    }

    #[tokio::test]
    async fn idle_for_the_timeout_proposes_a_sleep_once_and_touches_nothing_else() {
        let r = rig();
        let mut nudges = r.sleepers.nudges();
        let svc = service(sleepy(), false);
        assert_eq!(r.at(&svc, 0).await, Outcome::Stayed, "the clock starts");
        assert_eq!(r.at(&svc, 599).await, Outcome::Stayed);
        assert!(!r.sleepers.is_marked(&r.id));
        assert_eq!(r.at(&svc, 600).await, Outcome::Marked);
        assert!(r.sleepers.is_marked(&r.id));
        assert_eq!(
            nudges.try_recv().unwrap(),
            r.id,
            "the reconcile is asked for"
        );
        // Marked: not asked again, not proposed again.
        let asked = r.source.asked().len();
        assert_eq!(r.at(&svc, 9_999).await, Outcome::Stayed);
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
            Some(Mark {
                next_due_at: Some(50_000),
                wake_at_seen: Some("2026-01-01T00:00:00Z".to_owned()),
            })
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
        assert!(!r.sleepers.is_marked(&r.id));
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
        let out = step(&flip, &r.sleepers, &svc, 600).await;
        assert_eq!(out, Outcome::Stayed);
        assert!(!r.sleepers.is_marked(&r.id));
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
        assert!(!r.sleepers.is_marked(&r.id));
    }

    #[tokio::test]
    async fn removing_the_policy_forgets_the_proposal() {
        let r = rig();
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        r.at(&svc, 600).await;
        assert!(r.sleepers.is_marked(&r.id));
        r.at(&service(json!({}), false), 601).await;
        assert!(!r.sleepers.is_marked(&r.id));
    }
}
