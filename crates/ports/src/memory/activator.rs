//! Scriptable fakes of the activator's seams: [`MemoryWaker`], [`MemoryUpstream`] and [`MemoryCards`].

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::lock;
use crate::testkit::{CardsUnderTest, WakerUnderTest};
use crate::{
    Card, CardError, CardSource, RefusedReason, Route, RuntimeId, Upstream, UpstreamError,
    UpstreamRequest, UpstreamResponse, WakeError, WakeOutcome, Waker,
};

/// A counter shared by fakes, so a test can say which of two events came first without a clock.
#[derive(Clone, Debug, Default)]
pub struct Sequence(Arc<AtomicU64>);

impl Sequence {
    /// A counter at zero.
    pub fn new() -> Self {
        Self::default()
    }

    /// The next number: 1, 2, 3, ...
    pub fn next(&self) -> u64 {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
}

/// What a [`MemoryWaker`] does for one agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WakeScript {
    /// Complete as `Ready` after this long (zero is at once).
    ReadyAfter(Duration),
    /// Answer `TimedOut` at once, as an adapter does at the agent's `wakeTimeout`.
    TimedOut,
    /// Never complete.
    Never,
    /// Fail with `Unavailable`.
    Fail,
    /// Fail with `Refused`, for this reason.
    Refuse(RefusedReason),
}

#[derive(Default)]
struct WakerState {
    scripts: BTreeMap<RuntimeId, WakeScript>,
    calls: Vec<RuntimeId>,
    completed: Vec<u64>,
}

/// A scripted [`Waker`]. An agent with no script is ready at once. Every call is counted, and each completion
/// takes a number from the [`Sequence`], so a test can show a forward came after it.
#[derive(Clone, Default)]
pub struct MemoryWaker {
    state: Arc<Mutex<WakerState>>,
    sequence: Sequence,
}

impl MemoryWaker {
    /// Every agent ready at once, with a sequence of its own.
    pub fn new() -> Self {
        Self::default()
    }

    /// A waker whose completions are numbered by `sequence`.
    pub fn with_sequence(sequence: Sequence) -> Self {
        Self {
            state: Arc::default(),
            sequence,
        }
    }

    /// What `agent` does from now on.
    pub fn script(&self, agent: &RuntimeId, script: WakeScript) {
        lock(&self.state).scripts.insert(agent.clone(), script);
    }

    /// How many times `wake` was called, for any agent.
    pub fn calls(&self) -> usize {
        lock(&self.state).calls.len()
    }

    /// How many times `wake` was called for `agent`.
    pub fn calls_for(&self, agent: &RuntimeId) -> usize {
        lock(&self.state)
            .calls
            .iter()
            .filter(|a| *a == agent)
            .count()
    }

    /// The sequence number of each completed `Ready` or `TimedOut` answer, in order.
    pub fn completed(&self) -> Vec<u64> {
        lock(&self.state).completed.clone()
    }
}

impl Waker for MemoryWaker {
    async fn wake(&self, agent: &RuntimeId) -> Result<WakeOutcome, WakeError> {
        let script = {
            let mut s = lock(&self.state);
            s.calls.push(agent.clone());
            s.scripts
                .get(agent)
                .copied()
                .unwrap_or(WakeScript::ReadyAfter(Duration::ZERO))
        };
        let outcome = match script {
            WakeScript::ReadyAfter(d) => {
                if !d.is_zero() {
                    tokio::time::sleep(d).await;
                }
                WakeOutcome::Ready
            }
            WakeScript::TimedOut => WakeOutcome::TimedOut,
            WakeScript::Never => std::future::pending().await,
            WakeScript::Refuse(why) => return Err(WakeError::Refused(why)),
            WakeScript::Fail => {
                return Err(WakeError::Unavailable(
                    "scripted: operator unreachable".into(),
                ));
            }
        };
        let n = self.sequence.next();
        lock(&self.state).completed.push(n);
        Ok(outcome)
    }
}

impl WakerUnderTest for MemoryWaker {
    async fn script_ready(&self, agent: &RuntimeId) {
        self.script(agent, WakeScript::ReadyAfter(Duration::from_millis(5)));
    }

    async fn script_timed_out(&self, agent: &RuntimeId) {
        self.script(agent, WakeScript::TimedOut);
    }

    async fn script_failure(&self, agent: &RuntimeId) {
        self.script(agent, WakeScript::Fail);
    }

    async fn script_refuse(&self, agent: &RuntimeId, why: RefusedReason) {
        self.script(agent, WakeScript::Refuse(why));
    }
}

/// One request a [`MemoryUpstream`] saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Forwarded {
    /// The agent it was for.
    pub agent: RuntimeId,
    /// The route.
    pub route: Route,
    /// The request, as received.
    pub request: UpstreamRequest,
    /// The number the [`Sequence`] gave the call.
    pub seq: u64,
}

#[derive(Default)]
struct UpstreamState {
    seen: Vec<Forwarded>,
    answer: Option<UpstreamResponse>,
    broken: bool,
    hung: bool,
}

/// A scripted [`Upstream`]: it records each request and answers `200` with the body `ok` unless told otherwise.
#[derive(Clone, Default)]
pub struct MemoryUpstream {
    state: Arc<Mutex<UpstreamState>>,
    sequence: Sequence,
}

impl MemoryUpstream {
    /// Answers `200 ok`, with a sequence of its own.
    pub fn new() -> Self {
        Self::default()
    }

    /// An upstream whose calls are numbered by `sequence`.
    pub fn with_sequence(sequence: Sequence) -> Self {
        Self {
            state: Arc::default(),
            sequence,
        }
    }

    /// Answer `response` from now on.
    pub fn answer(&self, response: UpstreamResponse) {
        lock(&self.state).answer = Some(response);
    }

    /// Fail every call with `Unavailable`.
    pub fn break_upstream(&self) {
        lock(&self.state).broken = true;
    }

    /// Record every call, then never answer it.
    pub fn hang(&self) {
        lock(&self.state).hung = true;
    }

    /// The requests seen, in order.
    pub fn seen(&self) -> Vec<Forwarded> {
        lock(&self.state).seen.clone()
    }

    /// How many requests were forwarded.
    pub fn calls(&self) -> usize {
        lock(&self.state).seen.len()
    }
}

impl Upstream for MemoryUpstream {
    async fn forward(
        &self,
        agent: &RuntimeId,
        route: &Route,
        request: UpstreamRequest,
    ) -> Result<UpstreamResponse, UpstreamError> {
        let seq = self.sequence.next();
        let (hung, answer) = {
            let mut s = lock(&self.state);
            s.seen.push(Forwarded {
                agent: agent.clone(),
                route: route.clone(),
                request,
                seq,
            });
            let answer = if s.broken {
                Err(UpstreamError::Unavailable(
                    "scripted: agent unreachable".into(),
                ))
            } else {
                Ok(s.answer.clone().unwrap_or_else(|| UpstreamResponse {
                    status: 200,
                    headers: vec![("content-type".into(), "text/plain".into())],
                    body: b"ok".to_vec(),
                }))
            };
            (s.hung, answer)
        };
        if hung {
            return std::future::pending().await;
        }
        answer
    }
}

#[derive(Default)]
struct CardsState {
    cards: BTreeMap<RuntimeId, Card>,
    asked: usize,
    broken: bool,
}

/// A scripted [`CardSource`]: an agent with no card is `None`.
#[derive(Clone, Default)]
pub struct MemoryCards {
    state: Arc<Mutex<CardsState>>,
}

impl MemoryCards {
    /// No agent has a card.
    pub fn new() -> Self {
        Self::default()
    }

    /// `agent` serves `card` from now on.
    pub fn put(&self, agent: &RuntimeId, card: Card) {
        lock(&self.state).cards.insert(agent.clone(), card);
    }

    /// Fail every call with `Unavailable` until [`mend`](Self::mend).
    pub fn break_source(&self) {
        lock(&self.state).broken = true;
    }

    /// Undo [`break_source`](Self::break_source).
    pub fn mend(&self) {
        lock(&self.state).broken = false;
    }

    /// How many times a card was asked for.
    pub fn asked(&self) -> usize {
        lock(&self.state).asked
    }
}

impl CardSource for MemoryCards {
    async fn card(&self, agent: &RuntimeId) -> Result<Option<Card>, CardError> {
        let mut s = lock(&self.state);
        s.asked += 1;
        if s.broken {
            return Err(CardError::Unavailable("scripted: unreadable".into()));
        }
        Ok(s.cards.get(agent).cloned())
    }
}

impl CardsUnderTest for MemoryCards {
    async fn put(&self, agent: &RuntimeId, card: Card) {
        Self::put(self, agent, card);
    }

    async fn break_source(&self) {
        Self::break_source(self);
    }

    async fn mend(&self) {
        Self::mend(self);
    }
}

impl crate::testkit::UpstreamUnderTest for MemoryUpstream {
    async fn answer(&self, response: UpstreamResponse) {
        Self::answer(self, response);
    }

    async fn break_upstream(&self) {
        Self::break_upstream(self);
    }

    async fn received(&self) -> Vec<(RuntimeId, UpstreamRequest)> {
        self.seen()
            .into_iter()
            .map(|f| (f.agent, f.request))
            .collect()
    }
}
