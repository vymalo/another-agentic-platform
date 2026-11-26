//! The idle loop (AD-058, P2b): poll each agent that may sleep, and scale the one that has been idle long enough
//! to zero.
//!
//! [`sleep`](crate::sleep) decides; this module asks, remembers and acts. [`Sleepers`] is what the loop and the
//! service reconciler share: the idle clock of each agent and the set of agents put to sleep. The reconciler reads
//! the set, so that its next `ensure` keeps a sleeping agent at zero (an `ensure` of the plain spec would wake it
//! at once) and the status says `Sleeping`, not `Suspended`.
//!
//! Sleeping is **not stored**: a restart of the operator forgets the set, the agent is woken by the next pass and
//! sleeps again after another `idleTimeout`. Waking on a request, a due schedule or a push is P3.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use aap_api::AgentService;
use aap_ports::{
    Activity, ActivityError, ActivitySource, LeaderElection, RuntimeError, RuntimeId,
    RuntimeProvider,
};
use kube::{Api, Client, ResourceExt};
use tokio::sync::watch;

use crate::metrics::{Metrics, name};
use crate::sleep::{Decision, IdleState, SleepPolicy, decide};

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
#[derive(Debug, Default)]
pub struct Sleepers {
    inner: Mutex<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    idle: HashMap<String, IdleState>,
    sleeping: HashSet<String>,
}

fn key(id: &RuntimeId) -> String {
    format!("{}/{}", id.scope(), id.name())
}

impl Sleepers {
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Whether the loop has put this agent to sleep.
    pub fn is_sleeping(&self, id: &RuntimeId) -> bool {
        self.with(|i| i.sleeping.contains(&key(id)))
    }

    /// Forget everything about an agent: it was deleted, suspended by a person, or no longer asks to sleep.
    pub fn forget(&self, id: &RuntimeId) {
        self.with(|i| {
            i.idle.remove(&key(id));
            i.sleeping.remove(&key(id));
        });
    }

    /// Mark an agent as asleep (for the reconciler to keep it at zero).
    pub fn put_to_sleep(&self, id: &RuntimeId) {
        self.with(|i| {
            i.idle.remove(&key(id));
            i.sleeping.insert(key(id));
        });
    }

    /// Wake an agent: the next pass ensures it running. (P3 calls this on a request; P2b on a failed scale-down.)
    pub fn wake(&self, id: &RuntimeId) {
        self.with(|i| {
            i.sleeping.remove(&key(id));
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
    /// Nothing changed: no policy, a person's suspend, busy, not idle long enough, or already asleep.
    Stayed,
    /// The agent was scaled to zero.
    Slept,
}

/// One pass of the idle loop over one service at `now` (seconds since the Unix epoch).
///
/// The agent is asked twice before it is scaled down: once to run the idle clock, and again right before the
/// scale-down (the drain: a run claimed in between keeps the agent up, and so does an error). Whatever
/// `ActivitySource` says, `spec.suspend` and a missing policy win.
pub async fn step<R: RuntimeProvider>(
    runtime: &R,
    activity: &ActivityFn,
    sleepers: &Sleepers,
    service: &AgentService,
    now: i64,
    metrics: &Metrics,
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
    if sleepers.is_sleeping(&id) {
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
    // Mark first: a pass that runs from here on ensures the agent at zero, never running again.
    sleepers.put_to_sleep(&id);
    match runtime.suspend(&id).await {
        Ok(_) => {
            metrics.inc(name::SLEEPS, &[]);
            tracing::info!(service = %id.name(), "idle for {:?}, scaled to zero", policy.idle_timeout);
            Outcome::Slept
        }
        Err(e) => {
            sleepers.wake(&id);
            if !matches!(e, RuntimeError::NotFound(_)) {
                tracing::warn!(service = %id.name(), "could not scale to zero: {e}");
            }
            Outcome::Stayed
        }
    }
}

/// What the idle loop is made of.
pub(crate) struct IdleLoop {
    pub client: Client,
    pub activity: ActivityFn,
    pub sleepers: Arc<Sleepers>,
    pub scopes: Vec<String>,
    pub period: Duration,
    pub leader: Arc<dyn LeaderElection>,
    pub metrics: Arc<Metrics>,
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
                            step(
                                &runtime,
                                &l.activity,
                                &l.sleepers,
                                service,
                                unix_now(),
                                &l.metrics,
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
    use aap_ports::memory::{MemoryActivity, MemoryRuntime};
    use aap_ports::testkit::sample_spec;
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
        runtime: MemoryRuntime,
        source: MemoryActivity,
        activity: ActivityFn,
        sleepers: Sleepers,
        metrics: Metrics,
        id: RuntimeId,
    }

    async fn rig() -> Rig {
        let runtime = MemoryRuntime::new();
        let id = RuntimeId::new(NS, "a");
        runtime.ensure(&id, &sample_spec(&id)).await.unwrap();
        let source = MemoryActivity::new();
        Rig {
            activity: erase(source.clone()),
            runtime,
            source,
            sleepers: Sleepers::default(),
            metrics: Metrics::new(),
            id,
        }
    }

    impl Rig {
        async fn at(&self, svc: &AgentService, now: i64) -> Outcome {
            step(
                &self.runtime,
                &self.activity,
                &self.sleepers,
                svc,
                now,
                &self.metrics,
            )
            .await
        }
    }

    fn sleepy() -> serde_json::Value {
        json!({ "idleTimeout": "10m", "wakeAhead": "2m" })
    }

    #[tokio::test]
    async fn idle_for_the_timeout_scales_to_zero_once() {
        let r = rig().await;
        let svc = service(sleepy(), false);
        assert_eq!(r.at(&svc, 0).await, Outcome::Stayed, "the clock starts");
        assert_eq!(r.at(&svc, 599).await, Outcome::Stayed);
        assert!(!r.sleepers.is_sleeping(&r.id));
        assert_eq!(r.at(&svc, 600).await, Outcome::Slept);
        assert!(r.sleepers.is_sleeping(&r.id));
        assert_eq!(
            r.runtime.status(&r.id).await.unwrap().phase,
            aap_ports::Phase::Suspended
        );
        // Asleep: not asked again, not slept again.
        let asked = r.source.asked().len();
        assert_eq!(r.at(&svc, 9_999).await, Outcome::Stayed);
        assert_eq!(r.source.asked().len(), asked);
        assert_eq!(r.metrics.get(name::SLEEPS, &[]), 1);
    }

    #[tokio::test]
    async fn a_busy_agent_restarts_the_clock() {
        let r = rig().await;
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
        assert_eq!(r.at(&svc, 1300).await, Outcome::Slept);
    }

    #[tokio::test]
    async fn a_failed_poll_never_scales_down() {
        let r = rig().await;
        let svc = service(sleepy(), false);
        r.source.break_source(&r.id);
        for now in [0, 600, 6_000, 600_000] {
            assert_eq!(r.at(&svc, now).await, Outcome::Stayed);
        }
        assert!(!r.sleepers.is_sleeping(&r.id));
        assert_eq!(
            r.runtime.status(&r.id).await.unwrap().phase,
            aap_ports::Phase::Ready
        );
    }

    #[tokio::test]
    async fn a_schedule_inside_wake_ahead_never_scales_down() {
        let r = rig().await;
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
        assert_eq!(r.at(&svc, 10_601).await, Outcome::Slept);
    }

    #[tokio::test]
    async fn a_run_claimed_between_the_two_asks_keeps_the_agent_up() {
        // The first answer is idle, the second (the drain check) is busy: the loop asks twice before it scales.
        let r = rig().await;
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
        let out = step(&r.runtime, &flip, &r.sleepers, &svc, 600, &r.metrics).await;
        assert_eq!(out, Outcome::Stayed);
        assert!(!r.sleepers.is_sleeping(&r.id));
        assert_eq!(
            r.runtime.status(&r.id).await.unwrap().phase,
            aap_ports::Phase::Ready
        );
    }

    #[tokio::test]
    async fn suspend_and_a_missing_policy_win() {
        let r = rig().await;
        let suspended = service(sleepy(), true);
        let never = service(json!({}), false);
        for now in [0, 600, 6_000] {
            assert_eq!(r.at(&suspended, now).await, Outcome::Stayed);
            assert_eq!(r.at(&never, now).await, Outcome::Stayed);
        }
        assert!(r.source.asked().is_empty(), "no policy, nothing asked");
        assert!(!r.sleepers.is_sleeping(&r.id));
    }

    #[tokio::test]
    async fn removing_the_policy_wakes_a_sleeping_agent() {
        let r = rig().await;
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        r.at(&svc, 600).await;
        assert!(r.sleepers.is_sleeping(&r.id));
        r.at(&service(json!({}), false), 601).await;
        assert!(!r.sleepers.is_sleeping(&r.id));
    }

    #[tokio::test]
    async fn a_provider_that_fails_to_suspend_leaves_the_agent_running() {
        let r = rig().await;
        r.runtime.set_unavailable(true);
        let svc = service(sleepy(), false);
        r.at(&svc, 0).await;
        assert_eq!(r.at(&svc, 600).await, Outcome::Stayed);
        assert!(!r.sleepers.is_sleeping(&r.id), "the mark is undone");
    }
}
