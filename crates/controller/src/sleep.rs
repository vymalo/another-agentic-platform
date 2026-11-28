//! The idle decision (AD-058, P2a): when may an agent sleep?
//!
//! [`decide`] is pure over a time the caller gives, the last answer of an [`ActivitySource`](aap_ports::ActivitySource)
//! and what it remembered of the last pass, so a test travels in time with a number. Nothing here polls, scales or
//! writes: the controller's wiring (P2b) does, and asks this for each step.
//!
//! The rules, each named by a test:
//!
//! * an agent is idle only while its own answer says so: no run held, and no schedule due within `wakeAhead`;
//! * **a failed poll counts as busy**, so an unreachable agent is never scaled down on a guess;
//! * the idle clock starts at the first idle poll and restarts after any busy one;
//! * `spec.suspend` wins: a suspended agent is a person's choice, never put to sleep by this and never counted.

use std::time::Duration;

use aap_api::Scaling;
use aap_ports::{Activity, ActivityError};

/// `scaling.wakeAhead` when absent (AD-058).
pub const DEFAULT_WAKE_AHEAD: Duration = Duration::from_secs(2 * 60);

/// `scaling.wakeTimeout` when absent (AD-058, ANR-Q141).
pub const DEFAULT_WAKE_TIMEOUT: Duration = Duration::from_secs(60);

/// What `scaling` asks of the idle controller. Absent when the agent never sleeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SleepPolicy {
    /// How long the agent must report no work before it sleeps.
    pub idle_timeout: Duration,
    /// A schedule due within this long keeps the agent awake.
    pub wake_ahead: Duration,
    /// How long a wake may take before the operator says it is late (it never goes back to sleep for it).
    pub wake_timeout: Duration,
}

impl SleepPolicy {
    /// The policy of an agent's `scaling`, or `None` when it has no `idleTimeout` (it never sleeps). A value that
    /// is not a duration the CRD admits (`15m`, `90s`, `2h`) also gives `None`: an unreadable bound never makes
    /// an agent sleep.
    pub fn from_scaling(scaling: &Scaling) -> Option<Self> {
        let idle_timeout = parse_duration(scaling.idle_timeout.as_deref()?)?;
        let wake_ahead = match scaling.wake_ahead.as_deref() {
            Some(text) => parse_duration(text)?,
            None => DEFAULT_WAKE_AHEAD,
        };
        let wake_timeout = match scaling.wake_timeout.as_deref() {
            Some(text) => parse_duration(text)?,
            None => DEFAULT_WAKE_TIMEOUT,
        };
        Some(Self {
            idle_timeout,
            wake_ahead,
            wake_timeout,
        })
    }
}

/// `15m`, `90s`, `2h`: the one shape the CRD's bounds admit.
fn parse_duration(text: &str) -> Option<Duration> {
    let unit = text.chars().last()?;
    let number: u64 = text[..text.len() - unit.len_utf8()].parse().ok()?;
    let seconds = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        _ => return None,
    };
    (number > 0).then(|| Duration::from_secs(number.saturating_mul(seconds)))
}

/// What the controller remembers of one agent between passes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdleState {
    /// When the agent was first seen idle in the current idle stretch (seconds since the Unix epoch).
    pub idle_since: Option<i64>,
}

/// What to do about the agent now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Leave it as it is.
    Stay,
    /// Drain it and scale it to zero.
    Sleep,
}

/// The state to keep and the decision to act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    /// What the controller remembers for the next pass.
    pub state: IdleState,
    /// What to do now.
    pub decision: Decision,
}

/// One pass of the idle clock for one agent.
///
/// * `suspended`: `spec.suspend`; it wins over everything, forgets the idle stretch and never sleeps.
/// * `polled`: the agent's answer; an error is treated as busy.
/// * `now`: seconds since the Unix epoch.
pub fn decide(
    policy: &SleepPolicy,
    state: IdleState,
    suspended: bool,
    polled: &Result<Activity, ActivityError>,
    now: i64,
) -> Step {
    let stay = |state| Step {
        state,
        decision: Decision::Stay,
    };
    if suspended {
        return stay(IdleState::default());
    }
    let Ok(activity) = polled else {
        return stay(IdleState::default());
    };
    if activity.is_busy() {
        return stay(IdleState::default());
    }
    let ahead = i64::try_from(policy.wake_ahead.as_secs()).unwrap_or(i64::MAX);
    if activity
        .next_due_at
        .is_some_and(|due| due.saturating_sub(now) <= ahead)
    {
        return stay(IdleState::default());
    }
    let since = state.idle_since.unwrap_or(now);
    let timeout = i64::try_from(policy.idle_timeout.as_secs()).unwrap_or(i64::MAX);
    let state = IdleState {
        idle_since: Some(since),
    };
    if now.saturating_sub(since) >= timeout {
        Step {
            state,
            decision: Decision::Sleep,
        }
    } else {
        stay(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const POLICY: SleepPolicy = SleepPolicy {
        idle_timeout: Duration::from_secs(600),
        wake_ahead: Duration::from_secs(120),
        wake_timeout: Duration::from_secs(60),
    };

    fn busy(n: u32) -> Result<Activity, ActivityError> {
        Ok(Activity {
            busy: n,
            next_due_at: None,
        })
    }

    fn idle() -> Result<Activity, ActivityError> {
        Ok(Activity::IDLE)
    }

    fn down() -> Result<Activity, ActivityError> {
        Err(ActivityError::Unavailable {
            cause: aap_ports::UnavailableCause::Connect,
            source: "down".into(),
        })
    }

    /// Runs `polls` (time, answer) in order from a fresh state and returns every decision.
    fn run(polls: &[(i64, Result<Activity, ActivityError>)], suspended: bool) -> Vec<Decision> {
        let mut state = IdleState::default();
        polls
            .iter()
            .map(|(now, answer)| {
                let step = decide(&POLICY, state, suspended, answer, *now);
                state = step.state;
                step.decision
            })
            .collect()
    }

    #[test]
    fn busy_then_idle_for_the_timeout_sleeps() {
        let got = run(
            &[
                (0, busy(1)),
                (60, idle()),
                (359, idle()),
                (659, idle()),
                (660, idle()),
            ],
            false,
        );
        use Decision::{Sleep, Stay};
        // The idle stretch starts at 60: it reaches 600 s at 660, not before.
        assert_eq!(got, [Stay, Stay, Stay, Stay, Sleep]);
    }

    #[test]
    fn work_in_the_middle_restarts_the_clock() {
        let got = run(
            &[
                (0, idle()),
                (500, idle()),
                (550, busy(1)),
                (600, idle()),
                (1100, idle()),
                (1200, idle()),
            ],
            false,
        );
        use Decision::{Sleep, Stay};
        assert_eq!(got, [Stay, Stay, Stay, Stay, Stay, Sleep]);
    }

    #[test]
    fn a_poll_error_never_scales_down_and_counts_as_busy() {
        // Idle for 590 s, then the agent stops answering: the stretch is forgotten, so even a long outage
        // followed by one idle answer is not a sleep.
        let got = run(
            &[
                (0, idle()),
                (590, idle()),
                (700, down()),
                (5000, down()),
                (5001, idle()),
            ],
            false,
        );
        assert!(!got.contains(&Decision::Sleep), "{got:?}");
    }

    #[test]
    fn a_schedule_inside_wake_ahead_never_scales_down() {
        let due_soon = |now: i64| {
            Ok(Activity {
                busy: 0,
                next_due_at: Some(now + 120),
            })
        };
        let got = run(
            &[
                (0, due_soon(0)),
                (700, due_soon(700)),
                (1400, due_soon(1400)),
            ],
            false,
        );
        assert!(!got.contains(&Decision::Sleep), "{got:?}");
        // One second further out is outside the window: the agent may sleep.
        let far = |now: i64| {
            Ok(Activity {
                busy: 0,
                next_due_at: Some(now + 121),
            })
        };
        let got = run(&[(0, far(0)), (600, far(600))], false);
        assert_eq!(got, [Decision::Stay, Decision::Sleep]);
        // A schedule that is overdue is inside the window too.
        assert_eq!(
            run(
                &[
                    (
                        0,
                        Ok(Activity {
                            busy: 0,
                            next_due_at: Some(-30)
                        })
                    ),
                    (
                        900,
                        Ok(Activity {
                            busy: 0,
                            next_due_at: Some(-30)
                        })
                    )
                ],
                false
            ),
            [Decision::Stay, Decision::Stay]
        );
    }

    #[test]
    fn suspend_wins() {
        let got = run(&[(0, idle()), (600, idle()), (6000, idle())], true);
        assert!(!got.contains(&Decision::Sleep), "{got:?}");
        // Suspending in the middle of an idle stretch forgets it: resuming starts the clock again.
        let mut state = IdleState::default();
        state = decide(&POLICY, state, false, &idle(), 0).state;
        state = decide(&POLICY, state, true, &idle(), 500).state;
        assert_eq!(state, IdleState::default());
        let step = decide(&POLICY, state, false, &idle(), 700);
        assert_eq!(step.decision, Decision::Stay);
    }

    #[test]
    fn a_policy_reads_the_scaling_fields() {
        let mut scaling = Scaling::default();
        assert_eq!(
            SleepPolicy::from_scaling(&scaling),
            None,
            "no idleTimeout: never sleeps"
        );
        scaling.idle_timeout = Some("15m".into());
        assert_eq!(
            SleepPolicy::from_scaling(&scaling),
            Some(SleepPolicy {
                idle_timeout: Duration::from_secs(900),
                wake_ahead: DEFAULT_WAKE_AHEAD,
                wake_timeout: DEFAULT_WAKE_TIMEOUT,
            })
        );
        scaling.wake_timeout = Some("5m".into());
        assert_eq!(
            SleepPolicy::from_scaling(&scaling).unwrap().wake_timeout,
            Duration::from_secs(300)
        );
        scaling.wake_timeout = None;
        scaling.wake_ahead = Some("30s".into());
        assert_eq!(
            SleepPolicy::from_scaling(&scaling).unwrap().wake_ahead,
            Duration::from_secs(30)
        );
        scaling.idle_timeout = Some("soon".into());
        assert_eq!(
            SleepPolicy::from_scaling(&scaling),
            None,
            "an unreadable bound never sleeps"
        );
    }
}
