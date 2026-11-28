# Runtime, runs, leases and scale-to-zero — evidence

The page for people is [`docs/architecture/05-runtime.md`](../../../docs/architecture/05-runtime.md). This page holds the evidence that left it when the docs for people and the docs for agents were split (AD-074): test lists, review and commit ids, and the *verified* marks, under the `§N` headings they belong to. Text is quoted verbatim from the page for people.

## §21a. Sleep when idle: the plan

The *What is built, and what is not* table's run-pods row ended: `` `10-control-plane-and-crds.md` (run pool `idleTTLSeconds`), commit `cb6b5bc` ``.

P2b as built (the idle loop) ended:

> Tests: `sleeper.rs` (each rule of the decision through the loop's step, the drain; ~~a failed `suspend`~~ *superseded by P3a-1: that test is deleted, the loop no longer suspends*), `tests/pure.rs` (the phase and reason) and `tests/operator.rs` (`an_idle_agent_sleeps_and_a_removed_policy_wakes_it`, on the fake API server and a scripted source). A real cluster is unverified until P4.

ANR-1030a as built (the render half of the activity listener) carried the proof sentence:

> Proven by `crates/domain/tests/activity.rs`, `crates/runtime-kubernetes/tests/render.rs` and the new golden `coder-activity.yaml`, each guard shown to fail by mutation (PR body).

and its *Not proven* ended:

> the parity goldens stay at adam-rs `4bc6742` and cannot render it

ANR-1030b as built (the operator wired) carried, mid-paragraph:

> *Unverified:* the generator's and `PushSecret`'s exact fields are External Secrets Operator's and were not checked here;

P3a-1 as built (the operator's wake path): the mark-age rule reads "**A mark is a statement about one moment** (review of 60d4f70): …", and the paragraph ended:

> Tests, each shown to fail with its guard removed (mutation log in PR #138): `crates/controller/tests/operator.rs` (`a_wake_at_annotation_wakes_a_sleeping_agent_through_waking_to_ready`, `a_scheduled_wake_fires_at_next_due_at_minus_wake_ahead`, `an_operator_restart_keeps_a_sleeping_agent_asleep`, `a_stale_wake_at_from_before_the_sleep_does_not_wake`, `suspend_while_waking_clears_the_record`, `waking_past_wake_timeout_keeps_waking_and_says_so`, `a_mark_left_by_a_blocked_pass_never_sleeps_a_busy_agent`, `a_mark_whose_wake_came_before_the_sleep_is_dropped_not_recorded`, `an_agent_with_three_workers_wakes_to_three_replicas_not_one`), `sleeper.rs` (`losing_leadership_drops_the_marks`, `a_mark_older_than_the_bound_is_dropped_and_the_agent_is_polled_again`), `tests/pure.rs` and `wake.rs`. **Unverified:** a real cluster (P4); two near-simultaneous passes can read a status a little behind the write, which re-writes the same record with a later `since` and is harmless; a wake request that arrives while the idle loop's mark is being made is kept (the annotation is read at the decision).

## §26. Agent Runtime

The AD-022 blockquote's note on the adam-acp crate carried:

> (*verified 2026-10-04*, adam-rs at `0391809`: `docs/architecture.md`, `crates/adam-acp/README.md`)

The two notes after the drawings (ADK-Rust is a community implementation, `opencode acp` over stdio) were titled "**Verified notes (2026-09-28):**" — both were verified against the sources named there on 2026-09-28, and the ADK-Rust one was not exercised yet ("spike before it becomes load-bearing").
