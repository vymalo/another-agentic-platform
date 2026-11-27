//! The wake path (AD-058, P3a-1): what an agent that sleeps does next, decided from what the object says.
//!
//! Pure over a time the caller gives, like [`sleep`](crate::sleep). The idle loop *proposes* a sleep (a [`Mark`]);
//! the `AgentService` reconcile is the one writer of `status.sleep`, because it applies the whole status and a second
//! writer would be overwritten by it. So everything below is a function of the **object** (its status and its
//! annotation) and of the mark, and a restarted operator, whose marks are gone, reaches the same answers.
//!
//! ```mermaid
//! stateDiagram-v2
//!     [*] --> Running
//!     Running --> Sleeping: the idle loop's confirmed poll (a Mark), then the record is written and the workload held at zero
//!     Sleeping --> Waking: wake-at differs from wakeAtSeen, or now >= nextDueAt - wakeAhead
//!     Waking --> Running: the runtime is Ready (the record is removed)
//!     Sleeping --> Suspended: spec.suspend (the record is removed)
//!     Waking --> Suspended: spec.suspend (the record is removed)
//!     Sleeping --> Running: scaling.idleTimeout removed (the record is removed)
//!     Waking --> Waking: past wakeTimeout, still Waking (it says so, it never sleeps again for it)
//! ```
//!
//! **Why `differs`, not `>`** (ANR-Q134): the annotation is an RFC 3339 instant that nothing here parses, so a clock
//! that runs backwards, a value in another zone and a value that is not a date all wake the agent, and the decision
//! needs no time zone. A *removed* annotation is not a wake: nothing asked for it.

use std::time::Duration;

use aap_api::{SleepStatus, WakeCause, WakingStatus};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;

use crate::sleep::SleepPolicy;

/// What the idle loop decided about one agent, kept in memory until the reconcile has written the record.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Mark {
    /// The confirmed poll's `next_due_at` (seconds since the Unix epoch).
    pub next_due_at: Option<i64>,
    /// The `wake-at` annotation of the object the loop polled for, taken at the decision so a wake request that
    /// arrives between the decision and the write is not absorbed.
    pub wake_at_seen: Option<String>,
}

/// What [`plan`] reads.
#[derive(Debug)]
pub struct PlanInput<'a> {
    /// `status.sleep` of the object as the pass read it.
    pub record: Option<&'a SleepStatus>,
    /// The idle loop's proposal, if it made one.
    pub mark: Option<&'a Mark>,
    /// `None`: the agent has no `scaling.idleTimeout`, so it never sleeps.
    pub policy: Option<&'a SleepPolicy>,
    /// `spec.suspend`: a person's choice, which wins.
    pub suspend: bool,
    /// The value of the `agents.vymalo.com/wake-at` annotation.
    pub wake_at: Option<&'a str>,
    /// Seconds since the Unix epoch.
    pub now: i64,
}

/// The time of `seconds` since the Unix epoch, as the API writes it.
pub fn time_of(seconds: i64) -> Time {
    Time(
        k8s_openapi::jiff::Timestamp::from_second(seconds)
            .unwrap_or(k8s_openapi::jiff::Timestamp::UNIX_EPOCH),
    )
}

fn seconds_of(time: &Time) -> i64 {
    time.0.as_second()
}

fn secs(d: Duration) -> i64 {
    i64::try_from(d.as_secs()).unwrap_or(i64::MAX)
}

/// The record this pass holds the agent by, or `None` when the agent is awake. A record that exists is kept; one that
/// does not is made from the idle loop's mark; and a sleeping record whose agent is asked for, or due, gets `waking`.
pub fn plan(input: &PlanInput<'_>) -> Option<SleepStatus> {
    let policy = input.policy?;
    if input.suspend {
        return None;
    }
    let mut record = match (input.record, input.mark) {
        (Some(record), _) => record.clone(),
        (None, Some(mark)) => SleepStatus {
            since: time_of(input.now),
            next_due_at: mark.next_due_at.map(time_of),
            wake_at_seen: mark.wake_at_seen.clone(),
            waking: None,
        },
        (None, None) => return None,
    };
    if record.waking.is_none() {
        let asked = input
            .wake_at
            .is_some_and(|value| Some(value) != record.wake_at_seen.as_deref());
        let due = record.next_due_at.as_ref().is_some_and(|due| {
            input.now >= seconds_of(due).saturating_sub(secs(policy.wake_ahead))
        });
        let cause = if asked {
            Some(WakeCause::Request)
        } else if due {
            Some(WakeCause::Schedule)
        } else {
            None
        };
        if let Some(cause) = cause {
            record.waking = Some(WakingStatus {
                since: time_of(input.now),
                cause,
            });
        }
    }
    Some(record)
}

/// Whether a wake that began has outlasted `scaling.wakeTimeout` at `now`.
pub fn timed_out(record: &SleepStatus, policy: &SleepPolicy, now: i64) -> bool {
    record
        .waking
        .as_ref()
        .is_some_and(|w| now >= seconds_of(&w.since).saturating_add(secs(policy.wake_timeout)))
}

/// When the record next needs a look that no other event brings: the moment a scheduled wake is due, or the moment a
/// wake times out. At least a second, so a pass never spins.
pub fn recheck(record: &SleepStatus, policy: &SleepPolicy, now: i64) -> Option<Duration> {
    let at = match &record.waking {
        None => seconds_of(record.next_due_at.as_ref()?).saturating_sub(secs(policy.wake_ahead)),
        Some(w) => seconds_of(&w.since).saturating_add(secs(policy.wake_timeout)),
    };
    if at <= now && record.waking.is_some() {
        // Already late: said once, nothing more to wait for.
        return None;
    }
    Some(Duration::from_secs(
        u64::try_from(at.saturating_sub(now)).unwrap_or(0).max(1),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: SleepPolicy = SleepPolicy {
        idle_timeout: Duration::from_secs(600),
        wake_ahead: Duration::from_secs(120),
        wake_timeout: Duration::from_secs(60),
    };

    fn record(due: Option<i64>, seen: Option<&str>) -> SleepStatus {
        SleepStatus {
            since: time_of(1_000),
            next_due_at: due.map(time_of),
            wake_at_seen: seen.map(str::to_owned),
            waking: None,
        }
    }

    fn input<'a>(
        record: Option<&'a SleepStatus>,
        mark: Option<&'a Mark>,
        wake_at: Option<&'a str>,
        now: i64,
    ) -> PlanInput<'a> {
        PlanInput {
            record,
            mark,
            policy: Some(&POLICY),
            suspend: false,
            wake_at,
            now,
        }
    }

    #[test]
    fn a_mark_becomes_a_record_and_nothing_else_does() {
        let mark = Mark {
            next_due_at: Some(9_000),
            wake_at_seen: Some("a".into()),
        };
        let made = plan(&input(None, Some(&mark), Some("a"), 2_000)).unwrap();
        assert_eq!(
            made,
            SleepStatus {
                since: time_of(2_000),
                next_due_at: Some(time_of(9_000)),
                wake_at_seen: Some("a".into()),
                waking: None,
            }
        );
        assert_eq!(
            plan(&input(None, None, Some("b"), 2_000)),
            None,
            "awake ignores wake-at"
        );
    }

    #[test]
    fn only_a_value_that_differs_from_the_one_seen_asks_for_a_wake() {
        let r = record(None, Some("a"));
        assert!(
            plan(&input(Some(&r), None, Some("a"), 2_000))
                .unwrap()
                .waking
                .is_none()
        );
        assert!(
            plan(&input(Some(&r), None, None, 2_000))
                .unwrap()
                .waking
                .is_none(),
            "removed: no request"
        );
        let woken = plan(&input(Some(&r), None, Some("b"), 2_000)).unwrap();
        assert_eq!(woken.waking.unwrap().cause, WakeCause::Request);
        let r = record(None, None);
        assert!(
            plan(&input(Some(&r), None, Some("b"), 2_000))
                .unwrap()
                .waking
                .is_some(),
            "none then some"
        );
    }

    #[test]
    fn a_schedule_wakes_at_due_minus_wake_ahead_to_the_second() {
        let r = record(Some(10_000), None);
        assert!(
            plan(&input(Some(&r), None, None, 10_000 - 121))
                .unwrap()
                .waking
                .is_none()
        );
        let woken = plan(&input(Some(&r), None, None, 10_000 - 120)).unwrap();
        assert_eq!(woken.waking.unwrap().cause, WakeCause::Schedule);
    }

    #[test]
    fn a_wake_that_began_is_kept_and_never_restarted() {
        let mut r = record(Some(10_000), Some("a"));
        r.waking = Some(WakingStatus {
            since: time_of(1_500),
            cause: WakeCause::Request,
        });
        let kept = plan(&input(Some(&r), None, Some("c"), 20_000)).unwrap();
        assert_eq!(kept, r);
    }

    #[test]
    fn suspend_and_no_policy_end_the_sleep() {
        let r = record(None, None);
        let mut i = input(Some(&r), None, None, 2_000);
        i.suspend = true;
        assert_eq!(plan(&i), None);
        let mut i = input(Some(&r), None, None, 2_000);
        i.policy = None;
        assert_eq!(plan(&i), None);
    }

    #[test]
    fn late_is_at_the_timeout_and_recheck_names_the_moment() {
        let mut r = record(Some(10_000), None);
        assert_eq!(
            recheck(&r, &POLICY, 1_000),
            Some(Duration::from_secs(10_000 - 120 - 1_000))
        );
        assert_eq!(
            recheck(&r, &POLICY, 9_880),
            Some(Duration::from_secs(1)),
            "never less than a second"
        );
        r.waking = Some(WakingStatus {
            since: time_of(2_000),
            cause: WakeCause::Schedule,
        });
        assert!(!timed_out(&r, &POLICY, 2_059));
        assert!(timed_out(&r, &POLICY, 2_060));
        assert_eq!(recheck(&r, &POLICY, 2_000), Some(Duration::from_secs(60)));
        assert_eq!(recheck(&r, &POLICY, 2_060), None, "late is said once");
    }
}
