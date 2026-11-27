//! The activator's three suites fail the fakes they are meant to fail. Every case of every suite has at least one
//! deliberately broken implementation here, and `#[should_panic]` shows the suite sees it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use aap_ports::testkit::{self, CardsUnderTest, UpstreamUnderTest, WakerUnderTest};
use aap_ports::{
    Card, CardError, CardSource, RefusedReason, Route, RuntimeId, Upstream, UpstreamError,
    UpstreamRequest, UpstreamResponse, WakeError, WakeOutcome, Waker,
};

// ---------------------------------------------------------------------------------------------------------- Waker

#[derive(Clone, Copy, PartialEq)]
enum WakeFault {
    Sound,
    ReadyWhenTimedOut,
    ErrOnGiveUp,
    ReadyWhenUnreachable,
    OneAnswerForEveryAgent,
    FailsTheSecondWake,
    RefusalLosesItsReason,
    RefusalIsRetryable,
}

#[derive(Clone, Copy)]
enum Script {
    Ready,
    TimedOut,
    Fail,
    Refuse(RefusedReason),
}

#[derive(Clone)]
struct FaultyWaker {
    fault: WakeFault,
    scripts: Arc<Mutex<BTreeMap<RuntimeId, Script>>>,
    last: Arc<Mutex<Option<Script>>>,
    calls: Arc<Mutex<BTreeMap<RuntimeId, u32>>>,
}

fn waker(fault: WakeFault) -> FaultyWaker {
    FaultyWaker {
        fault,
        scripts: Arc::default(),
        last: Arc::default(),
        calls: Arc::default(),
    }
}

impl FaultyWaker {
    fn set(&self, agent: &RuntimeId, s: Script) {
        self.scripts.lock().unwrap().insert(agent.clone(), s);
        *self.last.lock().unwrap() = Some(s);
    }
}

impl Waker for FaultyWaker {
    async fn wake(&self, agent: &RuntimeId) -> Result<WakeOutcome, WakeError> {
        let n = {
            let mut c = self.calls.lock().unwrap();
            let n = c.entry(agent.clone()).or_default();
            *n += 1;
            *n
        };
        if self.fault == WakeFault::FailsTheSecondWake && n >= 2 {
            return Err(WakeError::Unavailable("second".into()));
        }
        let script = if self.fault == WakeFault::OneAnswerForEveryAgent {
            *self.last.lock().unwrap()
        } else {
            self.scripts.lock().unwrap().get(agent).copied()
        }
        .unwrap_or(Script::Ready);
        match script {
            Script::Ready => Ok(WakeOutcome::Ready),
            Script::TimedOut if self.fault == WakeFault::ReadyWhenTimedOut => {
                Ok(WakeOutcome::Ready)
            }
            Script::TimedOut if self.fault == WakeFault::ErrOnGiveUp => {
                Err(WakeError::Unavailable("gave up".into()))
            }
            Script::TimedOut => Ok(WakeOutcome::TimedOut),
            Script::Fail if self.fault == WakeFault::ReadyWhenUnreachable => Ok(WakeOutcome::Ready),
            Script::Fail => Err(WakeError::Unavailable("down".into())),
            Script::Refuse(_) if self.fault == WakeFault::RefusalLosesItsReason => {
                Err(WakeError::Refused(RefusedReason::Unknown))
            }
            Script::Refuse(_) if self.fault == WakeFault::RefusalIsRetryable => {
                Err(WakeError::Unavailable("refused".into()))
            }
            Script::Refuse(why) => Err(WakeError::Refused(why)),
        }
    }
}

impl WakerUnderTest for FaultyWaker {
    async fn script_ready(&self, agent: &RuntimeId) {
        self.set(agent, Script::Ready);
    }
    async fn script_timed_out(&self, agent: &RuntimeId) {
        self.set(agent, Script::TimedOut);
    }
    async fn script_failure(&self, agent: &RuntimeId) {
        self.set(agent, Script::Fail);
    }
    async fn script_refuse(&self, agent: &RuntimeId, why: RefusedReason) {
        self.set(agent, Script::Refuse(why));
    }
}

#[tokio::test]
#[should_panic(expected = "left: Ready")]
async fn a_waker_that_reports_ready_for_a_give_up_fails_the_suite() {
    testkit::activator::a_wake_that_gives_up_is_timed_out_not_an_error(waker(
        WakeFault::ReadyWhenTimedOut,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "gave up")]
async fn a_waker_that_errs_on_a_give_up_fails_the_suite() {
    testkit::activator::a_wake_that_gives_up_is_timed_out_not_an_error(waker(
        WakeFault::ErrOnGiveUp,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "on an `Ok` value: Ready")]
async fn a_waker_that_is_ready_when_the_operator_is_unreachable_fails_the_suite() {
    testkit::activator::an_unreachable_operator_is_a_transient_error(waker(
        WakeFault::ReadyWhenUnreachable,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "w.wake(&one).await.is_err()")]
async fn a_waker_with_one_answer_for_every_agent_fails_the_suite() {
    testkit::activator::agents_do_not_mix(waker(WakeFault::OneAnswerForEveryAgent)).await;
}

#[tokio::test]
#[should_panic(expected = "Unavailable(\"second\")")]
async fn a_waker_that_fails_the_second_wake_fails_the_suite() {
    testkit::activator::waking_an_agent_that_is_awake_is_ready_and_repeatable(waker(
        WakeFault::FailsTheSecondWake,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "Refused(Unknown)")]
async fn a_waker_that_loses_the_refusal_reason_fails_the_suite() {
    testkit::activator::a_refusal_carries_its_reason_and_is_not_retryable(waker(
        WakeFault::RefusalLosesItsReason,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "Unavailable(\"refused\")")]
async fn a_waker_that_makes_a_refusal_retryable_fails_the_suite() {
    testkit::activator::a_refusal_carries_its_reason_and_is_not_retryable(waker(
        WakeFault::RefusalIsRetryable,
    ))
    .await;
}

// -------------------------------------------------------------------------------------------------------- Upstream

#[derive(Clone, Copy, PartialEq)]
enum UpFault {
    Sound,
    DropsAHeader,
    DropsTheBody,
    ErrsOnA4xx,
    DuplicatesTheForward,
    OkWhenUnreachable,
    AltersTheAnswer,
}

#[derive(Clone)]
struct FaultyUpstream {
    fault: UpFault,
    answer: Arc<Mutex<Option<UpstreamResponse>>>,
    broken: Arc<Mutex<bool>>,
    seen: Arc<Mutex<Vec<(RuntimeId, UpstreamRequest)>>>,
}

fn upstream(fault: UpFault) -> FaultyUpstream {
    FaultyUpstream {
        fault,
        answer: Arc::default(),
        broken: Arc::default(),
        seen: Arc::default(),
    }
}

impl Upstream for FaultyUpstream {
    async fn forward(
        &self,
        agent: &RuntimeId,
        _route: &Route,
        mut request: UpstreamRequest,
    ) -> Result<UpstreamResponse, UpstreamError> {
        match self.fault {
            UpFault::DropsAHeader => {
                request.headers.pop();
            }
            UpFault::DropsTheBody => request.body.clear(),
            _ => {}
        }
        {
            let mut seen = self.seen.lock().unwrap();
            seen.push((agent.clone(), request.clone()));
            if self.fault == UpFault::DuplicatesTheForward {
                seen.push((agent.clone(), request));
            }
        }
        if *self.broken.lock().unwrap() {
            return if self.fault == UpFault::OkWhenUnreachable {
                Ok(ok())
            } else {
                Err(UpstreamError::Unavailable("down".into()))
            };
        }
        let mut answer = self.answer.lock().unwrap().clone().unwrap_or_else(ok);
        if self.fault == UpFault::ErrsOnA4xx && answer.status >= 400 {
            return Err(UpstreamError::Unavailable("4xx".into()));
        }
        if self.fault == UpFault::AltersTheAnswer {
            answer.body.push(b'!');
        }
        Ok(answer)
    }
}

fn ok() -> UpstreamResponse {
    UpstreamResponse {
        status: 200,
        headers: vec![],
        body: b"ok".to_vec(),
    }
}

impl UpstreamUnderTest for FaultyUpstream {
    async fn answer(&self, response: UpstreamResponse) {
        *self.answer.lock().unwrap() = Some(response);
    }
    async fn break_upstream(&self) {
        *self.broken.lock().unwrap() = true;
    }
    async fn received(&self) -> Vec<(RuntimeId, UpstreamRequest)> {
        self.seen.lock().unwrap().clone()
    }
}

#[tokio::test]
#[should_panic(expected = "headers: [(\"content-type\", \"application/json\")], body: [123")]
async fn an_upstream_that_drops_a_header_fails_the_suite() {
    testkit::activator::the_request_reaches_the_agent_unchanged(upstream(UpFault::DropsAHeader))
        .await;
}

#[tokio::test]
#[should_panic(expected = "body: [] })]")]
async fn an_upstream_that_drops_the_body_fails_the_suite() {
    testkit::activator::the_request_reaches_the_agent_unchanged(upstream(UpFault::DropsTheBody))
        .await;
}

#[tokio::test]
#[should_panic(expected = "Unavailable(\"4xx\")")]
async fn an_upstream_that_turns_a_4xx_into_an_error_fails_the_suite() {
    testkit::activator::the_answer_comes_back_unchanged_whatever_its_status(upstream(
        UpFault::ErrsOnA4xx,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "255, 33")]
async fn an_upstream_that_alters_the_answer_fails_the_suite() {
    testkit::activator::the_answer_comes_back_unchanged_whatever_its_status(upstream(
        UpFault::AltersTheAnswer,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "left: 4")]
async fn an_upstream_that_duplicates_a_forward_fails_the_suite() {
    testkit::activator::each_call_is_one_forward(upstream(UpFault::DuplicatesTheForward)).await;
}

#[tokio::test]
#[should_panic(expected = "on an `Ok` value: UpstreamResponse")]
async fn an_upstream_that_is_ok_when_unreachable_fails_the_suite() {
    testkit::activator::an_unreachable_agent_is_a_transient_error(upstream(
        UpFault::OkWhenUnreachable,
    ))
    .await;
}

// ---------------------------------------------------------------------------------------------------------- Cards

#[derive(Clone, Copy, PartialEq)]
enum CardFault {
    Sound,
    MutatesTheBytes,
    MutatesTheContentType,
    ErrsForAMissingCard,
    OneCardForEveryAgent,
    NeverErrors,
    NeverRecovers,
}

#[derive(Clone)]
struct FaultyCards {
    fault: CardFault,
    cards: Arc<Mutex<BTreeMap<RuntimeId, Card>>>,
    last: Arc<Mutex<Option<Card>>>,
    broken: Arc<Mutex<bool>>,
    ever_broken: Arc<Mutex<bool>>,
}

fn cards(fault: CardFault) -> FaultyCards {
    FaultyCards {
        fault,
        cards: Arc::default(),
        last: Arc::default(),
        broken: Arc::default(),
        ever_broken: Arc::default(),
    }
}

impl CardSource for FaultyCards {
    async fn card(&self, agent: &RuntimeId) -> Result<Option<Card>, CardError> {
        if *self.broken.lock().unwrap() && self.fault != CardFault::NeverErrors {
            return Err(CardError::Unavailable("down".into()));
        }
        if *self.ever_broken.lock().unwrap() && self.fault == CardFault::NeverRecovers {
            return Err(CardError::Unavailable("still down".into()));
        }
        let found = if self.fault == CardFault::OneCardForEveryAgent {
            self.last.lock().unwrap().clone()
        } else {
            self.cards.lock().unwrap().get(agent).cloned()
        };
        match found {
            None if self.fault == CardFault::ErrsForAMissingCard => {
                Err(CardError::Unavailable("missing".into()))
            }
            None => Ok(None),
            Some(mut c) => {
                match self.fault {
                    CardFault::MutatesTheBytes => c.bytes.reverse(),
                    CardFault::MutatesTheContentType => c.content_type.push_str("; charset=x"),
                    _ => {}
                }
                Ok(Some(c))
            }
        }
    }
}

impl CardsUnderTest for FaultyCards {
    async fn put(&self, agent: &RuntimeId, card: Card) {
        *self.last.lock().unwrap() = Some(card.clone());
        self.cards.lock().unwrap().insert(agent.clone(), card);
    }
    async fn break_source(&self) {
        *self.broken.lock().unwrap() = true;
        *self.ever_broken.lock().unwrap() = true;
    }
    async fn mend(&self) {
        *self.broken.lock().unwrap() = false;
    }
}

#[tokio::test]
#[should_panic(expected = "bytes: [125, 34, 120")]
async fn a_card_source_that_mutates_the_bytes_fails_the_suite() {
    testkit::activator::a_card_is_served_as_published(cards(CardFault::MutatesTheBytes)).await;
}

#[tokio::test]
#[should_panic(expected = "charset=x")]
async fn a_card_source_that_mutates_the_content_type_fails_the_suite() {
    testkit::activator::a_card_is_served_as_published(cards(CardFault::MutatesTheContentType))
        .await;
}

#[tokio::test]
#[should_panic(expected = "Unavailable(\"missing\")")]
async fn a_card_source_that_errs_for_a_missing_card_fails_the_suite() {
    testkit::activator::an_agent_with_no_card_is_none_not_an_error(cards(
        CardFault::ErrsForAMissingCard,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "right: None")]
async fn a_card_source_with_one_card_for_every_agent_fails_the_suite() {
    testkit::activator::cards_do_not_mix(cards(CardFault::OneCardForEveryAgent)).await;
}

#[tokio::test]
#[should_panic(expected = "on an `Ok` value: Some(Card")]
async fn a_card_source_that_never_errors_fails_the_suite() {
    testkit::activator::an_unreadable_source_is_an_error_then_recovers(cards(
        CardFault::NeverErrors,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "still down")]
async fn a_card_source_that_never_recovers_fails_the_suite() {
    testkit::activator::an_unreadable_source_is_an_error_then_recovers(cards(
        CardFault::NeverRecovers,
    ))
    .await;
}

// The controls: the same fakes with no fault pass every case, so each `should_panic` above is the fault's doing.

#[tokio::test]
async fn the_sound_waker_passes_every_case() {
    use testkit::activator as t;
    let w = || waker(WakeFault::Sound);
    t::a_scripted_wake_ends_ready(w()).await;
    t::waking_an_agent_that_is_awake_is_ready_and_repeatable(w()).await;
    t::a_wake_that_gives_up_is_timed_out_not_an_error(w()).await;
    t::an_unreachable_operator_is_a_transient_error(w()).await;
    t::a_refusal_carries_its_reason_and_is_not_retryable(w()).await;
    t::agents_do_not_mix(w()).await;
}

#[tokio::test]
async fn the_sound_upstream_passes_every_case() {
    use testkit::activator as t;
    let u = || upstream(UpFault::Sound);
    t::the_request_reaches_the_agent_unchanged(u()).await;
    t::the_answer_comes_back_unchanged_whatever_its_status(u()).await;
    t::each_call_is_one_forward(u()).await;
    t::an_unreachable_agent_is_a_transient_error(u()).await;
}

#[tokio::test]
async fn the_sound_card_source_passes_every_case() {
    use testkit::activator as t;
    let c = || cards(CardFault::Sound);
    t::a_card_is_served_as_published(c()).await;
    t::an_agent_with_no_card_is_none_not_an_error(c()).await;
    t::cards_do_not_mix(c()).await;
    t::an_unreadable_source_is_an_error_then_recovers(c()).await;
}
