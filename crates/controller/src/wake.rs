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
//!     Waking --> Running: scaling.idleTimeout removed (the record is removed)
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
    /// When the loop decided (seconds since the Unix epoch, on the clock the reconcile uses). A proposal is only a
    /// statement about that moment: a mark older than [`mark_max_age`] is **ignored and dropped**, because the agent
    /// may be busy now and nothing asked it again (a blocked pass leaves a mark behind, and so does a leader that
    /// lost its lease before its pass).
    pub made_at: i64,
}

/// How old a [`Mark`] may be and still be acted on: one idle-loop period plus one pass's time limit. Chosen because a
/// nudged pass that is going to consume the mark starts within the loop's period and ends within the pass limit; a
/// mark that outlives both was not consumed by the pass it asked for.
pub fn mark_max_age(sleep_poll: Duration, pass_timeout: Duration) -> Duration {
    sleep_poll.saturating_add(pass_timeout)
}

/// The most bytes of a `wake-at` value stored as is. An annotation may hold 256 KiB, and the status is sent to every
/// watcher: a longer value is stored as its digest.
pub const SEEN_MAX_BYTES: usize = 64;

/// What `status.sleep.wakeAtSeen` holds for an annotation value: the value itself when it is at most 64 bytes, else
/// `sha256:<64 hex>` (71 characters, the `maxLength` of the field). The decision compares through this function on
/// both sides, so a long value that did not change is not a wake and one that did is.
pub fn seen_of(value: &str) -> String {
    use sha2::{Digest, Sha256};
    if value.len() <= SEEN_MAX_BYTES {
        return value.to_owned();
    }
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::from("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
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
    /// See [`mark_max_age`].
    pub mark_max_age: Duration,
    /// The mark is claimed: its record is being (or was) written. A claimed mark is never aged out, as
    /// `Sleepers::is_marked` does not age it, or a pass whose object is behind its own write would erase the record.
    pub mark_claimed: bool,
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
/// does not is made from the idle loop's mark **if the mark is fresh**; and a sleeping record whose agent is asked
/// for, or due, gets `waking`.
///
/// A record that would be **born waking** (the annotation changed since the decision, or the schedule is already
/// inside `wakeAhead`) is not made: the agent never slept, so there is nothing to record and nothing to count. The
/// caller drops the mark (see [`stale_or_stillborn`]).
pub fn plan(input: &PlanInput<'_>) -> Option<SleepStatus> {
    let policy = input.policy?;
    if input.suspend {
        return None;
    }
    let mut record = match (input.record, input.mark) {
        (Some(record), _) => record.clone(),
        (None, Some(mark)) if is_fresh(mark, input) => SleepStatus {
            since: time_of(input.now),
            next_due_at: mark.next_due_at.map(time_of),
            wake_at_seen: mark.wake_at_seen.clone(), // already `seen_of` the annotation
            waking: None,
        },
        (None, _) => return None,
    };
    let born = input.record.is_none();
    if record.waking.is_none() {
        let asked = input
            .wake_at
            .is_some_and(|value| Some(seen_of(value)) != record.wake_at_seen);
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
            if born {
                return None;
            }
            record.waking = Some(WakingStatus {
                since: time_of(input.now),
                cause,
            });
        }
    }
    Some(record)
}

fn is_fresh(mark: &Mark, input: &PlanInput<'_>) -> bool {
    input.mark_claimed || input.now.saturating_sub(mark.made_at) <= secs(input.mark_max_age)
}

/// Whether the caller should drop the idle loop's mark after [`plan`] gave `planned`: the mark was there and no
/// record came out of it (it was too old, or the sleep would have been born waking), and there is no record to
/// keep it company.
pub fn stale_or_stillborn(input: &PlanInput<'_>, planned: &Option<SleepStatus>) -> bool {
    input.mark.is_some() && input.record.is_none() && planned.is_none()
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
            mark_max_age: Duration::from_secs(90),
            mark_claimed: false,
        }
    }

    #[test]
    fn a_mark_becomes_a_record_and_nothing_else_does() {
        let mark = Mark {
            next_due_at: Some(9_000),
            wake_at_seen: Some("a".into()),
            made_at: 1_990,
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

    #[test]
    fn a_mark_older_than_the_bound_makes_no_record_and_is_to_be_dropped() {
        let mark = Mark {
            next_due_at: None,
            wake_at_seen: None,
            made_at: 1_000,
        };
        let fresh = input(None, Some(&mark), None, 1_090);
        assert!(plan(&fresh).is_some(), "at the bound it is still good");
        let old = input(None, Some(&mark), None, 1_091);
        let planned = plan(&old);
        assert_eq!(planned, None);
        assert!(stale_or_stillborn(&old, &planned));
        assert_eq!(
            mark_max_age(Duration::from_secs(30), Duration::from_secs(60)),
            Duration::from_secs(90)
        );
    }

    #[test]
    fn a_sleep_that_would_be_born_waking_is_never_recorded() {
        let by_request = Mark {
            next_due_at: None,
            wake_at_seen: Some("a".into()),
            made_at: 2_000,
        };
        let i = input(None, Some(&by_request), Some("b"), 2_000);
        let planned = plan(&i);
        assert_eq!(planned, None, "the annotation changed after the decision");
        assert!(stale_or_stillborn(&i, &planned));
        let by_schedule = Mark {
            next_due_at: Some(2_100),
            wake_at_seen: None,
            made_at: 2_000,
        };
        assert_eq!(plan(&input(None, Some(&by_schedule), None, 2_000)), None);
        // An existing record is never "stillborn": it keeps its mark's company.
        let r = record(None, None);
        let i = input(Some(&r), Some(&by_request), None, 2_000);
        assert!(!stale_or_stillborn(&i, &plan(&i)));
    }

    #[test]
    fn a_long_annotation_is_stored_as_its_digest_and_compared_through_it() {
        assert_eq!(seen_of("2026-11-27T10:00:00Z"), "2026-11-27T10:00:00Z");
        let at_limit = "x".repeat(64);
        assert_eq!(seen_of(&at_limit), at_limit);
        let long = "y".repeat(256 * 1024);
        let seen = seen_of(&long);
        assert_eq!(seen.len(), 71);
        assert!(seen.starts_with("sha256:"));
        assert_eq!(seen_of(&"x".repeat(65)).len(), 71);
        assert_ne!(seen_of(&"x".repeat(65)), seen_of(&"x".repeat(66)));
        // The same long value is no wake; another is.
        let r = record(None, Some(&seen));
        assert!(
            plan(&input(Some(&r), None, Some(&long), 2_000))
                .unwrap()
                .waking
                .is_none()
        );
        let other = "z".repeat(1000);
        assert!(
            plan(&input(Some(&r), None, Some(&other), 2_000))
                .unwrap()
                .waking
                .is_some()
        );
    }

    #[test]
    fn a_claimed_mark_is_never_aged_out_by_plan() {
        let mark = Mark {
            next_due_at: None,
            wake_at_seen: None,
            made_at: 1_000,
        };
        let mut old = input(None, Some(&mark), None, 1_000 + 10_000);
        assert_eq!(plan(&old), None, "unclaimed, it is too old");
        old.mark_claimed = true;
        let record = plan(&old).expect("claimed: the record its write made is kept");
        assert!(record.waking.is_none());
        assert!(!stale_or_stillborn(&old, &Some(record)));
    }

    #[test]
    fn each_boundary_of_a_wake_is_exact() {
        // Schedule: `>=` at nextDueAt - wakeAhead (see also the 121 and 120 cases above).
        let r = record(Some(10_000), None);
        assert!(
            plan(&input(Some(&r), None, None, 9_879))
                .unwrap()
                .waking
                .is_none()
        );
        assert!(
            plan(&input(Some(&r), None, None, 9_880))
                .unwrap()
                .waking
                .is_some()
        );
        // Timeout: late exactly at since + wakeTimeout.
        let mut w = record(None, None);
        w.waking = Some(WakingStatus {
            since: time_of(2_000),
            cause: WakeCause::Request,
        });
        assert!(!timed_out(&w, &POLICY, 2_059));
        assert!(timed_out(&w, &POLICY, 2_060));
        // Mark age: good at the bound, gone one second after.
        let m = Mark {
            next_due_at: None,
            wake_at_seen: None,
            made_at: 1_000,
        };
        assert!(plan(&input(None, Some(&m), None, 1_090)).is_some());
        assert!(plan(&input(None, Some(&m), None, 1_091)).is_none());
    }
}
