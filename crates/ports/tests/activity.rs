//! The activity suite fails the sources it is meant to fail: one that calls an unreachable agent idle, one that
//! forgets the schedule, one that answers for the wrong service.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use aap_ports::testkit::{self, ActivityUnderTest};
use aap_ports::{Activity, ActivityError, ActivitySource, RuntimeId, UnavailableCause};

#[derive(Clone, Copy, PartialEq)]
enum Fault {
    UnreachableIsIdle,
    ForgetsTheSchedule,
    OneAnswerForEveryone,
}

#[derive(Clone)]
struct Faulty {
    fault: Fault,
    last: Arc<Mutex<Option<Activity>>>,
    broken: Arc<Mutex<Vec<RuntimeId>>>,
    answers: Arc<Mutex<Vec<(RuntimeId, Activity)>>>,
}

fn faulty(fault: Fault) -> Faulty {
    Faulty {
        fault,
        last: Arc::default(),
        broken: Arc::default(),
        answers: Arc::default(),
    }
}

impl ActivitySource for Faulty {
    async fn poll(&self, id: &RuntimeId) -> Result<Activity, ActivityError> {
        if self.broken.lock().unwrap().contains(id) {
            return if self.fault == Fault::UnreachableIsIdle {
                Ok(Activity::IDLE)
            } else {
                Err(ActivityError::Unavailable {
                    cause: UnavailableCause::Connect,
                    source: "down".into(),
                })
            };
        }
        if self.fault == Fault::OneAnswerForEveryone {
            return Ok((*self.last.lock().unwrap()).unwrap_or(Activity::IDLE));
        }
        let mut got = self
            .answers
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(i, _)| i == id)
            .map_or(Activity::IDLE, |(_, a)| *a);
        if self.fault == Fault::ForgetsTheSchedule {
            got.next_due_at = None;
        }
        Ok(got)
    }
}

impl ActivityUnderTest for Faulty {
    async fn set(&self, id: &RuntimeId, activity: Activity) {
        self.broken.lock().unwrap().retain(|i| i != id);
        *self.last.lock().unwrap() = Some(activity);
        self.answers.lock().unwrap().push((id.clone(), activity));
    }

    async fn break_source(&self, id: &RuntimeId) {
        self.broken.lock().unwrap().push(id.clone());
    }
}

#[tokio::test]
#[should_panic]
async fn a_source_that_calls_an_unreachable_agent_idle_fails_the_suite() {
    testkit::activity::an_unreachable_service_is_an_error_not_idleness(faulty(
        Fault::UnreachableIsIdle,
    ))
    .await;
}

#[tokio::test]
#[should_panic]
async fn a_source_that_forgets_the_schedule_fails_the_suite() {
    testkit::activity::the_next_schedule_is_reported(faulty(Fault::ForgetsTheSchedule)).await;
}

#[tokio::test]
#[should_panic]
async fn a_source_that_answers_the_same_for_every_service_fails_the_suite() {
    testkit::activity::services_do_not_mix(faulty(Fault::OneAnswerForEveryone)).await;
}
